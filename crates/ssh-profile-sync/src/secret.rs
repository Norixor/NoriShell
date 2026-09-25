use std::fmt;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::{Result, SyncCodecError};

pub const MAX_SECRET_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn new(value: Vec<u8>) -> Result<Self> {
        if value.is_empty() {
            return Err(SyncCodecError::InvalidBundle("secret payload is empty"));
        }
        if value.len() > MAX_SECRET_BYTES {
            return Err(SyncCodecError::BoundExceeded("secret payload"));
        }
        Ok(Self(Zeroizing::new(value)))
    }

    pub(crate) fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Copies plaintext only for the trusted NoriShell Core Vault adapter.
    /// Callers must keep the returned buffer inside the Core secret boundary.
    pub fn copy_for_vault(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.0.to_vec())
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes([REDACTED])")
    }
}

impl Serialize for SecretBytes {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64.encode(self.expose()))
    }
}

impl<'de> Deserialize<'de> for SecretBytes {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = Zeroizing::new(String::deserialize(deserializer)?);
        if encoded.len() > MAX_SECRET_BYTES.div_ceil(3) * 4 + 4 {
            return Err(de::Error::custom("secret payload exceeds supported bound"));
        }
        let decoded = Zeroizing::new(
            BASE64
                .decode(encoded.as_bytes())
                .map_err(|_| de::Error::custom("secret payload is not canonical base64"))?,
        );
        if BASE64.encode(decoded.as_slice()) != encoded.as_str() {
            return Err(de::Error::custom("secret payload is not canonical base64"));
        }
        SecretBytes::new(decoded.to_vec()).map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SyncKey(Zeroizing<[u8; 32]>);

impl SyncKey {
    pub fn from_bytes(value: [u8; 32]) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn generate() -> Result<Self> {
        let mut value = Zeroizing::new([0_u8; 32]);
        getrandom::fill(value.as_mut()).map_err(|_| SyncCodecError::Random)?;
        Ok(Self(value))
    }

    pub(crate) fn expose(&self) -> &[u8; 32] {
        &self.0
    }

    /// Copies key bytes only for encrypted local Vault persistence.
    pub fn copy_for_vault(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.0)
    }

    /// Returns a domain-separated HMAC-SHA256 suitable for non-secret local
    /// change detection. Unlike a raw plaintext digest, this does not provide
    /// an offline verifier to an attacker who obtains only SQLite metadata.
    pub fn baseline_digest(&self, bytes: &[u8]) -> [u8; 32] {
        const BLOCK_BYTES: usize = 64;
        const DOMAIN: &[u8] = b"norishell:ssh-sync:baseline:v1\0";
        let mut inner_pad = [0x36_u8; BLOCK_BYTES];
        let mut outer_pad = [0x5c_u8; BLOCK_BYTES];
        for (index, byte) in self.0.iter().enumerate() {
            inner_pad[index] ^= byte;
            outer_pad[index] ^= byte;
        }
        let mut inner = Sha256::new();
        inner.update(inner_pad);
        inner.update(DOMAIN);
        inner.update(bytes);
        let inner = inner.finalize();
        let mut outer = Sha256::new();
        outer.update(outer_pad);
        outer.update(inner);
        outer.finalize().into()
    }
}

impl fmt::Debug for SyncKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SyncKey([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryPassword(Zeroizing<Vec<u8>>);

impl RecoveryPassword {
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self> {
        let value = value.into();
        if !(8..=65_536).contains(&value.len()) {
            return Err(SyncCodecError::InvalidRecoveryPassword);
        }
        Ok(Self(Zeroizing::new(value)))
    }

    pub(crate) fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for RecoveryPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RecoveryPassword([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::{RecoveryPassword, SyncKey};

    #[test]
    fn recovery_password_accepts_eight_byte_passwords_and_existing_utf8_passwords() {
        assert!(RecoveryPassword::new(b"1234567".to_vec()).is_err());
        assert!(RecoveryPassword::new(b"12345678".to_vec()).is_ok());
        assert!(RecoveryPassword::new("密码密码".as_bytes().to_vec()).is_ok());
    }

    #[test]
    fn baseline_digest_is_domain_separated_hmac_sha256() {
        let digest = SyncKey::from_bytes([1; 32]).baseline_digest(b"payload");
        let encoded = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            encoded,
            "3e84101d63e20c8555c91455b02b01a72beb84d7dd52c605591241aaa0481dab"
        );
        assert_ne!(
            digest,
            SyncKey::from_bytes([2; 32]).baseline_digest(b"payload")
        );
        assert_ne!(
            digest,
            SyncKey::from_bytes([1; 32]).baseline_digest(b"changed")
        );
    }
}
