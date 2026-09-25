use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use norishell_core_api::{
    CoreApiError, ErrorCategory, RequestId, RetryStrategy, SecretRefId,
    VaultAutoUnlockDisableRequest, VaultAutoUnlockEnableRequest, VaultAutoUnlockFailure,
    VaultChangePasswordRequest, VaultCreateRequest, VaultLockRequest, VaultState, VaultStatus,
    VaultStatusRequest, VaultUnlockPolicy, VaultUnlockRequest, WireSequence,
};
use norishell_secret_vault::{
    CommandHistoryPayload, DeviceUnlockKey, SecretInsert, SecretKind, SecretRef, SecretValue,
    UnlockedVault, VaultError, VaultMergeSummary, VaultSyncKeyMaterial,
    cleanup_local_auto_unlock_password_staging, fingerprint_locked_vault, open_sync_key_material,
    prepare_encrypted_vault_import, read_bounded_file_in_private_directory,
    read_local_auto_unlock_password, remove_locked_vault_if_unchanged,
    write_local_auto_unlock_password, write_private_file,
};
use subtle::ConstantTimeEq;
use tauri::State;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::device_key_store::{DeviceKeyStore, DeviceKeyStoreError, platform_device_key_store};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

const AUTO_UNLOCK_SYSTEM_MARKER_V1: &[u8] = b"norishell-auto-unlock-v1\n";
const AUTO_UNLOCK_LOCAL_MARKER_V2: &[u8] = b"norishell-auto-unlock-v2-local\n";
const AUTO_UNLOCK_DISABLED_MARKER_V1: &[u8] = b"norishell-auto-unlock-disabled-v1\n";
const AUTO_UNLOCK_ENABLING_MARKER_V1: &[u8] = b"norishell-auto-unlock-enabling-v1\n";
const MAX_AUTO_UNLOCK_MARKER_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutoUnlockMode {
    System,
    Local,
}

impl AutoUnlockMode {
    const fn marker(self) -> &'static [u8] {
        match self {
            Self::System => AUTO_UNLOCK_SYSTEM_MARKER_V1,
            Self::Local => AUTO_UNLOCK_LOCAL_MARKER_V2,
        }
    }

    const fn policy(self) -> VaultUnlockPolicy {
        match self {
            Self::System => VaultUnlockPolicy::Automatic,
            Self::Local => VaultUnlockPolicy::AutomaticLocal,
        }
    }
}

/// A synchronized observation of whether an unlocked Vault can safely serve
/// plaintext. `epoch` advances whenever that fact changes and lets consumers
/// reject an out-of-order callback after a concurrent lock or poison event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VaultAvailability {
    pub(crate) available: bool,
    pub(crate) epoch: u64,
}

type VaultAvailabilityObserver = Arc<dyn Fn(VaultAvailability) + Send + Sync>;

#[derive(Clone)]
pub struct VaultService {
    path: PathBuf,
    device_unlock_slot_path: PathBuf,
    local_auto_unlock_password_path: PathBuf,
    auto_unlock_enabled_path: PathBuf,
    auto_unlock_blocked_path: PathBuf,
    /// Serializes every policy transition with its filesystem cleanup. A Vault
    /// mutex alone cannot cover disabling material that lives outside the
    /// encrypted envelope.
    auto_unlock_operation_lock: Arc<Mutex<()>>,
    unlocked: Arc<Mutex<Option<UnlockedVault>>>,
    device_key_store: Arc<dyn DeviceKeyStore>,
    auto_unlock_failure: Arc<Mutex<Option<VaultAutoUnlockFailure>>>,
    availability_observer: Arc<Mutex<VaultAvailabilityObserverState>>,
    reset_requires_restart: Arc<AtomicBool>,
}

struct VaultAvailabilityObserverState {
    observer: Option<VaultAvailabilityObserver>,
    availability: VaultAvailability,
}

