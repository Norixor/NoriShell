use std::{path::PathBuf, sync::Arc};

use norishell_secret_vault::DeviceUnlockKey;
use thiserror::Error;

#[cfg(target_os = "macos")]
const DEVICE_KEY_SERVICE: &str = "app.norishell.desktop.vault-auto-unlock";
#[cfg(target_os = "macos")]
const DEVICE_KEY_ACCOUNT: &str = "primary-device-key";

#[derive(Debug, Error)]
pub(crate) enum DeviceKeyStoreError {
    #[error("the operating-system secure store is unavailable")]
    Unavailable,
    #[error("the device key does not exist")]
    Missing,
    #[error("the operating-system secure store operation failed")]
    Platform,
    #[error(transparent)]
    Vault(#[from] norishell_secret_vault::VaultError),
}

pub(crate) trait DeviceKeyStore: Send + Sync {
    fn load(&self) -> Result<DeviceUnlockKey, DeviceKeyStoreError>;
    fn store(&self, key: &DeviceUnlockKey) -> Result<(), DeviceKeyStoreError>;
    fn delete(&self) -> Result<(), DeviceKeyStoreError>;
}

pub(crate) fn platform_device_key_store(app_data_directory: PathBuf) -> Arc<dyn DeviceKeyStore> {
    Arc::new(PlatformDeviceKeyStore::new(app_data_directory))
}

struct PlatformDeviceKeyStore {
    #[cfg(target_os = "windows")]
    protected_blob_path: PathBuf,
}

impl PlatformDeviceKeyStore {
    fn new(app_data_directory: PathBuf) -> Self {
        #[cfg(not(target_os = "windows"))]
        let _ = app_data_directory;
        Self {
            #[cfg(target_os = "windows")]
            protected_blob_path: app_data_directory.join("vault").join("device-key.dpapi"),
        }
    }
}

#[cfg(target_os = "macos")]
impl DeviceKeyStore for PlatformDeviceKeyStore {
    fn load(&self) -> Result<DeviceUnlockKey, DeviceKeyStoreError> {
        let entry = apple_entry()?;
        let bytes = match entry.get_secret() {
            Ok(bytes) => bytes,
            #[cfg(debug_assertions)]
            Err(error) if debug_keychain_fallback_allowed(&error) => {
                debug_apple_entry()?.get_secret().map_err(map_apple_error)?
            }
            Err(error) => return Err(map_apple_error(error)),
        };
        DeviceUnlockKey::from_bytes(&bytes).map_err(DeviceKeyStoreError::from)
    }

    fn store(&self, key: &DeviceUnlockKey) -> Result<(), DeviceKeyStoreError> {
        match apple_entry()?.set_secret(key.expose()) {
            Ok(()) => Ok(()),
            #[cfg(debug_assertions)]
            Err(error) if debug_keychain_fallback_allowed(&error) => debug_apple_entry()?
                .set_secret(key.expose())
                .map_err(map_apple_error),
            Err(error) => Err(map_apple_error(error)),
        }
    }

    fn delete(&self) -> Result<(), DeviceKeyStoreError> {
        let protected_result = apple_entry()?.delete_credential();
        #[cfg(debug_assertions)]
        if match protected_result.as_ref() {
            Ok(()) => true,
            Err(error) => debug_keychain_fallback_allowed(error),
        } {
            return match debug_apple_entry()?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(error) => Err(map_apple_error(error)),
            };
        }
        match protected_result {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_apple_error(error)),
        }
    }
}

#[cfg(all(test, target_os = "macos", debug_assertions))]
mod macos_debug_tests {
    use std::io;

    use super::debug_keychain_fallback_allowed;

    #[test]
    fn fallback_is_limited_to_missing_debug_identity_or_entry() {
        assert!(debug_keychain_fallback_allowed(
            &keyring_core::Error::NoEntry
        ));
        assert!(debug_keychain_fallback_allowed(
            &keyring_core::Error::PlatformFailure(Box::new(io::Error::other(
                "missing application identifier",
            )))
        ));
        assert!(!debug_keychain_fallback_allowed(
            &keyring_core::Error::NoStorageAccess(Box::new(io::Error::other("locked")))
        ));
    }
}

#[cfg(target_os = "macos")]
fn apple_entry() -> Result<keyring_core::Entry, DeviceKeyStoreError> {
    use apple_native_keyring_store::protected::{AccessPolicy, Cred};

    Cred::build(
        DEVICE_KEY_SERVICE,
        DEVICE_KEY_ACCOUNT,
        AccessPolicy::WhenUnlockedThisDeviceOnly,
        None,
        false,
    )
    .map_err(map_apple_error)
}

