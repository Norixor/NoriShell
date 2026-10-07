#[cfg(not(target_os = "android"))]
use std::path::Path;
#[cfg(all(unix, not(target_os = "android")))]
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
#[cfg(windows)]
use std::{ffi::OsString, fs::File};
use std::{io, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use norishell_core_api::{PluginHostRequest, PluginRuntimeOutput};
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "android"))]
use tempfile::TempDir;

use crate::plugin_host_protocol::{
    ChildFrame, PLUGIN_HOST_FRAME_MAX_BYTES, PLUGIN_HOST_PROTOCOL_MAJOR,
    PLUGIN_HOST_PROTOCOL_MINOR, ParentFrame, read_frame_with_timeout, write_frame,
};
#[cfg(all(unix, not(target_os = "android")))]
use crate::process_control::TerminationOutcome;
#[cfg(all(unix, not(target_os = "android")))]
use crate::process_control::{ManagedProcessGroup, TerminationPolicy};
#[cfg(windows)]
use crate::windows_process_control::{ManagedJobProcess, TerminationOutcome, TerminationPolicy};

const PLUGIN_HOST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, thiserror::Error)]
pub(crate) enum PluginHostProcessError {
    #[error("plugin host is unavailable on this platform")]
    #[allow(dead_code)]
    UnsupportedPlatform,
    #[error("plugin host process could not be started")]
    Spawn,
    #[error("plugin host protocol failed")]
    Protocol,
    #[error("plugin host rejected the request")]
    Rejected,
    #[error("plugin host exceeded its response deadline")]
    TimedOut,
    #[error("plugin host process did not stop cleanly")]
    Cleanup,
}

#[cfg(all(unix, not(target_os = "android")))]
type PluginHostOwnedProcess = ManagedProcessGroup;
#[cfg(windows)]
type PluginHostOwnedProcess = ManagedJobProcess;
#[cfg(target_os = "android")]
type PluginHostOwnedProcess = android_host::IsolatedHostProcess;
#[cfg(all(unix, not(target_os = "android")))]
type PluginHostInput = ChildStdin;
#[cfg(windows)]
type PluginHostInput = File;
#[cfg(target_os = "android")]
type PluginHostInput = android_host::HostSocket;
#[cfg(all(unix, not(target_os = "android")))]
type PluginHostOutput = ChildStdout;
#[cfg(windows)]
type PluginHostOutput = File;
#[cfg(target_os = "android")]
type PluginHostOutput = android_host::HostSocket;

#[cfg(any(unix, windows))]
pub(crate) struct PluginHostProcess {
    process: PluginHostOwnedProcess,
    input: PluginHostInput,
    output: PluginHostOutput,
    instance_generation: u64,
    nonce: String,
    next_sequence: u64,
    guest_protocol_major: u16,
    guest_protocol_minor: u16,
    #[cfg(not(target_os = "android"))]
    _working_directory: TempDir,
}

#[cfg(any(unix, windows))]
impl PluginHostProcess {
    pub(crate) fn spawn(
        module_bytes: &[u8],
        instance_generation: u64,
        guest_protocol_major: u16,
        guest_protocol_minor: u16,
    ) -> Result<Self, PluginHostProcessError> {
        if !norishell_core_api::plugin_protocol_is_compatible(
            guest_protocol_major,
            guest_protocol_minor,
        ) {
            return Err(PluginHostProcessError::Rejected);
        }
        if instance_generation == 0 || module_bytes.is_empty() {
            return Err(PluginHostProcessError::Spawn);
        }
        #[cfg(target_os = "android")]
        {
            // The isolated `PluginHostService` process owns the other socket end;
            // `current_exe()` is the zygote-forked app process and cannot be exec'd.
            let (process, channel) = android_host::IsolatedHostProcess::start()?;
            let output = channel
                .try_clone()
                .map_err(|_| PluginHostProcessError::Spawn)?;
            Self {
                process,
                input: channel,
                output,
                instance_generation,
                nonce: uuid::Uuid::new_v4().to_string(),
                next_sequence: 1,
                guest_protocol_major,
                guest_protocol_minor,
            }
            .initialize(module_bytes)
        }
        #[cfg(not(target_os = "android"))]
        {
            let executable = std::env::current_exe().map_err(|_| PluginHostProcessError::Spawn)?;
            Self::spawn_executable(
                &executable,
                module_bytes,
                instance_generation,
                guest_protocol_major,
                guest_protocol_minor,
            )
        }
    }

