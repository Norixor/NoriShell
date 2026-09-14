//! Core-only lifecycle for plugin-owned network credentials.
//!
//! Public plugin messages contain opaque handles only. Secret bytes enter this
//! service from a protected renderer and leave only as a short-lived network
//! injection lease guarded by the current runtime fence and durable revision.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use norishell_app_persistence::{
    AppPersistenceError, PluginCredentialCreateBegin, PluginCredentialCreateInput,
    PluginCredentialDurableState, PluginCredentialOwner, PluginCredentialRecord,
};
use norishell_core_api::{
    PluginCredentialInjection, PluginCredentialSummary, PluginCredentialTarget, PluginId,
    PluginNetworkCredentialRef, VaultState, WireSequence,
};
use norishell_secret_vault::{SecretKind, VaultError};
use reqwest::{Url, header::HeaderName};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{
    host_service::HostService,
    plugin_api::{ResourceFence, ResourceOwner},
    vault_service::{VaultSecretInsert, VaultService, VaultServiceError},
};

#[derive(Clone)]
pub(crate) struct PluginCredentialService {
    hosts: HostService,
    vault: VaultService,
    lifecycle: Arc<Mutex<()>>,
}

#[derive(Debug, Clone)]
pub(crate) struct PluginCredentialIntent {
    record: PluginCredentialRecord,
}

#[derive(Debug, Clone)]
pub(crate) enum PluginCredentialPreparation {
    Intent(PluginCredentialIntent),
    Existing(PluginCredentialSummary),
}

pub(crate) struct CredentialLease {
    pub(crate) header_name: String,
    pub(crate) header_value: Zeroizing<Vec<u8>>,
    pub(crate) fence: ResourceFence,
}

impl std::fmt::Debug for CredentialLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialLease")
            .field("header_name", &self.header_name)
            .field("header_value", &"[REDACTED]")
            .field("fence", &"[RESOURCE FENCE]")
            .finish()
    }
}

#[derive(Debug, Error)]
pub(crate) enum PluginCredentialServiceError {
    #[error("invalid plugin credential request")]
    InvalidRequest,
    #[error("plugin credential was not found")]
    NotFound,
    #[error("plugin credential revision or owner changed")]
    Conflict,
    #[error("plugin credential is revoked or not ready")]
    Revoked,
    #[error("vault does not exist")]
    VaultMissing,
    #[error("vault is locked")]
    VaultLocked,
    #[error("vault must be reloaded")]
    VaultRequiresReload,
    #[error("plugin credential persistence failed")]
    Persistence,
    #[error("plugin credential Vault operation failed")]
    Vault,
}

