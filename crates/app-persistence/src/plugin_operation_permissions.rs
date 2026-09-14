//! Durable, exact, non-secret plugin operation approvals.
//!
//! These records never store an operation payload. The Core computes a keyed
//! fingerprint before calling this module, and the caller remains responsible
//! for rechecking live session, focus, lease, generation, and expiry fences.

use norishell_core_api::{
    PluginApprovalOperation, PluginCapability, PluginId, PluginInstallState,
    PluginOperationPermission, PluginOperationPermissionList, WireSequence,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    AppPersistenceError, AppRepository, PluginInstalledRecord, PluginPermissionBinding, Result,
    plugin_capability_to_db, plugin_install_state_to_db, u64_to_i64, unix_time_ms, validate_digest,
    validate_plugin_permission_binding,
};

const MAX_OPERATION_PERMISSIONS_PER_PLUGIN: i64 = 256;
type CapabilityBindingRow = (
    bool,
    i64,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);
const MAX_TARGET_LABEL_BYTES: usize = 512;
const MAX_ACTION_LABEL_BYTES: usize = 128;

/// The installation and granted capability facts which bind one remembered
/// operation to the precise protected surface that approved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationPermissionBinding {
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
    pub package_sha256: String,
    /// Used only when writing: a stale secure-surface operation cannot save a
    /// record after installation state has changed.
    pub installation_revision: WireSequence,
    pub capability: PluginCapability,
    pub capability_major_version: u64,
    pub capability_revision: WireSequence,
    pub security_binding: PluginPermissionBinding,
}

