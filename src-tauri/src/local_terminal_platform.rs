//! Platform-owned local pseudo-terminal process.
//!
//! M1 intentionally exposes no launch customization: the current account's
//! default interactive shell always starts in that account's real home
//! directory, with the platform PTY owning its raw byte stream and process
//! tree.

#[cfg(test)]
use std::time::Duration;
use std::{io, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalPtySpawnFailure {
    AccountLookupFailed,
    InvalidDefaultShell,
    InvalidHomeDirectory,
    PtySpawnFailed,
}

#[derive(Debug)]
pub(crate) struct LocalPtySpawnError {
    pub(crate) failure: LocalPtySpawnFailure,
    pub(crate) source: io::Error,
}

impl LocalPtySpawnError {
    fn new(failure: LocalPtySpawnFailure, source: io::Error) -> Self {
        Self { failure, source }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalPtyMetadata {
    pub(crate) process_id: u32,
    pub(crate) shell_path: PathBuf,
    pub(crate) shell_name: String,
    pub(crate) home_directory: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalPtyExit {
    /// Unix exit codes and the full unsigned Windows exit-code range both fit
    /// without a lossy cast.
    pub(crate) code: Option<i64>,
    pub(crate) signal: Option<i32>,
}

#[cfg(test)]
const POLL_INTERVAL: Duration = Duration::from_millis(20);

fn checked_dimensions(rows: u16, cols: u16) -> io::Result<()> {
    if rows == 0 || cols == 0 {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local PTY dimensions must be non-zero",
        ))
    } else {
        Ok(())
    }
}

fn shell_name(shell_path: &std::path::Path) -> io::Result<String> {
    shell_path
        .file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::other("default shell path has no file name"))
}

#[cfg(unix)]
mod platform {
    use std::{
        ffi::CStr,
        fs::File,
        io::{self, Write},
        mem::MaybeUninit,
        os::unix::{
            ffi::OsStrExt,
            io::{AsRawFd, FromRawFd},
            process::{CommandExt, ExitStatusExt},
        },
        path::PathBuf,
        process::{Command, ExitStatus, Stdio},
        ptr,
        time::Duration,
    };

    #[cfg(test)]
    use std::time::Instant;

    use crate::process_control::{ManagedProcessGroup, TerminationPolicy};

    use super::{
        LocalPtyExit, LocalPtyMetadata, LocalPtySpawnError, LocalPtySpawnFailure,
        checked_dimensions, shell_name,
    };

    #[cfg(test)]
    use super::POLL_INTERVAL;

    const TERMINATION_POLICY: TerminationPolicy = TerminationPolicy::with_timeouts(
        Duration::from_millis(500),
        Duration::from_millis(500),
        Duration::from_secs(1),
    );
    const LOCAL_TERMINAL_TYPE: &str = "xterm-256color";
    const LOCAL_TERMINAL_COLOR_MODE: &str = "truecolor";

    pub(crate) struct LocalPtyProcess {
        process: ManagedProcessGroup,
        master: File,
        exit: Option<LocalPtyExit>,
    }

    pub(crate) struct LocalPtyWriter {
        master: File,
    }

    impl LocalPtyWriter {
        pub(crate) fn write_input(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.master.write_all(bytes)?;
            self.master.flush()
        }
    }

    impl LocalPtyProcess {
        pub(crate) fn spawn(
            rows: u16,
            cols: u16,
        ) -> Result<(LocalPtyMetadata, Self), LocalPtySpawnError> {
            checked_dimensions(rows, cols).map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::PtySpawnFailed, error)
            })?;
            let account = current_account()?;
            let shell = account.shell;
            let home = account.home;
            let shell_name = shell_name(&shell).map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::InvalidDefaultShell, error)
            })?;
            let (master, slave) = open_pty(rows, cols).map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::PtySpawnFailed, error)
            })?;

            let slave_stdout = slave.try_clone().map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::PtySpawnFailed, error)
            })?;
            let slave_stderr = slave.try_clone().map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::PtySpawnFailed, error)
            })?;
            let mut command = Command::new(&shell);
            command
                .arg("-i")
                .current_dir(&home)
                // The desktop process may itself start with TERM=dumb.  The
                // child owns a real PTY, so advertise its actual xterm
                // compatibility instead of inheriting that non-terminal hint.
                .env("TERM", LOCAL_TERMINAL_TYPE)
                .env("COLORTERM", LOCAL_TERMINAL_COLOR_MODE)
                .env("TERM_PROGRAM", "NoriShell")
                // The host may be launched by an automation environment that
                // sets this process-wide opt-out. A user-facing PTY must not
                // pass that host-only rendering decision to interactive CLIs.
                .env_remove("NO_COLOR")
                .stdin(Stdio::from(slave))
                .stdout(Stdio::from(slave_stdout))
                .stderr(Stdio::from(slave_stderr));

            // SAFETY: only async-signal-safe libc calls run between fork and
            // exec. The standard library has already installed fd 0 from the
            // supplied slave PTY before invoking this hook.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }

            // `spawn_session_leader` verifies that the setsid-created process
            // group is the child PID without also applying process_group(0).
            let process =
                ManagedProcessGroup::spawn_session_leader(&mut command).map_err(|error| {
                    LocalPtySpawnError::new(LocalPtySpawnFailure::PtySpawnFailed, error)
                })?;
            let metadata = LocalPtyMetadata {
                process_id: process.process_id(),
                shell_path: shell,
                shell_name,
                home_directory: home,
            };
            Ok((
                metadata,
                Self {
                    process,
                    master,
                    exit: None,
                },
            ))
        }

        pub(crate) fn try_clone_reader(&self) -> io::Result<File> {
            self.master.try_clone()
        }

        pub(crate) fn try_clone_writer(&self) -> io::Result<LocalPtyWriter> {
            Ok(LocalPtyWriter {
                master: self.master.try_clone()?,
            })
        }

        pub(crate) fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
            checked_dimensions(rows, cols)?;
            if self.exit.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "local PTY process has exited",
                ));
            }
            let size = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // SAFETY: master is a live PTY descriptor and size points to a
            // fully initialized winsize for the duration of the call.
            if unsafe {
                libc::ioctl(
                    self.master.as_raw_fd(),
                    libc::TIOCSWINSZ as libc::c_ulong,
                    &raw const size,
                )
            } < 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub(crate) fn poll_exit(&mut self) -> io::Result<Option<LocalPtyExit>> {
            if let Some(exit) = self.exit {
                return Ok(Some(exit));
            }
            let Some(root_status) = self.process.poll_root_exit()? else {
                return Ok(None);
            };

            // A shell may exit after starting a background process. Do not
            // publish the root status until the complete process group has
            // been actively reaped.
            if self.process.wait_for_exit(Duration::ZERO)?.is_none() {
                self.process.terminate_checked(TERMINATION_POLICY)?;
            }
            if self.process.wait_for_exit(Duration::ZERO)?.is_none() {
                return Err(io::Error::other(
                    "local PTY process tree did not stop after root exit",
                ));
            }
            let exit = unix_exit(root_status);
            self.exit = Some(exit);
            Ok(Some(exit))
        }

        #[cfg(test)]
        pub(crate) fn wait_for_exit(
            &mut self,
            timeout: Duration,
        ) -> io::Result<Option<LocalPtyExit>> {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(exit) = self.poll_exit()? {
                    return Ok(Some(exit));
                }
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(
                    POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }

        pub(crate) fn terminate(&mut self) -> io::Result<LocalPtyExit> {
            if let Some(exit) = self.poll_exit()? {
                return Ok(exit);
            }
            self.process.terminate_checked(TERMINATION_POLICY)?;
            let status = self
                .process
                .wait_for_exit(Duration::ZERO)?
                .ok_or_else(|| io::Error::other("local PTY process tree did not terminate"))?;
            let exit = unix_exit(status);
            self.exit = Some(exit);
            Ok(exit)
        }
    }

    fn unix_exit(status: ExitStatus) -> LocalPtyExit {
        LocalPtyExit {
            code: status.code().map(i64::from),
            signal: status.signal(),
        }
    }

    struct CurrentAccount {
        shell: PathBuf,
        home: PathBuf,
    }

    fn current_account() -> Result<CurrentAccount, LocalPtySpawnError> {
        let initial_size = match unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) } {
            value if value > 0 => usize::try_from(value).unwrap_or(16 * 1024),
            _ => 16 * 1024,
        };
        let mut buffer_size = initial_size.clamp(1024, 1024 * 1024);
        loop {
            let mut entry = MaybeUninit::<libc::passwd>::zeroed();
            let mut result = ptr::null_mut();
            let mut buffer = vec![0_u8; buffer_size];
            // SAFETY: entry, result, and buffer are valid writable storage;
            // getpwuid_r writes pointers only into buffer, which remains live
            // while both required path values are copied below.
            let status = unsafe {
                libc::getpwuid_r(
                    libc::geteuid(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &raw mut result,
                )
            };
            if status == libc::ERANGE && buffer_size < 1024 * 1024 {
                buffer_size = (buffer_size * 2).min(1024 * 1024);
                continue;
            }
            if status != 0 {
                return Err(LocalPtySpawnError::new(
                    LocalPtySpawnFailure::AccountLookupFailed,
                    io::Error::from_raw_os_error(status),
                ));
            }
            if result.is_null() {
                return Err(LocalPtySpawnError::new(
                    LocalPtySpawnFailure::AccountLookupFailed,
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "current Unix account was not found",
                    ),
                ));
            }
            // SAFETY: a successful non-null result initialized the passwd
            // entry, whose string pointers remain backed by buffer.
            let entry = unsafe { entry.assume_init() };
            let shell = passwd_path(entry.pw_shell, "default shell").map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::InvalidDefaultShell, error)
            })?;
            let home = passwd_path(entry.pw_dir, "home directory").map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::InvalidHomeDirectory, error)
            })?;
            validate_account_paths(&shell, &home)?;
            return Ok(CurrentAccount { shell, home });
        }
    }

    fn passwd_path(value: *const libc::c_char, label: &str) -> io::Result<PathBuf> {
        if value.is_null() {
            return Err(io::Error::other(format!("current account has no {label}")));
        }
        // SAFETY: getpwuid_r returned a NUL-terminated string in its live
        // caller-owned buffer.
        let bytes = unsafe { CStr::from_ptr(value) }.to_bytes();
        if bytes.is_empty() {
            return Err(io::Error::other(format!("current account has no {label}")));
        }
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }

    fn validate_account_paths(
        shell: &std::path::Path,
        home: &std::path::Path,
    ) -> Result<(), LocalPtySpawnError> {
        if !shell.is_absolute() || !shell.is_file() {
            return Err(LocalPtySpawnError::new(
                LocalPtySpawnFailure::InvalidDefaultShell,
                io::Error::other("current account default shell is not an existing absolute file"),
            ));
        }
        if !home.is_absolute() || !home.is_dir() {
            return Err(LocalPtySpawnError::new(
                LocalPtySpawnFailure::InvalidHomeDirectory,
                io::Error::other("current account home is not an existing absolute directory"),
            ));
        }
        Ok(())
    }

    fn open_pty(rows: u16, cols: u16) -> io::Result<(File, File)> {
        let mut size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut master_fd = -1;
        let mut slave_fd = -1;
        // SAFETY: both descriptor outputs are writable, the optional name and
        // termios are null, and size remains live for the call.
        if unsafe {
            libc::openpty(
                &raw mut master_fd,
                &raw mut slave_fd,
                ptr::null_mut(),
                ptr::null_mut(),
                &raw mut size,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openpty transfers unique ownership of both file
        // descriptors to the caller.
        let master = unsafe { File::from_raw_fd(master_fd) };
        let slave = unsafe { File::from_raw_fd(slave_fd) };
        set_close_on_exec(&master)?;
        set_close_on_exec(&slave)?;
        Ok((master, slave))
    }

    fn set_close_on_exec(file: &File) -> io::Result<()> {
        // SAFETY: file owns a live descriptor for both fcntl calls.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::{env, ffi::OsString, fs::File, io, path::PathBuf, time::Duration};

    #[cfg(test)]
    use std::time::Instant;

    use crate::windows_process_control::{ManagedConPtyProcess, TerminationPolicy};

    use super::{
        LocalPtyExit, LocalPtyMetadata, LocalPtySpawnError, LocalPtySpawnFailure,
        checked_dimensions, shell_name,
    };

    #[cfg(test)]
    use super::POLL_INTERVAL;

    const TERMINATION_POLICY: TerminationPolicy = TerminationPolicy::with_timeouts(
        Duration::from_millis(500),
        Duration::from_millis(500),
        Duration::from_secs(1),
    );

    pub(crate) struct LocalPtyProcess {
        process: ManagedConPtyProcess,
        exit: Option<LocalPtyExit>,
    }

    pub(crate) struct LocalPtyWriter {
        input: File,
    }

    impl LocalPtyWriter {
        pub(crate) fn write_input(&mut self, bytes: &[u8]) -> io::Result<()> {
            use std::io::Write;

            self.input.write_all(bytes)?;
            self.input.flush()
        }
    }

    impl LocalPtyProcess {
        pub(crate) fn spawn(
            rows: u16,
            cols: u16,
        ) -> Result<(LocalPtyMetadata, Self), LocalPtySpawnError> {
            checked_dimensions(rows, cols).map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::PtySpawnFailed, error)
            })?;
            let shell = required_absolute_file_env("COMSPEC", "default Windows shell").map_err(
                |error| LocalPtySpawnError::new(LocalPtySpawnFailure::InvalidDefaultShell, error),
            )?;
            let home = required_absolute_directory_env("USERPROFILE", "current user profile")
                .map_err(|error| {
                    LocalPtySpawnError::new(LocalPtySpawnFailure::InvalidHomeDirectory, error)
                })?;
            let shell_name = shell_name(&shell).map_err(|error| {
                LocalPtySpawnError::new(LocalPtySpawnFailure::InvalidDefaultShell, error)
            })?;
            let arguments: Vec<OsString> = Vec::new();
            let process = ManagedConPtyProcess::spawn(&shell, &arguments, Some(&home), rows, cols)
                .map_err(|error| {
                    LocalPtySpawnError::new(LocalPtySpawnFailure::PtySpawnFailed, error)
                })?;
            let metadata = LocalPtyMetadata {
                process_id: process.process_id(),
                shell_path: shell,
                shell_name,
                home_directory: home,
            };
            Ok((
                metadata,
                Self {
                    process,
                    exit: None,
                },
            ))
        }

        pub(crate) fn try_clone_reader(&self) -> io::Result<File> {
            self.process.try_clone_reader()
        }

        pub(crate) fn try_clone_writer(&self) -> io::Result<LocalPtyWriter> {
            Ok(LocalPtyWriter {
                input: self.process.try_clone_input_writer()?,
            })
        }

        pub(crate) fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
            checked_dimensions(rows, cols)?;
            if self.exit.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "local PTY process has exited",
                ));
            }
            self.process.resize(rows, cols)
        }

        pub(crate) fn poll_exit(&mut self) -> io::Result<Option<LocalPtyExit>> {
            if let Some(exit) = self.exit {
                return Ok(Some(exit));
            }
            let Some(root_code) = self.process.poll_root_exit()? else {
                return Ok(None);
            };
            if self.process.wait_for_exit(Duration::ZERO)?.is_none() {
                self.process
                    .terminate_without_input_checked(TERMINATION_POLICY)?;
            }
            if self.process.wait_for_exit(Duration::ZERO)?.is_none() {
                return Err(io::Error::other(
                    "local ConPTY process tree did not stop after root exit",
                ));
            }
            let exit = LocalPtyExit {
                code: Some(i64::from(root_code)),
                signal: None,
            };
            self.exit = Some(exit);
            Ok(Some(exit))
        }

        #[cfg(test)]
        pub(crate) fn wait_for_exit(
            &mut self,
            timeout: Duration,
        ) -> io::Result<Option<LocalPtyExit>> {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(exit) = self.poll_exit()? {
                    return Ok(Some(exit));
                }
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(
                    POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }

        pub(crate) fn terminate(&mut self) -> io::Result<LocalPtyExit> {
            if let Some(exit) = self.poll_exit()? {
                return Ok(exit);
            }
            self.process
                .terminate_without_input_checked(TERMINATION_POLICY)?;
            let code = self
                .process
                .wait_for_exit(Duration::ZERO)?
                .ok_or_else(|| io::Error::other("local ConPTY process tree did not terminate"))?;
            let exit = LocalPtyExit {
                code: Some(i64::from(code)),
                signal: None,
            };
            self.exit = Some(exit);
            Ok(exit)
        }
    }

    fn required_absolute_file_env(name: &str, label: &str) -> io::Result<PathBuf> {
        let path = required_env_path(name, label)?;
        if !path.is_file() {
            return Err(io::Error::other(format!("{label} is not an existing file")));
        }
        Ok(path)
    }

    fn required_absolute_directory_env(name: &str, label: &str) -> io::Result<PathBuf> {
        let path = required_env_path(name, label)?;
        if !path.is_dir() {
            return Err(io::Error::other(format!(
                "{label} is not an existing directory"
            )));
        }
        Ok(path)
    }

    fn required_env_path(name: &str, label: &str) -> io::Result<PathBuf> {
        let value = env::var_os(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| io::Error::other(format!("{label} is unavailable")))?;
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(io::Error::other(format!("{label} is not absolute")));
        }
        Ok(path)
    }
}