impl PluginCredentialService {
    pub(crate) fn new(hosts: HostService, vault: VaultService) -> Self {
        Self {
            hosts,
            vault,
            lifecycle: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn prepare_create(
        &self,
        owner: &ResourceOwner,
        operation_id: String,
        idempotency_key: String,
        label: String,
        target: PluginCredentialTarget,
        fence: ResourceFence,
    ) -> Result<PluginCredentialPreparation, PluginCredentialServiceError> {
        validate_target(&target)?;
        require_fence(&fence)?;
        let _lifecycle = self.lifecycle();
        let input = PluginCredentialCreateInput {
            owner: durable_owner(owner),
            operation_id,
            idempotency_key,
            label,
            target,
        };
        let begin = match self.hosts.with_plugin_repository(|repository| {
            repository.begin_plugin_credential_create(&input, fence.as_ref())
        }) {
            Ok(begin) => begin,
            Err(error) => {
                if let Ok(Some(record)) = self.hosts.with_plugin_repository(|repository| {
                    repository.find_plugin_credential_create(&input)
                }) && record.state == PluginCredentialDurableState::PendingVault
                    && let Ok(cleanup) = self.mark_cleanup_pending(&record, Arc::new(|| true))
                {
                    let _ = self.finish_cleanup(&cleanup);
                }
                return Err(map_persistence_error(error));
            }
        };
        if let Err(error) = require_fence(&fence) {
            if let PluginCredentialCreateBegin::Created(record) = &begin
                && let Ok(cleanup) = self.mark_cleanup_pending(record, Arc::new(|| true))
            {
                let _ = self.finish_cleanup(&cleanup);
            }
            return Err(error);
        }
        match begin {
            PluginCredentialCreateBegin::Created(record) => Ok(
                PluginCredentialPreparation::Intent(PluginCredentialIntent { record }),
            ),
            PluginCredentialCreateBegin::Existing(record)
                if record.state == PluginCredentialDurableState::Ready =>
            {
                Ok(PluginCredentialPreparation::Existing(record.summary()))
            }
            PluginCredentialCreateBegin::Existing(record)
                if matches!(
                    record.state,
                    PluginCredentialDurableState::CleanupPending
                        | PluginCredentialDurableState::Revoked
                ) =>
            {
                Err(PluginCredentialServiceError::Revoked)
            }
            PluginCredentialCreateBegin::Existing(_) => Err(PluginCredentialServiceError::Conflict),
        }
    }

    pub(crate) fn finish_create(
        &self,
        intent: PluginCredentialIntent,
        value: Zeroizing<Vec<u8>>,
        fence: ResourceFence,
    ) -> Result<PluginCredentialSummary, PluginCredentialServiceError> {
        require_fence(&fence)?;
        let _lifecycle = self.lifecycle();
        let current = self.require_exact_intent(&intent)?;
        if current.state != PluginCredentialDurableState::PendingVault {
            return Err(PluginCredentialServiceError::Conflict);
        }
        if !valid_secret_value(&value) {
            if let Ok(cleanup) = self.mark_cleanup_pending(&current, Arc::new(|| true)) {
                let _ = self.finish_cleanup(&cleanup);
            }
            return Err(PluginCredentialServiceError::InvalidRequest);
        }
        let insert = self.vault.insert_secrets(&[VaultSecretInsert {
            secret_ref_id: current.secret_ref_id.clone(),
            kind: SecretKind::PluginCredential,
            value,
        }]);
        if let Err(error) = insert {
            if let Ok(cleanup) = self.mark_cleanup_pending(&current, Arc::new(|| true))
                && error.secret_insert_definitely_absent()
            {
                let _ = self.finish_cleanup(&cleanup);
            }
            return Err(self.map_vault_error(error));
        }
        if let Err(error) = require_fence(&fence) {
            let cleanup = self.mark_cleanup_pending(&current, Arc::new(|| true));
            if let Ok(cleanup) = cleanup {
                let _ = self.finish_cleanup(&cleanup);
            }
            return Err(error);
        }
        match self.hosts.with_plugin_repository(|repository| {
            repository.mark_plugin_credential_ready(&current, fence.as_ref())
        }) {
            Ok(ready) => Ok(ready.summary()),
            Err(error) => {
                // A failed/uncertain SQLite publish must never be reported as
                // ready. Reconciliation inspects the durable row: pending rows
                // become cleanup candidates; an already-ready row is retained.
                if let Ok(Some(observed)) = self.hosts.with_plugin_repository(|repository| {
                    repository.get_plugin_credential(&current.owner, &current.handle)
                }) && observed.state == PluginCredentialDurableState::PendingVault
                {
                    let cleanup = self.mark_cleanup_pending(&observed, Arc::new(|| true));
                    if let Ok(cleanup) = cleanup {
                        let _ = self.finish_cleanup(&cleanup);
                    }
                }
                Err(map_persistence_error(error))
            }
        }
    }

    pub(crate) fn cancel_create(
        &self,
        intent: &PluginCredentialIntent,
    ) -> Result<PluginCredentialSummary, PluginCredentialServiceError> {
        let _lifecycle = self.lifecycle();
        let current = self.require_exact_intent(intent)?;
        if current.state == PluginCredentialDurableState::Revoked {
            return Ok(current.summary());
        }
        let cleanup = if current.state == PluginCredentialDurableState::CleanupPending {
            current
        } else {
            self.mark_cleanup_pending(&current, Arc::new(|| true))?
        };
        match self.finish_cleanup(&cleanup) {
            Ok(revoked) => Ok(revoked.summary()),
            Err(
                PluginCredentialServiceError::VaultLocked
                | PluginCredentialServiceError::VaultMissing
                | PluginCredentialServiceError::VaultRequiresReload,
            ) => Ok(cleanup.summary()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn list(
        &self,
        owner: &ResourceOwner,
        fence: ResourceFence,
    ) -> Result<Vec<PluginCredentialSummary>, PluginCredentialServiceError> {
        require_fence(&fence)?;
        let _lifecycle = self.lifecycle();
        self.hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_credentials(&durable_owner(owner), fence.as_ref())
            })
            .map(|records| records.into_iter().map(|record| record.summary()).collect())
            .map_err(map_persistence_error)
    }

    pub(crate) fn revoke(
        &self,
        owner: &ResourceOwner,
        handle: &str,
        expected_revision: WireSequence,
        fence: ResourceFence,
    ) -> Result<PluginCredentialSummary, PluginCredentialServiceError> {
        require_fence(&fence)?;
        let _lifecycle = self.lifecycle();
        let cleanup = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.mark_plugin_credential_cleanup_pending(
                    &durable_owner(owner),
                    handle,
                    expected_revision,
                    fence.as_ref(),
                )
            })
            .map_err(map_persistence_error)?;
        if cleanup.state == PluginCredentialDurableState::Revoked {
            return Ok(cleanup.summary());
        }
        match self.finish_cleanup(&cleanup) {
            Ok(revoked) => Ok(revoked.summary()),
            Err(
                PluginCredentialServiceError::VaultLocked
                | PluginCredentialServiceError::VaultMissing
                | PluginCredentialServiceError::VaultRequiresReload,
            ) => Ok(cleanup.summary()),
            Err(error) => Err(error),
        }
    }

    /// Reconciles abandoned create/revoke intents after start or Vault unlock.
    pub(crate) fn cleanup_pending(
        &self,
    ) -> Result<Vec<PluginCredentialSummary>, PluginCredentialServiceError> {
        let _lifecycle = self.lifecycle();
        self.cleanup(None)
    }

    /// Must run once before plugin calls are admitted. It is the only runtime
    /// path that reclassifies `pending_vault` as abandoned.
    pub(crate) fn recover_startup(
        &self,
    ) -> Result<Vec<PluginCredentialSummary>, PluginCredentialServiceError> {
        let _lifecycle = self.lifecycle();
        let records = self
            .hosts
            .with_plugin_repository(|repository| repository.recover_plugin_credentials_startup())
            .map_err(map_persistence_error)?;
        self.cleanup_records(records)
    }

    /// Durable uninstall hook. It invalidates ready rows before attempting
    /// Vault deletion and therefore remains safe while the Vault is locked.
    pub(crate) fn cleanup_owner(
        &self,
        owner: &PluginCredentialOwner,
    ) -> Result<Vec<PluginCredentialSummary>, PluginCredentialServiceError> {
        let _lifecycle = self.lifecycle();
        self.cleanup(Some(owner))
    }

    /// Core uninstall/retry hook. This covers all signer identities retained
    /// for the plugin even when the installation row is already gone.
    pub(crate) fn cleanup_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Vec<PluginCredentialSummary>, PluginCredentialServiceError> {
        let _lifecycle = self.lifecycle();
        let records = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.prepare_plugin_credential_cleanup_by_plugin(plugin_id)
            })
            .map_err(map_persistence_error)?;
        self.cleanup_records(records)
    }

    pub(crate) fn lease(
        &self,
        owner: &ResourceOwner,
        credential_ref: &PluginNetworkCredentialRef,
        exact_origin: &str,
        outer_fence: ResourceFence,
    ) -> Result<CredentialLease, PluginCredentialServiceError> {
        validate_origin(exact_origin)?;
        require_fence(&outer_fence)?;
        let _lifecycle = self.lifecycle();
        let durable_owner = durable_owner(owner);
        let record = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_credential(&durable_owner, &credential_ref.handle)
            })
            .map_err(map_persistence_error)?
            .ok_or(PluginCredentialServiceError::NotFound)?;
        if record.state != PluginCredentialDurableState::Ready
            || record.revision != credential_ref.expected_revision
            || record.target.origin != exact_origin
        {
            return Err(PluginCredentialServiceError::Revoked);
        }
        let availability_before = self
            .vault
            .with_vault_availability(|availability, _| availability);
        if !availability_before.available {
            return Err(self.map_unavailable_vault());
        }
        let secret = self
            .vault
            .read_secret(&record.secret_ref_id, SecretKind::PluginCredential)
            .map_err(|error| self.map_vault_error(error))?;
        let availability_after = self
            .vault
            .with_vault_availability(|availability, _| availability);
        if !availability_after.available || availability_after.epoch != availability_before.epoch {
            return Err(self.map_unavailable_vault());
        }
        require_fence(&outer_fence)?;

        let (header_name, header_value) = match &record.target.injection {
            PluginCredentialInjection::Bearer {} => {
                let mut value = Zeroizing::new(Vec::with_capacity(7 + secret.expose().len()));
                value.extend_from_slice(b"Bearer ");
                value.extend_from_slice(secret.expose());
                ("authorization".to_owned(), value)
            }
            PluginCredentialInjection::Header { name } => {
                (name.clone(), Zeroizing::new(secret.expose().to_vec()))
            }
        };
        let hosts = self.hosts.clone();
        let handle = record.handle.clone();
        let revision = record.revision;
        let owner = record.owner.clone();
        let vault = self.vault.clone();
        let vault_epoch = availability_after.epoch;
        let row_fence = Arc::new(move || {
            outer_fence()
                && vault.with_vault_availability(|availability, _| {
                    availability.available && availability.epoch == vault_epoch
                })
                && hosts
                    .with_plugin_repository(|repository| {
                        Ok(repository
                            .get_plugin_credential(&owner, &handle)?
                            .is_some_and(|record| {
                                record.state == PluginCredentialDurableState::Ready
                                    && record.revision == revision
                            }))
                    })
                    .unwrap_or(false)
        });
        if !row_fence() {
            return Err(PluginCredentialServiceError::Revoked);
        }
        Ok(CredentialLease {
            header_name,
            header_value,
            fence: row_fence,
        })
    }

    fn cleanup(
        &self,
        owner: Option<&PluginCredentialOwner>,
    ) -> Result<Vec<PluginCredentialSummary>, PluginCredentialServiceError> {
        let records = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.prepare_plugin_credential_cleanup(owner)
            })
            .map_err(map_persistence_error)?;
        self.cleanup_records(records)
    }

    fn cleanup_records(
        &self,
        records: Vec<PluginCredentialRecord>,
    ) -> Result<Vec<PluginCredentialSummary>, PluginCredentialServiceError> {
        let mut summaries = Vec::with_capacity(records.len());
        for record in records {
            match self.finish_cleanup(&record) {
                Ok(revoked) => summaries.push(revoked.summary()),
                Err(
                    PluginCredentialServiceError::VaultLocked
                    | PluginCredentialServiceError::VaultMissing
                    | PluginCredentialServiceError::VaultRequiresReload,
                ) => {
                    summaries.push(record.summary());
                }
                Err(error) => return Err(error),
            }
        }
        Ok(summaries)
    }

    fn finish_cleanup(
        &self,
        cleanup: &PluginCredentialRecord,
    ) -> Result<PluginCredentialRecord, PluginCredentialServiceError> {
        self.vault
            .delete_secrets(std::slice::from_ref(&cleanup.secret_ref_id))
            .map_err(|error| self.map_vault_error(error))?;
        self.hosts
            .with_plugin_repository(|repository| {
                repository.finish_plugin_credential_cleanup(cleanup)
            })
            .map_err(map_persistence_error)
    }

    fn mark_cleanup_pending(
        &self,
        record: &PluginCredentialRecord,
        fence: ResourceFence,
    ) -> Result<PluginCredentialRecord, PluginCredentialServiceError> {
        self.hosts
            .with_plugin_repository(|repository| {
                repository.mark_plugin_credential_cleanup_pending(
                    &record.owner,
                    &record.handle,
                    record.revision,
                    fence.as_ref(),
                )
            })
            .map_err(map_persistence_error)
    }

    fn require_exact_intent(
        &self,
        intent: &PluginCredentialIntent,
    ) -> Result<PluginCredentialRecord, PluginCredentialServiceError> {
        let current = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_credential(&intent.record.owner, &intent.record.handle)
            })
            .map_err(map_persistence_error)?
            .ok_or(PluginCredentialServiceError::NotFound)?;
        if current != intent.record {
            return Err(PluginCredentialServiceError::Conflict);
        }
        Ok(current)
    }

    fn lifecycle(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn map_vault_error(&self, error: VaultServiceError) -> PluginCredentialServiceError {
        match error {
            VaultServiceError::NotUnlocked => match self.vault.status().state {
                VaultState::Missing => PluginCredentialServiceError::VaultMissing,
                VaultState::Locked => PluginCredentialServiceError::VaultLocked,
                VaultState::RequiresReload => PluginCredentialServiceError::VaultRequiresReload,
                VaultState::Unlocked => PluginCredentialServiceError::Vault,
            },
            VaultServiceError::Vault(VaultError::ReloadRequired)
            | VaultServiceError::Vault(VaultError::CommitStateUnknown(_)) => {
                PluginCredentialServiceError::VaultRequiresReload
            }
            _ => PluginCredentialServiceError::Vault,
        }
    }

    fn map_unavailable_vault(&self) -> PluginCredentialServiceError {
        match self.vault.status().state {
            VaultState::Missing => PluginCredentialServiceError::VaultMissing,
            VaultState::Locked => PluginCredentialServiceError::VaultLocked,
            VaultState::RequiresReload | VaultState::Unlocked => {
                PluginCredentialServiceError::VaultRequiresReload
            }
        }
    }
}

