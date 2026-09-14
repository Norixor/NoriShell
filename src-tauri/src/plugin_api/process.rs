//! Core-owned local process resources.
//!
//! The protected broker constructs a [`FrozenPluginProcessPlan`] before this module runs. The
//! driver only enforces the frozen plan and resource fence; it never interprets guest text as a
//! command line, grants permission, or records argv, environment values, or stdin bytes.
//!
//! This isolation is deliberately narrower than an OS sandbox. The exact executable still runs
//! with the current user's OS authority, but in an empty allowlisted environment and an isolated
//! process tree that Core must reap before releasing its resource blocker.

use std::{
    io::{self, Write as _},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::{
    FrozenPluginProcessPlan, PluginApiErrorCode, PluginApiResourceEventKind,
    PluginProcessOutputStream, PluginProcessSendRequest,
};
use tokio::sync::watch;

use super::{
    ResourceCommandReceiver, ResourceEventWriter, ResourceFence, ResourceOwner, ResourceRegistry,
};

const MAX_PROCESS_STDIN_BYTES: usize = 8 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);
const PROCESS_TERMINATION_POLICY: Duration = Duration::from_millis(500);

pub(crate) struct ProcessDriver;

impl ProcessDriver {
    /// Starts only an already-authorized Core plan. `plan` is intentionally not deserialized from
    /// a plugin request, so this method cannot be used as an authorization fallback.
    pub(crate) fn start(
        resources: &ResourceRegistry,
        owner: ResourceOwner,
        plan: FrozenPluginProcessPlan,
        fence: ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        plan.validate_bounds()?;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        resources.spawn_with_commands(
            owner,
            "process",
            move |cancel, events, commands| async move {
                tokio::task::spawn_blocking(move || {
                    run_process(plan, cancel, events, commands, fence)
                })
                .await
                .unwrap_or(Err(PluginApiErrorCode::CleanupIncomplete))
            },
        )
    }