pub(crate) use platform::{LocalPtyProcess, LocalPtyWriter};

#[cfg(all(test, unix))]
mod tests {
    use std::{
        io::{self, Read},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use super::{LocalPtyExit, LocalPtyProcess};

    fn write_input(process: &LocalPtyProcess, bytes: &[u8]) -> io::Result<()> {
        process.try_clone_writer()?.write_input(bytes)
    }

    #[test]
    fn default_shell_starts_in_real_home_and_preserves_terminal_bytes() {
        let (metadata, mut process) = LocalPtyProcess::spawn(24, 80).expect("spawn local PTY");
        assert!(metadata.shell_path.is_absolute());
        assert!(metadata.shell_path.is_file());
        assert!(metadata.home_directory.is_absolute());
        assert!(metadata.home_directory.is_dir());
        assert_eq!(
            metadata.shell_name,
            metadata
                .shell_path
                .file_name()
                .expect("shell file name")
                .to_string_lossy()
        );
        assert_ne!(metadata.process_id, 0);

        let output = collect_output(&process);
        write_input(
            &process,
            b"printf '__NVX_PWD__:%s\\n' \"$PWD\"; printf '__NVX_TERM__:%s|%s|%s|%s\\n' \"$TERM\" \"$COLORTERM\" \"$TERM_PROGRAM\" \"${NO_COLOR-unset}\"; printf '\\033[31m__NVX_ANSI__\\033[0m\\n'; printf '__NVX_UTF8__:\\344\\270\\255\\346\\226\\207\\n'; exit 0\r",
        )
        .expect("write shell commands");
        assert_eq!(
            process
                .wait_for_exit(Duration::from_secs(8))
                .expect("wait for shell"),
            Some(LocalPtyExit {
                code: Some(0),
                signal: None,
            })
        );
        drop(process);
        let output = output.finish();
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains(&format!(
                "__NVX_PWD__:{}",
                metadata.home_directory.to_string_lossy()
            )),
            "{text}"
        );
        assert!(
            text.contains("__NVX_TERM__:xterm-256color|truecolor|NoriShell|unset"),
            "{text}"
        );
        assert!(
            output
                .windows(b"\x1b[31m__NVX_ANSI__\x1b[0m".len())
                .any(|window| window == b"\x1b[31m__NVX_ANSI__\x1b[0m")
        );
        assert!(text.contains("__NVX_UTF8__:中文"), "{text}");
    }

