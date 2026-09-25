//! Password-protected, local NoriShell backup envelope.
//!
//! The encrypted payload is owned by the desktop Core. This codec has no
//! access to Vault, SQLite, file pickers, or plugin identity.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{RecoveryPassword, Result, SyncCodecError};

const FORMAT: &str = "norishell-offline-backup-v1";
const KDF: &str = "argon2id-v1:m=65536,t=3,p=1";
const CIPHER: &str = "xchacha20poly1305-v1";
const AAD: &[u8] = b"norishell:offline-backup:v1:argon2id-v1:m=65536,t=3,p=1:xchacha20poly1305-v1";
const SALT_BYTES: usize = 16;
const NONCE_BYTES: usize = 24;
const KEY_BYTES: usize = 32;
/// Includes a portable profile section and, optionally, the encrypted Vault
/// snapshot and its metadata. The outer Base64 JSON can be larger.
pub const MAX_OFFLINE_BACKUP_PLAINTEXT_BYTES: usize = 192 * 1024 * 1024;
pub const MAX_OFFLINE_BACKUP_FILE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OfflineBackupEnvelope {
    format: String,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// Encrypts a versioned Core payload for explicit export to a local file.
/// Contents and its manifest remain hidden until the backup password is
/// supplied. The caller must validate the payload schema separately.
pub fn encrypt_offline_backup(password: &RecoveryPassword, plaintext: &[u8]) -> Result<Vec<u8>> {
    if plaintext.is_empty() {
        return Err(SyncCodecError::InvalidBundle(
            "offline backup payload is empty",
        ));
    }
    if plaintext.len() > MAX_OFFLINE_BACKUP_PLAINTEXT_BYTES {
        return Err(SyncCodecError::BoundExceeded("offline backup payload"));
    }
    let mut salt = [0_u8; SALT_BYTES];
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut salt).map_err(|_| SyncCodecError::Random)?;
    getrandom::fill(&mut nonce).map_err(|_| SyncCodecError::Random)?;
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .map_err(|_| SyncCodecError::KeyDerivation)?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: AAD,
            },
        )
        .map_err(|_| SyncCodecError::AuthenticationFailed)?;
    let envelope = OfflineBackupEnvelope {
        format: FORMAT.to_owned(),
        kdf: KDF.to_owned(),
        cipher: CIPHER.to_owned(),
        salt: BASE64.encode(salt),
        nonce: BASE64.encode(nonce),
        ciphertext: BASE64.encode(ciphertext),
    };
    let encoded = serde_json::to_vec(&envelope)?;
    if encoded.len() > MAX_OFFLINE_BACKUP_FILE_BYTES {
        return Err(SyncCodecError::BoundExceeded("offline backup file"));
    }
    Ok(encoded)
}

/// Opens a local backup only after validating its bounded, fixed encryption
/// profile. The returned plaintext is erased when dropped.
pub fn decrypt_offline_backup(
    password: &RecoveryPassword,
    encoded: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    if encoded.is_empty() || encoded.len() > MAX_OFFLINE_BACKUP_FILE_BYTES {
        return Err(SyncCodecError::BoundExceeded("offline backup file"));
    }
    let envelope: OfflineBackupEnvelope = serde_json::from_slice(encoded)?;
    if envelope.format != FORMAT || envelope.cipher != CIPHER {
        return Err(SyncCodecError::InvalidBundle("offline backup format"));
    }
    if envelope.kdf != KDF {
        return Err(SyncCodecError::UnsupportedKdfProfile);
    }
    let salt = decode_fixed::<SALT_BYTES>(&envelope.salt, "offline backup salt")?;
    let nonce = decode_fixed::<NONCE_BYTES>(&envelope.nonce, "offline backup nonce")?;
    if envelope.ciphertext.len() > (MAX_OFFLINE_BACKUP_PLAINTEXT_BYTES + 16).div_ceil(3) * 4 + 4 {
        return Err(SyncCodecError::BoundExceeded("offline backup ciphertext"));
    }
    let ciphertext = Zeroizing::new(
        BASE64
            .decode(envelope.ciphertext.as_bytes())
            .map_err(|_| SyncCodecError::InvalidBundle("offline backup ciphertext"))?,
    );
    if BASE64.encode(ciphertext.as_slice()) != envelope.ciphertext {
        return Err(SyncCodecError::InvalidBundle("offline backup ciphertext"));
    }
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_slice())
        .map_err(|_| SyncCodecError::KeyDerivation)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: ciphertext.as_slice(),
                    aad: AAD,
                },
            )
            .map_err(|_| SyncCodecError::AuthenticationFailed)?,
    );
    if plaintext.is_empty() || plaintext.len() > MAX_OFFLINE_BACKUP_PLAINTEXT_BYTES {
        return Err(SyncCodecError::BoundExceeded("offline backup payload"));
    }
    Ok(plaintext)
}

