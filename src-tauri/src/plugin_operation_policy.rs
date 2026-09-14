//! Remembered decisions replace an exact-operation prompt, never the underlying capability.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use norishell_app_persistence::{
    AppPersistenceError, OperationPermissionBinding, PluginInstalledRecord,
    PluginPermissionBinding, RememberOperationPermission,
};
use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PluginApprovalExpiry, PluginApprovalOperation, PluginCapability,
    PluginId, PluginOperationPermissionList, WireSequence,
};
use norishell_secret_vault::ApprovalFingerprintKey;
use serde::Serialize;
use zeroize::Zeroizing;

use crate::{host_service::HostService, time::unix_time_ms};

type Result<T> = std::result::Result<T, AppPersistenceError>;

#[derive(Clone)]
pub(crate) struct OperationPolicyService {
    hosts: HostService,
    key: Option<Arc<ApprovalFingerprintKey>>,
    /// Serializes final admission with explicit policy revocation. I/O is never performed
    /// under this lock; after admission a revoke cancels work rather than claiming rollback.
    dispatch: Arc<Mutex<()>>,
}

#[derive(Clone)]
pub(crate) struct PreparedOperationPolicy {
    service: OperationPolicyService,
    binding: OperationPermissionBinding,
    fingerprint: [u8; 32],
    operation: PluginApprovalOperation,
    action_label: String,
    target_label: String,
    policy_revision: Option<WireSequence>,
}

#[derive(Clone)]
pub(crate) struct ApprovedOperationPolicy {
    service: OperationPolicyService,
    prepared: PreparedOperationPolicy,
    policy_revision: WireSequence,
}

impl OperationPolicyService {
    pub fn new(hosts: HostService, key_path: &Path) -> Self {
        Self {
            hosts,
            key: ApprovalFingerprintKey::load_or_create(key_path)
                .ok()
                .map(Arc::new),
            dispatch: Arc::new(Mutex::new(())),
        }
    }

    /// Capture the exact policy revision when preparing the protected prompt. A stale
    /// prompt cannot recreate a permission after the user has revoked it elsewhere.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &self,
        installed: &PluginInstalledRecord,
        capability: PluginCapability,
        security_binding: PluginPermissionBinding,
        operation: PluginApprovalOperation,
        action_label: &str,
        target_label: &str,
        scope: &impl Serialize,
    ) -> Result<PreparedOperationPolicy> {
        let key = self.key.as_ref().ok_or(AppPersistenceError::InvalidInput(
            "operation policy key unavailable",
        ))?;
        let encoded = Zeroizing::new(
            serde_json::to_vec(&(1_u16, operation, scope))
                .map_err(|_| AppPersistenceError::InvalidInput("invalid operation policy"))?,
        );
        let fingerprint = key.fingerprint(&encoded);
        let (capability_revision, policy_revision) =
            self.hosts.with_plugin_repository(|repository| {
                let grant = repository
                    .list_plugin_capability_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        u64::from(PLUGIN_PROTOCOL_MAJOR),
                    )?
                    .into_iter()
                    .find(|grant| {
                        grant.capability == capability
                            && grant.granted
                            && grant.binding.as_ref() == Some(&security_binding)
                    })
                    .ok_or(AppPersistenceError::Conflict)?;
                Ok((
                    grant.state_version,
                    repository.plugin_operation_permission_policy_revision(&installed.plugin_id)?,
                ))
            })?;
        Ok(PreparedOperationPolicy {
            service: self.clone(),
            binding: OperationPermissionBinding {
                plugin_id: installed.plugin_id.clone(),
                signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
                package_sha256: installed.package_sha256.clone(),
                installation_revision: installed.state_version,
                capability,
                capability_major_version: u64::from(PLUGIN_PROTOCOL_MAJOR),
                capability_revision,
                security_binding,
            },
            fingerprint,
            operation,
            action_label: action_label.to_owned(),
            target_label: target_label.to_owned(),
            policy_revision,
        })
    }

    pub fn find(
        &self,
        prepared: &PreparedOperationPolicy,
    ) -> Result<Option<ApprovedOperationPolicy>> {
        self.hosts
            .with_plugin_repository(|repository| {
                repository.find_plugin_operation_permission(
                    &prepared.binding,
                    prepared.operation,
                    prepared.fingerprint,
                )
            })
            .map(|found| {
                found.map(|found| ApprovedOperationPolicy {
                    service: self.clone(),
                    prepared: prepared.clone(),
                    policy_revision: found.policy_revision,
                })
            })
    }

    pub fn remember(
        &self,
        prepared: &PreparedOperationPolicy,
        expiry: PluginApprovalExpiry,
    ) -> Result<ApprovedOperationPolicy> {
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = self.hosts.with_plugin_repository(|repository| {
            repository.remember_plugin_operation_permission(&RememberOperationPermission {
                binding: prepared.binding.clone(),
                fingerprint: prepared.fingerprint,
                operation: prepared.operation,
                action_label: prepared.action_label.clone(),
                target_label: prepared.target_label.clone(),
                expected_policy_revision: prepared.policy_revision,
                expires_at_unix_ms: remembered_expiry(expiry),
            })
        })?;
        Ok(ApprovedOperationPolicy {
            service: self.clone(),
            prepared: prepared.clone(),
            policy_revision: found.policy_revision,
        })
    }

    pub fn list(&self, plugin_id: &PluginId) -> Result<PluginOperationPermissionList> {
        self.hosts.with_plugin_repository(|repository| {
            repository.list_plugin_operation_permissions(plugin_id)
        })
    }

    /// Guest-visible operation summaries are limited to the exact active
    /// installation and capability binding; host management may use `list` to
    /// display historical or expired rows.
    pub fn list_current(
        &self,
        installed: &PluginInstalledRecord,
    ) -> Result<PluginOperationPermissionList> {
        self.hosts.with_plugin_repository(|repository| {
            repository.list_current_plugin_operation_permissions(installed)
        })
    }

    pub fn revoke(
        &self,
        plugin_id: &PluginId,
        permission_id: &str,
        expected: Option<WireSequence>,
    ) -> Result<WireSequence> {
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.hosts.with_plugin_repository(|repository| {
            repository.revoke_plugin_operation_permission(
                plugin_id,
                permission_id,
                expected.ok_or(AppPersistenceError::Conflict)?,
            )
        })
    }

    pub fn revoke_current(
        &self,
        installed: &PluginInstalledRecord,
        permission_id: &str,
        expected: WireSequence,
    ) -> Result<WireSequence> {
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.hosts.with_plugin_repository(|repository| {
            repository.revoke_current_plugin_operation_permission(
                installed,
                permission_id,
                expected,
            )
        })
    }

    pub fn clear(&self, plugin_id: &PluginId, expected: Option<WireSequence>) -> Result<()> {
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.hosts
            .with_plugin_repository(|repository| {
                repository.clear_plugin_operation_permissions(
                    plugin_id,
                    expected.ok_or(AppPersistenceError::Conflict)?,
                )
            })
            .map(|_| ())
    }

    pub fn clear_current(
        &self,
        installed: &PluginInstalledRecord,
        expected: Option<WireSequence>,
    ) -> Result<()> {
        let _dispatch = self
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.hosts
            .with_plugin_repository(|repository| {
                repository.clear_current_plugin_operation_permissions(
                    installed,
                    expected.ok_or(AppPersistenceError::Conflict)?,
                )
            })
            .map(|_| ())
    }
}

