use std::{
    io::{self, Read, Write},
    time::Duration,
};

use norishell_core_api::{PluginHostRequest, PluginRuntimeOutput};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(crate) const PLUGIN_HOST_PROTOCOL_MAJOR: u16 = 1;
pub(crate) const PLUGIN_HOST_PROTOCOL_MINOR: u16 = 0;
pub(crate) const PLUGIN_HOST_FRAME_MAX_BYTES: usize = 512 * 1024;
pub(crate) const PLUGIN_HOST_MODULE_MAX_BYTES: usize = 16 * 1024 * 1024;
const PLUGIN_HOST_INITIALIZE_FRAME_MAX_BYTES: usize = 24 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ParentFrame {
    Initialize {
        protocol_major: u16,
        protocol_minor: u16,
        instance_generation: u64,
        nonce: String,
        module_sha256: String,
        module_base64: String,
    },
    Execute {
        instance_generation: u64,
        nonce: String,
        sequence: u64,
        request: PluginHostRequest,
    },
    Shutdown {
        instance_generation: u64,
        nonce: String,
        sequence: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ChildFrame {
    Ready {
        protocol_major: u16,
        protocol_minor: u16,
        instance_generation: u64,
        nonce: String,
        module_sha256: String,
    },
    Result {
        instance_generation: u64,
        nonce: String,
        sequence: u64,
        outputs: Vec<PluginRuntimeOutput>,
        fuel_consumed: u64,
        elapsed_milliseconds: u64,
    },
    Rejected {
        instance_generation: u64,
        nonce: String,
        sequence: u64,
        code: PluginHostRejection,
    },
    Stopped {
        instance_generation: u64,
        nonce: String,
        sequence: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PluginHostRejection {
    IncompatibleProtocol,
    InvalidFrame,
    InvalidModule,
    InvalidSequence,
    RuntimeQuotaExceeded,
    RuntimeFailed,
}

pub(crate) fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> io::Result<T> {
    read_frame_bounded(reader, PLUGIN_HOST_INITIALIZE_FRAME_MAX_BYTES)
}

pub(crate) fn read_frame_bounded<R: Read, T: DeserializeOwned>(
    reader: &mut R,
    maximum: usize,
) -> io::Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame length overflow"))?;
    if length == 0 || length > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin host frame exceeds limit",
        ));
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid plugin host frame"))
}

#[cfg(unix)]
pub(crate) fn read_frame_with_timeout<R, T>(
    reader: &mut R,
    maximum: usize,
    timeout: Duration,
) -> io::Result<T>
where
    R: Read + std::os::fd::AsRawFd,
    T: DeserializeOwned,
{
    use std::time::Instant;

    let descriptor = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    struct RestoreFlags {
        descriptor: std::os::fd::RawFd,
        flags: libc::c_int,
    }
    impl Drop for RestoreFlags {
        fn drop(&mut self) {
            unsafe {
                libc::fcntl(self.descriptor, libc::F_SETFL, self.flags);
            }
        }
    }
    let _restore = RestoreFlags { descriptor, flags };
    let deadline = Instant::now() + timeout;
    let mut length = [0_u8; 4];
    read_exact_until(reader, descriptor, &mut length, deadline)?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame length overflow"))?;
    if length == 0 || length > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin host frame exceeds limit",
        ));
    }
    let mut bytes = vec![0_u8; length];
    read_exact_until(reader, descriptor, &mut bytes, deadline)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid plugin host frame"))
}

#[cfg(windows)]
pub(crate) fn read_frame_with_timeout<R, T>(
    reader: &mut R,
    maximum: usize,
    timeout: Duration,
) -> io::Result<T>
where
    R: Read + std::os::windows::io::AsRawHandle,
    T: DeserializeOwned,
{
    use std::time::Instant;

    let handle = reader.as_raw_handle().cast();
    let deadline = Instant::now() + timeout;
    let mut length = [0_u8; 4];
    read_exact_until_windows(reader, handle, &mut length, deadline)?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame length overflow"))?;
    if length == 0 || length > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin host frame exceeds limit",
        ));
    }
    let mut bytes = vec![0_u8; length];
    read_exact_until_windows(reader, handle, &mut bytes, deadline)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid plugin host frame"))
}