    #[test]
    fn raw_input_round_trips_without_utf8_coercion() {
        let (_, mut process) = LocalPtyProcess::spawn(24, 80).expect("spawn local PTY");
        let output = collect_output(&process);
        write_input(&process, b"stty raw -echo; printf '__NVX_RAW_READY__'; dd bs=1 count=4 2>/dev/null | od -An -t x1; stty sane; exit 0\r")
            .expect("configure raw reader");
        thread::sleep(Duration::from_millis(500));
        write_input(&process, &[0x00, 0xff, b'A', b'\n']).expect("write raw bytes");
        assert_eq!(
            process
                .wait_for_exit(Duration::from_secs(8))
                .expect("wait for shell"),
            Some(LocalPtyExit {
                code: Some(0),
                signal: None,
            })
        );
        drop(process);
        let output = output.finish();
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("__NVX_RAW_READY__"), "{text}");
        let tokens = text.split_ascii_whitespace().collect::<Vec<_>>();
        assert!(
            tokens
                .windows(4)
                .any(|values| values == ["00", "ff", "41", "0a"]),
            "{text}"
        );
    }

    #[test]
    fn resize_updates_the_live_slave_dimensions() {
        let (_, mut process) = LocalPtyProcess::spawn(24, 80).expect("spawn local PTY");
        let output = collect_output(&process);
        process.resize(41, 101).expect("resize PTY");
        write_input(&process, b"printf '__NVX_SIZE__:'; stty size; exit 0\r")
            .expect("request terminal size");
        assert!(
            process
                .wait_for_exit(Duration::from_secs(8))
                .expect("wait for shell")
                .is_some()
        );
        drop(process);
        let text = String::from_utf8_lossy(&output.finish()).into_owned();
        assert!(text.contains("__NVX_SIZE__:41 101"), "{text}");
    }

    #[test]
    fn normal_and_nonzero_exit_are_structured() {
        assert_eq!(run_exit(0).code, Some(0));
        assert_eq!(run_exit(7).code, Some(7));
    }

    #[test]
    fn exited_root_reaps_its_remaining_process_group_before_reporting() {
        let (_, mut process) = LocalPtyProcess::spawn(24, 80).expect("spawn local PTY");
        let output = collect_output(&process);
        write_input(
            &process,
            b"set +m; nohup sleep 30 >/dev/null 2>&1 & printf '__NVX_CHILD__:%s\\n' $!; exec /usr/bin/true\r",
        )
        .expect("start background child and exit root");
        assert_eq!(
            process
                .wait_for_exit(Duration::from_secs(8))
                .expect("wait for process tree cleanup"),
            Some(LocalPtyExit {
                code: Some(0),
                signal: None,
            })
        );
        drop(process);
        let text = String::from_utf8_lossy(&output.finish()).into_owned();
        let child = parse_child_pid(&text);
        assert!(!process_exists(child), "background child {child} survived");
    }

    #[test]
    fn terminate_is_idempotent_and_drop_guarded() {
        let (_, mut process) = LocalPtyProcess::spawn(24, 80).expect("spawn local PTY");
        let first = process.terminate().expect("terminate local PTY");
        let second = process.terminate().expect("repeat termination");
        assert_eq!(second, first);
        assert!(first.code.is_some() || first.signal.is_some());
        assert!(write_input(&process, b"rejected").is_err());
        assert!(process.resize(25, 81).is_err());

        let (metadata, process) = LocalPtyProcess::spawn(24, 80).expect("spawn guarded local PTY");
        drop(process);
        assert!(
            !process_exists(i32::try_from(metadata.process_id).expect("Unix PID")),
            "Drop left root process {} alive",
            metadata.process_id
        );
    }

    #[test]
    fn zero_dimensions_are_rejected() {
        assert!(LocalPtyProcess::spawn(0, 80).is_err());
        assert!(LocalPtyProcess::spawn(24, 0).is_err());
    }

    fn run_exit(code: i32) -> LocalPtyExit {
        let (_, mut process) = LocalPtyProcess::spawn(24, 80).expect("spawn local PTY");
        let output = collect_output(&process);
        write_input(&process, format!("exit {code}\r").as_bytes()).expect("write exit");
        let exit = process
            .wait_for_exit(Duration::from_secs(8))
            .expect("wait for exit");
        drop(process);
        let bytes = output.finish();
        exit.unwrap_or_else(|| panic!("shell did not exit: {}", String::from_utf8_lossy(&bytes)))
    }

    struct CollectedOutput {
        receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>,
        thread: thread::JoinHandle<()>,
    }

    impl CollectedOutput {
        fn finish(self) -> Vec<u8> {
            let output = self
                .receiver
                .recv_timeout(Duration::from_secs(3))
                .expect("bounded PTY reader")
                .expect("read PTY output");
            self.thread.join().expect("join PTY reader");
            output
        }
    }

    fn collect_output(process: &LocalPtyProcess) -> CollectedOutput {
        let mut reader = process.try_clone_reader().expect("clone PTY reader");
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut output = Vec::new();
            let result = reader.read_to_end(&mut output).map(|_| output);
            let _ = sender.send(result);
        });
        CollectedOutput { receiver, thread }
    }

    fn parse_child_pid(output: &str) -> i32 {
        output
            .rsplit("__NVX_CHILD__:")
            .next()
            .and_then(|tail| {
                tail.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .ok()
            })
            .unwrap_or_else(|| panic!("missing child pid in {output:?}"))
    }

    fn process_exists(process_id: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // SAFETY: signal 0 performs a read-only existence probe.
            let result = unsafe { libc::kill(process_id, 0) };
            if result < 0 && io_error_is_esrch() {
                return false;
            }
            if Instant::now() >= deadline {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn io_error_is_esrch() -> bool {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}
