use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use norishell_core_api::{
    CoreApiError, ErrorCategory, RequestId, RetryStrategy, SecretRefId,
    VaultAutoUnlockDisableRequest, VaultAutoUnlockEnableRequest, VaultAutoUnlockFailure,
    VaultChangePasswordRequest, VaultCreateRequest, VaultLockRequest, VaultState, VaultStatus,
    VaultStatusRequest, VaultUnlockPolicy, VaultUnlockRequest, WireSequence,
};
use norishell_secret_vault::{
    CommandHistoryPayload, DeviceUnlockKey, SecretInsert, SecretKind, SecretRef, SecretValue,
    UnlockedVault, VaultError, VaultSyncKeyMaterial, open_sync_key_material,
};
use subtle::ConstantTimeEq;
use tauri::State;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::device_key_store::{DeviceKeyStore, DeviceKeyStoreError, platform_device_key_store};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

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
    auto_unlock_enabled_path: PathBuf,
    auto_unlock_blocked_path: PathBuf,
    unlocked: Arc<Mutex<Option<UnlockedVault>>>,
    device_key_store: Arc<dyn DeviceKeyStore>,
    auto_unlock_failure: Arc<Mutex<Option<VaultAutoUnlockFailure>>>,
    availability_observer: Arc<Mutex<VaultAvailabilityObserverState>>,
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
    #[error("vault availability changed during the operation")]
    AvailabilityChanged,
    #[error("the requested vault secret does not exist")]
    SecretNotFound,
    #[error("the requested vault secret reference is bound to different material")]
    SecretConflict,
    #[error(transparent)]
    DeviceKeyStore(#[from] DeviceKeyStoreError),
    #[error(transparent)]
    Vault(#[from] VaultError),
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
            auto_unlock_enabled_path: app_data_directory.join("vault").join("auto-unlock.enabled"),
            auto_unlock_blocked_path: app_data_directory.join("vault").join("auto-unlock.blocked"),
            unlocked: Arc::new(Mutex::new(None)),
            device_key_store,
            auto_unlock_failure: Arc::new(Mutex::new(None)),
            availability_observer: Arc::new(Mutex::new(VaultAvailabilityObserverState::default())),
        };
        if !skip_auto_unlock {
            service.try_auto_unlock();
        }
        service
    }

    pub(crate) fn status(&self) -> VaultStatus {
        let guard = self
            .unlocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.status_from(guard.as_ref())
    }

    fn status_from(&self, vault: Option<&UnlockedVault>) -> VaultStatus {
        status_from(
            &self.path,
            vault,
            if self.auto_unlock_is_enabled() {
                VaultUnlockPolicy::Automatic
            } else {
                VaultUnlockPolicy::CurrentSession
            },
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

    fn auto_unlock_is_enabled(&self) -> bool {
        !self.auto_unlock_blocked_path.exists()
            && std::fs::read(&self.auto_unlock_enabled_path)
                .is_ok_and(|value| value == b"norishell-auto-unlock-v1\n")
    }

    fn try_auto_unlock(&self) {
        if !self.path.exists() || !self.auto_unlock_is_enabled() {
            return;
        }
        if !self.device_unlock_slot_path.exists() {
            self.set_auto_unlock_failure(Some(VaultAutoUnlockFailure::DeviceUnlockRejected));
            return;
        }

        let device_key = match self.device_key_store.load() {
            Ok(device_key) => device_key,
            Err(DeviceKeyStoreError::Missing) => {
                self.set_auto_unlock_failure(Some(VaultAutoUnlockFailure::DeviceKeyMissing));
                return;
            }
            Err(DeviceKeyStoreError::Unavailable | DeviceKeyStoreError::Platform) => {
                self.set_auto_unlock_failure(Some(
                    VaultAutoUnlockFailure::SecureStorageUnavailable,
                ));
                return;
            }
            Err(DeviceKeyStoreError::Vault(_)) => {
                self.set_auto_unlock_failure(Some(VaultAutoUnlockFailure::DeviceUnlockRejected));
                return;
            }
        };

        match UnlockedVault::unlock_with_device_key(
            &self.path,
            &self.device_unlock_slot_path,
            &device_key,
        ) {
            Ok(vault) => {
                self.with_vault_guard(|guard| {
                    *guard = Some(vault);
                });
                self.set_auto_unlock_failure(None);
            }
            Err(_) => {
                self.set_auto_unlock_failure(Some(VaultAutoUnlockFailure::DeviceUnlockRejected))
            }
        }
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

    pub(crate) fn open_synchronized_key_material(
        &self,
        envelope: &[u8],
        vault_password: &[u8],
    ) -> Result<VaultSyncKeyMaterial, VaultServiceError> {
        self.with_vault_guard(|guard| {
            let vault = guard.as_ref().ok_or(VaultServiceError::NotUnlocked)?;
            vault.verify_password(vault_password)?;
            // Keep the original Vault mutex lifetime across verification and
            // envelope opening. An explicit lock must not interleave between
            // these two halves of the synchronized-key operation.
            Ok(open_sync_key_material(envelope, vault_password)?)
        })
    }

    pub(crate) fn create(&self, password: &[u8]) -> Result<VaultStatus, VaultServiceError> {
        self.with_vault_guard(|guard| {
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
        self.with_vault_guard(|guard| {
            if guard.as_ref().is_some_and(|vault| !vault.requires_reload()) {
                return Err(VaultServiceError::AlreadyUnlocked);
            }
            guard.take();
            *guard = Some(UnlockedVault::unlock(&self.path, password)?);
            Ok(self.status_from(guard.as_ref()))
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

    fn revoke_auto_unlock(&self) -> Result<(), VaultServiceError> {
        if !self.auto_unlock_enabled_path.exists()
            && !self.device_unlock_slot_path.exists()
            && !self.auto_unlock_blocked_path.exists()
        {
            self.set_auto_unlock_failure(None);
            return Ok(());
        }
        self.write_auto_unlock_marker(
            &self.auto_unlock_blocked_path,
            b"norishell-auto-unlock-disabled-v1\n",
        )?;
        let marker_result = match std::fs::remove_file(&self.auto_unlock_enabled_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        let slot_result = match std::fs::remove_file(&self.device_unlock_slot_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        let delete_result = self.device_key_store.delete();
        self.set_auto_unlock_failure(None);
        marker_result
            .map_err(VaultError::Io)
            .map_err(VaultServiceError::from)?;
        slot_result
            .map_err(VaultError::Io)
            .map_err(VaultServiceError::from)?;
        delete_result.map_err(VaultServiceError::from)
    }

    fn write_auto_unlock_marker(&self, path: &Path, value: &[u8]) -> Result<(), VaultServiceError> {
        let parent = path
            .parent()
            .ok_or_else(|| VaultError::Io(std::io::Error::other("invalid Vault path")))?;
        std::fs::create_dir_all(parent).map_err(VaultError::Io)?;
        if path.exists() {
            let mut file = std::fs::OpenOptions::new()
                .truncate(true)
                .write(true)
                .open(path)
                .map_err(VaultError::Io)?;
            use std::io::Write as _;
            file.write_all(value).map_err(VaultError::Io)?;
            file.sync_all().map_err(VaultError::Io)?;
            return Ok(());
        }
        let temporary = path.with_extension("marker.staging");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(VaultError::Io)?;
        use std::io::Write as _;
        file.write_all(value).map_err(VaultError::Io)?;
        file.sync_all().map_err(VaultError::Io)?;
        std::fs::rename(&temporary, path).map_err(VaultError::Io)?;
        Ok(())
    }

    fn enable_auto_unlock(&self, password: &[u8]) -> Result<VaultStatus, VaultServiceError> {
        self.with_vault_guard(|guard| {
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

            self.write_auto_unlock_marker(
                &self.auto_unlock_blocked_path,
                b"norishell-auto-unlock-enabling-v1\n",
            )?;

            match std::fs::remove_file(&self.auto_unlock_enabled_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(VaultError::Io(error).into()),
            }

            let device_key = DeviceUnlockKey::generate()?;
            self.device_key_store.store(&device_key)?;
            if let Err(error) =
                vault.write_device_unlock_slot(&self.device_unlock_slot_path, &device_key)
            {
                let _ = self.device_key_store.delete();
                let _ = std::fs::remove_file(&self.device_unlock_slot_path);
                return Err(error.into());
            }
            if let Err(error) = self.write_auto_unlock_marker(
                &self.auto_unlock_enabled_path,
                b"norishell-auto-unlock-v1\n",
            ) {
                let _ = std::fs::remove_file(&self.device_unlock_slot_path);
                let _ = self.device_key_store.delete();
                return Err(error);
            }
            if let Err(error) = std::fs::remove_file(&self.auto_unlock_blocked_path) {
                let _ = std::fs::remove_file(&self.auto_unlock_enabled_path);
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
        self.revoke_auto_unlock()?;
        Ok(self.status())
    }

    fn lock(&self) -> Result<VaultStatus, VaultServiceError> {
        let revoke_result = self.revoke_auto_unlock();
        self.with_vault_guard(|guard| {
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
        self.with_vault_guard(|guard| {
            let vault = guard.as_mut().ok_or(VaultServiceError::NotUnlocked)?;
            vault.change_password(current_password, new_password)?;
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
    service
        .enable_auto_unlock(&password)
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
        VaultServiceError::AvailabilityChanged => (
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
        VaultServiceError::Vault(VaultError::AlreadyExists) => (
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
    use std::sync::{Arc, Mutex};

    use norishell_core_api::{SecretRefId, VaultAutoUnlockFailure, VaultState, VaultUnlockPolicy};
    use norishell_secret_vault::{DeviceUnlockKey, SecretKind, VaultError};
    use uuid::Uuid;
    use zeroize::Zeroizing;

    use crate::device_key_store::{DeviceKeyStore, DeviceKeyStoreError};

    use super::{
        VaultSecretInsert, VaultService, VaultServiceError, unlock_with_non_blocking_follow_up,
    };

    const PASSWORD: &[u8] = b"correct horse battery staple";

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
    fn synchronized_key_material_requires_the_current_local_vault_password() {
        let source_directory = tempfile::tempdir().expect("source tempdir");
        let source = VaultService::start(source_directory.path());
        source.create(PASSWORD).expect("create source Vault");
        let exported = source
            .export_sync_key_material()
            .expect("export synchronized key material");

        let target_directory = tempfile::tempdir().expect("target tempdir");
        let target = VaultService::start(target_directory.path());
        target.create(PASSWORD).expect("create target Vault");
        let opened = target
            .open_synchronized_key_material(exported.envelope(), PASSWORD)
            .expect("same local Vault password opens synchronized key");
        assert_eq!(opened.key_bytes(), exported.key_bytes());
        assert!(
            target
                .open_synchronized_key_material(
                    exported.envelope(),
                    b"different local vault password",
                )
                .is_err()
        );
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