/// Core-generated input for a durable approval. There is intentionally no
/// user-controlled `granted` flag: only the secure approval path may call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RememberOperationPermission {
    pub binding: OperationPermissionBinding,
    pub fingerprint: [u8; 32],
    pub operation: PluginApprovalOperation,
    /// A Core-generated, non-secret stable target label; never a command,
    /// stdin fragment, raw input, or secret.
    pub target_label: String,
    /// A Core-generated, non-secret stable action name. This differentiates
    /// several catalog operations on the same target without storing a
    /// command, reason, or request payload.
    pub action_label: String,
    /// The policy fence captured when the secure surface opened.
    pub expected_policy_revision: Option<WireSequence>,
    /// An absolute Core-calculated deadline. `None` is an unlimited approval;
    /// it is never derived again while reopening this database.
    pub expires_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOperationPermissionMatch {
    pub permission: PluginOperationPermission,
    pub policy_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOperationPermissionClear {
    pub policy_revision: WireSequence,
    pub cleared_count: u32,
}

impl AppRepository {
    pub fn plugin_operation_permission_policy_revision(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<WireSequence>> {
        self.connection
            .query_row(
                "SELECT revision FROM plugin_operation_approval_policies WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                |row| {
                    let value = row.get::<_, i64>(0)?;
                    u64::try_from(value)
                        .map(WireSequence::new)
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })
                },
            )
            .optional()
            .map_err(AppPersistenceError::from)
    }

    pub fn list_plugin_operation_permissions(
        &self,
        plugin_id: &PluginId,
    ) -> Result<PluginOperationPermissionList> {
        let policy_revision = self.plugin_operation_permission_policy_revision(plugin_id)?;
        let mut statement = self.connection.prepare(
            "SELECT permission_id, operation, target_label, action_label, created_at_ms,
                    expires_at_unix_ms
             FROM plugin_operation_permissions
             WHERE plugin_id = ?1
             ORDER BY created_at_ms DESC, permission_id DESC",
        )?;
        let permissions = statement
            .query_map([plugin_id.as_str()], |row| {
                Ok(PluginOperationPermission {
                    permission_id: row.get(0)?,
                    plugin_id: plugin_id.clone(),
                    operation: operation_from_db(&row.get::<_, String>(1)?)?,
                    target_label: row.get(2)?,
                    action_label: row.get(3)?,
                    created_at_unix_ms: row.get(4)?,
                    expires_at_unix_ms: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(PluginOperationPermissionList {
            policy_revision,
            permissions,
        })
    }

    /// Lists only the operation records owned by this exact currently enabled
    /// installation and a still-current capability binding. Host management
    /// intentionally uses the broader history list above; a guest must never
    /// learn or operate on records made by a previous signer/package.
    pub fn list_current_plugin_operation_permissions(
        &self,
        installed: &PluginInstalledRecord,
    ) -> Result<PluginOperationPermissionList> {
        let policy_revision =
            self.plugin_operation_permission_policy_revision(&installed.plugin_id)?;
        let mut statement = self.connection.prepare(
            "SELECT permission.permission_id, permission.operation, permission.target_label,
                    permission.action_label, permission.created_at_ms, permission.expires_at_unix_ms
             FROM plugin_operation_permissions AS permission
             JOIN plugin_installations AS installation
               ON installation.plugin_id = permission.plugin_id
             JOIN plugin_capability_grants AS capability_grant
               ON capability_grant.plugin_id = permission.plugin_id
              AND capability_grant.signer_fingerprint_sha256 = permission.signer_fingerprint_sha256
              AND capability_grant.major_version = permission.capability_major_version
              AND capability_grant.capability = permission.capability
             WHERE permission.plugin_id = ?1
               AND permission.signer_fingerprint_sha256 = ?2
               AND permission.package_sha256 = ?3
               AND installation.signer_fingerprint_sha256 = ?2
               AND installation.package_sha256 = ?3
               AND installation.state = 'enabled'
               AND installation.state_version = ?4
               AND capability_grant.granted = 1
               AND capability_grant.state_version = permission.capability_revision
               AND capability_grant.artifact_sha256 = permission.artifact_sha256
               AND capability_grant.app_version_major = permission.app_version_major
               AND capability_grant.app_version_minor = permission.app_version_minor
               AND capability_grant.secure_surface_contract_revision
                     = permission.secure_surface_contract_revision
             ORDER BY permission.created_at_ms DESC, permission.permission_id DESC",
        )?;
        let permissions = statement
            .query_map(
                params![
                    installed.plugin_id.as_str(),
                    installed.signer_fingerprint_sha256,
                    installed.package_sha256,
                    u64_to_i64(installed.state_version.get())?,
                ],
                |row| {
                    Ok(PluginOperationPermission {
                        permission_id: row.get(0)?,
                        plugin_id: installed.plugin_id.clone(),
                        operation: operation_from_db(&row.get::<_, String>(1)?)?,
                        target_label: row.get(2)?,
                        action_label: row.get(3)?,
                        created_at_unix_ms: row.get(4)?,
                        expires_at_unix_ms: row.get(5)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(PluginOperationPermissionList {
            policy_revision,
            permissions,
        })
    }

    /// Finds an exact valid approval. This intentionally does not compare the
    /// stored installation revision: disabling and re-enabling the unchanged
    /// package must not erase a user decision. Package, signer, enabled state,
    /// capability epoch, and secure-surface binding are still matched exactly.
    pub fn find_plugin_operation_permission(
        &self,
        binding: &OperationPermissionBinding,
        operation: PluginApprovalOperation,
        fingerprint: [u8; 32],
    ) -> Result<Option<PluginOperationPermissionMatch>> {
        validate_operation_binding(binding, operation, None)?;
        self.connection
            .query_row(
                "SELECT permission.permission_id, permission.target_label, permission.action_label,
                        permission.created_at_ms, permission.expires_at_unix_ms,
                        policy.revision
                 FROM plugin_operation_permissions AS permission
                 JOIN plugin_operation_approval_policies AS policy
                   ON policy.plugin_id = permission.plugin_id
                 JOIN plugin_installations AS installation
                   ON installation.plugin_id = permission.plugin_id
                 JOIN plugin_capability_grants AS capability_grant
                   ON capability_grant.plugin_id = installation.plugin_id
                  AND capability_grant.signer_fingerprint_sha256 = installation.signer_fingerprint_sha256
                  AND capability_grant.major_version = permission.capability_major_version
                  AND capability_grant.capability = permission.capability
                 WHERE permission.plugin_id = ?1
                   AND permission.signer_fingerprint_sha256 = ?2
                   AND permission.package_sha256 = ?3
                   AND permission.operation = ?4
                   AND permission.capability = ?5
                   AND permission.capability_major_version = ?6
                   AND permission.capability_revision = ?7
                   AND permission.artifact_sha256 = ?8
                   AND permission.app_version_major = ?9
                   AND permission.app_version_minor = ?10
                   AND permission.secure_surface_contract_revision = ?11
                   AND permission.target_fingerprint = ?12
                   AND installation.signer_fingerprint_sha256 = ?2
                   AND installation.package_sha256 = ?3
                   AND installation.state = 'enabled'
                   AND capability_grant.granted = 1
                   AND capability_grant.state_version = ?7
                   AND capability_grant.artifact_sha256 = ?8
                   AND capability_grant.app_version_major = ?9
                   AND capability_grant.app_version_minor = ?10
                   AND capability_grant.secure_surface_contract_revision = ?11
                   AND (permission.expires_at_unix_ms IS NULL
                        OR permission.expires_at_unix_ms > ?13)",
                params![
                    binding.plugin_id.as_str(),
                    binding.signer_fingerprint_sha256,
                    binding.package_sha256,
                    operation_to_db(operation),
                    plugin_capability_to_db(binding.capability),
                    u64_to_i64(binding.capability_major_version)?,
                    u64_to_i64(binding.capability_revision.get())?,
                    binding.security_binding.artifact_sha256,
                    u64_to_i64(binding.security_binding.app_version_major)?,
                    u64_to_i64(binding.security_binding.app_version_minor)?,
                    u64_to_i64(binding.security_binding.secure_surface_contract_revision)?,
                    &fingerprint,
                    unix_time_ms(),
                ],
                |row| {
                    let revision = u64::try_from(row.get::<_, i64>(5)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?;
                    Ok(PluginOperationPermissionMatch {
                        permission: PluginOperationPermission {
                            permission_id: row.get(0)?,
                            plugin_id: binding.plugin_id.clone(),
                            operation,
                            target_label: row.get(1)?,
                            action_label: row.get(2)?,
                            created_at_unix_ms: row.get(3)?,
                            expires_at_unix_ms: row.get(4)?,
                        },
                        policy_revision: WireSequence::new(revision),
                    })
                },
            )
            .optional()
            .map_err(AppPersistenceError::from)
    }

    pub fn remember_plugin_operation_permission(
        &mut self,
        input: &RememberOperationPermission,
    ) -> Result<PluginOperationPermissionMatch> {
        if input
            .expires_at_unix_ms
            .is_some_and(|expires_at_unix_ms| expires_at_unix_ms <= unix_time_ms())
        {
            return Err(AppPersistenceError::InvalidInput(
                "operation permission expiry must be in the future",
            ));
        }
        validate_operation_binding(
            &input.binding,
            input.operation,
            Some((&input.target_label, &input.action_label)),
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        assert_current_binding(&transaction, &input.binding, true)?;
        let current_policy = policy_revision(&transaction, &input.binding.plugin_id)?;
        if current_policy != input.expected_policy_revision {
            return Err(AppPersistenceError::Conflict);
        }
        let already_present: bool = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM plugin_operation_permissions
                WHERE plugin_id = ?1 AND operation = ?2 AND target_fingerprint = ?3
             )",
            params![
                input.binding.plugin_id.as_str(),
                operation_to_db(input.operation),
                &input.fingerprint,
            ],
            |row| row.get(0),
        )?;
        if !already_present {
            let count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM plugin_operation_permissions WHERE plugin_id = ?1",
                [input.binding.plugin_id.as_str()],
                |row| row.get(0),
            )?;
            if count >= MAX_OPERATION_PERMISSIONS_PER_PLUGIN {
                return Err(AppPersistenceError::InvalidInput(
                    "too many remembered plugin operation permissions",
                ));
            }
        }
        let now = unix_time_ms();
        let next_policy = current_policy
            .map(next_policy_revision)
            .transpose()?
            .unwrap_or(1);
        transaction.execute(
            "INSERT INTO plugin_operation_approval_policies (plugin_id, revision, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(plugin_id) DO UPDATE SET revision = excluded.revision,
               updated_at_ms = excluded.updated_at_ms",
            params![
                input.binding.plugin_id.as_str(),
                u64_to_i64(next_policy)?,
                now,
            ],
        )?;
        let permission_id = if already_present {
            transaction.query_row(
                "SELECT permission_id FROM plugin_operation_permissions
                 WHERE plugin_id = ?1 AND operation = ?2 AND target_fingerprint = ?3",
                params![
                    input.binding.plugin_id.as_str(),
                    operation_to_db(input.operation),
                    &input.fingerprint,
                ],
                |row| row.get::<_, String>(0),
            )?
        } else {
            Uuid::new_v4().to_string()
        };
        transaction.execute(
            "INSERT INTO plugin_operation_permissions
             (permission_id, plugin_id, signer_fingerprint_sha256, package_sha256, operation,
              capability, capability_major_version, capability_revision, artifact_sha256,
              app_version_major, app_version_minor, secure_surface_contract_revision,
              target_fingerprint, target_label, action_label, created_at_ms, expires_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(plugin_id, operation, target_fingerprint) DO UPDATE SET
               signer_fingerprint_sha256 = excluded.signer_fingerprint_sha256,
               package_sha256 = excluded.package_sha256,
               capability = excluded.capability,
               capability_major_version = excluded.capability_major_version,
               capability_revision = excluded.capability_revision,
               artifact_sha256 = excluded.artifact_sha256,
               app_version_major = excluded.app_version_major,
               app_version_minor = excluded.app_version_minor,
               secure_surface_contract_revision = excluded.secure_surface_contract_revision,
               target_label = excluded.target_label,
               action_label = excluded.action_label,
               created_at_ms = excluded.created_at_ms,
               expires_at_unix_ms = excluded.expires_at_unix_ms",
            params![
                permission_id,
                input.binding.plugin_id.as_str(),
                input.binding.signer_fingerprint_sha256,
                input.binding.package_sha256,
                operation_to_db(input.operation),
                plugin_capability_to_db(input.binding.capability),
                u64_to_i64(input.binding.capability_major_version)?,
                u64_to_i64(input.binding.capability_revision.get())?,
                input.binding.security_binding.artifact_sha256,
                u64_to_i64(input.binding.security_binding.app_version_major)?,
                u64_to_i64(input.binding.security_binding.app_version_minor)?,
                u64_to_i64(
                    input
                        .binding
                        .security_binding
                        .secure_surface_contract_revision
                )?,
                &input.fingerprint,
                input.target_label,
                input.action_label,
                now,
                input.expires_at_unix_ms,
            ],
        )?;
        transaction.commit()?;
        Ok(PluginOperationPermissionMatch {
            permission: PluginOperationPermission {
                permission_id,
                plugin_id: input.binding.plugin_id.clone(),
                operation: input.operation,
                target_label: input.target_label.clone(),
                action_label: input.action_label.clone(),
                created_at_unix_ms: now,
                expires_at_unix_ms: input.expires_at_unix_ms,
            },
            policy_revision: WireSequence::new(next_policy),
        })
    }

    pub fn revoke_plugin_operation_permission(
        &mut self,
        plugin_id: &PluginId,
        permission_id: &str,
        expected_policy_revision: WireSequence,
    ) -> Result<WireSequence> {
        if Uuid::parse_str(permission_id).is_err() {
            return Err(AppPersistenceError::InvalidInput(
                "invalid operation permission id",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current =
            policy_revision(&transaction, plugin_id)?.ok_or(AppPersistenceError::NotFound)?;
        if current != expected_policy_revision {
            return Err(AppPersistenceError::Conflict);
        }
        let removed = transaction.execute(
            "DELETE FROM plugin_operation_permissions WHERE plugin_id = ?1 AND permission_id = ?2",
            params![plugin_id.as_str(), permission_id],
        )?;
        if removed != 1 {
            return Err(AppPersistenceError::NotFound);
        }
        let next = next_policy_revision(current)?;
        let changed = transaction.execute(
            "UPDATE plugin_operation_approval_policies
             SET revision = ?1, updated_at_ms = ?2 WHERE plugin_id = ?3 AND revision = ?4",
            params![
                u64_to_i64(next)?,
                unix_time_ms(),
                plugin_id.as_str(),
                u64_to_i64(current.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(WireSequence::new(next))
    }

    /// Guest-facing individual revoke. The permission id is not sufficient:
    /// the row must belong to the exact enabled package/signer snapshot and
    /// its capability security binding must still be effective.
    pub fn revoke_current_plugin_operation_permission(
        &mut self,
        installed: &PluginInstalledRecord,
        permission_id: &str,
        expected_policy_revision: WireSequence,
    ) -> Result<WireSequence> {
        if Uuid::parse_str(permission_id).is_err() {
            return Err(AppPersistenceError::InvalidInput(
                "invalid operation permission id",
            ));
        }
        validate_digest(&installed.signer_fingerprint_sha256)?;
        validate_digest(&installed.package_sha256)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = policy_revision(&transaction, &installed.plugin_id)?
            .ok_or(AppPersistenceError::NotFound)?;
        if current != expected_policy_revision {
            return Err(AppPersistenceError::Conflict);
        }
        assert_current_operation_permission_owner(&transaction, installed)?;
        let removed = transaction.execute(
            "DELETE FROM plugin_operation_permissions
             WHERE plugin_id = ?1 AND permission_id = ?2
               AND signer_fingerprint_sha256 = ?3 AND package_sha256 = ?4
               AND EXISTS (
                 SELECT 1 FROM plugin_capability_grants AS capability_grant
                  WHERE capability_grant.plugin_id = plugin_operation_permissions.plugin_id
                    AND capability_grant.signer_fingerprint_sha256
                          = plugin_operation_permissions.signer_fingerprint_sha256
                    AND capability_grant.major_version
                          = plugin_operation_permissions.capability_major_version
                    AND capability_grant.capability = plugin_operation_permissions.capability
                    AND capability_grant.granted = 1
                    AND capability_grant.state_version
                          = plugin_operation_permissions.capability_revision
                    AND capability_grant.artifact_sha256
                          = plugin_operation_permissions.artifact_sha256
                    AND capability_grant.app_version_major
                          = plugin_operation_permissions.app_version_major
                    AND capability_grant.app_version_minor
                          = plugin_operation_permissions.app_version_minor
                    AND capability_grant.secure_surface_contract_revision
                          = plugin_operation_permissions.secure_surface_contract_revision
               )",
            params![
                installed.plugin_id.as_str(),
                permission_id,
                installed.signer_fingerprint_sha256,
                installed.package_sha256,
            ],
        )?;
        if removed != 1 {
            return Err(AppPersistenceError::NotFound);
        }
        let next = next_policy_revision(current)?;
        let changed = transaction.execute(
            "UPDATE plugin_operation_approval_policies
             SET revision = ?1, updated_at_ms = ?2 WHERE plugin_id = ?3 AND revision = ?4",
            params![
                u64_to_i64(next)?,
                unix_time_ms(),
                installed.plugin_id.as_str(),
                u64_to_i64(current.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(WireSequence::new(next))
    }

    /// Clears remembered operations only for the exact current guest owner.
    /// Historical rows made by a previous signer/package intentionally remain
    /// available to host-side permission management.
    pub fn clear_current_plugin_operation_permissions(
        &mut self,
        installed: &PluginInstalledRecord,
        expected_policy_revision: WireSequence,
    ) -> Result<PluginOperationPermissionClear> {
        validate_digest(&installed.signer_fingerprint_sha256)?;
        validate_digest(&installed.package_sha256)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = policy_revision(&transaction, &installed.plugin_id)?
            .ok_or(AppPersistenceError::NotFound)?;
        if current != expected_policy_revision {
            return Err(AppPersistenceError::Conflict);
        }
        assert_current_operation_permission_owner(&transaction, installed)?;
        let cleared_count = transaction.execute(
            "DELETE FROM plugin_operation_permissions
             WHERE plugin_id = ?1
               AND signer_fingerprint_sha256 = ?2 AND package_sha256 = ?3
               AND EXISTS (
                 SELECT 1 FROM plugin_capability_grants AS capability_grant
                  WHERE capability_grant.plugin_id = plugin_operation_permissions.plugin_id
                    AND capability_grant.signer_fingerprint_sha256
                          = plugin_operation_permissions.signer_fingerprint_sha256
                    AND capability_grant.major_version
                          = plugin_operation_permissions.capability_major_version
                    AND capability_grant.capability = plugin_operation_permissions.capability
                    AND capability_grant.granted = 1
                    AND capability_grant.state_version
                          = plugin_operation_permissions.capability_revision
                    AND capability_grant.artifact_sha256
                          = plugin_operation_permissions.artifact_sha256
                    AND capability_grant.app_version_major
                          = plugin_operation_permissions.app_version_major
                    AND capability_grant.app_version_minor
                          = plugin_operation_permissions.app_version_minor
                    AND capability_grant.secure_surface_contract_revision
                          = plugin_operation_permissions.secure_surface_contract_revision
               )",
            params![
                installed.plugin_id.as_str(),
                installed.signer_fingerprint_sha256,
                installed.package_sha256,
            ],
        )?;
        let next = next_policy_revision(current)?;
        let changed = transaction.execute(
            "UPDATE plugin_operation_approval_policies
             SET revision = ?1, updated_at_ms = ?2 WHERE plugin_id = ?3 AND revision = ?4",
            params![
                u64_to_i64(next)?,
                unix_time_ms(),
                installed.plugin_id.as_str(),
                u64_to_i64(current.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(PluginOperationPermissionClear {
            policy_revision: WireSequence::new(next),
            cleared_count: u32::try_from(cleared_count)
                .map_err(|_| AppPersistenceError::InvalidStoredData)?,
        })
    }

    pub fn clear_plugin_operation_permissions(
        &mut self,
        plugin_id: &PluginId,
        expected_policy_revision: WireSequence,
    ) -> Result<PluginOperationPermissionClear> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current =
            policy_revision(&transaction, plugin_id)?.ok_or(AppPersistenceError::NotFound)?;
        if current != expected_policy_revision {
            return Err(AppPersistenceError::Conflict);
        }
        let cleared_count = transaction.execute(
            "DELETE FROM plugin_operation_permissions WHERE plugin_id = ?1",
            [plugin_id.as_str()],
        )?;
        let next = next_policy_revision(current)?;
        let changed = transaction.execute(
            "UPDATE plugin_operation_approval_policies
             SET revision = ?1, updated_at_ms = ?2 WHERE plugin_id = ?3 AND revision = ?4",
            params![
                u64_to_i64(next)?,
                unix_time_ms(),
                plugin_id.as_str(),
                u64_to_i64(current.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        Ok(PluginOperationPermissionClear {
            policy_revision: WireSequence::new(next),
            cleared_count: u32::try_from(cleared_count)
                .map_err(|_| AppPersistenceError::InvalidStoredData)?,
        })
    }
}

fn validate_operation_binding(
    binding: &OperationPermissionBinding,
    operation: PluginApprovalOperation,
    labels: Option<(&str, &str)>,
) -> Result<()> {
    validate_digest(&binding.signer_fingerprint_sha256)?;
    validate_digest(&binding.package_sha256)?;
    validate_plugin_permission_binding(&binding.security_binding)?;
    if binding.signer_fingerprint_sha256 != binding.signer_fingerprint_sha256.to_ascii_lowercase()
        || binding.package_sha256 != binding.package_sha256.to_ascii_lowercase()
        || binding.security_binding.artifact_sha256
            != binding
                .security_binding
                .artifact_sha256
                .to_ascii_lowercase()
        || binding.security_binding.artifact_sha256 != binding.package_sha256
        || binding.installation_revision.get() == 0
        || binding.capability_revision.get() == 0
        || required_capability(operation) != binding.capability
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid operation permission binding",
        ));
    }
    if let Some((target_label, action_label)) = labels
        && (invalid_non_secret_label(target_label, MAX_TARGET_LABEL_BYTES)
            || invalid_non_secret_label(action_label, MAX_ACTION_LABEL_BYTES))
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid operation permission target label",
        ));
    }
    Ok(())
}

fn invalid_non_secret_label(value: &str, maximum_bytes: usize) -> bool {
    value.is_empty() || value.len() > maximum_bytes || value.chars().any(char::is_control)
}

fn assert_current_binding(
    transaction: &Transaction<'_>,
    binding: &OperationPermissionBinding,
    require_installation_revision: bool,
) -> Result<()> {
    let installation: Option<(String, String, String, i64)> = transaction
        .query_row(
            "SELECT signer_fingerprint_sha256, package_sha256, state, state_version
             FROM plugin_installations WHERE plugin_id = ?1",
            [binding.plugin_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((signer, package, state, revision)) = installation else {
        return Err(AppPersistenceError::NotFound);
    };
    if signer != binding.signer_fingerprint_sha256
        || package != binding.package_sha256
        || state != plugin_install_state_to_db(PluginInstallState::Enabled)
        || (require_installation_revision
            && revision != u64_to_i64(binding.installation_revision.get())?)
    {
        return Err(AppPersistenceError::Conflict);
    }
    let capability: Option<CapabilityBindingRow> = transaction
        .query_row(
            "SELECT granted, state_version, artifact_sha256, app_version_major,
                        app_version_minor, secure_surface_contract_revision
                 FROM plugin_capability_grants
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND major_version = ?3 AND capability = ?4",
            params![
                binding.plugin_id.as_str(),
                binding.signer_fingerprint_sha256,
                u64_to_i64(binding.capability_major_version)?,
                plugin_capability_to_db(binding.capability),
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((granted, revision, artifact, app_major, app_minor, contract)) = capability else {
        return Err(AppPersistenceError::Conflict);
    };
    if !granted
        || revision != u64_to_i64(binding.capability_revision.get())?
        || artifact.as_deref() != Some(binding.security_binding.artifact_sha256.as_str())
        || app_major != Some(u64_to_i64(binding.security_binding.app_version_major)?)
        || app_minor != Some(u64_to_i64(binding.security_binding.app_version_minor)?)
        || contract
            != Some(u64_to_i64(
                binding.security_binding.secure_surface_contract_revision,
            )?)
    {
        return Err(AppPersistenceError::Conflict);
    }
    Ok(())
}

fn assert_current_operation_permission_owner(
    transaction: &Transaction<'_>,
    installed: &PluginInstalledRecord,
) -> Result<()> {
    if installed.state != PluginInstallState::Enabled {
        return Err(AppPersistenceError::Conflict);
    }
    let installation: Option<(String, String, String, i64)> = transaction
        .query_row(
            "SELECT signer_fingerprint_sha256, package_sha256, state, state_version
             FROM plugin_installations WHERE plugin_id = ?1",
            [installed.plugin_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((signer, package, state, state_version)) = installation else {
        return Err(AppPersistenceError::NotFound);
    };
    if signer != installed.signer_fingerprint_sha256
        || package != installed.package_sha256
        || state != plugin_install_state_to_db(PluginInstallState::Enabled)
        || state_version != u64_to_i64(installed.state_version.get())?
    {
        return Err(AppPersistenceError::Conflict);
    }
    Ok(())
}

fn policy_revision(
    transaction: &Transaction<'_>,
    plugin_id: &PluginId,
) -> Result<Option<WireSequence>> {
    transaction
        .query_row(
            "SELECT revision FROM plugin_operation_approval_policies WHERE plugin_id = ?1",
            [plugin_id.as_str()],
            |row| {
                let revision = row.get::<_, i64>(0)?;
                u64::try_from(revision)
                    .map(WireSequence::new)
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })
            },
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn next_policy_revision(current: WireSequence) -> Result<u64> {
    current
        .get()
        .checked_add(1)
        .ok_or(AppPersistenceError::Conflict)
}

fn required_capability(operation: PluginApprovalOperation) -> PluginCapability {
    match operation {
        PluginApprovalOperation::RemoteExecute
        | PluginApprovalOperation::ForwardStart
        | PluginApprovalOperation::ForwardStop => PluginCapability::RemoteExecRequest,
        PluginApprovalOperation::NetworkRequest => PluginCapability::NetworkDomain,
        PluginApprovalOperation::FileAccess => PluginCapability::LocalFiles,
        PluginApprovalOperation::SftpRead => PluginCapability::SftpRead,
        PluginApprovalOperation::SftpWrite => PluginCapability::SftpWrite,
        PluginApprovalOperation::SerialAccess => PluginCapability::DeviceSerial,
        PluginApprovalOperation::LocalExecute => PluginCapability::LocalProcess,
        PluginApprovalOperation::TerminalInput => PluginCapability::TerminalRequestInput,
        PluginApprovalOperation::HostMutation => PluginCapability::HostMutationPropose,
        PluginApprovalOperation::HostSession => PluginCapability::HostSessionRequest,
    }
}

fn operation_to_db(operation: PluginApprovalOperation) -> &'static str {
    match operation {
        PluginApprovalOperation::RemoteExecute => "remote_execute",
        PluginApprovalOperation::ForwardStart => "forward_start",
        PluginApprovalOperation::ForwardStop => "forward_stop",
        PluginApprovalOperation::NetworkRequest => "network_request",
        PluginApprovalOperation::FileAccess => "file_access",
        PluginApprovalOperation::SftpRead => "sftp_read",
        PluginApprovalOperation::SftpWrite => "sftp_write",
        PluginApprovalOperation::SerialAccess => "serial_access",
        PluginApprovalOperation::LocalExecute => "local_execute",
        PluginApprovalOperation::TerminalInput => "terminal_input",
        PluginApprovalOperation::HostMutation => "host_mutation",
        PluginApprovalOperation::HostSession => "host_session",
    }
}

fn operation_from_db(operation: &str) -> rusqlite::Result<PluginApprovalOperation> {
    match operation {
        "remote_execute" => Ok(PluginApprovalOperation::RemoteExecute),
        "forward_start" => Ok(PluginApprovalOperation::ForwardStart),
        "forward_stop" => Ok(PluginApprovalOperation::ForwardStop),
        "network_request" => Ok(PluginApprovalOperation::NetworkRequest),
        "file_access" => Ok(PluginApprovalOperation::FileAccess),
        "sftp_read" => Ok(PluginApprovalOperation::SftpRead),
        "sftp_write" => Ok(PluginApprovalOperation::SftpWrite),
        "serial_access" => Ok(PluginApprovalOperation::SerialAccess),
        "local_execute" => Ok(PluginApprovalOperation::LocalExecute),
        "terminal_input" => Ok(PluginApprovalOperation::TerminalInput),
        "host_mutation" => Ok(PluginApprovalOperation::HostMutation),
        "host_session" => Ok(PluginApprovalOperation::HostSession),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid remembered plugin operation",
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{PluginCapability, PluginId};
    use rusqlite::params;

    use super::*;

    fn fixture() -> (tempfile::TempDir, AppRepository, OperationPermissionBinding) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data").join("norishell.sqlite3");
        let repository = AppRepository::open(path).unwrap();
        let plugin_id = PluginId::parse("org.norishell.remembered-operation").unwrap();
        let signer = "a".repeat(64);
        let package = "b".repeat(64);
        repository
            .connection
            .execute(
                "INSERT INTO plugin_installations
                 (plugin_id, name, publisher, signer_fingerprint_sha256, active_version,
                  package_sha256, capabilities_json, state, state_version,
                  installed_at_ms, updated_at_ms)
                 VALUES (?1, 'Remembered operation', 'NoriShell', ?2, '1.0.0', ?3,
                         ?4, 'enabled', 7, 1, 1)",
                params![
                    plugin_id.as_str(),
                    signer,
                    package,
                    serde_json::to_string(&vec![PluginCapability::RemoteExecRequest]).unwrap(),
                ],
            )
            .unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO plugin_capability_grants
                 (plugin_id, signer_fingerprint_sha256, major_version, capability, granted,
                  state_version, updated_at_ms, artifact_sha256, app_version_major,
                  app_version_minor, secure_surface_contract_revision)
                 VALUES (?1, ?2, 10, 'remote.exec.request', 1, 5, 1, ?3, 0, 1, 9)",
                params![plugin_id.as_str(), signer, package],
            )
            .unwrap();
        (
            directory,
            repository,
            OperationPermissionBinding {
                plugin_id,
                signer_fingerprint_sha256: signer,
                package_sha256: package.clone(),
                installation_revision: WireSequence::new(7),
                capability: PluginCapability::RemoteExecRequest,
                capability_major_version: 10,
                capability_revision: WireSequence::new(5),
                security_binding: PluginPermissionBinding {
                    artifact_sha256: package,
                    app_version_major: 0,
                    app_version_minor: 1,
                    secure_surface_contract_revision: 9,
                },
            },
        )
    }

    fn input(
        binding: OperationPermissionBinding,
        fingerprint: [u8; 32],
        expected_policy_revision: Option<WireSequence>,
    ) -> RememberOperationPermission {
        RememberOperationPermission {
            binding,
            fingerprint,
            operation: PluginApprovalOperation::RemoteExecute,
            target_label: "QA host".to_owned(),
            action_label: "process.list".to_owned(),
            expected_policy_revision,
            expires_at_unix_ms: None,
        }
    }

    #[test]
    fn migrates_v32_and_persists_only_non_secret_exact_approval_data() {
        let (_directory, mut repository, binding) = fixture();
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::super::SCHEMA_VERSION);
        let fingerprint = [7; 32];
        let saved = repository
            .remember_plugin_operation_permission(&input(binding.clone(), fingerprint, None))
            .unwrap();
        assert_eq!(saved.policy_revision, WireSequence::new(1));
        assert_eq!(saved.permission.target_label, "QA host");
        assert_eq!(saved.permission.action_label, "process.list");
        let listed = repository
            .list_plugin_operation_permissions(&binding.plugin_id)
            .unwrap();
        assert_eq!(listed.policy_revision, Some(saved.policy_revision));
        assert_eq!(listed.permissions, vec![saved.permission.clone()]);
        assert_eq!(
            repository
                .find_plugin_operation_permission(
                    &binding,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap(),
            Some(saved)
        );
        let columns: Vec<String> = repository
            .connection
            .prepare("PRAGMA table_info(plugin_operation_permissions)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(!columns.iter().any(|column| {
            column.contains("command") || column.contains("stdin") || column.contains("input")
        }));
    }

    #[test]
    fn finite_expiry_survives_reopen_and_expired_permission_cannot_admit() {
        let (directory, mut repository, binding) = fixture();
        let path = directory.path().join("data").join("norishell.sqlite3");
        let deadline = unix_time_ms().saturating_add(60_000);
        let fingerprint = [17; 32];
        let mut request = input(binding.clone(), fingerprint, None);
        request.expires_at_unix_ms = Some(deadline);
        let saved = repository
            .remember_plugin_operation_permission(&request)
            .unwrap();
        assert_eq!(saved.permission.expires_at_unix_ms, Some(deadline));
        assert_eq!(
            repository
                .find_plugin_operation_permission(
                    &binding,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap(),
            Some(saved.clone())
        );

        drop(repository);
        let repository = AppRepository::open(path).unwrap();
        assert_eq!(
            repository
                .list_plugin_operation_permissions(&binding.plugin_id)
                .unwrap()
                .permissions,
            vec![saved.permission.clone()],
            "reopening must preserve the Core-calculated deadline instead of extending it"
        );
        assert!(
            repository
                .find_plugin_operation_permission(
                    &binding,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap()
                .is_some()
        );

        let elapsed_deadline = unix_time_ms().saturating_sub(1);
        repository
            .connection
            .execute(
                "UPDATE plugin_operation_permissions
                 SET expires_at_unix_ms = ?1 WHERE permission_id = ?2",
                params![elapsed_deadline, saved.permission.permission_id],
            )
            .unwrap();
        let listed = repository
            .list_plugin_operation_permissions(&binding.plugin_id)
            .unwrap();
        assert_eq!(
            listed.permissions[0].expires_at_unix_ms,
            Some(elapsed_deadline)
        );
        assert!(
            repository
                .find_plugin_operation_permission(
                    &binding,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn guest_current_owner_cannot_list_revoke_or_forget_previous_package_permissions() {
        let (_directory, mut repository, binding) = fixture();
        let saved = repository
            .remember_plugin_operation_permission(&input(binding.clone(), [21; 32], None))
            .unwrap();
        let original = repository
            .get_plugin_installation(&binding.plugin_id)
            .unwrap();
        assert_eq!(
            repository
                .list_current_plugin_operation_permissions(&original)
                .unwrap()
                .permissions,
            vec![saved.permission.clone()]
        );

        let replacement_signer = "c".repeat(64);
        let replacement_package = "d".repeat(64);
        repository
            .connection
            .execute(
                "UPDATE plugin_installations
                 SET signer_fingerprint_sha256 = ?1, package_sha256 = ?2,
                     state_version = 8
                 WHERE plugin_id = ?3",
                params![
                    replacement_signer,
                    replacement_package,
                    binding.plugin_id.as_str(),
                ],
            )
            .unwrap();
        repository
            .connection
            .execute(
                "UPDATE plugin_capability_grants
                 SET signer_fingerprint_sha256 = ?1, artifact_sha256 = ?2
                 WHERE plugin_id = ?3 AND capability = 'remote.exec.request'",
                params![
                    replacement_signer,
                    replacement_package,
                    binding.plugin_id.as_str(),
                ],
            )
            .unwrap();
        let replacement = repository
            .get_plugin_installation(&binding.plugin_id)
            .unwrap();

        assert!(
            repository
                .list_current_plugin_operation_permissions(&replacement)
                .unwrap()
                .permissions
                .is_empty()
        );
        assert!(matches!(
            repository.revoke_current_plugin_operation_permission(
                &replacement,
                &saved.permission.permission_id,
                saved.policy_revision,
            ),
            Err(AppPersistenceError::NotFound)
        ));
        let forgotten = repository
            .clear_current_plugin_operation_permissions(&replacement, saved.policy_revision)
            .unwrap();
        assert_eq!(forgotten.cleared_count, 0);
        assert_eq!(forgotten.policy_revision, WireSequence::new(2));
        assert_eq!(
            repository
                .list_plugin_operation_permissions(&binding.plugin_id)
                .unwrap()
                .permissions,
            vec![saved.permission],
            "host management retains prior signer/package history"
        );
    }

    #[test]
    fn v38_migration_adds_expiry_revoke_workflow_tag_and_scrubs_old_selection_labels() {
        let (directory, mut repository, binding) = fixture();
        let first = repository
            .remember_plugin_operation_permission(&input(binding.clone(), [31; 32], None))
            .unwrap();
        let second = repository
            .remember_plugin_operation_permission(&input(
                binding.clone(),
                [32; 32],
                Some(first.policy_revision),
            ))
            .unwrap();
        crate::remove_schema_added_after_fixture_version(&repository.connection, 38).unwrap();
        repository
            .connection
            .execute(
                "UPDATE plugin_operation_permissions
                 SET operation = 'file_access', capability = 'local.files',
                     target_label = '/Users/someone/Private/selection.txt'
                 WHERE permission_id = ?1",
                [first.permission.permission_id.as_str()],
            )
            .unwrap();
        repository
            .connection
            .execute(
                "UPDATE plugin_operation_permissions
                 SET operation = 'serial_access', capability = 'device.serial',
                     target_label = '/dev/cu.sensitive-device'
                 WHERE permission_id = ?1",
                [second.permission.permission_id.as_str()],
            )
            .unwrap();
        repository
            .connection
            .pragma_update(None, "user_version", 38)
            .unwrap();
        drop(repository);

        let repository =
            AppRepository::open(directory.path().join("data").join("norishell.sqlite3")).unwrap();
        let permissions = repository
            .list_plugin_operation_permissions(&binding.plugin_id)
            .unwrap()
            .permissions;
        let file = permissions
            .iter()
            .find(|permission| permission.permission_id == first.permission.permission_id)
            .unwrap();
        assert_eq!(
            file.target_label,
            format!(
                "Selected local file/directory · {}",
                &first.permission.permission_id[..8]
            )
        );
        assert_eq!(file.expires_at_unix_ms, None);
        let serial = permissions
            .iter()
            .find(|permission| permission.permission_id == second.permission.permission_id)
            .unwrap();
        assert_eq!(
            serial.target_label,
            format!(
                "Selected serial device · {}",
                &second.permission.permission_id[..8]
            )
        );
        assert_eq!(serial.expires_at_unix_ms, None);
        let workflow_sql: String = repository
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'plugin_workflow_task_steps'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(workflow_sql.contains("permission_revoke"));
    }

    #[test]
    fn v32_migration_rebuilds_capability_and_operation_checks_for_local_process() {
        let (directory, repository, _) = fixture();
        crate::remove_schema_added_after_fixture_version(&repository.connection, 32).unwrap();
        repository
            .connection
            .pragma_update(None, "user_version", 32)
            .unwrap();
        drop(repository);
        let repository =
            AppRepository::open(directory.path().join("data").join("norishell.sqlite3")).unwrap();
        let capability_sql: String = repository
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'plugin_capability_grants'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let operation_sql: String = repository
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'plugin_operation_permissions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(capability_sql.contains("local.process"));
        assert!(operation_sql.contains("local_execute"));
        assert!(operation_sql.contains("local.process"));
    }

    #[test]
    fn v33_migration_preserves_approvals_and_separates_sftp_rights() {
        let (directory, mut repository, binding) = fixture();
        let saved = repository
            .remember_plugin_operation_permission(&input(binding.clone(), [19; 32], None))
            .unwrap();
        crate::remove_schema_added_after_fixture_version(&repository.connection, 33).unwrap();
        repository
            .connection
            .pragma_update(None, "user_version", 33)
            .unwrap();
        drop(repository);
        let repository =
            AppRepository::open(directory.path().join("data").join("norishell.sqlite3")).unwrap();
        assert_eq!(
            repository
                .list_plugin_operation_permissions(&binding.plugin_id)
                .unwrap()
                .permissions,
            vec![saved.permission]
        );
        let sql: String = repository
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'plugin_operation_permissions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        for (operation, capability, tag) in [
            (
                PluginApprovalOperation::SftpRead,
                PluginCapability::SftpRead,
                "sftp_read",
            ),
            (
                PluginApprovalOperation::SftpWrite,
                PluginCapability::SftpWrite,
                "sftp_write",
            ),
        ] {
            assert_eq!(required_capability(operation), capability);
            assert_eq!(operation_to_db(operation), tag);
            assert_eq!(operation_from_db(tag).unwrap(), operation);
            assert!(sql.contains(tag));
        }
    }

    #[test]
    fn v31_migration_creates_the_policy_and_permission_tables() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data").join("norishell.sqlite3");
        let repository = AppRepository::open(&path).unwrap();
        crate::remove_schema_added_after_fixture_version(&repository.connection, 31).unwrap();
        repository
            .connection
            .execute_batch(
                "DROP TABLE IF EXISTS plugin_operation_permissions;
                 DROP TABLE IF EXISTS plugin_operation_approval_policies;
                 PRAGMA user_version = 31;",
            )
            .unwrap();
        drop(repository);
        let repository = AppRepository::open(path).unwrap();
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::super::SCHEMA_VERSION);
        let tables: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
                 AND name IN ('plugin_operation_approval_policies', 'plugin_operation_permissions')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 2);
    }

    #[test]
    fn v31_capability_check_is_rebuilt_before_local_files_is_persisted() {
        let (directory, repository, binding) = fixture();
        let path = directory.path().join("data").join("norishell.sqlite3");
        crate::remove_schema_added_after_fixture_version(&repository.connection, 31).unwrap();
        repository
            .connection
            .execute_batch(
                "DROP TABLE IF EXISTS plugin_operation_permissions;
                 DROP TABLE IF EXISTS plugin_operation_approval_policies;
                 ALTER TABLE plugin_capability_grants RENAME TO plugin_capability_grants_v31_test;
                 CREATE TABLE plugin_capability_grants (
                   plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
                   signer_fingerprint_sha256 TEXT NOT NULL CHECK(length(signer_fingerprint_sha256) = 64),
                   major_version INTEGER NOT NULL CHECK(major_version >= 0),
                   capability TEXT NOT NULL CHECK(capability IN
                     ('ui.panel', 'ui.navigation', 'ui.page', 'ui.webview.isolated',
                      'ui.hostDom.observe', 'ui.hostDom.mutate', 'ui.hostCss',
                      'clipboard.write', 'terminal.metadata', 'terminal.observe',
                      'terminal.annotation', 'terminal.proposeInput', 'terminal.requestInput',
                      'host.metadata.read', 'host.mutation.propose', 'host.session.request',
                      'remote.inspect', 'remote.exec.request',
                      'network.domain', 'storage.plugin', 'sftp.read', 'sftp.write',
                      'metrics.read', 'ssh.sync')),
                   granted INTEGER NOT NULL CHECK(granted IN (0, 1)),
                   state_version INTEGER NOT NULL CHECK(state_version >= 1),
                   updated_at_ms INTEGER NOT NULL,
                   artifact_sha256 TEXT,
                   app_version_major INTEGER,
                   app_version_minor INTEGER,
                   secure_surface_contract_revision INTEGER,
                   PRIMARY KEY(plugin_id, signer_fingerprint_sha256, major_version, capability),
                   CHECK(
                     (artifact_sha256 IS NULL AND app_version_major IS NULL
                       AND app_version_minor IS NULL AND secure_surface_contract_revision IS NULL)
                     OR
                     (artifact_sha256 IS NOT NULL AND app_version_major IS NOT NULL
                       AND app_version_minor IS NOT NULL
                       AND secure_surface_contract_revision IS NOT NULL
                       AND length(artifact_sha256) = 64 AND artifact_sha256 = lower(artifact_sha256)
                       AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
                       AND app_version_major >= 0 AND app_version_minor >= 0
                       AND secure_surface_contract_revision >= 1)
                   )
                 ) STRICT;
                 INSERT INTO plugin_capability_grants SELECT * FROM plugin_capability_grants_v31_test;
                 DROP TABLE plugin_capability_grants_v31_test;
                 PRAGMA user_version = 31;",
            )
            .unwrap();
        drop(repository);
        let repository = AppRepository::open(path).unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO plugin_capability_grants
                 (plugin_id, signer_fingerprint_sha256, major_version, capability, granted,
                  state_version, updated_at_ms, artifact_sha256, app_version_major,
                  app_version_minor, secure_surface_contract_revision)
                 VALUES (?1, ?2, 1, 'local.files', 1, 8, 2, ?3, 0, 1, 9)",
                params![
                    binding.plugin_id.as_str(),
                    binding.signer_fingerprint_sha256,
                    binding.package_sha256,
                ],
            )
            .unwrap();
    }

    #[test]
    fn network_request_uses_only_the_network_domain_capability_and_db_tag() {
        assert_eq!(
            required_capability(PluginApprovalOperation::NetworkRequest),
            PluginCapability::NetworkDomain
        );
        assert_eq!(
            operation_to_db(PluginApprovalOperation::NetworkRequest),
            "network_request"
        );
        assert_eq!(
            operation_from_db("network_request").unwrap(),
            PluginApprovalOperation::NetworkRequest
        );
    }

    #[test]
    fn file_access_uses_only_the_local_files_capability_and_db_tag() {
        assert_eq!(
            required_capability(PluginApprovalOperation::FileAccess),
            PluginCapability::LocalFiles
        );
        assert_eq!(
            operation_to_db(PluginApprovalOperation::FileAccess),
            "file_access"
        );
        assert_eq!(
            operation_from_db("file_access").unwrap(),
            PluginApprovalOperation::FileAccess
        );
    }

    #[test]
    fn local_execute_uses_only_the_local_process_capability_and_db_tag() {
        assert_eq!(
            required_capability(PluginApprovalOperation::LocalExecute),
            PluginCapability::LocalProcess
        );
        assert_eq!(
            operation_to_db(PluginApprovalOperation::LocalExecute),
            "local_execute"
        );
        assert_eq!(
            operation_from_db("local_execute").unwrap(),
            PluginApprovalOperation::LocalExecute
        );
    }

    #[test]
    fn rejects_foreign_package_and_capability_epoch_but_survives_disable_reenable() {
        let (_directory, mut repository, binding) = fixture();
        let fingerprint = [8; 32];
        let saved = repository
            .remember_plugin_operation_permission(&input(binding.clone(), fingerprint, None))
            .unwrap();
        let mut foreign_package = binding.clone();
        foreign_package.package_sha256 = "c".repeat(64);
        foreign_package.security_binding.artifact_sha256 = foreign_package.package_sha256.clone();
        assert!(
            repository
                .find_plugin_operation_permission(
                    &foreign_package,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap()
                .is_none()
        );

        repository
            .connection
            .execute(
                "UPDATE plugin_installations SET state = 'disabled', state_version = 8
                 WHERE plugin_id = ?1",
                [binding.plugin_id.as_str()],
            )
            .unwrap();
        assert!(
            repository
                .find_plugin_operation_permission(
                    &binding,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap()
                .is_none()
        );
        repository
            .connection
            .execute(
                "UPDATE plugin_installations SET state = 'enabled', state_version = 9
                 WHERE plugin_id = ?1",
                [binding.plugin_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            repository
                .find_plugin_operation_permission(
                    &binding,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap(),
            Some(saved.clone())
        );
        repository
            .connection
            .execute(
                "UPDATE plugin_capability_grants SET state_version = 6
                 WHERE plugin_id = ?1 AND capability = 'remote.exec.request'",
                [binding.plugin_id.as_str()],
            )
            .unwrap();
        assert!(
            repository
                .find_plugin_operation_permission(
                    &binding,
                    PluginApprovalOperation::RemoteExecute,
                    fingerprint,
                )
                .unwrap()
                .is_none()
        );
        let stale = input(binding, fingerprint, Some(saved.policy_revision));
        assert!(matches!(
            repository.remember_plugin_operation_permission(&stale),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn revoke_advances_policy_and_blocks_a_stale_secure_surface_save() {
        let (_directory, mut repository, binding) = fixture();
        let saved = repository
            .remember_plugin_operation_permission(&input(binding.clone(), [9; 32], None))
            .unwrap();
        let revision = repository
            .revoke_plugin_operation_permission(
                &binding.plugin_id,
                &saved.permission.permission_id,
                saved.policy_revision,
            )
            .unwrap();
        assert_eq!(revision, WireSequence::new(2));
        assert!(matches!(
            repository.remember_plugin_operation_permission(&input(
                binding,
                [10; 32],
                Some(saved.policy_revision),
            )),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn caps_records_and_uninstall_cascades_them() {
        let (_directory, mut repository, binding) = fixture();
        let mut revision = None;
        for byte in 0_u8..=255 {
            let saved = repository
                .remember_plugin_operation_permission(&input(binding.clone(), [byte; 32], revision))
                .unwrap();
            revision = Some(saved.policy_revision);
        }
        let mut extra_fingerprint = [0; 32];
        extra_fingerprint[31] = 1;
        assert!(matches!(
            repository.remember_plugin_operation_permission(&input(
                binding.clone(),
                extra_fingerprint,
                revision
            )),
            Err(AppPersistenceError::InvalidInput(
                "too many remembered plugin operation permissions"
            ))
        ));
        repository
            .connection
            .execute(
                "DELETE FROM plugin_installations WHERE plugin_id = ?1",
                [binding.plugin_id.as_str()],
            )
            .unwrap();
        let remaining: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM plugin_operation_permissions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