impl Default for VaultAvailabilityObserverState {
    fn default() -> Self {
        Self {
            observer: None,
            availability: VaultAvailability {
                available: false,
                epoch: 0,
            },
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum VaultServiceError {
    #[error("vault password confirmation does not match")]
    PasswordConfirmationMismatch,
    #[error("vault is already unlocked")]
    AlreadyUnlocked,
    #[error("vault is not unlocked")]
    NotUnlocked,
    #[error("the requested policy cannot enable Vault auto-unlock")]
    InvalidAutoUnlockPolicy,
    #[error("vault availability changed during the operation")]
    AvailabilityChanged,
    #[error("vault reset requires application restart")]
    ResetRequiresRestart,
    #[error("the requested vault secret does not exist")]
    SecretNotFound,
    #[error("the requested vault secret reference is bound to different material")]
    SecretConflict,
    #[error(transparent)]
    DeviceKeyStore(#[from] DeviceKeyStoreError),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SyncKeyRecoveryError {
    VaultUnavailable,
    RemoteKeyAuthenticationFailed,
    RemoteKeyEnvelopeInvalid,
}

/// Distinguishes a Vault-envelope failure from a caller-owned fresh-target
/// guard failure. The latter can occur after the file commit, so callers must
/// not infer that a failed guard leaves no local Vault file behind.
#[derive(Debug)]
pub(crate) enum VaultEnvelopeImportError<E> {
    /// The import contract deliberately does not surface Vault failure detail
    /// to the offline-backup boundary, which must never turn password or
    /// envelope parsing information into a user-visible error.
    Vault,
    FreshTarget(E),
}

impl<E> From<VaultServiceError> for VaultEnvelopeImportError<E> {
    fn from(_: VaultServiceError) -> Self {
        Self::Vault
    }
}

impl<E> From<VaultError> for VaultEnvelopeImportError<E> {
    fn from(_: VaultError) -> Self {
        Self::Vault
    }
}

impl VaultServiceError {
    /// True only when insert returned before committing the caller-minted
    /// reference. Existing/conflicting references and poisoned outcomes must
    /// instead remain durable cleanup candidates.
    pub(crate) fn secret_insert_definitely_absent(&self) -> bool {
        !matches!(
            self,
            Self::SecretConflict
                | Self::Vault(VaultError::CommitStateUnknown(_) | VaultError::ReloadRequired)
        )
    }
}

impl VaultService {
    pub(crate) fn requires_restart_after_reset(&self) -> bool {
        self.reset_requires_restart.load(Ordering::SeqCst)
    }

    pub(crate) fn locked_reset_fingerprint(&self) -> Result<[u8; 32], VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.requires_restart_after_reset() {
            return Err(VaultServiceError::ResetRequiresRestart);
        }
        self.with_vault_guard(|guard| {
            if guard.is_some() {
                return Err(VaultServiceError::AlreadyUnlocked);
            }
            Ok(fingerprint_locked_vault(&self.path)?)
        })
    }

    pub(crate) fn reset_local_locked_vault(
        &self,
        expected: &[u8; 32],
    ) -> Result<(), VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_vault_guard(|guard| {
            if self.requires_restart_after_reset() {
                return Err(VaultServiceError::ResetRequiresRestart);
            }
            if guard.is_some() {
                return Err(VaultServiceError::AlreadyUnlocked);
            }
            if fingerprint_locked_vault(&self.path)? != *expected {
                return Err(VaultError::VaultChanged.into());
            }
            // Install a durable fail-closed barrier before touching the envelope.
            self.revoke_auto_unlock_while_serialized(self.auto_unlock_mode(), true)?;
            let result = remove_locked_vault_if_unchanged(&self.path, expected);
            if result.is_ok() || matches!(result, Err(VaultError::CommitStateUnknown(_))) {
                self.reset_requires_restart.store(true, Ordering::SeqCst);
            }
            result.map_err(Into::into)
        })
    }

    pub fn start(app_data_directory: impl AsRef<Path>) -> Self {
        let app_data_directory = app_data_directory.as_ref().to_path_buf();
        // Debug launch opt-out leaves the encrypted Vault locked and its policy intact.
        let skip_auto_unlock = cfg!(debug_assertions)
            && std::env::args_os().any(|arg| arg == "--skip-startup-auto-unlock");
        Self::start_with_options(
            &app_data_directory,
            platform_device_key_store(app_data_directory.clone()),
            skip_auto_unlock,
        )
    }

    #[cfg(test)]
    fn start_with_device_key_store(
        app_data_directory: impl AsRef<Path>,
        device_key_store: Arc<dyn DeviceKeyStore>,
    ) -> Self {
        Self::start_with_options(app_data_directory, device_key_store, false)
    }

    fn start_with_options(
        app_data_directory: impl AsRef<Path>,
        device_key_store: Arc<dyn DeviceKeyStore>,
        skip_auto_unlock: bool,
    ) -> Self {
        let app_data_directory = app_data_directory.as_ref();
        let service = Self {
            path: app_data_directory.join("vault").join("vault.nvx"),
            device_unlock_slot_path: app_data_directory.join("vault").join("auto-unlock.nvx"),
            local_auto_unlock_password_path: app_data_directory
                .join("vault")
                .join("auto-unlock.password"),
            auto_unlock_enabled_path: app_data_directory.join("vault").join("auto-unlock.enabled"),
            auto_unlock_blocked_path: app_data_directory.join("vault").join("auto-unlock.blocked"),
            auto_unlock_operation_lock: Arc::new(Mutex::new(())),
            unlocked: Arc::new(Mutex::new(None)),
            device_key_store,
            auto_unlock_failure: Arc::new(Mutex::new(None)),
            availability_observer: Arc::new(Mutex::new(VaultAvailabilityObserverState::default())),
            reset_requires_restart: Arc::new(AtomicBool::new(false)),
        };
        if cleanup_local_auto_unlock_password_staging(&service.local_auto_unlock_password_path)
            .is_err()
        {
            service.set_auto_unlock_failure(Some(VaultAutoUnlockFailure::DeviceUnlockRejected));
        } else if !skip_auto_unlock {
            service.try_auto_unlock();
        }
        service
    }

    pub(crate) fn status(&self) -> VaultStatus {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let guard = self
            .unlocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.status_from(guard.as_ref())
    }

    fn status_from(&self, vault: Option<&UnlockedVault>) -> VaultStatus {
        if self.reset_requires_restart.load(Ordering::SeqCst) {
            return VaultStatus {
                state: VaultState::RequiresReload,
                vault_id: None,
                revision: None,
                entry_count: None,
                unlock_policy: VaultUnlockPolicy::CurrentSession,
                auto_unlock_failure: None,
            };
        }
        status_from(
            &self.path,
            vault,
            self.auto_unlock_mode()
                .map(AutoUnlockMode::policy)
                .unwrap_or(VaultUnlockPolicy::CurrentSession),
            *self
                .auto_unlock_failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn set_auto_unlock_failure(&self, failure: Option<VaultAutoUnlockFailure>) {
        *self
            .auto_unlock_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = failure;
    }

    fn auto_unlock_mode(&self) -> Option<AutoUnlockMode> {
        // Treat every unreadable or malformed block marker as a block. The
        // marker is the crash barrier for transitions, so a path substitution
        // must never make an old enabled marker effective again.
        match self.read_auto_unlock_marker(&self.auto_unlock_blocked_path) {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => return None,
        }
        match self.read_auto_unlock_marker(&self.auto_unlock_enabled_path) {
            Ok(Some(value)) => match value.as_slice() {
                AUTO_UNLOCK_SYSTEM_MARKER_V1 => Some(AutoUnlockMode::System),
                AUTO_UNLOCK_LOCAL_MARKER_V2 => Some(AutoUnlockMode::Local),
                _ => None,
            },
            Ok(None) | Err(_) => None,
        }
    }

    fn read_auto_unlock_marker(&self, path: &Path) -> Result<Option<Vec<u8>>, VaultError> {
        match read_bounded_file_in_private_directory(path, MAX_AUTO_UNLOCK_MARKER_BYTES) {
            Ok(value) => Ok(Some(value)),
            Err(VaultError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn auto_unlock_path_exists(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }

    fn has_auto_unlock_artifacts(&self) -> bool {
        Self::auto_unlock_path_exists(&self.auto_unlock_enabled_path)
            || Self::auto_unlock_path_exists(&self.device_unlock_slot_path)
            || Self::auto_unlock_path_exists(&self.local_auto_unlock_password_path)
            || Self::auto_unlock_path_exists(&self.auto_unlock_blocked_path)
    }

    fn try_auto_unlock(&self) {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(mode) = self.auto_unlock_mode() else {
            return;
        };
        if !self.path.exists() {
            return;
        }

        let result = match mode {
            AutoUnlockMode::System => self.try_system_auto_unlock(),
            AutoUnlockMode::Local => self.try_local_auto_unlock(),
        };
        match result {
            Ok(vault) => {
                self.with_vault_guard(|guard| {
                    *guard = Some(vault);
                });
                self.set_auto_unlock_failure(None);
            }
            Err(failure) => self.set_auto_unlock_failure(Some(failure)),
        }
    }

    fn try_system_auto_unlock(&self) -> Result<UnlockedVault, VaultAutoUnlockFailure> {
        if !self.device_unlock_slot_path.exists() {
            return Err(VaultAutoUnlockFailure::DeviceUnlockRejected);
        }
        let device_key = match self.device_key_store.load() {
            Ok(device_key) => device_key,
            Err(DeviceKeyStoreError::Missing) => {
                return Err(VaultAutoUnlockFailure::DeviceKeyMissing);
            }
            Err(DeviceKeyStoreError::Unavailable | DeviceKeyStoreError::Platform) => {
                return Err(VaultAutoUnlockFailure::SecureStorageUnavailable);
            }
            Err(DeviceKeyStoreError::Vault(_)) => {
                return Err(VaultAutoUnlockFailure::DeviceUnlockRejected);
            }
        };
        UnlockedVault::unlock_with_device_key(
            &self.path,
            &self.device_unlock_slot_path,
            &device_key,
        )
        .map_err(|_| VaultAutoUnlockFailure::DeviceUnlockRejected)
    }

    fn try_local_auto_unlock(&self) -> Result<UnlockedVault, VaultAutoUnlockFailure> {
        let password = match read_local_auto_unlock_password(&self.local_auto_unlock_password_path)
        {
            Ok(password) => password,
            Err(VaultError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(VaultAutoUnlockFailure::DeviceKeyMissing);
            }
            Err(_) => return Err(VaultAutoUnlockFailure::DeviceUnlockRejected),
        };
        UnlockedVault::unlock(&self.path, &password)
            .map_err(|_| VaultAutoUnlockFailure::DeviceUnlockRejected)
    }

    pub(crate) fn is_unlocked(&self) -> bool {
        self.unlocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|vault| !vault.requires_reload())
    }

    fn availability_from_guard(guard: &Option<UnlockedVault>) -> bool {
        guard.as_ref().is_some_and(|vault| !vault.requires_reload())
    }

    /// Advances the availability epoch while `unlocked` is held. This makes a
    /// snapshot of Vault usability and its epoch one synchronized fact; no
    /// consumer ever authorizes plaintext from an independently cached bool.
    fn synchronize_availability_while_locked(
        &self,
        guard: &Option<UnlockedVault>,
    ) -> (VaultAvailability, Option<VaultAvailabilityObserver>) {
        let available = Self::availability_from_guard(guard);
        let mut state = self
            .availability_observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.availability.available != available {
            state.availability.available = available;
            state.availability.epoch = state.availability.epoch.saturating_add(1);
            (state.availability, state.observer.clone())
        } else {
            (state.availability, None)
        }
    }

    /// Installs the single runtime listener for consumers that cache material
    /// derived from the unlocked Vault. The initial callback is deliberate:
    /// startup auto-unlock happens before desktop services are constructed.
    pub(crate) fn set_availability_observer(&self, observer: VaultAvailabilityObserver) {
        let availability = {
            let guard = self
                .unlocked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let available = Self::availability_from_guard(&guard);
            let mut state = self
                .availability_observer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.availability.available != available {
                state.availability.available = available;
                state.availability.epoch = state.availability.epoch.saturating_add(1);
            }
            state.observer = Some(Arc::clone(&observer));
            state.availability
        };
        observer(availability);
    }

    /// Executes an operation while the actual Vault availability decision is
    /// protected by the Vault mutex. Consumers may lock their own state inside
    /// the closure (Vault -> consumer is the required lock order). A later
    /// lock/poison advances the epoch before another operation can observe it.
    pub(crate) fn with_vault_availability<T>(
        &self,
        operation: impl FnOnce(VaultAvailability, Option<&UnlockedVault>) -> T,
    ) -> T {
        let (result, availability, observer) = {
            let guard = self
                .unlocked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (availability, observer) = self.synchronize_availability_while_locked(&guard);
            let vault = availability.available.then(|| {
                guard
                    .as_ref()
                    .expect("available Vault must have an unlocked instance")
            });
            (operation(availability, vault), availability, observer)
        };
        if let Some(observer) = observer {
            observer(availability);
        }
        result
    }

    /// Runs a Vault access or mutation and captures its availability transition
    /// before releasing the actual Vault mutex. This is deliberately stronger
    /// than observing `is_unlocked()` after the fact: a lock followed by an
    /// unlock on another thread must produce two ordered epochs, even when the
    /// callbacks themselves arrive out of order.
    fn with_vault_guard<T>(&self, operation: impl FnOnce(&mut Option<UnlockedVault>) -> T) -> T {
        let (result, availability, observer) = {
            let mut guard = self
                .unlocked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let result = operation(&mut guard);
            let (availability, observer) = self.synchronize_availability_while_locked(&guard);
            (result, availability, observer)
        };
        if let Some(observer) = observer {
            observer(availability);
        }
        result
    }

    pub(crate) fn export_sync_key_material(
        &self,
    ) -> Result<VaultSyncKeyMaterial, VaultServiceError> {
        self.with_vault_guard(|guard| {
            let vault = guard.as_ref().ok_or(VaultServiceError::NotUnlocked)?;
            Ok(vault.export_sync_key_material()?)
        })
    }

    /// Returns the exact encrypted Vault envelope for an explicitly authorized
    /// offline backup. A locked Vault cannot be exported through this path.
    pub(crate) fn export_encrypted_envelope(&self) -> Result<Vec<u8>, VaultServiceError> {
        self.with_vault_guard(|guard| {
            let vault = guard.as_ref().ok_or(VaultServiceError::NotUnlocked)?;
            Ok(vault.export_encrypted_envelope()?)
        })
    }

    /// Merges an authenticated backup into the current unlocked Vault without
    /// changing its identity, password, or existing references.
    pub(crate) fn merge_encrypted_envelope(
        &self,
        encrypted_envelope: &[u8],
        source_password: &[u8],
        reserved_refs: &[SecretRef],
    ) -> Result<VaultMergeSummary, VaultServiceError> {
        let (result, availability, observer) = {
            let mut guard = self
                .unlocked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let result = (|| {
                let vault = guard.as_mut().ok_or(VaultServiceError::NotUnlocked)?;
                Ok(vault.merge_encrypted_envelope(
                    encrypted_envelope,
                    source_password,
                    reserved_refs,
                )?)
            })();
            let (availability, observer) = self.synchronize_availability_while_locked(&guard);
            if result.is_ok() {
                // History writers carry this epoch. Invalidate queued writes
                // before releasing the Vault mutex, then reload their cache.
                let mut state = self
                    .availability_observer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.availability.epoch = state.availability.epoch.saturating_add(1);
                (result, state.availability, state.observer.clone())
            } else {
                (result, availability, observer)
            }
        };
        if let Some(observer) = observer {
            observer(availability);
        }
        result
    }

    /// Imports a fully encrypted Vault only while a caller-owned fresh-target
    /// guard is active. The callback must acquire Host/SQLite state after this
    /// service already holds its auto-unlock and Vault guards, then invoke the
    /// supplied commit closure exactly once while that external guard remains
    /// live.
    ///
    /// The service installs the returned Vault instance only after the outer
    /// guard has completed successfully. If a post-commit guard failure occurs,
    /// the encrypted file may exist but this service remains locked and never
    /// reports an import success.
    pub(crate) fn import_encrypted_envelope_if_missing_with_fresh_target<E>(
        &self,
        encrypted_envelope: &[u8],
        vault_password: &[u8],
        with_fresh_target: impl FnOnce(
            Box<dyn FnOnce() -> Result<UnlockedVault, VaultServiceError> + '_>,
        ) -> Result<Result<UnlockedVault, VaultServiceError>, E>,
    ) -> Result<VaultStatus, VaultEnvelopeImportError<E>> {
        if vault_path_has_entry(&self.path)? {
            return Err(VaultEnvelopeImportError::Vault);
        }
        // This decrypts and validates every payload field before the
        // fail-closed auto-unlock transition below. An incorrect password or
        // altered backup therefore leaves the local filesystem unchanged.
        let prepared = prepare_encrypted_vault_import(encrypted_envelope, vault_password)
            .map_err(VaultServiceError::from)
            .map_err(|_| VaultEnvelopeImportError::Vault)?;
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_vault_guard(|guard| {
            if self.requires_restart_after_reset() {
                return Err(VaultEnvelopeImportError::Vault);
            }
            if guard.is_some() || vault_path_has_entry(&self.path)? {
                return Err(VaultEnvelopeImportError::Vault);
            }
            let commit = Box::new(|| {
                // Device slots and local-password files never travel with the
                // raw encrypted envelope. Revoke any stale local material
                // before the new file becomes visible, so startup remains
                // manual after import.
                self.revoke_auto_unlock_while_serialized(self.auto_unlock_mode(), false)?;
                Ok(prepared.commit_if_missing(&self.path)?)
            });
            let vault = with_fresh_target(commit)
                .map_err(VaultEnvelopeImportError::FreshTarget)?
                .map_err(|_| VaultEnvelopeImportError::Vault)?;
            *guard = Some(vault);
            self.set_auto_unlock_failure(None);
            Ok(self.status_from(guard.as_ref()))
        })
    }

    pub(crate) fn open_synchronized_key_material(
        &self,
        envelope: &[u8],
        vault_password: &[u8],
    ) -> Result<VaultSyncKeyMaterial, SyncKeyRecoveryError> {
        self.with_vault_guard(|guard| {
            guard
                .as_ref()
                .ok_or(SyncKeyRecoveryError::VaultUnavailable)?;
            // The local Vault is already unlocked. This password belongs to
            // the remote key envelope and can differ from the local password.
            // Keep the Vault mutex until the envelope has been authenticated.
            open_sync_key_material(envelope, vault_password).map_err(|error| match error {
                VaultError::AuthenticationFailed => {
                    SyncKeyRecoveryError::RemoteKeyAuthenticationFailed
                }
                _ => SyncKeyRecoveryError::RemoteKeyEnvelopeInvalid,
            })
        })
    }

    pub(crate) fn create(&self, password: &[u8]) -> Result<VaultStatus, VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_vault_guard(|guard| {
            if self.requires_restart_after_reset() {
                return Err(VaultServiceError::ResetRequiresRestart);
            }
            if guard.is_some() {
                return Err(VaultServiceError::AlreadyUnlocked);
            }
            *guard = Some(UnlockedVault::create(&self.path, password)?);
            self.set_auto_unlock_failure(None);
            Ok(self.status_from(guard.as_ref()))
        })
    }

    /// Only an explicit create decision may initialize a missing Vault.
    /// Existing files and concurrent successful creation are never treated as success.
    pub(crate) fn create_for_protected_operation(
        &self,
        password: &[u8],
        confirmation: &[u8],
    ) -> Result<VaultStatus, VaultServiceError> {
        if !confirmations_match(password, confirmation) {
            return Err(VaultServiceError::PasswordConfirmationMismatch);
        }
        self.create(password)
    }

    #[cfg(test)]
    pub(crate) fn create_for_tests(
        &self,
        password: &[u8],
    ) -> Result<VaultStatus, VaultServiceError> {
        self.create(password)
    }

    fn unlock(&self, password: &[u8]) -> Result<VaultStatus, VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_vault_guard(|guard| {
            if self.requires_restart_after_reset() {
                return Err(VaultServiceError::ResetRequiresRestart);
            }
            if guard.as_ref().is_some_and(|vault| !vault.requires_reload()) {
                return Err(VaultServiceError::AlreadyUnlocked);
            }
            guard.take();
            *guard = Some(UnlockedVault::unlock(&self.path, password)?);
            Ok(self.status_from(guard.as_ref()))
        })
    }

    /// An explicit main-window unlock may reuse the password the user chose to
    /// keep locally. This never grants background or plugin callers a prompt.
    pub(crate) fn unlock_with_saved_local_password(
        &self,
    ) -> Result<VaultStatus, VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.requires_restart_after_reset() {
            return Err(VaultServiceError::ResetRequiresRestart);
        }
        if self.auto_unlock_mode() != Some(AutoUnlockMode::Local) {
            return Err(VaultServiceError::InvalidAutoUnlockPolicy);
        }
        self.with_vault_guard(|guard| {
            if guard.as_ref().is_some_and(|vault| !vault.requires_reload()) {
                return Ok(self.status_from(guard.as_ref()));
            }
            guard.take();
            match self.try_local_auto_unlock() {
                Ok(vault) => {
                    *guard = Some(vault);
                    self.set_auto_unlock_failure(None);
                    Ok(self.status_from(guard.as_ref()))
                }
                Err(failure) => {
                    self.set_auto_unlock_failure(Some(failure));
                    Err(VaultServiceError::NotUnlocked)
                }
            }
        })
    }

    /// Unlocks for a protected Core operation, including concurrent successful unlocks.
    pub(crate) fn unlock_for_protected_operation(
        &self,
        password: &[u8],
    ) -> Result<VaultStatus, VaultServiceError> {
        match self.unlock(password) {
            Err(VaultServiceError::AlreadyUnlocked) if self.is_unlocked() => Ok(self.status()),
            result => result,
        }
    }

    fn remove_auto_unlock_file(path: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => {
                #[cfg(unix)]
                std::fs::File::open(
                    path.parent()
                        .ok_or_else(|| std::io::Error::other("auto-unlock path has no parent"))?,
                )?
                .sync_all()?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Clears both the legacy device-key material and the local-password
    /// material. The caller must first install the blocked marker, so a crash
    /// between individual removals can only leave a manual-unlock Vault.
    fn clear_auto_unlock_materials(
        &self,
        prior_mode: Option<AutoUnlockMode>,
    ) -> Result<(), VaultServiceError> {
        let clear_device_key =
            prior_mode == Some(AutoUnlockMode::System) || self.device_unlock_slot_path.exists();
        let marker_result = Self::remove_auto_unlock_file(&self.auto_unlock_enabled_path);
        let slot_result = Self::remove_auto_unlock_file(&self.device_unlock_slot_path);
        let local_password_result =
            Self::remove_auto_unlock_file(&self.local_auto_unlock_password_path);
        let staging_result =
            cleanup_local_auto_unlock_password_staging(&self.local_auto_unlock_password_path);
        let device_key_result = if clear_device_key {
            self.device_key_store.delete()
        } else {
            Ok(())
        };

        marker_result.map_err(VaultError::Io)?;
        slot_result.map_err(VaultError::Io)?;
        local_password_result.map_err(VaultError::Io)?;
        staging_result?;
        device_key_result?;
        Ok(())
    }

    /// Runs while `auto_unlock_operation_lock` is held. `force_block` is used
    /// before an enable or mode switch so old material cannot regain effect
    /// while new material is being written.
    fn revoke_auto_unlock_while_serialized(
        &self,
        prior_mode: Option<AutoUnlockMode>,
        force_block: bool,
    ) -> Result<(), VaultServiceError> {
        if !force_block && !self.has_auto_unlock_artifacts() {
            self.set_auto_unlock_failure(None);
            return Ok(());
        }
        if self
            .write_auto_unlock_marker(
                &self.auto_unlock_blocked_path,
                AUTO_UNLOCK_DISABLED_MARKER_V1,
            )
            .is_err()
        {
            // A failed or durability-unknown block write must not leave the
            // previous enabled marker effective. Removing that marker is an
            // equivalent fail-closed barrier, including after a crash.
            Self::remove_auto_unlock_file(&self.auto_unlock_enabled_path)
                .map_err(VaultError::Io)?;
        }
        self.clear_auto_unlock_materials(prior_mode)?;
        self.set_auto_unlock_failure(None);
        Ok(())
    }

    fn write_auto_unlock_marker(&self, path: &Path, value: &[u8]) -> Result<(), VaultServiceError> {
        write_private_file(path, value).map_err(Into::into)
    }

    fn install_auto_unlock_material(
        &self,
        mode: AutoUnlockMode,
        vault: &UnlockedVault,
        password: &[u8],
    ) -> Result<(), VaultServiceError> {
        match mode {
            AutoUnlockMode::System => {
                let device_key = DeviceUnlockKey::generate()?;
                self.device_key_store.store(&device_key)?;
                vault.write_device_unlock_slot(&self.device_unlock_slot_path, &device_key)?;
            }
            AutoUnlockMode::Local => {
                write_local_auto_unlock_password(&self.local_auto_unlock_password_path, password)?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn enable_auto_unlock(&self, password: &[u8]) -> Result<VaultStatus, VaultServiceError> {
        self.enable_auto_unlock_with_policy(password, None)
    }

    fn enable_auto_unlock_with_policy(
        &self,
        password: &[u8],
        requested_policy: Option<VaultUnlockPolicy>,
    ) -> Result<VaultStatus, VaultServiceError> {
        let mode = match requested_policy.unwrap_or(VaultUnlockPolicy::Automatic) {
            VaultUnlockPolicy::Automatic => AutoUnlockMode::System,
            VaultUnlockPolicy::AutomaticLocal => AutoUnlockMode::Local,
            VaultUnlockPolicy::CurrentSession => {
                return Err(VaultServiceError::InvalidAutoUnlockPolicy);
            }
        };
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_vault_guard(|guard| {
            if self.requires_restart_after_reset() {
                return Err(VaultServiceError::ResetRequiresRestart);
            }
            let mut newly_unlocked = None;
            let vault = if guard.as_ref().is_some_and(|vault| !vault.requires_reload()) {
                let vault = guard.as_ref().expect("usable unlocked vault");
                vault.verify_password(password)?;
                vault
            } else {
                guard.take();
                newly_unlocked = Some(UnlockedVault::unlock(&self.path, password)?);
                newly_unlocked.as_ref().expect("newly unlocked vault")
            };

            let prior_mode = self.auto_unlock_mode();
            self.revoke_auto_unlock_while_serialized(prior_mode, true)?;
            self.write_auto_unlock_marker(
                &self.auto_unlock_blocked_path,
                AUTO_UNLOCK_ENABLING_MARKER_V1,
            )?;
            if let Err(error) = self.install_auto_unlock_material(mode, vault, password) {
                let _ = self.clear_auto_unlock_materials(Some(mode));
                return Err(error);
            }
            if let Err(error) =
                self.write_auto_unlock_marker(&self.auto_unlock_enabled_path, mode.marker())
            {
                let _ = self.clear_auto_unlock_materials(Some(mode));
                return Err(error);
            }
            if let Err(error) = Self::remove_auto_unlock_file(&self.auto_unlock_blocked_path) {
                let _ = self.clear_auto_unlock_materials(Some(mode));
                return Err(VaultError::Io(error).into());
            }
            if let Some(vault) = newly_unlocked {
                *guard = Some(vault);
            }
            self.set_auto_unlock_failure(None);
            Ok(self.status_from(guard.as_ref()))
        })
    }

    fn disable_auto_unlock(&self) -> Result<VaultStatus, VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.revoke_auto_unlock_while_serialized(self.auto_unlock_mode(), false)?;
        self.with_vault_guard(|guard| Ok(self.status_from(guard.as_ref())))
    }

    fn lock(&self) -> Result<VaultStatus, VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_vault_guard(|guard| {
            // Local mode intentionally keeps the user-approved password file:
            // Lock ends this session, while disabling the mode removes it.
            let mode = self.auto_unlock_mode();
            let revoke_result = if mode == Some(AutoUnlockMode::Local) {
                Ok(())
            } else {
                self.revoke_auto_unlock_while_serialized(mode, false)
            };
            guard.take();
            revoke_result?;
            Ok(self.status_from(None))
        })
    }

    #[cfg(test)]
    pub(crate) fn lock_for_tests(&self) -> Result<VaultStatus, VaultServiceError> {
        self.lock()
    }

    fn change_password(
        &self,
        current_password: &[u8],
        new_password: &[u8],
    ) -> Result<VaultStatus, VaultServiceError> {
        let _operation = self
            .auto_unlock_operation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.with_vault_guard(|guard| {
            let prior_mode = self.auto_unlock_mode();
            {
                let vault = guard.as_mut().ok_or(VaultServiceError::NotUnlocked)?;
                vault.change_password(current_password, new_password)?;
            }
            if prior_mode == Some(AutoUnlockMode::Local)
                && write_local_auto_unlock_password(
                    &self.local_auto_unlock_password_path,
                    new_password,
                )
                .is_err()
            {
                // The envelope has already committed the new password. A stale
                // local copy must never remain an enabled policy: disable it
                // and report the successful password change as manual mode.
                if let Err(error) = self.revoke_auto_unlock_while_serialized(prior_mode, false) {
                    // If durable revocation cannot be established after the
                    // envelope changed, do not leave plaintext available in a
                    // session whose restart policy is uncertain.
                    guard.take();
                    return Err(error);
                }
            }
            Ok(self.status_from(guard.as_ref()))
        })
    }

    pub(crate) fn insert_secrets(
        &self,
        secrets: &[VaultSecretInsert],
    ) -> Result<VaultStatus, VaultServiceError> {
        self.with_vault_guard(|guard| {
            let vault = guard.as_mut().ok_or(VaultServiceError::NotUnlocked)?;
            if vault.requires_reload() {
                return Err(VaultError::ReloadRequired.into());
            }
            let references = secrets
                .iter()
                .map(|secret| SecretRef::parse(secret.secret_ref_id.as_str()))
                .collect::<Result<Vec<_>, _>>()?;
            let inserts = secrets
                .iter()
                .zip(references.iter().copied())
                .map(|(secret, secret_ref)| {
                    SecretInsert::new(secret_ref, secret.kind, secret.value.as_slice())
                })
                .collect::<Vec<_>>();
            match vault.insert_secrets_with_refs(&inserts) {
                Ok(()) => {}
                Err(VaultError::DuplicateSecretRef(_)) => {
                    for (secret, secret_ref) in secrets.iter().zip(references.iter().copied()) {
                        let Some(existing) = vault.read_secret(secret_ref, secret.kind)? else {
                            return Err(VaultServiceError::SecretConflict);
                        };
                        if existing.expose().len() != secret.value.len()
                            || !bool::from(existing.expose().ct_eq(secret.value.as_slice()))
                        {
                            return Err(VaultServiceError::SecretConflict);
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            }
            Ok(self.status_from(Some(vault)))
        })
    }

    pub(crate) fn read_secret(
        &self,
        secret_ref_id: &SecretRefId,
        expected: SecretKind,
    ) -> Result<SecretValue, VaultServiceError> {
        self.with_vault_guard(|guard| {
            let vault = guard.as_ref().ok_or(VaultServiceError::NotUnlocked)?;
            if vault.requires_reload() {
                return Err(VaultError::ReloadRequired.into());
            }
            let secret_ref = SecretRef::parse(secret_ref_id.as_str())?;
            vault
                .read_secret(secret_ref, expected)?
                .ok_or(VaultServiceError::SecretNotFound)
        })
    }

    pub(crate) fn replace_secret(
        &self,
        secret: &VaultSecretInsert,
    ) -> Result<VaultStatus, VaultServiceError> {
        self.with_vault_guard(|guard| {
            let vault = guard.as_mut().ok_or(VaultServiceError::NotUnlocked)?;
            if vault.requires_reload() {
                return Err(VaultError::ReloadRequired.into());
            }
            let secret_ref = SecretRef::parse(secret.secret_ref_id.as_str())?;
            vault.replace_secret_with_ref(SecretInsert::new(
                secret_ref,
                secret.kind,
                secret.value.as_slice(),
            ))?;
            Ok(self.status_from(Some(vault)))
        })
    }

    pub(crate) fn delete_secrets(
        &self,
        secret_ref_ids: &[SecretRefId],
    ) -> Result<VaultStatus, VaultServiceError> {
        self.with_vault_guard(|guard| {
            let vault = guard.as_mut().ok_or(VaultServiceError::NotUnlocked)?;
            if vault.requires_reload() {
                return Err(VaultError::ReloadRequired.into());
            }
            let secret_refs = secret_ref_ids
                .iter()
                .map(|secret_ref_id| SecretRef::parse(secret_ref_id.as_str()))
                .collect::<Result<Vec<_>, _>>()?;
            vault.delete_secrets(&secret_refs)?;
            Ok(self.status_from(Some(vault)))
        })
    }

    /// Replaces history only when the caller's Vault-availability epoch still
    /// names this exact unlocked instance. The check and write share the real
    /// Vault mutex, so a queued payload from before lock/reunlock cannot land
    /// in the newly unlocked Vault and resurrect old command history.
    pub(crate) fn replace_command_history_at_epoch(
        &self,
        history: CommandHistoryPayload,
        expected_availability_epoch: u64,
    ) -> Result<(), VaultServiceError> {
        self.with_vault_guard(|guard| {
            let availability = self
                .availability_observer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .availability;
            if !availability.available || availability.epoch != expected_availability_epoch {
                return Err(VaultServiceError::AvailabilityChanged);
            }
            let vault = guard.as_mut().ok_or(VaultServiceError::NotUnlocked)?;
            if vault.requires_reload() {
                return Err(VaultError::ReloadRequired.into());
            }
            vault.replace_command_history(history)?;
            Ok(())
        })
    }
}

pub(crate) struct VaultSecretInsert {
    pub secret_ref_id: SecretRefId,
    pub kind: SecretKind,
    pub value: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for VaultSecretInsert {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultSecretInsert")
            .field("secret_ref_id", &"[REDACTED]")
            .field("kind", &self.kind)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

fn status_from(
    path: &Path,
    vault: Option<&UnlockedVault>,
    unlock_policy: VaultUnlockPolicy,
    auto_unlock_failure: Option<VaultAutoUnlockFailure>,
) -> VaultStatus {
    let Some(vault) = vault else {
        return VaultStatus {
            state: if path.exists() {
                VaultState::Locked
            } else {
                VaultState::Missing
            },
            vault_id: None,
            revision: None,
            entry_count: None,
            unlock_policy,
            auto_unlock_failure,
        };
    };
    VaultStatus {
        state: if vault.requires_reload() {
            VaultState::RequiresReload
        } else {
            VaultState::Unlocked
        },
        vault_id: Some(vault.vault_id().to_string()),
        revision: Some(WireSequence::new(vault.revision())),
        entry_count: Some(u32::try_from(vault.entry_count()).unwrap_or(u32::MAX)),
        unlock_policy,
        auto_unlock_failure,
    }
}

fn take_secret(value: &mut String) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(std::mem::take(value).into_bytes())
}

pub(crate) fn confirmations_match(left: &[u8], right: &[u8]) -> bool {
    bool::from(left.ct_eq(right))
}

fn vault_path_has_entry(path: &Path) -> Result<bool, VaultError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(VaultError::Io(error)),
    }
}

#[tauri::command]
pub fn vault_status(
    request: VaultStatusRequest,
    service: State<'_, VaultService>,
) -> CoreResult<VaultStatus> {
    let _request_id = request.meta.request_id;
    Ok(service.status())
}

#[tauri::command]
pub fn vault_create(
    mut request: VaultCreateRequest,
    service: State<'_, VaultService>,
) -> CoreResult<VaultStatus> {
    let request_id = request.meta.request_id.clone();
    let password = take_secret(&mut request.password);
    let confirmation = take_secret(&mut request.password_confirmation);
    let status = service
        .create_for_protected_operation(&password, &confirmation)
        .map_err(|error| map_service_error(request_id, error))?;
    Ok(status)
}

#[tauri::command]
pub async fn vault_unlock(
    mut request: VaultUnlockRequest,
    service: State<'_, VaultService>,
    ssh_sync: State<'_, crate::ssh_sync_exchange_local::NoriShellSshSyncLocalAdapter>,
) -> CoreResult<VaultStatus> {
    let request_id = request.meta.request_id.clone();
    let password = take_secret(&mut request.password);
    let (status, reconciliation) = unlock_with_non_blocking_follow_up(
        &service,
        &password,
        ssh_sync.reconcile_after_vault_unlock(),
    )
    .await
    .map_err(|error| map_service_error(request_id.clone(), error))?;
    if reconciliation.is_err() {
        eprintln!(
            "SSH sync cleanup remains pending after Vault unlock; background reconciliation will retry"
        );
    }
    Ok(status)
}

async fn unlock_with_non_blocking_follow_up<E, F>(
    service: &VaultService,
    password: &[u8],
    follow_up: F,
) -> Result<(VaultStatus, Result<(), E>), VaultServiceError>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    let status = service.unlock(password)?;
    let follow_up = follow_up.await;
    Ok((status, follow_up))
}

#[tauri::command]
pub fn vault_lock(
    request: VaultLockRequest,
    service: State<'_, VaultService>,
    plugins: State<'_, crate::plugin_service::PluginService>,
) -> CoreResult<VaultStatus> {
    let request_id = request.meta.request_id;
    let status = service
        .lock()
        .map_err(|error| map_service_error(request_id, error))?;
    plugins.invalidate_all_ssh_sync_browser_snapshots();
    Ok(status)
}

#[tauri::command]
pub fn vault_auto_unlock_enable(
    mut request: VaultAutoUnlockEnableRequest,
    service: State<'_, VaultService>,
) -> CoreResult<VaultStatus> {
    let request_id = request.meta.request_id.clone();
    let password = take_secret(&mut request.password);
    let policy = request.policy;
    service
        .enable_auto_unlock_with_policy(&password, policy)
        .map_err(|error| map_service_error(request_id, error))
}

#[tauri::command]
pub fn vault_auto_unlock_disable(
    request: VaultAutoUnlockDisableRequest,
    service: State<'_, VaultService>,
) -> CoreResult<VaultStatus> {
    let request_id = request.meta.request_id;
    service
        .disable_auto_unlock()
        .map_err(|error| map_service_error(request_id, error))
}

#[tauri::command]
pub fn vault_change_password(
    mut request: VaultChangePasswordRequest,
    service: State<'_, VaultService>,
) -> CoreResult<VaultStatus> {
    let request_id = request.meta.request_id.clone();
    let current_password = take_secret(&mut request.current_password);
    let new_password = take_secret(&mut request.new_password);
    let confirmation = take_secret(&mut request.new_password_confirmation);
    if !confirmations_match(&new_password, &confirmation) {
        return Err(map_service_error(
            request_id,
            VaultServiceError::PasswordConfirmationMismatch,
        ));
    }
    service
        .change_password(&current_password, &new_password)
        .map_err(|error| map_service_error(request_id, error))
}

pub(crate) fn map_service_error(
    request_id: RequestId,
    error: VaultServiceError,
) -> Box<CoreApiError> {
    let (code, category, retry_strategy, message_key) = match error {
        VaultServiceError::PasswordConfirmationMismatch => (
            "vault.password_confirmation_mismatch",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.vault.passwordConfirmationMismatch",
        ),
        VaultServiceError::AlreadyUnlocked => (
            "vault.already_unlocked",
            ErrorCategory::Conflict,
            RetryStrategy::RefreshSnapshot,
            "errors.vault.alreadyUnlocked",
        ),
        VaultServiceError::NotUnlocked => (
            "vault.locked",
            ErrorCategory::Conflict,
            RetryStrategy::WaitForUser,
            "errors.vault.locked",
        ),
        VaultServiceError::InvalidAutoUnlockPolicy => (
            "vault.invalid_auto_unlock_policy",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.vault.policyUpdate",
        ),
        VaultServiceError::AvailabilityChanged | VaultServiceError::ResetRequiresRestart => (
            "vault.availability_changed",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::RefreshSnapshot,
            "errors.vault.requiresReload",
        ),
        VaultServiceError::SecretNotFound
        | VaultServiceError::Vault(VaultError::SecretNotFound(_)) => (
            "vault.secret_not_found",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::Reconcile,
            "errors.vault.secretNotFound",
        ),
        VaultServiceError::SecretConflict => (
            "vault.secret_conflict",
            ErrorCategory::Conflict,
            RetryStrategy::Reconcile,
            "errors.vault.secretConflict",
        ),
        VaultServiceError::DeviceKeyStore(DeviceKeyStoreError::Unavailable) => (
            "vault.secure_storage_unavailable",
            ErrorCategory::Unavailable,
            RetryStrategy::WaitForUser,
            "errors.vault.secureStorageUnavailable",
        ),
        VaultServiceError::DeviceKeyStore(DeviceKeyStoreError::Missing) => (
            "vault.device_key_missing",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::WaitForUser,
            "errors.vault.deviceKeyMissing",
        ),
        VaultServiceError::DeviceKeyStore(
            DeviceKeyStoreError::Platform | DeviceKeyStoreError::Vault(_),
        ) => (
            "vault.secure_storage_failed",
            ErrorCategory::Unavailable,
            RetryStrategy::WaitForUser,
            "errors.vault.secureStorageFailed",
        ),
        VaultServiceError::Vault(VaultError::AlreadyExists | VaultError::VaultChanged) => (
            "vault.already_exists",
            ErrorCategory::Conflict,
            RetryStrategy::RefreshSnapshot,
            "errors.vault.alreadyExists",
        ),
        VaultServiceError::Vault(VaultError::Locked) => (
            "vault.in_use",
            ErrorCategory::Conflict,
            RetryStrategy::WaitForUser,
            "errors.vault.inUse",
        ),
        VaultServiceError::Vault(
            VaultError::AuthenticationFailed
            | VaultError::InvalidDeviceUnlockKey
            | VaultError::DeviceUnlockVaultMismatch,
        ) => (
            "vault.authentication_failed",
            ErrorCategory::Validation,
            RetryStrategy::WaitForUser,
            "errors.vault.authenticationFailed",
        ),
        VaultServiceError::Vault(VaultError::WeakPassword) => (
            "vault.weak_password",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.vault.weakPassword",
        ),
        VaultServiceError::Vault(
            VaultError::InvalidSecretRef
            | VaultError::EmptySecretBatch
            | VaultError::SecretBatchTooLarge { .. }
            | VaultError::SecretBatchValueTooLarge { .. }
            | VaultError::EmptySecret { .. }
            | VaultError::SecretTooLarge { .. }
            | VaultError::InvalidSecretLength { .. },
        ) => (
            "vault.invalid_secret",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.vault.invalidSecret",
        ),
        VaultServiceError::Vault(VaultError::DuplicateSecretRef(_)) => (
            "vault.secret_ref_conflict",
            ErrorCategory::Conflict,
            RetryStrategy::Reconcile,
            "errors.vault.secretRefConflict",
        ),
        VaultServiceError::Vault(VaultError::SecretKindMismatch { .. }) => (
            "vault.secret_kind_mismatch",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::Reconcile,
            "errors.vault.secretKindMismatch",
        ),
        VaultServiceError::Vault(VaultError::InsecurePermissions) => (
            "vault.insecure_permissions",
            ErrorCategory::Permission,
            RetryStrategy::WaitForUser,
            "errors.vault.insecurePermissions",
        ),
        VaultServiceError::Vault(
            VaultError::CommitStateUnknown(_)
            | VaultError::DeviceUnlockCommitStateUnknown(_)
            | VaultError::ReloadRequired,
        ) => (
            "vault.requires_reload",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::Reconcile,
            "errors.vault.requiresReload",
        ),
        VaultServiceError::Vault(
            VaultError::InvalidEnvelope(_)
            | VaultError::UnsupportedFormat(_)
            | VaultError::TooLarge
            | VaultError::KeyDerivation
            | VaultError::Serialization(_)
            | VaultError::Io(_),
        ) => {
            return Box::new(CoreApiError::safe_internal(
                request_id,
                Uuid::new_v4().to_string(),
            ));
        }
    };
    Box::new(CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        sync::{Arc, Mutex},
    };

    use norishell_core_api::{
        SecretRefId, VaultAutoUnlockFailure, VaultState, VaultStatus, VaultUnlockPolicy,
    };
    use norishell_secret_vault::{
        CommandHistoryEntry, CommandHistoryPayload, DeviceUnlockKey, SecretKind, VaultError,
    };
    use uuid::Uuid;
    use zeroize::Zeroizing;

    use crate::device_key_store::{DeviceKeyStore, DeviceKeyStoreError};

    use super::{
        SyncKeyRecoveryError, VaultEnvelopeImportError, VaultSecretInsert, VaultService,
        VaultServiceError, unlock_with_non_blocking_follow_up,
    };

    const PASSWORD: &[u8] = b"correct horse battery staple";
    const NEW_PASSWORD: &[u8] = b"new correct horse battery staple";

    fn import_into_fresh_target(
        service: &VaultService,
        encrypted_envelope: &[u8],
        vault_password: &[u8],
    ) -> Result<VaultStatus, VaultEnvelopeImportError<Infallible>> {
        service.import_encrypted_envelope_if_missing_with_fresh_target(
            encrypted_envelope,
            vault_password,
            |commit| Ok(commit()),
        )
    }

    #[derive(Default)]
    struct MemoryDeviceKeyStore {
        key: Mutex<Option<Vec<u8>>>,
        loads: std::sync::atomic::AtomicUsize,
    }

    impl DeviceKeyStore for MemoryDeviceKeyStore {
        fn load(&self) -> Result<DeviceUnlockKey, DeviceKeyStoreError> {
            self.loads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let key = self
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .ok_or(DeviceKeyStoreError::Missing)?;
            DeviceUnlockKey::from_bytes(&key).map_err(DeviceKeyStoreError::from)
        }

        fn store(&self, key: &DeviceUnlockKey) -> Result<(), DeviceKeyStoreError> {
            *self
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(key.expose().to_vec());
            Ok(())
        }

        fn delete(&self) -> Result<(), DeviceKeyStoreError> {
            self.key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            Ok(())
        }
    }

    #[test]
    fn locked_reset_removes_local_vault_and_auto_unlock_without_resuming_this_process() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");
        service.lock().expect("lock");
        let fingerprint = service.locked_reset_fingerprint().expect("fingerprint");
        service
            .reset_local_locked_vault(&fingerprint)
            .expect("reset");
        assert!(!service.path.exists());
        assert!(!service.local_auto_unlock_password_path.exists());
        assert!(!service.auto_unlock_enabled_path.exists());
        assert_eq!(service.status().state, VaultState::RequiresReload);
        assert!(matches!(
            service.create(PASSWORD),
            Err(VaultServiceError::ResetRequiresRestart)
        ));
        drop(service);

        let restarted = VaultService::start_with_device_key_store(directory.path(), key_store);
        assert_eq!(restarted.status().state, VaultState::Missing);
        assert_eq!(
            restarted
                .create(PASSWORD)
                .expect("explicit new vault")
                .state,
            VaultState::Unlocked
        );
    }

    #[test]
    fn locked_reset_rejects_a_changed_vault_without_deleting_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service = VaultService::start_with_device_key_store(directory.path(), key_store);
        service.create(PASSWORD).expect("create");
        service.lock().expect("lock");
        let fingerprint = service.locked_reset_fingerprint().expect("fingerprint");
        let mut contents = std::fs::read(&service.path).expect("vault bytes");
        let last = contents.last_mut().expect("nonempty vault");
        *last ^= 1;
        std::fs::write(&service.path, contents).expect("change vault");
        assert!(matches!(
            service.reset_local_locked_vault(&fingerprint),
            Err(VaultServiceError::Vault(VaultError::VaultChanged))
        ));
        assert!(service.path.exists());
        assert_eq!(service.status().state, VaultState::Locked);
    }

    #[test]
    fn automatic_unlock_is_opt_in_and_explicit_lock_revokes_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        let enabled = service
            .enable_auto_unlock(PASSWORD)
            .expect("enable automatic unlock");
        assert_eq!(enabled.state, VaultState::Unlocked);
        assert_eq!(enabled.unlock_policy, VaultUnlockPolicy::Automatic);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            // Earlier v1 markers were control records rather than secret
            // files, so their permission bits must remain readable after the
            // hardened marker reader was introduced.
            std::fs::set_permissions(
                &service.auto_unlock_enabled_path,
                std::fs::Permissions::from_mode(0o644),
            )
            .expect("preserve a legacy marker permission mode");
        }
        drop(service);

        let restarted =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        let status = restarted.status();
        assert_eq!(status.state, VaultState::Unlocked);
        assert_eq!(status.unlock_policy, VaultUnlockPolicy::Automatic);
        assert_eq!(status.auto_unlock_failure, None);

        let locked = restarted.lock().expect("explicit lock");
        assert_eq!(locked.state, VaultState::Locked);
        assert_eq!(locked.unlock_policy, VaultUnlockPolicy::CurrentSession);
        drop(restarted);

        let restarted = VaultService::start_with_device_key_store(directory.path(), key_store);
        assert_eq!(restarted.status().state, VaultState::Locked);
        assert_eq!(
            restarted.status().unlock_policy,
            VaultUnlockPolicy::CurrentSession
        );
    }

    #[test]
    fn local_automatic_unlock_stores_the_password_and_never_loads_the_device_store() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");

        let enabled = service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");
        assert_eq!(enabled.state, VaultState::Unlocked);
        assert_eq!(enabled.unlock_policy, VaultUnlockPolicy::AutomaticLocal);
        assert_eq!(
            std::fs::read(&service.local_auto_unlock_password_path)
                .expect("read local password material"),
            PASSWORD
        );
        assert!(!service.device_unlock_slot_path.exists());
        assert!(
            key_store
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
        drop(service);

        key_store
            .loads
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let restarted =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        let status = restarted.status();
        assert_eq!(status.state, VaultState::Unlocked);
        assert_eq!(status.unlock_policy, VaultUnlockPolicy::AutomaticLocal);
        assert_eq!(status.auto_unlock_failure, None);
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn local_lock_keeps_the_saved_password_for_explicit_unlock() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");

        let locked = service.lock().expect("lock current session");
        assert_eq!(locked.state, VaultState::Locked);
        assert_eq!(locked.unlock_policy, VaultUnlockPolicy::AutomaticLocal);
        assert!(service.local_auto_unlock_password_path.exists());
        assert_eq!(
            service
                .unlock_with_saved_local_password()
                .expect("explicit local unlock")
                .state,
            VaultState::Unlocked
        );
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );

        service.lock().expect("lock again");
        let disabled = service
            .disable_auto_unlock()
            .expect("remove saved password");
        assert_eq!(disabled.unlock_policy, VaultUnlockPolicy::CurrentSession);
        assert!(!service.local_auto_unlock_password_path.exists());
        assert!(service.unlock_with_saved_local_password().is_err());
    }

    #[test]
    fn missing_saved_local_password_falls_back_to_manual_unlock() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start(directory.path());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");
        service.lock().expect("lock current session");
        std::fs::remove_file(&service.local_auto_unlock_password_path).expect("remove saved file");

        assert!(service.unlock_with_saved_local_password().is_err());
        let status = service.status();
        assert_eq!(status.state, VaultState::Locked);
        assert_eq!(
            status.auto_unlock_failure,
            Some(VaultAutoUnlockFailure::DeviceKeyMissing)
        );
        assert_eq!(
            service.unlock(PASSWORD).expect("manual fallback").state,
            VaultState::Unlocked
        );
    }

    #[test]
    fn local_automatic_unlock_removes_abandoned_password_staging_on_start_and_disable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local policy");
        let staging = service
            .local_auto_unlock_password_path
            .with_file_name(format!(".auto-unlock.password.{}.staging", Uuid::new_v4()));
        std::fs::write(&staging, NEW_PASSWORD).expect("simulate crashed staging write");
        drop(service);

        let restarted = VaultService::start_with_device_key_store(directory.path(), key_store);
        assert!(!staging.exists());
        assert_eq!(restarted.status().state, VaultState::Unlocked);
        std::fs::write(&staging, NEW_PASSWORD).expect("simulate second crashed write");
        restarted
            .disable_auto_unlock()
            .expect("disable local policy");
        assert!(!staging.exists());
        assert!(!restarted.local_auto_unlock_password_path.exists());
    }

