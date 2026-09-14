// Modified for NoriShell; see vendor/README.md at the repository root for upstream provenance.
use std::io;

use thiserror::Error;

/// A deliberately non-diagnostic protocol error. Callers map this into their
/// own public error vocabulary instead of surfacing server-provided strings.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VncError {
    #[error("authenticationRejected")]
    AuthenticationRejected,
    #[error("unsupportedAuthentication")]
    UnsupportedAuthentication,
    #[error("unsupportedOperation")]
    UnsupportedOperation,
    #[error("protocolError")]
    Protocol,
    #[error("resourceLimit")]
    ResourceLimit,
    #[error("connectionLost")]
    ConnectionLost,
    #[error("timeout")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("closed")]
    Closed,
    #[error("invalidPixelFormat")]
    WrongPixelFormat,
    #[error("invalidImageData")]
    InvalidImageData,
}

impl From<io::Error> for VncError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::TimedOut => Self::Timeout,
            io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
            | io::ErrorKind::BrokenPipe => Self::ConnectionLost,
            _ => Self::Protocol,
        }
    }
}