    #[cfg(all(test, not(target_os = "android")))]
    pub(crate) fn spawn_with_executable_for_tests(
        executable: &Path,
        module_bytes: &[u8],
        instance_generation: u64,
    ) -> Result<Self, PluginHostProcessError> {
        Self::spawn_executable(
            executable,
            module_bytes,
            instance_generation,
            norishell_core_api::PLUGIN_PROTOCOL_MAJOR,
            norishell_core_api::PLUGIN_PROTOCOL_MINOR,
        )
    }

    #[cfg(not(target_os = "android"))]
    fn spawn_executable(
        executable: &Path,
        module_bytes: &[u8],
        instance_generation: u64,
        guest_protocol_major: u16,
        guest_protocol_minor: u16,
    ) -> Result<Self, PluginHostProcessError> {
        let working_directory = tempfile::tempdir().map_err(|_| PluginHostProcessError::Spawn)?;
        #[cfg(all(unix, not(target_os = "android")))]
        let (process, input, output) = {
            let mut command = Command::new(executable);
            command
                .arg("--plugin-host")
                .env_clear()
                .current_dir(working_directory.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            {
                use std::os::unix::process::CommandExt;
                // SAFETY: only async-signal-safe setrlimit calls execute between
                // fork and exec. These are defense-in-depth quotas, not an OS
                // permission sandbox.
                unsafe {
                    command.pre_exec(|| {
                        set_limit(libc::RLIMIT_CORE, 0)?;
                        set_limit(libc::RLIMIT_FSIZE, 0)?;
                        set_limit(libc::RLIMIT_NOFILE, 32)?;
                        Ok(())
                    });
                }
            }
            let mut process = ManagedProcessGroup::spawn(&mut command)
                .map_err(|_| PluginHostProcessError::Spawn)?;
            let input = process.take_stdin().ok_or(PluginHostProcessError::Spawn)?;
            let output = process.take_stdout().ok_or(PluginHostProcessError::Spawn)?;
            (process, input, output)
        };
        #[cfg(windows)]
        let (process, input, output) = {
            let arguments = [OsString::from("--plugin-host")];
            let (process, stdio) = ManagedJobProcess::spawn_with_stdio_empty_environment(
                executable,
                &arguments,
                Some(working_directory.path()),
            )
            .map_err(|_| PluginHostProcessError::Spawn)?;
            drop(stdio.stderr);
            (process, stdio.stdin, stdio.stdout)
        };
        Self {
            process,
            input,
            output,
            instance_generation,
            nonce: uuid::Uuid::new_v4().to_string(),
            next_sequence: 1,
            guest_protocol_major,
            guest_protocol_minor,
            _working_directory: working_directory,
        }
        .initialize(module_bytes)
    }

    /// Initialize/Ready handshake shared by every platform transport. Dropping
    /// `self` on failure releases the child through its platform owner.
    fn initialize(mut self, module_bytes: &[u8]) -> Result<Self, PluginHostProcessError> {
        let host = &mut self;
        let nonce = host.nonce.clone();
        let instance_generation = host.instance_generation;
        let module_sha256 = hex::encode(Sha256::digest(module_bytes));
        write_frame(
            &mut host.input,
            &ParentFrame::Initialize {
                protocol_major: PLUGIN_HOST_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_HOST_PROTOCOL_MINOR,
                instance_generation,
                nonce: nonce.clone(),
                module_sha256: module_sha256.clone(),
                module_base64: STANDARD.encode(module_bytes),
            },
        )
        .map_err(|error| host.channel_error(&error))?;
        let ready: ChildFrame = read_frame_with_timeout(
            &mut host.output,
            PLUGIN_HOST_FRAME_MAX_BYTES,
            PLUGIN_HOST_RESPONSE_TIMEOUT,
        )
        .map_err(|error| host.channel_error(&error))?;
        if ready
            != (ChildFrame::Ready {
                protocol_major: PLUGIN_HOST_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_HOST_PROTOCOL_MINOR,
                instance_generation,
                nonce,
                module_sha256,
            })
        {
            return Err(PluginHostProcessError::Protocol);
        }
        Ok(self)
    }

    /// Maps a channel failure and lets the platform owner remember that the
    /// child side is gone (Android uses it to classify system reclaims).
    fn channel_error(&mut self, error: &io::Error) -> PluginHostProcessError {
        #[cfg(target_os = "android")]
        if matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionReset
        ) {
            self.process.mark_peer_closed();
        }
        if error.kind() == io::ErrorKind::TimedOut {
            PluginHostProcessError::TimedOut
        } else {
            PluginHostProcessError::Protocol
        }
    }