    #[test]
    fn switching_to_local_automatic_unlock_removes_legacy_material_before_enabling() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock(PASSWORD)
            .expect("enable system automatic unlock");
        assert!(service.device_unlock_slot_path.exists());
        assert!(
            key_store
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
        );

        let switched = service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("switch to local automatic unlock");
        assert_eq!(switched.unlock_policy, VaultUnlockPolicy::AutomaticLocal);
        assert_eq!(
            std::fs::read(&service.auto_unlock_enabled_path).expect("read local marker"),
            super::AUTO_UNLOCK_LOCAL_MARKER_V2
        );
        assert!(!service.device_unlock_slot_path.exists());
        assert_eq!(
            std::fs::read(&service.local_auto_unlock_password_path)
                .expect("read local password material"),
            PASSWORD
        );
        assert!(
            key_store
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
        drop(service);

        key_store
            .loads
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let restarted =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        assert_eq!(restarted.status().state, VaultState::Unlocked);
        assert_eq!(
            restarted.status().unlock_policy,
            VaultUnlockPolicy::AutomaticLocal
        );
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn local_automatic_unlock_failures_stay_locked_and_never_load_the_device_store() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");
        std::fs::remove_file(&service.local_auto_unlock_password_path)
            .expect("remove local password material");
        drop(service);

        key_store
            .loads
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let missing =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        let status = missing.status();
        assert_eq!(status.state, VaultState::Locked);
        assert_eq!(status.unlock_policy, VaultUnlockPolicy::AutomaticLocal);
        assert_eq!(
            status.auto_unlock_failure,
            Some(VaultAutoUnlockFailure::DeviceKeyMissing)
        );
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        drop(missing);

        norishell_secret_vault::write_local_auto_unlock_password(
            directory.path().join("vault").join("auto-unlock.password"),
            NEW_PASSWORD,
        )
        .expect("write wrong local password material");
        let rejected =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        let status = rejected.status();
        assert_eq!(status.state, VaultState::Locked);
        assert_eq!(status.unlock_policy, VaultUnlockPolicy::AutomaticLocal);
        assert_eq!(
            status.auto_unlock_failure,
            Some(VaultAutoUnlockFailure::DeviceUnlockRejected)
        );
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn changing_the_password_updates_local_automatic_unlock_material() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");

        let changed = service
            .change_password(PASSWORD, NEW_PASSWORD)
            .expect("change password");
        assert_eq!(changed.unlock_policy, VaultUnlockPolicy::AutomaticLocal);
        assert_eq!(
            std::fs::read(&service.local_auto_unlock_password_path)
                .expect("read updated local password material"),
            NEW_PASSWORD
        );
        drop(service);

        key_store
            .loads
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let restarted =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        assert_eq!(restarted.status().state, VaultState::Unlocked);
        assert_eq!(
            restarted.status().unlock_policy,
            VaultUnlockPolicy::AutomaticLocal
        );
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn password_change_locks_when_local_material_revocation_cannot_finish() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start_with_device_key_store(
            directory.path(),
            Arc::new(MemoryDeviceKeyStore::default()),
        );
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");
        std::fs::remove_file(&service.local_auto_unlock_password_path)
            .expect("remove local password material");
        std::fs::create_dir(&service.local_auto_unlock_password_path)
            .expect("make local password path non-removable as a file");

        assert!(service.change_password(PASSWORD, NEW_PASSWORD).is_err());
        let status = service.status();
        assert_eq!(status.state, VaultState::Locked);
        assert_eq!(status.unlock_policy, VaultUnlockPolicy::CurrentSession);
        assert!(service.auto_unlock_blocked_path.exists());
        assert!(!service.auto_unlock_enabled_path.exists());
        service
            .unlock(NEW_PASSWORD)
            .expect("new password remains the committed password");
    }