    /// The registry rechecks exact owner and open state, then waits for an actual stdin write ACK.
    pub(crate) async fn send(
        resources: &ResourceRegistry,
        owner: &ResourceOwner,
        request: PluginProcessSendRequest,
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        let bytes = if request.close_stdin {
            if !request.data_base64.is_empty() {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            // The Core-only command channel treats a zero-length frame as EOF. Ordinary writes
            // below reject it, so the frame cannot be confused with guest data.
            Vec::new()
        } else {
            if request.data_base64.is_empty()
                || request.data_base64.len() > MAX_PROCESS_STDIN_BYTES.saturating_mul(2)
            {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            let bytes = BASE64
                .decode(request.data_base64)
                .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
            if bytes.is_empty() || bytes.len() > MAX_PROCESS_STDIN_BYTES {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            bytes
        };
        resources
            .send(owner, &request.handle, bytes, fence.as_ref())
            .await
    }
}

#[cfg(unix)]
fn run_process(
    plan: FrozenPluginProcessPlan,
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    commands: ResourceCommandReceiver,
    fence: ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    use std::process::Command;

    use crate::process_control::ManagedProcessGroup;

    if !fence() {
        return Ok(());
    }
    let mut command = Command::new(plan.program());
    command
        .args(plan.arguments())
        .current_dir(plan.user_home())
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = match ManagedProcessGroup::spawn(&mut command) {
        Ok(process) => process,
        Err(_) => {
            let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
            return Ok(());
        }
    };
    let stdin = match process.take_stdin() {
        Some(stdin) => stdin,
        None => return missing_stdio(process, &events),
    };
    let stdout = match process.take_stdout() {
        Some(stdout) => stdout,
        None => return missing_stdio(process, &events),
    };
    let stderr = match process.take_stderr() {
        Some(stderr) => stderr,
        None => return missing_stdio(process, &events),
    };
    let stdout_reader = spawn_reader(
        stdout,
        PluginProcessOutputStream::Stdout,
        events.clone(),
        cancel.clone(),
        fence.clone(),
    );
    let stderr_reader = spawn_reader(
        stderr,
        PluginProcessOutputStream::Stderr,
        events.clone(),
        cancel.clone(),
        fence.clone(),
    );
    let result = drive_unix_process(
        &mut process,
        Some(stdin),
        commands,
        &mut cancel,
        &events,
        &fence,
        plan.timeout_ms(),
    );
    join_readers(stdout_reader, stderr_reader)?;
    result
}

#[cfg(unix)]
fn drive_unix_process(
    process: &mut crate::process_control::ManagedProcessGroup,
    mut stdin: Option<std::process::ChildStdin>,
    mut commands: ResourceCommandReceiver,
    cancel: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    timeout_ms: u32,
) -> Result<(), PluginApiErrorCode> {
    use crate::process_control::TerminationOutcome;

    let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
    loop {
        if *cancel.borrow_and_update() || !fence() {
            let outcome = process.terminate(unix_termination_policy());
            drop(stdin.take());
            drain_cancelled(&mut commands);
            if matches!(outcome, TerminationOutcome::Unknown) {
                return Err(PluginApiErrorCode::CleanupIncomplete);
            }
            let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
            return Ok(());
        }
        if Instant::now() >= deadline {
            let outcome = process.terminate(unix_termination_policy());
            drop(stdin.take());
            drain_cancelled(&mut commands);
            if matches!(outcome, TerminationOutcome::Unknown) {
                return Err(PluginApiErrorCode::CleanupIncomplete);
            }
            let _ = events.emit(PluginApiResourceEventKind::ProcessExited { exit_code: None });
            return Ok(());
        }
        while let Some(command) = commands.try_recv() {
            let result = if command.payload().is_empty() {
                stdin
                    .take()
                    .map(|_| ())
                    .ok_or(PluginApiErrorCode::Cancelled)
            } else {
                stdin
                    .as_mut()
                    .ok_or(PluginApiErrorCode::Cancelled)
                    .and_then(|stdin| {
                        stdin
                            .write_all(command.payload())
                            .and_then(|()| stdin.flush())
                            .map_err(|_| PluginApiErrorCode::Cancelled)
                    })
            };
            command.finish(result);
        }
        match process
            .wait_for_exit(Duration::ZERO)
            .map_err(|_| PluginApiErrorCode::CleanupIncomplete)?
        {
            Some(status) => {
                drop(stdin.take());
                drain_cancelled(&mut commands);
                let _ = events.emit(PluginApiResourceEventKind::ProcessExited {
                    exit_code: status.code().map(i64::from),
                });
                return Ok(());
            }
            None => thread::sleep(PROCESS_POLL_INTERVAL),
        }
    }
}

#[cfg(unix)]
fn missing_stdio(
    mut process: crate::process_control::ManagedProcessGroup,
    events: &ResourceEventWriter,
) -> Result<(), PluginApiErrorCode> {
    let outcome = process.terminate(unix_termination_policy());
    if matches!(outcome, crate::process_control::TerminationOutcome::Unknown) {
        return Err(PluginApiErrorCode::CleanupIncomplete);
    }
    // Launch failure is not a durable process blocker once the verified group is empty.
    let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
    Ok(())
}

#[cfg(windows)]
fn run_process(
    plan: FrozenPluginProcessPlan,
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    commands: ResourceCommandReceiver,
    fence: ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    use crate::windows_process_control::{ManagedJobProcess, TerminationOutcome};

    if !fence() {
        return Ok(());
    }
    // `spawn_with_stdio_empty_environment` gives a stricter subset of the platform allowlist:
    // the executable is absolute and no ambient token-bearing environment value is inherited.
    let (mut process, stdio) = match ManagedJobProcess::spawn_with_stdio_empty_environment(
        plan.program(),
        plan.arguments(),
        Some(plan.user_home()),
    ) {
        Ok(value) => value,
        Err(_) => {
            let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
            return Ok(());
        }
    };
    let stdout_reader = spawn_reader(
        stdio.stdout,
        PluginProcessOutputStream::Stdout,
        events.clone(),
        cancel.clone(),
        fence.clone(),
    );
    let stderr_reader = spawn_reader(
        stdio.stderr,
        PluginProcessOutputStream::Stderr,
        events.clone(),
        cancel.clone(),
        fence.clone(),
    );
    let result = drive_windows_process(
        &mut process,
        Some(stdio.stdin),
        commands,
        &mut cancel,
        &events,
        &fence,
        plan.timeout_ms(),
    );
    join_readers(stdout_reader, stderr_reader)?;
    result
}

#[cfg(windows)]
fn drive_windows_process(
    process: &mut crate::windows_process_control::ManagedJobProcess,
    mut stdin: Option<std::fs::File>,
    mut commands: ResourceCommandReceiver,
    cancel: &mut watch::Receiver<bool>,
    events: &ResourceEventWriter,
    fence: &ResourceFence,
    timeout_ms: u32,
) -> Result<(), PluginApiErrorCode> {
    use crate::windows_process_control::TerminationOutcome;

    let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
    loop {
        if *cancel.borrow_and_update() || !fence() {
            let outcome = process.terminate_with(windows_termination_policy(), |_| Ok(()));
            drop(stdin.take());
            drain_cancelled(&mut commands);
            if matches!(outcome, TerminationOutcome::Unknown) {
                return Err(PluginApiErrorCode::CleanupIncomplete);
            }
            let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
            return Ok(());
        }
        if Instant::now() >= deadline {
            let outcome = process.terminate_with(windows_termination_policy(), |_| Ok(()));
            drop(stdin.take());
            drain_cancelled(&mut commands);
            if matches!(outcome, TerminationOutcome::Unknown) {
                return Err(PluginApiErrorCode::CleanupIncomplete);
            }
            let _ = events.emit(PluginApiResourceEventKind::ProcessExited { exit_code: None });
            return Ok(());
        }
        while let Some(command) = commands.try_recv() {
            let result = if command.payload().is_empty() {
                stdin
                    .take()
                    .map(|_| ())
                    .ok_or(PluginApiErrorCode::Cancelled)
            } else {
                stdin
                    .as_mut()
                    .ok_or(PluginApiErrorCode::Cancelled)
                    .and_then(|stdin| {
                        stdin
                            .write_all(command.payload())
                            .and_then(|()| stdin.flush())
                            .map_err(|_| PluginApiErrorCode::Cancelled)
                    })
            };
            command.finish(result);
        }
        match process
            .wait_for_exit(Duration::ZERO)
            .map_err(|_| PluginApiErrorCode::CleanupIncomplete)?
        {
            Some(exit_code) => {
                drop(stdin.take());
                drain_cancelled(&mut commands);
                let _ = events.emit(PluginApiResourceEventKind::ProcessExited {
                    exit_code: Some(i64::from(exit_code)),
                });
                return Ok(());
            }
            None => thread::sleep(PROCESS_POLL_INTERVAL),
        }
    }
}

#[cfg(unix)]
fn unix_termination_policy() -> crate::process_control::TerminationPolicy {
    crate::process_control::TerminationPolicy::with_timeouts(
        PROCESS_TERMINATION_POLICY,
        PROCESS_TERMINATION_POLICY,
        PROCESS_TERMINATION_POLICY,
    )
}

#[cfg(windows)]
fn windows_termination_policy() -> crate::windows_process_control::TerminationPolicy {
    crate::windows_process_control::TerminationPolicy::with_timeouts(
        PROCESS_TERMINATION_POLICY,
        PROCESS_TERMINATION_POLICY,
        PROCESS_TERMINATION_POLICY,
    )
}

fn spawn_reader<R>(
    mut reader: R,
    stream: PluginProcessOutputStream,
    events: ResourceEventWriter,
    mut cancel: watch::Receiver<bool>,
    fence: ResourceFence,
) -> thread::JoinHandle<()>
where
    R: io::Read + Send + 'static,
{
    let runtime = tokio::runtime::Handle::current();
    thread::spawn(move || {
        let mut buffer = [0_u8; MAX_PROCESS_STDIN_BYTES];
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => count,
            };
            let event = PluginApiResourceEventKind::ProcessOutput {
                stream,
                data_base64: BASE64.encode(&buffer[..count]),
            };
            if runtime
                .block_on(events.emit_backpressured(event, &mut cancel, fence.as_ref()))
                .is_err()
            {
                return;
            }
        }
    })
}

fn join_readers(
    stdout: thread::JoinHandle<()>,
    stderr: thread::JoinHandle<()>,
) -> Result<(), PluginApiErrorCode> {
    stdout
        .join()
        .map_err(|_| PluginApiErrorCode::CleanupIncomplete)?;
    stderr
        .join()
        .map_err(|_| PluginApiErrorCode::CleanupIncomplete)
}

fn drain_cancelled(commands: &mut ResourceCommandReceiver) {
    while let Some(command) = commands.try_recv() {
        command.finish(Err(PluginApiErrorCode::Cancelled));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        ffi::OsString,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use norishell_core_api::{
        FrozenPluginProcessPlan, PluginApiResourceEventKind, PluginId, PluginProcessSendRequest,
        WireSequence,
    };

    use super::{ProcessDriver, ResourceFence, ResourceOwner, ResourceRegistry};

    fn owner() -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse("org.norishell.process-test").unwrap(),
            signer: "test-signer".into(),
            package: "test-package".into(),
            generation: WireSequence::new(1),
        }
    }