fn durable_owner(owner: &ResourceOwner) -> PluginCredentialOwner {
    PluginCredentialOwner {
        plugin_id: owner.plugin_id.clone(),
        signer_fingerprint_sha256: owner.signer.clone(),
    }
}

fn validate_target(target: &PluginCredentialTarget) -> Result<(), PluginCredentialServiceError> {
    validate_origin(&target.origin)?;
    if let PluginCredentialInjection::Header { name } = &target.injection {
        let parsed = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| PluginCredentialServiceError::InvalidRequest)?;
        if is_forbidden_header(parsed.as_str()) {
            return Err(PluginCredentialServiceError::InvalidRequest);
        }
    }
    Ok(())
}

fn valid_secret_value(value: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= 4_096
        && value
            .iter()
            .all(|byte| *byte == b'\t' || matches!(*byte, 0x20..=0x7e))
}

fn validate_origin(origin: &str) -> Result<(), PluginCredentialServiceError> {
    let url = Url::parse(origin).map_err(|_| PluginCredentialServiceError::InvalidRequest)?;
    if !matches!(url.scheme(), "https" | "wss")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.origin().ascii_serialization() != origin
    {
        return Err(PluginCredentialServiceError::InvalidRequest);
    }
    Ok(())
}

fn is_forbidden_header(name: &str) -> bool {
    let lowercase = name.to_ascii_lowercase();
    lowercase.starts_with("proxy-")
        || lowercase.starts_with("sec-websocket-")
        || matches!(
            lowercase.as_str(),
            "cookie"
                | "set-cookie"
                | "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "te"
                | "trailer"
                | "upgrade"
                | "proxy-authorization"
                | "proxy-authenticate"
                | "keep-alive"
                | "origin"
        )
}