    #[cfg(unix)]
    #[test]
    fn failed_block_marker_write_removes_the_enabled_marker_before_locking() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock(PASSWORD)
            .expect("enable system automatic unlock");
        std::fs::create_dir(&service.auto_unlock_blocked_path)
            .expect("make invalid block marker path");

        let locked = service.lock().expect("lock with invalid block marker path");
        assert_eq!(locked.state, VaultState::Locked);
        assert_eq!(locked.unlock_policy, VaultUnlockPolicy::CurrentSession);
        assert!(!service.auto_unlock_enabled_path.exists());
        assert!(!service.device_unlock_slot_path.exists());
        assert!(
            key_store
                .key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
        drop(service);

        let restarted = VaultService::start_with_device_key_store(directory.path(), key_store);
        assert_eq!(restarted.status().state, VaultState::Locked);
        assert_eq!(
            restarted.status().unlock_policy,
            VaultUnlockPolicy::CurrentSession
        );
    }

    #[cfg(unix)]
    #[test]
    fn startup_rejects_an_enabled_marker_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");
        let target = directory.path().join("vault").join("marker-target");
        std::fs::write(&target, super::AUTO_UNLOCK_LOCAL_MARKER_V2).expect("write marker target");
        std::fs::remove_file(&service.auto_unlock_enabled_path).expect("remove enabled marker");
        symlink(&target, &service.auto_unlock_enabled_path).expect("replace marker with symlink");
        drop(service);