impl PreparedOperationPolicy {
    pub fn find(&self) -> Result<Option<ApprovedOperationPolicy>> {
        self.service.find(self)
    }

    pub fn remember(&self, expiry: PluginApprovalExpiry) -> Result<ApprovedOperationPolicy> {
        self.service.remember(self, expiry)
    }

    pub fn same_operation(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.fingerprint == other.fingerprint
            && self.operation == other.operation
    }
}

fn remembered_expiry(expiry: PluginApprovalExpiry) -> Option<i64> {
    const FIFTEEN_MINUTES_MS: i64 = 15 * 60 * 1_000;
    const ONE_HOUR_MS: i64 = 60 * 60 * 1_000;
    const TWENTY_FOUR_HOURS_MS: i64 = 24 * 60 * 60 * 1_000;

    match expiry {
        PluginApprovalExpiry::FifteenMinutes => {
            Some(unix_time_ms().saturating_add(FIFTEEN_MINUTES_MS))
        }
        PluginApprovalExpiry::OneHour => Some(unix_time_ms().saturating_add(ONE_HOUR_MS)),
        PluginApprovalExpiry::TwentyFourHours => {
            Some(unix_time_ms().saturating_add(TWENTY_FOUR_HOURS_MS))
        }
        PluginApprovalExpiry::Unlimited => None,
    }
}

impl ApprovedOperationPolicy {
    pub fn matches_prepared(&self, prepared: &PreparedOperationPolicy) -> bool {
        self.prepared.same_operation(prepared)
    }

    pub fn current(&self) -> bool {
        self.service.find(&self.prepared).is_ok_and(|found| {
            found.is_some_and(|found| found.policy_revision == self.policy_revision)
        })
    }

