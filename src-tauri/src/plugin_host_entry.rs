use std::{
    io,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use norishell_core_api::{PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR};
use norishell_plugin_platform::{PluginPlatformError, RuntimeLimits, WasmRuntime};
use sha2::{Digest, Sha256};

use crate::plugin_host_protocol::{
    ChildFrame, PLUGIN_HOST_FRAME_MAX_BYTES, PLUGIN_HOST_MODULE_MAX_BYTES,
    PLUGIN_HOST_PROTOCOL_MAJOR, PLUGIN_HOST_PROTOCOL_MINOR, ParentFrame, PluginHostRejection,
    read_frame, read_frame_bounded, write_frame,
};

const PLUGIN_HOST_ARGUMENT: &str = "--plugin-host";
const MAX_REQUESTS_PER_WINDOW: usize = 64;
const REQUEST_RATE_WINDOW: Duration = Duration::from_secs(1);

/// Returns an exit code only when this process was explicitly launched as the
/// isolated Plugin Host. `main` calls this before entering any Tauri/Core setup.
pub fn run_if_requested() -> Option<i32> {
    let mut arguments = std::env::args_os();
    let _executable = arguments.next();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new(PLUGIN_HOST_ARGUMENT)) {
        return None;
    }
    if arguments.next().is_some() {
        return Some(64);
    }
    Some(match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("plugin host stopped: {error}");
            65
        }
    })
}

fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let initialization: ParentFrame = read_frame(&mut reader)?;
    let ParentFrame::Initialize {
        protocol_major,
        protocol_minor,
        instance_generation,
        nonce,
        module_sha256,
        module_base64,
    } = initialization
    else {
        return Err(invalid_data());
    };
    if protocol_major != PLUGIN_HOST_PROTOCOL_MAJOR
        || protocol_minor > PLUGIN_HOST_PROTOCOL_MINOR
        || instance_generation == 0
        || !valid_nonce(&nonce)
        || !valid_sha256(&module_sha256)
    {
        return Err(invalid_data());
    }
    let module_bytes = STANDARD
        .decode(module_base64.as_bytes())
        .map_err(|_| invalid_data())?;
    if module_bytes.is_empty() || module_bytes.len() > PLUGIN_HOST_MODULE_MAX_BYTES {
        return Err(invalid_data());
    }
    let digest = hex::encode(Sha256::digest(&module_bytes));
    if digest != module_sha256 {
        return Err(invalid_data());
    }
    write_frame(
        &mut writer,
        &ChildFrame::Ready {
            protocol_major: PLUGIN_HOST_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_HOST_PROTOCOL_MINOR,
            instance_generation,
            nonce: nonce.clone(),
            module_sha256,
        },
    )?;

    let mut expected_sequence = 1_u64;
    let mut rate_window_started = Instant::now();
    let mut requests_in_window = 0_usize;
    let mut runtime = None;
    loop {
        let frame: ParentFrame = read_frame_bounded(&mut reader, PLUGIN_HOST_FRAME_MAX_BYTES)?;
        match frame {
            ParentFrame::Execute {
                instance_generation: observed_generation,
                nonce: observed_nonce,
                sequence,
                request,
            } => {
                if observed_generation != instance_generation
                    || observed_nonce != nonce
                    || sequence != expected_sequence
                {
                    write_rejection(
                        &mut writer,
                        instance_generation,
                        &nonce,
                        sequence,
                        PluginHostRejection::InvalidSequence,
                    )?;
                    return Err(invalid_data());
                }
                expected_sequence = expected_sequence.checked_add(1).ok_or_else(invalid_data)?;
                if rate_window_started.elapsed() >= REQUEST_RATE_WINDOW {
                    rate_window_started = Instant::now();
                    requests_in_window = 0;
                }
                requests_in_window = requests_in_window.saturating_add(1);
                if requests_in_window > MAX_REQUESTS_PER_WINDOW {
                    write_rejection(
                        &mut writer,
                        instance_generation,
                        &nonce,
                        sequence,
                        PluginHostRejection::RuntimeQuotaExceeded,
                    )?;
                    continue;
                }
                if request.protocol_major != PLUGIN_PROTOCOL_MAJOR
                    || request.protocol_minor != PLUGIN_PROTOCOL_MINOR
                {
                    write_rejection(
                        &mut writer,
                        instance_generation,
                        &nonce,
                        sequence,
                        PluginHostRejection::IncompatibleProtocol,
                    )?;
                    continue;
                }
                if runtime.is_none() {
                    runtime = Some(
                        match WasmRuntime::new(&module_bytes, RuntimeLimits::default()) {
                            Ok(runtime) => Box::new(runtime),
                            Err(error) => {
                                write_rejection(
                                    &mut writer,
                                    instance_generation,
                                    &nonce,
                                    sequence,
                                    map_runtime_error(&error),
                                )?;
                                continue;
                            }
                        },
                    );
                }
                let runtime = runtime.as_mut().expect("runtime was constructed");
                let mut outputs = Vec::new();
                let execution = runtime.execute(&request, |output| {
                    outputs.push(output);
                    Ok::<(), String>(())
                });
                match execution {
                    Ok(report) => {
                        write_frame(
                            &mut writer,
                            &ChildFrame::Result {
                                instance_generation,
                                nonce: nonce.clone(),
                                sequence,
                                outputs,
                                fuel_consumed: report.fuel_consumed,
                                elapsed_milliseconds: u64::try_from(report.elapsed_milliseconds)
                                    .unwrap_or(u64::MAX),
                            },
                        )?;
                    }
                    Err(error) => write_rejection(
                        &mut writer,
                        instance_generation,
                        &nonce,
                        sequence,
                        map_runtime_error(&error),
                    )?,
                }
            }
            ParentFrame::Shutdown {
                instance_generation: observed_generation,
                nonce: observed_nonce,
                sequence,
            } => {
                if observed_generation != instance_generation
                    || observed_nonce != nonce
                    || sequence != expected_sequence
                {
                    return Err(invalid_data());
                }
                write_frame(
                    &mut writer,
                    &ChildFrame::Stopped {
                        instance_generation,
                        nonce,
                        sequence,
                    },
                )?;
                return Ok(());
            }
            ParentFrame::Initialize { .. } => return Err(invalid_data()),
        }
    }
}

fn write_rejection(
    writer: &mut impl io::Write,
    instance_generation: u64,
    nonce: &str,
    sequence: u64,
    code: PluginHostRejection,
) -> io::Result<()> {
    write_frame(
        writer,
        &ChildFrame::Rejected {
            instance_generation,
            nonce: nonce.to_owned(),
            sequence,
            code,
        },
    )
}

fn map_runtime_error(error: &PluginPlatformError) -> PluginHostRejection {
    match error {
        PluginPlatformError::InvalidWasmAbi => PluginHostRejection::InvalidModule,
        PluginPlatformError::RuntimeQuotaExceeded | PluginPlatformError::RuntimeTimedOut => {
            PluginHostRejection::RuntimeQuotaExceeded
        }
        _ => PluginHostRejection::RuntimeFailed,
    }
}

fn valid_nonce(value: &str) -> bool {
    (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid_data() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid plugin host request")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_identity_fields_are_strict_and_non_secret() {
        assert!(valid_nonce("019d-safe-instance"));
        assert!(!valid_nonce("short"));
        assert!(!valid_nonce("019d unsafe instance"));
        assert!(valid_sha256(&"a".repeat(64)));
        assert!(!valid_sha256(&"A".repeat(64)));
    }
}