        key_store
            .loads
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let restarted =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        assert_eq!(restarted.status().state, VaultState::Locked);
        assert_eq!(
            restarted.status().unlock_policy,
            VaultUnlockPolicy::CurrentSession
        );
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn startup_treats_a_block_marker_symlink_as_a_block() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable local automatic unlock");
        symlink(
            directory.path().join("vault").join("missing-block-target"),
            &service.auto_unlock_blocked_path,
        )
        .expect("replace block marker with a dangling symlink");
        drop(service);

        key_store
            .loads
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let restarted =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        assert_eq!(restarted.status().state, VaultState::Locked);
        assert_eq!(
            restarted.status().unlock_policy,
            VaultUnlockPolicy::CurrentSession
        );
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn availability_observer_tracks_service_transitions_without_reloading_on_writes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start_with_device_key_store(
            directory.path(),
            Arc::new(MemoryDeviceKeyStore::default()),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        service.set_availability_observer(Arc::new({
            let events = Arc::clone(&events);
            move |availability| {
                events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((availability.available, availability.epoch));
            }
        }));

        service.create(PASSWORD).expect("create");
        service
            .change_password(PASSWORD, b"new correct horse battery staple")
            .expect("change password");
        service.lock().expect("lock");
        service
            .unlock(b"new correct horse battery staple")
            .expect("unlock");

        let events = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            events
                .iter()
                .map(|(available, _)| *available)
                .collect::<Vec<_>>(),
            vec![false, true, false, true]
        );
        assert!(events.windows(2).all(|pair| pair[0].1 < pair[1].1));
    }

    #[test]
    fn history_write_rejects_a_stale_vault_availability_epoch() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start_with_device_key_store(
            directory.path(),
            Arc::new(MemoryDeviceKeyStore::default()),
        );
        service.create(PASSWORD).expect("create");
        let old_epoch = service.with_vault_availability(|availability, _| availability.epoch);
        service.lock().expect("lock");
        service.unlock(PASSWORD).expect("unlock");

        assert!(matches!(
            service.replace_command_history_at_epoch(
                norishell_secret_vault::CommandHistoryPayload::default(),
                old_epoch,
            ),
            Err(VaultServiceError::AvailabilityChanged)
        ));
    }

    #[test]
    fn vault_merge_advances_history_epoch_and_rejects_queued_old_history() {
        let source_directory = tempfile::tempdir().expect("source tempdir");
        let source = VaultService::start(source_directory.path());
        source.create(PASSWORD).expect("create source");
        let source_epoch = source.with_vault_availability(|availability, _| availability.epoch);
        let imported_history = CommandHistoryPayload {
            schema_version: 1,
            entries: vec![CommandHistoryEntry {
                entry_id: Uuid::new_v4(),
                scope: "host:imported".to_owned(),
                command: "imported command".to_owned(),
                completed_at_unix_ms: 123,
                elapsed_millis: 1,
                exit_code: Some(0),
            }],
        };
        source
            .replace_command_history_at_epoch(imported_history, source_epoch)
            .expect("persist source history");
        let envelope = source.export_encrypted_envelope().expect("export source");

        let target_directory = tempfile::tempdir().expect("target tempdir");
        let target = VaultService::start(target_directory.path());
        target.create(PASSWORD).expect("create target");
        let events = Arc::new(Mutex::new(Vec::new()));
        target.set_availability_observer(Arc::new({
            let events = Arc::clone(&events);
            move |availability| {
                events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(availability);
            }
        }));
        let old_epoch = target.with_vault_availability(|availability, _| availability.epoch);
        target
            .merge_encrypted_envelope(&envelope, PASSWORD, &[])
            .expect("merge source history");
        let new_epoch = target.with_vault_availability(|availability, _| availability.epoch);
        assert!(new_epoch > old_epoch);
        assert_eq!(
            events.lock().expect("events").last().unwrap().epoch,
            new_epoch
        );
        assert!(matches!(
            target.replace_command_history_at_epoch(CommandHistoryPayload::default(), old_epoch),
            Err(VaultServiceError::AvailabilityChanged)
        ));
        target.with_vault_guard(|guard| {
            let history = guard
                .as_ref()
                .expect("target remains unlocked")
                .command_history()
                .expect("read merged history");
            assert_eq!(history.entries.len(), 1);
            assert_eq!(history.entries[0].command, "imported command");
        });
    }

    #[test]
    fn startup_opt_out_does_not_access_device_store_or_change_persistent_policy() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service.enable_auto_unlock(PASSWORD).expect("enable");
        let vault = std::fs::read(&service.path).expect("vault");
        let slot = std::fs::read(&service.device_unlock_slot_path).expect("slot");
        let marker = std::fs::read(&service.auto_unlock_enabled_path).expect("marker");
        drop(service);
        key_store
            .loads
            .store(0, std::sync::atomic::Ordering::Relaxed);

        let skipped = VaultService::start_with_options(directory.path(), key_store.clone(), true);
        assert_eq!(skipped.status().state, VaultState::Locked);
        assert_eq!(skipped.status().unlock_policy, VaultUnlockPolicy::Automatic);
        assert_eq!(skipped.status().auto_unlock_failure, None);
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(std::fs::read(&skipped.path).expect("vault"), vault);
        assert_eq!(
            std::fs::read(&skipped.device_unlock_slot_path).expect("slot"),
            slot
        );
        assert_eq!(
            std::fs::read(&skipped.auto_unlock_enabled_path).expect("marker"),
            marker
        );
        skipped.unlock(PASSWORD).expect("manual unlock");
        assert!(skipped.is_unlocked());
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        drop(skipped);

        let normal = VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        assert!(normal.is_unlocked());
        assert_eq!(
            key_store.loads.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn automatic_unlock_failure_falls_back_to_locked_manual_mode() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_store = Arc::new(MemoryDeviceKeyStore::default());
        let service =
            VaultService::start_with_device_key_store(directory.path(), key_store.clone());
        service.create(PASSWORD).expect("create");
        service
            .enable_auto_unlock(PASSWORD)
            .expect("enable automatic unlock");
        drop(service);
        *key_store
            .key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(vec![7; 32]);

        let restarted = VaultService::start_with_device_key_store(directory.path(), key_store);
        let status = restarted.status();
        assert_eq!(status.state, VaultState::Locked);
        assert_eq!(status.unlock_policy, VaultUnlockPolicy::Automatic);
        assert_eq!(
            status.auto_unlock_failure,
            Some(VaultAutoUnlockFailure::DeviceUnlockRejected)
        );
    }

    #[test]
    fn protected_creation_requires_confirmation_and_never_replaces_an_existing_vault() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start(directory.path());
        assert!(matches!(
            service.create_for_protected_operation(PASSWORD, b"different-password"),
            Err(VaultServiceError::PasswordConfirmationMismatch)
        ));
        assert_eq!(service.status().state, VaultState::Missing);
        assert!(service.unlock_for_protected_operation(PASSWORD).is_err());
        assert_eq!(service.status().state, VaultState::Missing);
        service
            .create_for_protected_operation(PASSWORD, PASSWORD)
            .expect("explicit create");
        let original = std::fs::read(&service.path).expect("vault bytes");
        assert!(
            service
                .create_for_protected_operation(PASSWORD, PASSWORD)
                .is_err()
        );
        service.lock().expect("lock");
        assert!(
            service
                .create_for_protected_operation(PASSWORD, PASSWORD)
                .is_err()
        );
        assert_eq!(std::fs::read(&service.path).unwrap(), original);
        assert_eq!(service.status().state, VaultState::Locked);
        service
            .unlock_for_protected_operation(PASSWORD)
            .expect("explicit unlock");
    }

    #[test]
    fn protected_creation_rejects_a_vault_created_while_another_prompt_was_pending() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = VaultService::start(directory.path());
        let second = VaultService::start(directory.path());
        assert_eq!(first.status().state, VaultState::Missing);
        second
            .create_for_protected_operation(PASSWORD, PASSWORD)
            .expect("other prompt creates");
        let original = std::fs::read(&first.path).unwrap();
        assert!(
            first
                .create_for_protected_operation(b"new-vault-password", b"new-vault-password")
                .is_err()
        );
        assert_eq!(std::fs::read(&first.path).unwrap(), original);
        assert_eq!(first.status().state, VaultState::Locked);
    }

    #[test]
    fn full_encrypted_envelope_import_is_fresh_unlocked_and_manual_policy_only() {
        let source_directory = tempfile::tempdir().expect("source tempdir");
        let source_key_store = Arc::new(MemoryDeviceKeyStore::default());
        let source =
            VaultService::start_with_device_key_store(source_directory.path(), source_key_store);
        source.create(PASSWORD).expect("create source Vault");
        let secret_ref_id = SecretRefId::parse(Uuid::new_v4().to_string()).expect("secret ref");
        source
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: secret_ref_id.clone(),
                kind: SecretKind::LoginAutomation,
                value: Zeroizing::new(b"source login secret".to_vec()),
            }])
            .expect("insert source secret");
        let history = CommandHistoryPayload {
            schema_version: 1,
            entries: vec![CommandHistoryEntry {
                entry_id: Uuid::now_v7(),
                scope: "host:example".to_owned(),
                command: "source command history".to_owned(),
                completed_at_unix_ms: 123,
                elapsed_millis: 456,
                exit_code: Some(0),
            }],
        };
        source
            .with_vault_guard(|guard| {
                guard
                    .as_mut()
                    .expect("source is unlocked")
                    .replace_command_history(history.clone())
            })
            .expect("persist source history");
        source
            .enable_auto_unlock_with_policy(PASSWORD, Some(VaultUnlockPolicy::AutomaticLocal))
            .expect("enable source local auto-unlock");
        assert!(source.local_auto_unlock_password_path.exists());
        let source_status = source.status();
        let encrypted_envelope = source
            .export_encrypted_envelope()
            .expect("export complete encrypted envelope");
        assert_eq!(
            encrypted_envelope,
            std::fs::read(&source.path).expect("read source Vault")
        );

        let target_directory = tempfile::tempdir().expect("target tempdir");
        let target_key_store = Arc::new(MemoryDeviceKeyStore::default());
        let target =
            VaultService::start_with_device_key_store(target_directory.path(), target_key_store);
        assert!(matches!(
            import_into_fresh_target(&target, &encrypted_envelope, b"wrong password"),
            Err(VaultEnvelopeImportError::Vault)
        ));
        assert_eq!(target.status().state, VaultState::Missing);
        assert!(!target.path.exists());

        let imported = import_into_fresh_target(&target, &encrypted_envelope, PASSWORD)
            .expect("import fresh Vault");
        assert_eq!(imported.state, VaultState::Unlocked);
        assert_eq!(imported.unlock_policy, VaultUnlockPolicy::CurrentSession);
        assert_eq!(imported.vault_id, source_status.vault_id);
        assert_eq!(
            std::fs::read(&target.path).expect("read imported Vault"),
            encrypted_envelope
        );
        assert!(!target.auto_unlock_enabled_path.exists());
        assert!(!target.device_unlock_slot_path.exists());
        assert!(!target.local_auto_unlock_password_path.exists());
        assert_eq!(
            target
                .read_secret(&secret_ref_id, SecretKind::LoginAutomation)
                .expect("read imported secret")
                .expose(),
            b"source login secret"
        );
        target.with_vault_guard(|guard| {
            let restored = guard
                .as_ref()
                .expect("target stays unlocked")
                .command_history()
                .expect("read imported history");
            assert_eq!(restored.entries.len(), 1);
            assert_eq!(restored.entries[0].entry_id, history.entries[0].entry_id);
            assert_eq!(restored.entries[0].command, history.entries[0].command);
        });
    }

    #[derive(Debug)]
    struct FreshTargetGuardFailure;

    #[test]
    fn full_encrypted_envelope_import_stays_locked_when_the_fresh_guard_fails_after_commit() {
        let source_directory = tempfile::tempdir().expect("source tempdir");
        let source = VaultService::start(source_directory.path());
        source.create(PASSWORD).expect("create source Vault");
        let encrypted_envelope = source
            .export_encrypted_envelope()
            .expect("export encrypted envelope");

        let target_directory = tempfile::tempdir().expect("target tempdir");
        let target = VaultService::start(target_directory.path());
        let error = target
            .import_encrypted_envelope_if_missing_with_fresh_target(
                &encrypted_envelope,
                PASSWORD,
                |commit| match commit() {
                    Ok(vault) => {
                        drop(vault);
                        Err(FreshTargetGuardFailure)
                    }
                    Err(error) => Ok(Err(error)),
                },
            )
            .expect_err("guard failure must not report a successful import");

        assert!(matches!(
            error,
            VaultEnvelopeImportError::FreshTarget(FreshTargetGuardFailure)
        ));
        assert!(target.path.exists());
        assert_eq!(target.status().state, VaultState::Locked);
        assert!(!target.auto_unlock_enabled_path.exists());
        assert!(!target.auto_unlock_blocked_path.exists());
        assert!(!target.device_unlock_slot_path.exists());
        assert!(!target.local_auto_unlock_password_path.exists());
    }

    #[test]
    fn full_encrypted_envelope_import_never_replaces_a_present_vault() {
        let source_directory = tempfile::tempdir().expect("source tempdir");
        let source = VaultService::start(source_directory.path());
        source.create(PASSWORD).expect("create source Vault");
        let encrypted_envelope = source
            .export_encrypted_envelope()
            .expect("export encrypted envelope");

        let target_directory = tempfile::tempdir().expect("target tempdir");
        let target = VaultService::start(target_directory.path());
        target.create(NEW_PASSWORD).expect("create target Vault");
        let original = std::fs::read(&target.path).expect("read target Vault");
        assert!(matches!(
            import_into_fresh_target(&target, &encrypted_envelope, PASSWORD),
            Err(VaultEnvelopeImportError::Vault)
        ));
        assert_eq!(
            std::fs::read(&target.path).expect("read unchanged target"),
            original
        );
        assert_eq!(target.status().state, VaultState::Unlocked);
    }

    #[test]
    fn keeps_the_vault_locked_across_service_restarts() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start(directory.path());
        assert_eq!(service.status().state, VaultState::Missing);
        assert_eq!(
            service.create(PASSWORD).expect("create").state,
            VaultState::Unlocked
        );
        assert_eq!(service.lock().expect("lock").state, VaultState::Locked);
        drop(service);

        let restarted = VaultService::start(directory.path());
        assert_eq!(restarted.status().state, VaultState::Locked);
        assert_eq!(
            restarted.unlock(PASSWORD).expect("unlock").state,
            VaultState::Unlocked
        );
    }

    #[test]
    fn synchronized_key_material_uses_remote_password_with_an_unlocked_local_vault() {
        let source_directory = tempfile::tempdir().expect("source tempdir");
        let source = VaultService::start(source_directory.path());
        source.create(PASSWORD).expect("create source Vault");
        let exported = source
            .export_sync_key_material()
            .expect("export synchronized key material");

        let target_directory = tempfile::tempdir().expect("target tempdir");
        let target = VaultService::start(target_directory.path());
        target
            .create(b"different local vault password")
            .expect("create target Vault with a different password");
        let opened = target
            .open_synchronized_key_material(exported.envelope(), PASSWORD)
            .expect("remote password opens synchronized key");
        assert_eq!(opened.key_bytes(), exported.key_bytes());
        assert!(matches!(
            target.open_synchronized_key_material(
                exported.envelope(),
                b"different local vault password",
            ),
            Err(SyncKeyRecoveryError::RemoteKeyAuthenticationFailed)
        ));
    }

    #[tokio::test]
    async fn successful_unlock_is_reported_when_follow_up_cleanup_must_retry() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start(directory.path());
        service.create(PASSWORD).expect("create");
        service.lock().expect("lock");

        let (status, follow_up) = unlock_with_non_blocking_follow_up(
            &service,
            PASSWORD,
            std::future::ready(Err::<(), _>("retry cleanup")),
        )
        .await
        .expect("unlock remains successful");

        assert_eq!(status.state, VaultState::Unlocked);
        assert_eq!(service.status().state, VaultState::Unlocked);
        assert_eq!(follow_up, Err("retry cleanup"));
    }

    #[test]
    fn inserts_and_borrows_fixed_secrets_only_while_unlocked() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = VaultService::start(directory.path());
        service.create(PASSWORD).expect("create");
        let password_ref = SecretRefId::parse(Uuid::new_v4().to_string()).expect("password ref");
        let passphrase_ref =
            SecretRefId::parse(Uuid::new_v4().to_string()).expect("passphrase ref");
        service
            .insert_secrets(&[
                VaultSecretInsert {
                    secret_ref_id: password_ref.clone(),
                    kind: SecretKind::Password,
                    value: Zeroizing::new(b"server-password".to_vec()),
                },
                VaultSecretInsert {
                    secret_ref_id: passphrase_ref.clone(),
                    kind: SecretKind::Passphrase,
                    value: Zeroizing::new(b"key-passphrase".to_vec()),
                },
            ])
            .expect("insert batch");
        service
            .insert_secrets(&[
                VaultSecretInsert {
                    secret_ref_id: password_ref.clone(),
                    kind: SecretKind::Password,
                    value: Zeroizing::new(b"server-password".to_vec()),
                },
                VaultSecretInsert {
                    secret_ref_id: passphrase_ref.clone(),
                    kind: SecretKind::Passphrase,
                    value: Zeroizing::new(b"key-passphrase".to_vec()),
                },
            ])
            .expect("reconcile identical batch");
        assert!(matches!(
            service.insert_secrets(&[VaultSecretInsert {
                secret_ref_id: password_ref.clone(),
                kind: SecretKind::Password,
                value: Zeroizing::new(b"different-password".to_vec()),
            }]),
            Err(VaultServiceError::SecretConflict)
        ));

        let password = service
            .read_secret(&password_ref, SecretKind::Password)
            .expect("read password");
        assert_eq!(password.expose(), b"server-password");
        assert!(matches!(
            service.read_secret(&passphrase_ref, SecretKind::Password),
            Err(VaultServiceError::Vault(
                VaultError::SecretKindMismatch { .. }
            ))
        ));

        service
            .delete_secrets(std::slice::from_ref(&password_ref))
            .expect("delete secret");
        service
            .delete_secrets(std::slice::from_ref(&password_ref))
            .expect("delete replay");
        assert!(matches!(
            service.read_secret(&password_ref, SecretKind::Password),
            Err(VaultServiceError::SecretNotFound)
        ));

        service.lock().expect("lock");
        assert!(matches!(
            service.read_secret(&password_ref, SecretKind::Password),
            Err(VaultServiceError::NotUnlocked)
        ));
        assert!(matches!(
            service.delete_secrets(std::slice::from_ref(&password_ref)),
            Err(VaultServiceError::NotUnlocked)
        ));
    }
}