    fn plan(program: &str, arguments: &[&str]) -> FrozenPluginProcessPlan {
        FrozenPluginProcessPlan::new(
            PathBuf::from(program),
            arguments.iter().map(OsString::from).collect(),
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/".into()),
            5_000,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn cat_stdin_is_acknowledged_and_stdout_and_exit_are_distinct() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        let handle = ProcessDriver::start(
            &registry,
            owner.clone(),
            plan("/bin/cat", &[]),
            fence.clone(),
        )
        .unwrap();
        ProcessDriver::send(
            &registry,
            &owner,
            PluginProcessSendRequest {
                handle: handle.clone(),
                data_base64: BASE64.encode(b"process-test\n"),
                close_stdin: false,
            },
            &fence,
        )
        .await
        .expect("write acknowledgement");
        let mut output_seen = false;
        for _ in 0..50 {
            let (events, _) = registry.take_events(&owner, &handle, 32).unwrap();
            output_seen |= events.iter().any(|event| matches!(
                &event.kind,
                PluginApiResourceEventKind::ProcessOutput { stream: norishell_core_api::PluginProcessOutputStream::Stdout, data_base64 }
                    if BASE64.decode(data_base64).ok().as_deref() == Some(b"process-test\n")
            ));
            if output_seen {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(output_seen);
        ProcessDriver::send(
            &registry,
            &owner,
            PluginProcessSendRequest {
                handle: handle.clone(),
                data_base64: String::new(),
                close_stdin: true,
            },
            &fence,
        )
        .await
        .expect("EOF acknowledgement");
        assert!(
            ProcessDriver::send(
                &registry,
                &owner,
                PluginProcessSendRequest {
                    handle: handle.clone(),
                    data_base64: BASE64.encode(b"after-eof"),
                    close_stdin: false,
                },
                &fence,
            )
            .await
            .is_err()
        );
        let mut exited = false;
        for _ in 0..50 {
            let (events, _) = registry.take_events(&owner, &handle, 32).unwrap();
            exited |= events.iter().any(|event| {
                matches!(
                    event.kind,
                    PluginApiResourceEventKind::ProcessExited { exit_code: Some(0) }
                )
            });
            if exited {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(exited, "cat exits only after the explicit EOF frame");
        registry
            .close(&owner, &handle)
            .await
            .expect("complete cleanup");
    }

    #[tokio::test]
    async fn stderr_and_normal_exit_have_distinct_events() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        // awk is an exact test executable; its program text is argv data, never shell input.
        let handle = ProcessDriver::start(
            &registry,
            owner.clone(),
            plan(
                "/usr/bin/awk",
                &["BEGIN { print \"process-stderr\" > \"/dev/stderr\"; exit 7 }"],
            ),
            fence,
        )
        .unwrap();
        let mut stderr_seen = false;
        let mut exit_seen = false;
        for _ in 0..100 {
            let (events, _) = registry.take_events(&owner, &handle, 32).unwrap();
            stderr_seen |= events.iter().any(|event| matches!(
                &event.kind,
                PluginApiResourceEventKind::ProcessOutput { stream: norishell_core_api::PluginProcessOutputStream::Stderr, data_base64 }
                    if BASE64.decode(data_base64).ok().as_deref() == Some(b"process-stderr\n")
            ));
            exit_seen |= events.iter().any(|event| {
                matches!(
                    event.kind,
                    PluginApiResourceEventKind::ProcessExited { exit_code: Some(7) }
                )
            });
            if stderr_seen && exit_seen {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(stderr_seen && exit_seen);
        registry.close(&owner, &handle).await.unwrap();
    }

    #[tokio::test]
    async fn fence_revocation_cancels_the_whole_managed_process_group() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let allowed = Arc::new(AtomicBool::new(true));
        let fence: ResourceFence = {
            let allowed = allowed.clone();
            Arc::new(move || allowed.load(Ordering::Acquire))
        };
        let handle = ProcessDriver::start(
            &registry,
            owner.clone(),
            plan("/bin/cat", &[]),
            fence.clone(),
        )
        .unwrap();
        // Let the registered driver own and isolate its process group before revocation.
        tokio::time::sleep(Duration::from_millis(50)).await;
        allowed.store(false, Ordering::Release);
        let mut cancelled = false;
        for _ in 0..100 {
            let (events, _) = registry.take_events(&owner, &handle, 32).unwrap();
            cancelled |= events
                .iter()
                .any(|event| matches!(event.kind, PluginApiResourceEventKind::Cancelled {}));
            if cancelled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(cancelled);
        registry.close(&owner, &handle).await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_reaps_a_ready_child_process_tree() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        // This fixed test executable forks before it writes ready; no user shell is launched and
        // the driver still receives only an exact executable plus argv.
        let handle = ProcessDriver::start(
            &registry,
            owner.clone(),
            plan(
                "/usr/bin/perl",
                &[
                    "-e",
                    "$|=1; my $child=fork(); exit 1 if !defined $child; if (!$child) { sleep 30; exit 0; } print \"ready\\n\"; sleep 30;",
                ],
            ),
            fence,
        )
        .unwrap();
        let mut ready = false;
        for _ in 0..100 {
            let (events, _) = registry.take_events(&owner, &handle, 32).unwrap();
            ready |= events.iter().any(|event| matches!(
                &event.kind,
                PluginApiResourceEventKind::ProcessOutput { stream: norishell_core_api::PluginProcessOutputStream::Stdout, data_base64 }
                    if BASE64.decode(data_base64).ok().as_deref() == Some(b"ready\n")
            ));
            if ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ready, "child must exist before cleanup begins");
        registry
            .close(&owner, &handle)
            .await
            .expect("complete child-tree cleanup");
    }

    #[tokio::test]
    async fn owner_and_fence_are_rechecked_before_process_io() {
        let registry = ResourceRegistry::default();
        let owner = owner();
        let fence: ResourceFence = Arc::new(|| true);
        let handle = ProcessDriver::start(
            &registry,
            owner.clone(),
            plan("/bin/cat", &[]),
            fence.clone(),
        )
        .unwrap();
        let other = ResourceOwner {
            generation: WireSequence::new(2),
            ..owner.clone()
        };
        assert!(
            ProcessDriver::send(
                &registry,
                &other,
                PluginProcessSendRequest {
                    handle: handle.clone(),
                    data_base64: BASE64.encode(b"nope"),
                    close_stdin: false,
                },
                &fence,
            )
            .await
            .is_err()
        );
        assert!(
            ProcessDriver::start(
                &registry,
                owner.clone(),
                plan("/bin/cat", &[]),
                Arc::new(|| false),
            )
            .is_err()
        );
        registry.close(&owner, &handle).await.unwrap();
    }
}
