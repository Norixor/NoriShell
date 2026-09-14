use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminationOutcome {
    AlreadyExited,
    Interrupted,
    Terminated,
    Killed,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TerminationPolicy {
    interrupt_wait: Duration,
    terminate_wait: Duration,
    kill_wait: Duration,
}

impl TerminationPolicy {
    pub(crate) const fn with_timeouts(
        interrupt_wait: Duration,
        terminate_wait: Duration,
        kill_wait: Duration,
    ) -> Self {
        Self {
            interrupt_wait,
            terminate_wait,
            kill_wait,
        }
    }
}

impl Default for TerminationPolicy {
    fn default() -> Self {
        Self::with_timeouts(
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_secs(3),
        )
    }
}

/// Owns one Unix process group and never equates the root process exit with
/// the complete process tree having stopped.
pub(crate) struct ManagedProcessGroup {
    child: Child,
    process_group_id: i32,
    exit_status: Option<ExitStatus>,
    fully_stopped: bool,
}

impl ManagedProcessGroup {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0);
        Self::spawn_and_verify(command)
    }

    /// Starts a command whose `pre_exec` setup creates a new session with the
    /// child PID as its process group. This deliberately does not also request
    /// `process_group(0)`: `setpgid` would make the child a process-group leader
    /// before `setsid`, causing the latter to fail with `EPERM`.
    pub(crate) fn spawn_session_leader(command: &mut Command) -> io::Result<Self> {
        Self::spawn_and_verify(command)
    }

    fn spawn_and_verify(command: &mut Command) -> io::Result<Self> {
        let mut child = command.spawn()?;
        let process_group_id = match i32::try_from(child.id()) {
            Ok(value) => value,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other(
                    "child process id is outside the Unix pid range",
                ));
            }
        };
        let observed_group = match observed_process_group(process_group_id) {
            Ok(value) => value,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if observed_group != process_group_id {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(
                "child did not enter its isolated process group",
            ));
        }
        Ok(Self {
            child,
            process_group_id,
            exit_status: None,
            fully_stopped: false,
        })
    }

    pub(crate) fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn terminate(&mut self, policy: TerminationPolicy) -> TerminationOutcome {
        match self.terminate_checked(policy) {
            Ok(outcome) => outcome,
            Err(_) => TerminationOutcome::Unknown,
        }
    }

    pub(crate) fn terminate_checked(
        &mut self,
        policy: TerminationPolicy,
    ) -> io::Result<TerminationOutcome> {
        self.terminate_inner(policy)
    }

    pub(crate) fn poll_root_exit(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.exit_status.is_none() {
            self.exit_status = self.child.try_wait()?;
        }
        Ok(self.exit_status)
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub(crate) fn wait_for_exit(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        if self.wait_until_stopped(timeout)? {
            Ok(self.exit_status)
        } else {
            Ok(None)
        }
    }

    fn terminate_inner(&mut self, policy: TerminationPolicy) -> io::Result<TerminationOutcome> {
        if self.wait_until_stopped(Duration::ZERO)? {
            return Ok(TerminationOutcome::AlreadyExited);
        }

        self.signal_group(libc::SIGINT)?;
        if self.wait_until_stopped(policy.interrupt_wait)? {
            return Ok(TerminationOutcome::Interrupted);
        }

        self.signal_group(libc::SIGTERM)?;
        if self.wait_until_stopped(policy.terminate_wait)? {
            return Ok(TerminationOutcome::Terminated);
        }

        self.signal_group(libc::SIGKILL)?;
        if self.wait_until_stopped(policy.kill_wait)? {
            return Ok(TerminationOutcome::Killed);
        }
        Ok(TerminationOutcome::Unknown)
    }

    fn signal_group(&mut self, signal: i32) -> io::Result<()> {
        if !process_group_exists(self.process_group_id)? {
            return Ok(());
        }
        // The pid is negative so the signal is confined to the verified child
        // process group, never the Core's own process group.
        let result = unsafe { libc::kill(-self.process_group_id, signal) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn wait_until_stopped(&mut self, timeout: Duration) -> io::Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_root_exit()?;
            let group_exists = process_group_exists(self.process_group_id)?;
            if self.exit_status.is_some() && !group_exists {
                self.fully_stopped = true;
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ManagedProcessGroup {
    fn drop(&mut self) {
        if self.fully_stopped {
            return;
        }
        let _ = self.signal_group(libc::SIGKILL);
        let _ = self.wait_until_stopped(Duration::from_secs(1));
    }
}

fn observed_process_group(process_id: i32) -> io::Result<i32> {
    // getpgid is a read-only identity check for the child created above.
    let process_group_id = unsafe { libc::getpgid(process_id) };
    if process_group_id < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(process_group_id)
    }
}

fn process_group_exists(process_group_id: i32) -> io::Result<bool> {
    // Signal 0 probes existence and permission without changing the process.
    let result = unsafe { libc::kill(-process_group_id, 0) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        process::{Command, Stdio},
        time::Duration,
    };

    use tempfile::tempdir;

    use super::{ManagedProcessGroup, TerminationOutcome, TerminationPolicy};

    const FAST_POLICY: TerminationPolicy = TerminationPolicy::with_timeouts(
        Duration::from_millis(500),
        Duration::from_millis(500),
        Duration::from_secs(1),
    );

    #[test]
    fn graceful_interrupt_reaps_the_complete_process_group() {
        let directory = tempdir().expect("temporary directory");
        let readiness = directory.path().join("ready");
        let mut command = shell_process_tree(
            "trap 'kill -TERM \"$child\" 2>/dev/null; wait \"$child\"; exit 0' INT; sleep 30 & child=$!; : > \"$1\"; wait \"$child\"",
            &readiness,
        );
        let mut process = ManagedProcessGroup::spawn(&mut command).expect("managed group");
        assert!(wait_for_path(&readiness));

        assert_eq!(
            process.terminate(FAST_POLICY),
            TerminationOutcome::Interrupted
        );
    }

    #[test]
    fn escalation_reaches_kill_when_the_process_tree_ignores_soft_signals() {
        let directory = tempdir().expect("temporary directory");
        let readiness = directory.path().join("ready");
        let mut command = shell_process_tree(
            "trap '' INT TERM; sleep 30 & child=$!; : > \"$1\"; wait \"$child\"",
            &readiness,
        );
        let mut process = ManagedProcessGroup::spawn(&mut command).expect("managed group");
        assert!(wait_for_path(&readiness));

        assert_eq!(process.terminate(FAST_POLICY), TerminationOutcome::Killed);
    }

    #[test]
    fn an_already_exited_root_is_confirmed_only_after_the_group_is_gone() {
        let mut command = Command::new("/usr/bin/true");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut process = ManagedProcessGroup::spawn(&mut command).expect("managed group");
        std::thread::sleep(Duration::from_millis(50));

        assert_eq!(
            process.terminate(FAST_POLICY),
            TerminationOutcome::AlreadyExited
        );
    }

    fn shell_process_tree(script: &str, readiness: &std::path::Path) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script, "norishell-process-control-test"])
            .arg(readiness)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn wait_for_path(path: &std::path::Path) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !path.exists() {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }
}