    /// True only when the platform proved the host was reclaimed by the OS
    /// (Android low-memory kill) rather than failing on its own. Such exits
    /// must not escalate a plugin to Quarantined.
    #[cfg(target_os = "android")]
    pub(crate) fn reclaimed_by_system(&mut self) -> bool {
        self.process.reclaimed_by_system()
    }

    pub(crate) fn process_id(&self) -> u32 {
        self.process.process_id()
    }

    pub(crate) fn execute(
        &mut self,
        mut request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginHostProcessError> {
        // Every entry point (actions, callbacks, tasks, protocol events) uses the verified guest contract.
        // Future payload additions must also be projected here for this minor. Host IPC is independent.
        bind_guest_protocol(
            &mut request,
            self.guest_protocol_major,
            self.guest_protocol_minor,
        )?;
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(PluginHostProcessError::Protocol)?;
        write_frame(
            &mut self.input,
            &ParentFrame::Execute {
                instance_generation: self.instance_generation,
                nonce: self.nonce.clone(),
                sequence,
                request,
            },
        )
        .map_err(|error| self.channel_error(&error))?;
        let response: io::Result<ChildFrame> = read_frame_with_timeout(
            &mut self.output,
            PLUGIN_HOST_FRAME_MAX_BYTES,
            PLUGIN_HOST_RESPONSE_TIMEOUT,
        );
        let response = response.map_err(|error| self.channel_error(&error))?;
        match response {
            ChildFrame::Result {
                instance_generation,
                nonce,
                sequence: observed_sequence,
                outputs,
                ..
            } if instance_generation == self.instance_generation
                && nonce == self.nonce
                && observed_sequence == sequence =>
            {
                Ok(outputs)
            }
            ChildFrame::Rejected {
                instance_generation,
                nonce,
                sequence: observed_sequence,
                ..
            } if instance_generation == self.instance_generation
                && nonce == self.nonce
                && observed_sequence == sequence =>
            {
                Err(PluginHostProcessError::Rejected)
            }
            _ => Err(PluginHostProcessError::Protocol),
        }
    }

    /// Stops the complete Plugin Host process group. The caller retains this
    /// owner when cleanup cannot be proven, so it can remain an exit blocker
    /// and retry instead of relying on best-effort `Drop` cleanup.
    pub(crate) fn shutdown(&mut self) -> Result<(), PluginHostProcessError> {
        let sequence = self.next_sequence;
        let graceful = write_frame(
            &mut self.input,
            &ParentFrame::Shutdown {
                instance_generation: self.instance_generation,
                nonce: self.nonce.clone(),
                sequence,
            },
        )
        .map_err(|error| self.channel_error(&error))
        .and_then(|()| {
            let response: io::Result<ChildFrame> = read_frame_with_timeout(
                &mut self.output,
                PLUGIN_HOST_FRAME_MAX_BYTES,
                PLUGIN_HOST_RESPONSE_TIMEOUT,
            );
            let response = response.map_err(|error| self.channel_error(&error))?;
            if matches!(
                response,
                ChildFrame::Stopped {
                    instance_generation,
                    nonce,
                    sequence: observed_sequence,
                } if instance_generation == self.instance_generation
                    && nonce == self.nonce
                    && observed_sequence == sequence
            ) {
                Ok(())
            } else {
                Err(PluginHostProcessError::Protocol)
            }
        });
        #[cfg(target_os = "android")]
        {
            // Unbinding lets the system kill the isolated process even when the
            // graceful exchange failed; only a confirmed death is clean.
            let _ = graceful;
            self.process.stop_and_confirm()
        }
        #[cfg(not(target_os = "android"))]
        self.stop_desktop_process(graceful.is_ok())
    }

    #[cfg(not(target_os = "android"))]
    fn stop_desktop_process(&mut self, graceful: bool) -> Result<(), PluginHostProcessError> {
        let exited_after_graceful = graceful
            && self
                .process
                .wait_for_exit(Duration::from_secs(2))
                .is_ok_and(|status| status.is_some());
        if exited_after_graceful {
            return Ok(());
        }
        #[cfg(unix)]
        let outcome = self.process.terminate(TerminationPolicy::default());
        #[cfg(windows)]
        let outcome = self
            .process
            .terminate_with(TerminationPolicy::default(), |_| Ok(()));
        if matches!(outcome, TerminationOutcome::Unknown) {
            Err(PluginHostProcessError::Cleanup)
        } else {
            Ok(())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "android"))]
type RlimitResource = libc::c_int;
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
type RlimitResource = libc::__rlimit_resource_t;

#[cfg(unix)]
pub(crate) fn set_limit(resource: RlimitResource, value: libc::rlim_t) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if unsafe { libc::setrlimit(resource, &raw const limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Android transport: one `android:isolatedProcess` `PluginHostService` per
/// plugin instance, reached over a socketpair whose peer end Kotlin passes to
/// the service over Binder. The Kotlin side owns binding, death detection and
/// exit classification; this side owns the frame protocol and the bounded
/// confirmation that the process is gone.
#[cfg(target_os = "android")]
pub(crate) mod android_host {
    use std::{
        io::{self, Read, Write},
        os::{
            fd::{AsRawFd, RawFd},
            unix::net::UnixStream,
        },
    };

    use serde::Deserialize;
    use tauri_plugin_norishell_android as bridge;

    use super::PluginHostProcessError;

    /// Exit classification reported by Kotlin for a system low-memory kill.
    const REASON_LOW_MEMORY: &str = "low_memory";

    /// Socket end used by both the parent and the isolated child. Writes use
    /// `MSG_NOSIGNAL` so a dead peer surfaces as `BrokenPipe` instead of a
    /// process-wide SIGPIPE inside ART.
    pub(crate) struct HostSocket(UnixStream);

    impl HostSocket {
        pub(crate) fn new(stream: UnixStream) -> Self {
            Self(stream)
        }

        pub(crate) fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Self)
        }
    }

    impl Read for HostSocket {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            (&self.0).read(buffer)
        }
    }

