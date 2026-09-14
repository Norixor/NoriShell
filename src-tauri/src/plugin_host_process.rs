#[cfg(unix)]
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
#[cfg(windows)]
use std::{ffi::OsString, fs::File};
use std::{io, path::Path, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use norishell_core_api::{PluginHostRequest, PluginRuntimeOutput};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::plugin_host_protocol::{
    ChildFrame, PLUGIN_HOST_FRAME_MAX_BYTES, PLUGIN_HOST_PROTOCOL_MAJOR,
    PLUGIN_HOST_PROTOCOL_MINOR, ParentFrame, read_frame_with_timeout, write_frame,
};
#[cfg(unix)]
use crate::process_control::TerminationOutcome;
#[cfg(unix)]
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

#[cfg(unix)]
type PluginHostOwnedProcess = ManagedProcessGroup;
#[cfg(windows)]
type PluginHostOwnedProcess = ManagedJobProcess;
#[cfg(unix)]
type PluginHostInput = ChildStdin;
#[cfg(windows)]
type PluginHostInput = File;
#[cfg(unix)]
type PluginHostOutput = ChildStdout;
#[cfg(windows)]
type PluginHostOutput = File;

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
        let executable = std::env::current_exe().map_err(|_| PluginHostProcessError::Spawn)?;
        Self::spawn_executable(
            &executable,
            module_bytes,
            instance_generation,
            guest_protocol_major,
            guest_protocol_minor,
        )
    }

    #[cfg(test)]
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

    fn spawn_executable(
        executable: &Path,
        module_bytes: &[u8],
        instance_generation: u64,
        guest_protocol_major: u16,
        guest_protocol_minor: u16,
    ) -> Result<Self, PluginHostProcessError> {
        let working_directory = tempfile::tempdir().map_err(|_| PluginHostProcessError::Spawn)?;
        #[cfg(unix)]
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
        let nonce = uuid::Uuid::new_v4().to_string();
        let module_sha256 = hex::encode(Sha256::digest(module_bytes));
        let mut host = Self {
            process,
            input,
            output,
            instance_generation,
            nonce: nonce.clone(),
            next_sequence: 1,
            guest_protocol_major,
            guest_protocol_minor,
            _working_directory: working_directory,
        };
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
        .map_err(|_| PluginHostProcessError::Protocol)?;
        let ready: ChildFrame = read_frame_with_timeout(
            &mut host.output,
            PLUGIN_HOST_FRAME_MAX_BYTES,
            PLUGIN_HOST_RESPONSE_TIMEOUT,
        )
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                PluginHostProcessError::TimedOut
            } else {
                PluginHostProcessError::Protocol
            }
        })?;
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
        Ok(host)
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
        .map_err(|_| PluginHostProcessError::Protocol)?;
        let response: ChildFrame = read_frame_with_timeout(
            &mut self.output,
            PLUGIN_HOST_FRAME_MAX_BYTES,
            PLUGIN_HOST_RESPONSE_TIMEOUT,
        )
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                PluginHostProcessError::TimedOut
            } else {
                PluginHostProcessError::Protocol
            }
        })?;
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
        .map_err(|_| PluginHostProcessError::Protocol)
        .and_then(|()| {
            let response: ChildFrame = read_frame_with_timeout(
                &mut self.output,
                PLUGIN_HOST_FRAME_MAX_BYTES,
                PLUGIN_HOST_RESPONSE_TIMEOUT,
            )
            .map_err(|error| {
                if error.kind() == io::ErrorKind::TimedOut {
                    PluginHostProcessError::TimedOut
                } else {
                    PluginHostProcessError::Protocol
                }
            })?;
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
        let exited_after_graceful = graceful.is_ok()
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

#[cfg(target_os = "macos")]
type RlimitResource = libc::c_int;
#[cfg(all(unix, not(target_os = "macos")))]
type RlimitResource = libc::__rlimit_resource_t;

#[cfg(unix)]
fn set_limit(resource: RlimitResource, value: libc::rlim_t) -> io::Result<()> {
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