/// Unsigned and ad-hoc debug executables have no application-identifier
/// entitlement, so macOS rejects Data Protection Keychain operations with
/// errSecMissingEntitlement. Daily preview builds still use the current user's
/// non-synchronizing login Keychain for the random device key; release builds
/// never compile this fallback and remain on the Data Protection Keychain.
#[cfg(all(target_os = "macos", debug_assertions))]
fn debug_apple_entry() -> Result<keyring_core::Entry, DeviceKeyStoreError> {
    use apple_native_keyring_store::keychain::{Cred, MacKeychainDomain};

    Cred::build(
        MacKeychainDomain::User,
        DEVICE_KEY_SERVICE,
        DEVICE_KEY_ACCOUNT,
    )
    .map_err(map_apple_error)
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn debug_keychain_fallback_allowed(error: &keyring_core::Error) -> bool {
    matches!(
        error,
        keyring_core::Error::PlatformFailure(_) | keyring_core::Error::NoEntry
    )
}

#[cfg(target_os = "macos")]
fn map_apple_error(error: keyring_core::Error) -> DeviceKeyStoreError {
    match error {
        keyring_core::Error::NoEntry => DeviceKeyStoreError::Missing,
        keyring_core::Error::NoStorageAccess(_) | keyring_core::Error::NotSupportedByStore(_) => {
            DeviceKeyStoreError::Unavailable
        }
        _ => DeviceKeyStoreError::Platform,
    }
}

#[cfg(target_os = "windows")]
impl DeviceKeyStore for PlatformDeviceKeyStore {
    fn load(&self) -> Result<DeviceUnlockKey, DeviceKeyStoreError> {
        let protected = match std::fs::read(&self.protected_blob_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(DeviceKeyStoreError::Missing);
            }
            Err(_) => return Err(DeviceKeyStoreError::Platform),
        };
        let plaintext = windows_dpapi::unprotect(&protected)?;
        DeviceUnlockKey::from_bytes(&plaintext).map_err(DeviceKeyStoreError::from)
    }

    fn store(&self, key: &DeviceUnlockKey) -> Result<(), DeviceKeyStoreError> {
        let protected = windows_dpapi::protect(key.expose())?;
        if let Some(parent) = self.protected_blob_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| DeviceKeyStoreError::Platform)?;
        }
        let temporary = self.protected_blob_path.with_extension("dpapi.staging");
        std::fs::write(&temporary, protected).map_err(|_| DeviceKeyStoreError::Platform)?;
        match std::fs::remove_file(&self.protected_blob_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(DeviceKeyStoreError::Platform),
        }
        std::fs::rename(&temporary, &self.protected_blob_path)
            .map_err(|_| DeviceKeyStoreError::Platform)
    }

    fn delete(&self) -> Result<(), DeviceKeyStoreError> {
        match std::fs::remove_file(&self.protected_blob_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(DeviceKeyStoreError::Platform),
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_dpapi {
    use std::{
        ptr::{null, null_mut},
        slice,
    };

    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    use zeroize::Zeroizing;

    use super::DeviceKeyStoreError;

    const OPTIONAL_ENTROPY: &[u8] = b"NoriShell vault device key v1";

    pub(super) fn protect(plaintext: &[u8]) -> Result<Vec<u8>, DeviceKeyStoreError> {
        let protected = crypt(plaintext, true)?;
        Ok(protected.to_vec())
    }

    pub(super) fn unprotect(ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, DeviceKeyStoreError> {
        crypt(ciphertext, false)
    }

    fn crypt(input: &[u8], protect: bool) -> Result<Zeroizing<Vec<u8>>, DeviceKeyStoreError> {
        let input_len = u32::try_from(input.len()).map_err(|_| DeviceKeyStoreError::Platform)?;
        let entropy_len =
            u32::try_from(OPTIONAL_ENTROPY.len()).map_err(|_| DeviceKeyStoreError::Platform)?;
        let input_blob = CRYPT_INTEGER_BLOB {
            cbData: input_len,
            pbData: input.as_ptr().cast_mut(),
        };
        let entropy_blob = CRYPT_INTEGER_BLOB {
            cbData: entropy_len,
            pbData: OPTIONAL_ENTROPY.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let result = unsafe {
            if protect {
                CryptProtectData(
                    &input_blob,
                    null(),
                    &entropy_blob,
                    null(),
                    null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input_blob,
                    null_mut(),
                    &entropy_blob,
                    null(),
                    null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            }
        };
        if result == 0 || output.pbData.is_null() {
            return Err(DeviceKeyStoreError::Platform);
        }
        let bytes = unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize) };
        let copied = Zeroizing::new(bytes.to_vec());
        unsafe {
            for offset in 0..output.cbData as usize {
                output.pbData.add(offset).write_volatile(0);
            }
            LocalFree(output.pbData.cast());
        }
        Ok(copied)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl DeviceKeyStore for PlatformDeviceKeyStore {
    fn load(&self) -> Result<DeviceUnlockKey, DeviceKeyStoreError> {
        Err(DeviceKeyStoreError::Unavailable)
    }

    fn store(&self, _key: &DeviceUnlockKey) -> Result<(), DeviceKeyStoreError> {
        Err(DeviceKeyStoreError::Unavailable)
    }

    fn delete(&self) -> Result<(), DeviceKeyStoreError> {
        Ok(())
    }
}