fn require_fence(fence: &ResourceFence) -> Result<(), PluginCredentialServiceError> {
    if fence() {
        Ok(())
    } else {
        Err(PluginCredentialServiceError::Revoked)
    }
}

fn map_persistence_error(error: AppPersistenceError) -> PluginCredentialServiceError {
    match error {
        AppPersistenceError::InvalidInput(_) => PluginCredentialServiceError::InvalidRequest,
        AppPersistenceError::NotFound => PluginCredentialServiceError::NotFound,
        AppPersistenceError::Conflict | AppPersistenceError::IdempotencyConflict => {
            PluginCredentialServiceError::Conflict
        }
        _ => PluginCredentialServiceError::Persistence,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use norishell_core_api::{
        PluginCredentialInjection, PluginCredentialState, PluginCredentialTarget,
        PluginNetworkCredentialRef, WireSequence,
    };
    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::*;

    const VAULT_PASSWORD: &[u8] = b"correct horse battery staple";
    const TOKEN: &[u8] = b"p07-super-secret-token-never-in-sqlite";

    fn fence() -> ResourceFence {
        Arc::new(|| true)
    }

    fn resource_owner(plugin_id: &str, signer: char) -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse(plugin_id).expect("plugin id"),
            signer: signer.to_string().repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        }
    }

    fn install_plugin(directory: &TempDir, owner: &ResourceOwner) {
        let connection = Connection::open(directory.path().join("ssh").join("norishell.sqlite3"))
            .expect("open application database");
        connection
            .execute(
                "INSERT INTO plugin_installations
                 (plugin_id, name, publisher, signer_fingerprint_sha256,
                  active_version, package_sha256, capabilities_json, state,
                  state_version, installed_at_ms, updated_at_ms)
                 VALUES (?1, 'Fixture', 'Fixture', ?2, '1.0.0', ?3,
                         '[\"credentialsPlugin\"]', 'enabled', 1, 1, 1)",
                rusqlite::params![owner.plugin_id.as_str(), owner.signer, owner.package],
            )
            .expect("install fixture plugin");
    }

    fn setup() -> (TempDir, HostService, VaultService, ResourceOwner) {
        let directory = TempDir::new().expect("temp directory");
        let hosts = HostService::start(directory.path()).expect("host service");
        let owner = resource_owner("org.example.credentials", 'a');
        install_plugin(&directory, &owner);
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(VAULT_PASSWORD)
            .expect("create Vault");
        (directory, hosts, vault, owner)
    }

    fn target() -> PluginCredentialTarget {
        PluginCredentialTarget {
            origin: "https://api.example.test".to_owned(),
            injection: PluginCredentialInjection::Bearer {},
        }
    }

    fn prepare(
        service: &PluginCredentialService,
        owner: &ResourceOwner,
        suffix: &str,
    ) -> PluginCredentialIntent {
        match service
            .prepare_create(
                owner,
                format!("operation-{suffix}"),
                format!("idempotency-{suffix}"),
                "API token".to_owned(),
                target(),
                fence(),
            )
            .expect("prepare credential")
        {
            PluginCredentialPreparation::Intent(intent) => intent,
            PluginCredentialPreparation::Existing(_) => panic!("unexpected replay"),
        }
    }

    #[test]
    fn create_is_idempotent_and_lease_is_owner_origin_revision_and_vault_epoch_bound() {
        let (directory, hosts, vault, owner) = setup();
        let service = PluginCredentialService::new(hosts, vault.clone());
        let intent = prepare(&service, &owner, "create");

        assert!(matches!(
            service.prepare_create(
                &owner,
                "operation-create".to_owned(),
                "idempotency-create".to_owned(),
                "API token".to_owned(),
                target(),
                fence(),
            ),
            Err(PluginCredentialServiceError::Conflict)
        ));

        let late_cancel = intent.clone();
        let ready = service
            .finish_create(intent, Zeroizing::new(TOKEN.to_vec()), fence())
            .expect("finish credential");
        assert_eq!(ready.state, PluginCredentialState::Ready);
        assert!(matches!(
            service.cancel_create(&late_cancel),
            Err(PluginCredentialServiceError::Conflict)
        ));
        assert!(matches!(
            service
                .prepare_create(
                    &owner,
                    "operation-create".to_owned(),
                    "idempotency-create".to_owned(),
                    "API token".to_owned(),
                    target(),
                    fence(),
                )
                .expect("ready replay"),
            PluginCredentialPreparation::Existing(summary) if summary == ready
        ));

        let lease = service
            .lease(
                &owner,
                &PluginNetworkCredentialRef {
                    handle: ready.handle.clone(),
                    expected_revision: ready.revision,
                },
                "https://api.example.test",
                fence(),
            )
            .expect("credential lease");
        assert_eq!(lease.header_name, "authorization");
        assert_eq!(
            lease.header_value.as_slice(),
            b"Bearer p07-super-secret-token-never-in-sqlite"
        );
        assert!((lease.fence)());
        assert!(matches!(
            service.lease(
                &owner,
                &PluginNetworkCredentialRef {
                    handle: ready.handle.clone(),
                    expected_revision: ready.revision,
                },
                "https://other.example.test",
                fence(),
            ),
            Err(PluginCredentialServiceError::Revoked)
        ));
        let other_signer = resource_owner("org.example.credentials", 'c');
        assert!(matches!(
            service.lease(
                &other_signer,
                &PluginNetworkCredentialRef {
                    handle: ready.handle.clone(),
                    expected_revision: ready.revision,
                },
                "https://api.example.test",
                fence(),
            ),
            Err(PluginCredentialServiceError::NotFound)
        ));

        vault.lock_for_tests().expect("lock Vault");
        assert!(
            !(lease.fence)(),
            "a lease must expire when the Vault epoch changes"
        );

        for path in [
            directory.path().join("ssh").join("norishell.sqlite3"),
            directory.path().join("ssh").join("norishell.sqlite3-wal"),
        ] {
            if let Ok(bytes) = fs::read(path) {
                assert!(
                    !bytes.windows(TOKEN.len()).any(|window| window == TOKEN),
                    "SQLite metadata must not contain credential plaintext"
                );
            }
        }
    }

    #[test]
    fn locked_revoke_survives_restart_and_cleanup_is_idempotent() {
        let (directory, hosts, vault, owner) = setup();
        let service = PluginCredentialService::new(hosts.clone(), vault.clone());
        let intent = prepare(&service, &owner, "revoke");
        let ready = service
            .finish_create(intent, Zeroizing::new(TOKEN.to_vec()), fence())
            .expect("finish credential");
        drop(service);
        drop(vault);

        let locked_vault = VaultService::start(directory.path());
        let restarted = PluginCredentialService::new(hosts, locked_vault.clone());
        let pending = restarted
            .revoke(&owner, &ready.handle, ready.revision, fence())
            .expect("durable locked revoke");
        assert_eq!(pending.state, PluginCredentialState::CleanupPending);
        assert!(matches!(
            restarted.lease(
                &owner,
                &PluginNetworkCredentialRef {
                    handle: ready.handle.clone(),
                    expected_revision: ready.revision,
                },
                "https://api.example.test",
                fence(),
            ),
            Err(PluginCredentialServiceError::Revoked)
        ));
        assert_eq!(
            restarted.cleanup_pending().expect("locked cleanup")[0].state,
            PluginCredentialState::CleanupPending
        );

        locked_vault
            .unlock_for_protected_operation(VAULT_PASSWORD)
            .expect("unlock Vault");
        let reconciled = restarted.cleanup_pending().expect("reconcile cleanup");
        assert_eq!(reconciled[0].state, PluginCredentialState::Revoked);
        assert!(
            restarted
                .cleanup_pending()
                .expect("repeat cleanup")
                .is_empty()
        );
    }

    #[test]
    fn startup_recovery_alone_classifies_abandoned_create_and_uninstall_cleanup_needs_no_install_row()
     {
        let (directory, hosts, vault, owner) = setup();
        let service = PluginCredentialService::new(hosts.clone(), vault.clone());
        let _abandoned = prepare(&service, &owner, "abandoned");
        assert!(
            service
                .cleanup_pending()
                .expect("runtime cleanup")
                .is_empty()
        );
        assert_eq!(
            service.list(&owner, fence()).expect("list")[0].state,
            PluginCredentialState::PendingVault
        );

        let recovered = service.recover_startup().expect("startup recovery");
        assert_eq!(recovered[0].state, PluginCredentialState::Revoked);

        let ready = service
            .finish_create(
                prepare(&service, &owner, "uninstall"),
                Zeroizing::new(TOKEN.to_vec()),
                fence(),
            )
            .expect("ready credential");
        let connection = Connection::open(directory.path().join("ssh").join("norishell.sqlite3"))
            .expect("open application database");
        connection
            .execute(
                "DELETE FROM plugin_installations WHERE plugin_id = ?1",
                [owner.plugin_id.as_str()],
            )
            .expect("remove installation row");
        drop(connection);
        let cleaned = service
            .cleanup_plugin(&owner.plugin_id)
            .expect("cleanup removed plugin");
        assert!(cleaned.iter().any(|summary| {
            summary.handle == ready.handle && summary.state == PluginCredentialState::Revoked
        }));
    }

    #[test]
    fn post_intent_fence_failure_and_invalid_header_bytes_leave_only_revoked_metadata() {
        let (_directory, hosts, vault, owner) = setup();
        let service = PluginCredentialService::new(hosts, vault);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_fence = Arc::clone(&calls);
        let expiring_fence: ResourceFence =
            Arc::new(move || calls_for_fence.fetch_add(1, Ordering::SeqCst) < 3);
        assert!(matches!(
            service.prepare_create(
                &owner,
                "operation-expired".to_owned(),
                "idempotency-expired".to_owned(),
                "Expired".to_owned(),
                target(),
                expiring_fence,
            ),
            Err(PluginCredentialServiceError::Revoked)
        ));
        assert_eq!(
            service.list(&owner, fence()).expect("list expired")[0].state,
            PluginCredentialState::Revoked
        );

        let invalid = prepare(&service, &owner, "invalid-value");
        assert!(matches!(
            service.finish_create(
                invalid,
                Zeroizing::new(b"header\r\nsmuggling".to_vec()),
                fence(),
            ),
            Err(PluginCredentialServiceError::InvalidRequest)
        ));
        assert!(
            service
                .list(&owner, fence())
                .expect("list invalid")
                .iter()
                .all(|summary| summary.state == PluginCredentialState::Revoked)
        );
    }
}
