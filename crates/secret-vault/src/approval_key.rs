//! A device-local key used to fingerprint exact protected-plugin operations.
//!
//! This deliberately is not Vault payload data: callers must be able to
//! compare non-secret operation scopes while the Vault is locked. The key is
//! still a private, independently formatted file so database copies cannot be
//! used as an offline dictionary for remembered operation payloads.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::{Result, VaultError, ensure_private_parent_directory};

const MAGIC: &[u8] = b"NORISHELL_APPROVAL_FINGERPRINT_KEY\0";
const FORMAT_VERSION: u8 = 1;
const KEY_BYTES: usize = 32;
const ENCODED_BYTES: usize = MAGIC.len() + 1 + KEY_BYTES;
const DOMAIN: &[u8] = b"NoriShell/plugin-operation-permission-fingerprint/v1\0";

/// Private material for keyed fingerprints of exact operation scopes.
///
/// The material is never serializable, cloneable, or exposed. `Debug` only
/// identifies the type so logs cannot disclose fingerprint-key bytes.
pub struct ApprovalFingerprintKey(Zeroizing<[u8; KEY_BYTES]>);

impl fmt::Debug for ApprovalFingerprintKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApprovalFingerprintKey([REDACTED])")
    }
}

impl ApprovalFingerprintKey {
    /// Loads an existing private key, or atomically creates one without
    /// replacing any existing file. Invalid, oversized, unsafe, or malformed
    /// files fail closed and are never regenerated automatically.
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        ensure_private_parent_directory(path)?;
        match open_existing_private_key(path) {
            Ok(file) => Self::from_file(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = [0_u8; KEY_BYTES];
                getrandom::fill(&mut bytes).map_err(|error| {
                    VaultError::Io(std::io::Error::other(format!(
                        "operating system randomness unavailable: {error}"
                    )))
                })?;
                let mut file = match create_private_key(path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Self::from_file(open_existing_private_key(path)?);
                    }
                    Err(error) => return Err(VaultError::Io(error)),
                };
                let mut encoded = Vec::with_capacity(ENCODED_BYTES);
                encoded.extend_from_slice(MAGIC);
                encoded.push(FORMAT_VERSION);
                encoded.extend_from_slice(&bytes);
                if let Err(error) = (|| -> std::io::Result<()> {
                    file.write_all(&encoded)?;
                    file.flush()?;
                    file.sync_all()
                })() {
                    drop(file);
                    let _ = fs::remove_file(path);
                    return Err(VaultError::Io(error));
                }
                Ok(Self(Zeroizing::new(bytes)))
            }
            Err(error) => Err(VaultError::Io(error)),
        }
    }

    /// HMAC-SHA-256 with a fixed product-domain separator.
    #[must_use]
    pub fn fingerprint(&self, typed_scope: &[u8]) -> [u8; KEY_BYTES] {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.0.as_ref())
            .expect("the approval fingerprint key is always 32 bytes");
        mac.update(DOMAIN);
        mac.update(typed_scope);
        mac.finalize().into_bytes().into()
    }

    fn from_file(mut file: File) -> Result<Self> {
        let length = file.metadata()?.len();
        if length != ENCODED_BYTES as u64 {
            return Err(VaultError::InvalidEnvelope(
                "approval fingerprint key is malformed",
            ));
        }
        let mut encoded = [0_u8; ENCODED_BYTES];
        file.read_exact(&mut encoded)?;
        if &encoded[..MAGIC.len()] != MAGIC || encoded[MAGIC.len()] != FORMAT_VERSION {
            return Err(VaultError::InvalidEnvelope(
                "approval fingerprint key format is invalid",
            ));
        }
        let mut key = [0_u8; KEY_BYTES];
        key.copy_from_slice(&encoded[MAGIC.len() + 1..]);
        Ok(Self(Zeroizing::new(key)))
    }
}

fn open_existing_private_key(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::{fs::OpenOptionsExt, fs::PermissionsExt};
        options.custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path)?;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "approval fingerprint key file permissions are not private",
            ));
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
        };
        options
            .access_mode(FILE_GENERIC_READ | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let file = options.open(path)?;
        crate::windows_acl::secure_file(&file)?;
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        options.open(path)
    }
}

fn create_private_key(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::{fs::OpenOptionsExt, fs::PermissionsExt};
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, READ_CONTROL,
            WRITE_DAC, WRITE_OWNER,
        };
        options
            .access_mode(
                FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
            )
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        let file = options.open(path)?;
        if let Err(error) = crate::windows_acl::secure_file(&file) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(error);
        }
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        options.open(path)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn reopens_to_the_same_domain_separated_fingerprint() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private").join("approval.key");
        let first = ApprovalFingerprintKey::load_or_create(&path).unwrap();
        let first_fingerprint = first.fingerprint(b"typed scope");
        assert!(!format!("{first:?}").contains("typed scope"));
        drop(first);
        let reopened = ApprovalFingerprintKey::load_or_create(&path).unwrap();
        assert_eq!(reopened.fingerprint(b"typed scope"), first_fingerprint);
        assert_ne!(
            reopened.fingerprint(b"typed scope"),
            reopened.fingerprint(b"other")
        );
    }

    #[test]
    fn malformed_existing_key_is_not_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("approval.key");
        fs::write(&path, b"not a key").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(ApprovalFingerprintKey::load_or_create(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not a key");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_and_public_key_file() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("private");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let target = private.join("target");
        fs::write(&target, b"not a key").unwrap();
        let link = private.join("approval.key");
        symlink(&target, &link).unwrap();
        assert!(ApprovalFingerprintKey::load_or_create(&link).is_err());

        let path = private.join("public.key");
        let key = ApprovalFingerprintKey::load_or_create(&path).unwrap();
        drop(key);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(ApprovalFingerprintKey::load_or_create(&path).is_err());
    }
}