    /// The permission ordering point immediately before submitting the exact operation.
    pub fn admit_dispatch(&self) -> bool {
        let _dispatch = self
            .service
            .dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (
        tempfile::TempDir,
        OperationPolicyService,
        PluginInstalledRecord,
        PluginPermissionBinding,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let hosts = HostService::start(directory.path()).unwrap();
        let connection =
            rusqlite::Connection::open(directory.path().join("ssh/norishell.sqlite3")).unwrap();
        let plugin_id = PluginId::parse("org.norishell.policy-fixture").unwrap();
        let signer = "a".repeat(64);
        let package = "b".repeat(64);
        connection.execute(
            "INSERT INTO plugin_installations (plugin_id,name,publisher,signer_fingerprint_sha256,active_version,package_sha256,capabilities_json,state,state_version,installed_at_ms,updated_at_ms) VALUES (?1,'Policy fixture','NoriShell',?2,'1.0.0',?3,?4,'enabled',7,1,1)",
            rusqlite::params![plugin_id.as_str(), signer, package, serde_json::to_string(&vec![PluginCapability::RemoteExecRequest]).unwrap()],
        ).unwrap();
        connection.execute(
            "INSERT INTO plugin_capability_grants (plugin_id,signer_fingerprint_sha256,major_version,capability,granted,state_version,updated_at_ms,artifact_sha256,app_version_major,app_version_minor,secure_surface_contract_revision) VALUES (?1,?2,?3,'remote.exec.request',1,5,1,?4,0,1,9)",
            rusqlite::params![plugin_id.as_str(), signer, PLUGIN_PROTOCOL_MAJOR, package],
        ).unwrap();
        let installed = hosts
            .with_plugin_repository(|repository| repository.get_plugin_installation(&plugin_id))
            .unwrap();
        let binding = PluginPermissionBinding {
            artifact_sha256: package,
            app_version_major: 0,
            app_version_minor: 1,
            secure_surface_contract_revision: 9,
        };
        let service =
            OperationPolicyService::new(hosts, &directory.path().join("policy/approval.key"));
        (directory, service, installed, binding)
    }

    #[test]
    fn remembered_decision_survives_its_own_revision_change_but_not_revocation() {
        let (_directory, service, installed, binding) = fixture();
        let prepare = |payload: &str| {
            service
                .prepare(
                    &installed,
                    PluginCapability::RemoteExecRequest,
                    binding.clone(),
                    PluginApprovalOperation::RemoteExecute,
                    "inspect",
                    "QA Host",
                    &payload,
                )
                .unwrap()
        };
        let pending = prepare("exact command");
        let stale = prepare("different command");
        let approved = pending.remember(PluginApprovalExpiry::Unlimited).unwrap();
        let current = prepare("exact command");
        assert!(
            approved.matches_prepared(&current),
            "saving permission must not invalidate its own handoff"
        );
        assert!(approved.admit_dispatch());
        assert!(prepare("changed parameter").find().unwrap().is_none());
        assert!(
            stale.remember(PluginApprovalExpiry::Unlimited).is_err(),
            "an older prompt cannot overwrite a new policy revision"
        );
        let before = service.list(&installed.plugin_id).unwrap();
        assert!(current.find().unwrap().unwrap().admit_dispatch());
        assert_eq!(
            before,
            service.list(&installed.plugin_id).unwrap(),
            "automatic use must not rewrite approval"
        );
        service
            .clear(&installed.plugin_id, before.policy_revision)
            .unwrap();
        assert!(!approved.admit_dispatch());
        assert!(
            current.remember(PluginApprovalExpiry::Unlimited).is_err(),
            "revoked pending decisions cannot resurrect permission"
        );
    }

    #[test]
    fn finite_remembered_decision_uses_core_deadline_and_stops_at_expiry() {
        let (directory, service, installed, binding) = fixture();
        let prepared = service
            .prepare(
                &installed,
                PluginCapability::RemoteExecRequest,
                binding,
                PluginApprovalOperation::RemoteExecute,
                "inspect",
                "QA Host",
                &"exact command",
            )
            .unwrap();
        let before = unix_time_ms();
        let approved = prepared
            .remember(PluginApprovalExpiry::FifteenMinutes)
            .unwrap();
        let permission = service
            .list(&installed.plugin_id)
            .unwrap()
            .permissions
            .into_iter()
            .next()
            .unwrap();
        let deadline = permission.expires_at_unix_ms.unwrap();
        assert!(deadline >= before.saturating_add(15 * 60 * 1_000));
        assert!(deadline <= unix_time_ms().saturating_add(15 * 60 * 1_000));
        assert!(approved.admit_dispatch());

        // Simulate elapsed wall time without waiting. The admission check must
        // reread the persisted absolute deadline before dispatch.
        let connection =
            rusqlite::Connection::open(directory.path().join("ssh/norishell.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE plugin_operation_permissions
                 SET expires_at_unix_ms = 1 WHERE permission_id = ?1",
                [permission.permission_id],
            )
            .unwrap();
        assert!(!approved.current());
        assert!(!approved.admit_dispatch());
        assert!(prepared.find().unwrap().is_none());
    }

    #[test]
    fn remembered_expiry_maps_each_bounded_choice_and_unlimited() {
        let before = unix_time_ms();
        assert_eq!(remembered_expiry(PluginApprovalExpiry::Unlimited), None);
        for (choice, duration_ms) in [
            (PluginApprovalExpiry::FifteenMinutes, 15 * 60 * 1_000),
            (PluginApprovalExpiry::OneHour, 60 * 60 * 1_000),
            (PluginApprovalExpiry::TwentyFourHours, 24 * 60 * 60 * 1_000),
        ] {
            let deadline = remembered_expiry(choice).unwrap();
            assert!(deadline >= before.saturating_add(duration_ms));
            assert!(deadline <= unix_time_ms().saturating_add(duration_ms));
        }
    }
}