fn derive_key(
    password: &RecoveryPassword,
    salt: &[u8; SALT_BYTES],
) -> Result<Zeroizing<[u8; KEY_BYTES]>> {
    let params =
        Params::new(65_536, 3, 1, Some(KEY_BYTES)).map_err(|_| SyncCodecError::KeyDerivation)?;
    let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.expose(), salt, key.as_mut())
        .map_err(|_| SyncCodecError::KeyDerivation)?;
    Ok(key)
}

fn decode_fixed<const N: usize>(value: &str, field: &'static str) -> Result<[u8; N]> {
    let decoded = BASE64
        .decode(value.as_bytes())
        .map_err(|_| SyncCodecError::InvalidBundle(field))?;
    if BASE64.encode(&decoded) != value {
        return Err(SyncCodecError::InvalidBundle(field));
    }
    decoded
        .try_into()
        .map_err(|_| SyncCodecError::InvalidBundle(field))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password(value: &str) -> RecoveryPassword {
        RecoveryPassword::new(value.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn round_trip_keeps_manifest_inside_ciphertext() {
        let payload = br#"{"formatVersion":1,"sections":["portable","fullVault"]}"#;
        let file = encrypt_offline_backup(&password("long-backup-password"), payload).unwrap();
        assert!(
            !file
                .windows(b"portable".len())
                .any(|window| window == b"portable")
        );
        let opened = decrypt_offline_backup(&password("long-backup-password"), &file).unwrap();
        assert_eq!(opened.as_slice(), payload);
    }

    #[test]
    fn wrong_password_and_ciphertext_change_fail_authentication() {
        let file = encrypt_offline_backup(&password("long-backup-password"), b"secret").unwrap();
        assert!(matches!(
            decrypt_offline_backup(&password("different-password"), &file),
            Err(SyncCodecError::AuthenticationFailed)
        ));
        let mut envelope: OfflineBackupEnvelope = serde_json::from_slice(&file).unwrap();
        let mut ciphertext = BASE64.decode(&envelope.ciphertext).unwrap();
        ciphertext[0] ^= 1;
        envelope.ciphertext = BASE64.encode(ciphertext);
        let changed = serde_json::to_vec(&envelope).unwrap();
        assert!(matches!(
            decrypt_offline_backup(&password("long-backup-password"), &changed),
            Err(SyncCodecError::AuthenticationFailed)
        ));
    }

    #[test]
    fn unsupported_profile_and_large_input_fail_before_kdf() {
        let file = encrypt_offline_backup(&password("long-backup-password"), b"secret").unwrap();
        let mut envelope: OfflineBackupEnvelope = serde_json::from_slice(&file).unwrap();
        envelope.kdf = "argon2id-v2".to_owned();
        let changed = serde_json::to_vec(&envelope).unwrap();
        assert!(matches!(
            decrypt_offline_backup(&password("long-backup-password"), &changed),
            Err(SyncCodecError::UnsupportedKdfProfile)
        ));
        assert!(matches!(
            decrypt_offline_backup(
                &password("long-backup-password"),
                &vec![0; MAX_OFFLINE_BACKUP_FILE_BYTES + 1]
            ),
            Err(SyncCodecError::BoundExceeded(_))
        ));
    }
}