#[cfg(windows)]
fn read_exact_until_windows(
    reader: &mut impl Read,
    handle: windows_sys::Win32::Foundation::HANDLE,
    buffer: &mut [u8],
    deadline: std::time::Instant,
) -> io::Result<()> {
    use std::{ptr::null_mut, thread};

    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let mut offset = 0;
    while offset < buffer.len() {
        let mut available = 0_u32;
        // SAFETY: handle is the live parent read end of the anonymous pipe and
        // only the available-byte counter is requested.
        if unsafe {
            PeekNamedPipe(
                handle,
                null_mut(),
                0,
                null_mut(),
                &raw mut available,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if available == 0 {
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "plugin host timed out",
                ));
            }
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let available = usize::try_from(available).unwrap_or(usize::MAX);
        let maximum_read = available.min(buffer.len() - offset);
        match reader.read(&mut buffer[offset..offset + maximum_read]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "plugin host closed",
                ));
            }
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_exact_until(
    reader: &mut impl Read,
    descriptor: std::os::fd::RawFd,
    buffer: &mut [u8],
    deadline: std::time::Instant,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        match reader.read(&mut buffer[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "plugin host closed",
                ));
            }
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "plugin host timed out",
                    ));
                }
                let timeout_ms = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
                let mut pollfd = libc::pollfd {
                    fd: descriptor,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let result = unsafe { libc::poll(&raw mut pollfd, 1, timeout_ms) };
                if result == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "plugin host timed out",
                    ));
                }
                if result < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid plugin host frame"))?;
    if bytes.is_empty() || bytes.len() > PLUGIN_HOST_INITIALIZE_FRAME_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "plugin host frame exceeds limit",
        ));
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame length overflow"))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
    };

    use super::*;

    #[test]
    fn length_prefixed_protocol_round_trips_exact_instance_nonce_and_sequence() {
        let frame = ParentFrame::Execute {
            instance_generation: 7,
            nonce: "019d-host-nonce".to_owned(),
            sequence: 3,
            request: PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: "019d-request".to_owned(),
                kind: PluginHostMessageKind::Invoke,
                payload_json: r#"{"action":"render"}"#.to_owned(),
            },
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).expect("write frame");
        let decoded: ParentFrame = read_frame(&mut bytes.as_slice()).expect("read frame");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn oversized_or_unknown_frames_fail_closed() {
        let oversized = u32::try_from(PLUGIN_HOST_INITIALIZE_FRAME_MAX_BYTES + 1)
            .expect("bounded test length")
            .to_be_bytes();
        assert_eq!(
            read_frame::<_, ParentFrame>(&mut oversized.as_slice())
                .expect_err("oversized frame")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let unknown = br#"{"kind":"execute","instanceGeneration":1,"nonce":"n","sequence":1,"request":{"protocolMajor":1,"protocolMinor":0,"requestId":"r","kind":"invoke","payloadJson":"{}"},"unexpected":true}"#;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::try_from(unknown.len()).expect("length").to_be_bytes());
        bytes.extend_from_slice(unknown);
        assert_eq!(
            read_frame::<_, ParentFrame>(&mut bytes.as_slice())
                .expect_err("unknown field")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let normal_oversized = u32::try_from(PLUGIN_HOST_FRAME_MAX_BYTES + 1)
            .expect("bounded normal frame")
            .to_be_bytes();
        assert_eq!(
            read_frame_bounded::<_, ParentFrame>(
                &mut normal_oversized.as_slice(),
                PLUGIN_HOST_FRAME_MAX_BYTES,
            )
            .expect_err("oversized execute frame")
            .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_response_read_has_a_real_wall_deadline() {
        let (mut reader, _writer) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        let error = read_frame_with_timeout::<_, ChildFrame>(
            &mut reader,
            PLUGIN_HOST_FRAME_MAX_BYTES,
            Duration::from_millis(20),
        )
        .expect_err("silent host must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
