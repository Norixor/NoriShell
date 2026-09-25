use thiserror::Error;

pub type Result<T> = std::result::Result<T, SyncCodecError>;

#[derive(Debug, Error)]
pub enum SyncCodecError {
    #[error("SSH profile bundle is invalid: {0}")]
    InvalidBundle(&'static str),
    #[error("SSH profile bundle exceeds the supported bound: {0}")]
    BoundExceeded(&'static str),
    #[error("SSH profile bundle contains a duplicate portable object ID")]
    DuplicateObjectId,
    #[error("SSH profile bundle contains a dangling portable object reference")]
    DanglingObjectReference,
    #[error("sync key must contain exactly 32 bytes")]
    InvalidSyncKey,
    #[error("sync nonce must contain exactly 24 bytes")]
    InvalidNonce,
    #[error("sync object binding does not match the expected owner or revision")]
    BindingMismatch,
    #[error("sync object authentication failed")]
    AuthenticationFailed,
    #[error("recovery password must contain between 8 and 65536 UTF-8 bytes")]
    InvalidRecoveryPassword,
    #[error("recovery key derivation failed")]
    KeyDerivation,
    #[error("recovery envelope uses an unsupported KDF profile")]
    UnsupportedKdfProfile,
    #[error("random number generation failed")]
    Random,
    #[error("serialization failed")]
    Serialization(#[from] serde_json::Error),
}