    impl Write for HostSocket {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            loop {
                // SAFETY: the descriptor is owned by `self.0` and the buffer is
                // valid for `buffer.len()` bytes for the duration of the call.
                let written = unsafe {
                    libc::send(
                        self.0.as_raw_fd(),
                        buffer.as_ptr().cast(),
                        buffer.len(),
                        libc::MSG_NOSIGNAL,
                    )
                };
                if written >= 0 {
                    return Ok(written.unsigned_abs());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl AsRawFd for HostSocket {
        fn as_raw_fd(&self) -> RawFd {
            self.0.as_raw_fd()
        }
    }

    #[derive(Deserialize)]
    struct Started {
        pid: u32,
    }

    #[derive(Deserialize)]
    struct Stopped {
        alive: bool,
        reason: String,
    }

    pub(crate) struct IsolatedHostProcess {
        instance_id: String,
        process_id: u32,
        peer_closed: bool,
        /// Last confirmed exit classification; `None` while not confirmed dead.
        exit_reason: Option<String>,
    }

    impl IsolatedHostProcess {
        /// Binds a fresh isolated service instance and hands it the child end
        /// of a new socketpair. The child end stays open here until Kotlin has
        /// duplicated it (the command resolves only after the service took it).
        pub(crate) fn start() -> Result<(Self, HostSocket), PluginHostProcessError> {
            let (parent, child) = UnixStream::pair().map_err(|_| PluginHostProcessError::Spawn)?;
            let instance_id = uuid::Uuid::new_v4().simple().to_string();
            let started: Result<Started, _> = bridge::run_command(
                "pluginHostStart",
                serde_json::json!({ "instanceId": instance_id, "fd": child.as_raw_fd() }),
            );
            drop(child);
            let started = started.map_err(|error| {
                bridge::log::warn(&format!("plugin host start failed: {}", error.code()));
                PluginHostProcessError::Spawn
            })?;
            let process = Self {
                instance_id,
                process_id: started.pid,
                peer_closed: false,
                exit_reason: None,
            };
            if started.pid == 0 {
                return Err(PluginHostProcessError::Spawn);
            }
            Ok((process, HostSocket::new(parent)))
        }

        pub(crate) fn process_id(&self) -> u32 {
            self.process_id
        }

        pub(crate) fn mark_peer_closed(&mut self) {
            self.peer_closed = true;
        }

        /// Unbinds the service (the system kills an unbound isolated process)
        /// and waits, bounded on the Kotlin side, for Binder death. Idempotent:
        /// a confirmed exit is remembered so retries do not rebind or re-wait.
        pub(crate) fn stop_and_confirm(&mut self) -> Result<(), PluginHostProcessError> {
            if self.exit_reason.is_some() {
                return Ok(());
            }
            let stopped: Stopped = bridge::run_command(
                "pluginHostStop",
                serde_json::json!({ "instanceId": self.instance_id }),
            )
            .map_err(|error| {
                bridge::log::warn(&format!("plugin host stop failed: {}", error.code()));
                PluginHostProcessError::Cleanup
            })?;
            if stopped.alive {
                bridge::log::warn("plugin host death was not confirmed");
                return Err(PluginHostProcessError::Cleanup);
            }
            self.exit_reason = Some(stopped.reason);
            Ok(())
        }

        /// Only a host whose channel already closed is asked about; a live
        /// host that merely timed out or broke protocol is the plugin's fault.
        pub(crate) fn reclaimed_by_system(&mut self) -> bool {
            if !self.peer_closed || self.stop_and_confirm().is_err() {
                return false;
            }
            self.exit_reason.as_deref() == Some(REASON_LOW_MEMORY)
        }
    }

    impl Drop for IsolatedHostProcess {
        fn drop(&mut self) {
            // Failed handshakes drop the owner without `shutdown`; unbind so
            // the isolated process cannot outlive its owner. Best effort only:
            // callers that need proof keep the owner and call `shutdown`.
            if self.exit_reason.is_none() {
                let _ = self.stop_and_confirm();
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) struct PluginHostProcess;

#[cfg(not(any(unix, windows)))]
impl PluginHostProcess {
    pub(crate) fn spawn(_: &[u8], _: u64, _: u16, _: u16) -> Result<Self, PluginHostProcessError> {
        Err(PluginHostProcessError::UnsupportedPlatform)
    }
}

/// Requests are constructed by multiple host brokers; none may choose the guest ABI.
fn bind_guest_protocol(
    request: &mut PluginHostRequest,
    major: u16,
    minor: u16,
) -> Result<(), PluginHostProcessError> {
    if !norishell_core_api::plugin_protocol_is_compatible(major, minor)
        || norishell_core_api::plugin_message_min_protocol_minor(request.kind) > minor
    {
        return Err(PluginHostProcessError::Rejected);
    }
    request.protocol_major = major;
    request.protocol_minor = minor;
    Ok(())
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use norishell_core_api::PluginHostMessageKind;

    #[test]
    fn every_request_kind_is_bound_to_verified_guest_minor() {
        for kind in [
            PluginHostMessageKind::Initialize,
            PluginHostMessageKind::Invoke,
            PluginHostMessageKind::UiAction,
            PluginHostMessageKind::SshSyncResult,
            PluginHostMessageKind::BrokerResult,
            PluginHostMessageKind::TerminalObservation,
            PluginHostMessageKind::ProtocolEvent,
            PluginHostMessageKind::WorkflowEvent,
        ] {
            let mut request = PluginHostRequest {
                protocol_major: 2,
                protocol_minor: 99,
                request_id: "bound".into(),
                kind,
                payload_json: "{}".into(),
            };
            bind_guest_protocol(&mut request, 1, 13).unwrap();
            assert_eq!((request.protocol_major, request.protocol_minor), (1, 13));
            assert_eq!(request.kind, kind);
            assert_eq!(request.payload_json, "{}");
        }
    }

    #[test]
    fn old_abi_is_rejected_before_starting_a_child() {
        for minor in [9, 12, norishell_core_api::PLUGIN_PROTOCOL_MINOR + 1] {
            assert!(matches!(
                PluginHostProcess::spawn(b"wasm", 1, 1, minor),
                Err(PluginHostProcessError::Rejected)
            ));
        }
    }
}
