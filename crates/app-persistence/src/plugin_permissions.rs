//! Durable, signer/major/Host-scoped plugin permissions.

use std::collections::{BTreeMap, BTreeSet};

use norishell_core_api::{HostId, PluginCapability, PluginId, WireSequence};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    AppPersistenceError, AppRepository, PluginCapabilityGrantRecord, PluginPermissionBinding,
    Result, complete_plugin_capability_decision, next_revision, permission_binding_from_columns,
    plugin_capability_from_db, plugin_capability_to_db, special_plugin_capability, u64_to_i64,
    unix_time_ms, validate_digest, validate_plugin_permission_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHostScopeGrantRecord {
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
    pub major_version: u64,
    pub host_id: HostId,
    pub capability: PluginCapability,
    pub state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHostScopeSetRecord {
    pub state_version: WireSequence,
    pub binding: Option<PluginPermissionBinding>,
}

impl AppRepository {
    /// Atomically replaces the complete capability decision and the selected
    /// Host scopes used by the protected special-permission surface.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_plugin_special_permissions(
        &mut self,
        plugin_id: &PluginId,
        signer_fingerprint_sha256: &str,
        major_version: u64,
        expected_plugin_state_version: WireSequence,
        expected_grant_state_version: Option<WireSequence>,
        expected_scope_state_version: Option<WireSequence>,
        binding: &PluginPermissionBinding,
        grants: &[(PluginCapability, bool)],
        host_scopes: &[(HostId, PluginCapability)],
    ) -> Result<(
        Vec<PluginCapabilityGrantRecord>,
        Vec<PluginHostScopeGrantRecord>,
    )> {
        validate_digest(signer_fingerprint_sha256)?;
        validate_plugin_permission_binding(binding)?;
        if grants.len() > 64 || host_scopes.len() > 1_536 {
            return Err(AppPersistenceError::InvalidInput(
                "too many plugin permission decisions",
            ));
        }
        let mut unique_scopes = BTreeSet::new();
        for (host_id, capability) in host_scopes {
            if !host_scoped_capability(*capability)
                || !unique_scopes.insert((host_id.as_str().to_owned(), *capability))
            {
                return Err(AppPersistenceError::InvalidInput(
                    "invalid plugin Host scope grant",
                ));
            }
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (installed_signer, capabilities_json, plugin_state_version): (String, String, i64) =
            transaction
                .query_row(
                    "SELECT signer_fingerprint_sha256, capabilities_json, state_version
                     FROM plugin_installations WHERE plugin_id = ?1",
                    [plugin_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
                .ok_or(AppPersistenceError::NotFound)?;
        let capabilities: Vec<PluginCapability> = serde_json::from_str(&capabilities_json)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?;
        if installed_signer != signer_fingerprint_sha256
            || plugin_state_version != u64_to_i64(expected_plugin_state_version.get())?
            || !complete_plugin_capability_decision(&capabilities, grants)
        {
            return Err(AppPersistenceError::Conflict);
        }
        let current_grant_version: Option<i64> = transaction.query_row(
            "SELECT MAX(state_version) FROM plugin_capability_grants
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?
            ],
            |row| row.get(0),
        )?;
        if current_grant_version.map(|value| WireSequence::new(value as u64))
            != expected_grant_state_version
        {
            return Err(AppPersistenceError::Conflict);
        }
        let current_scope_version: Option<i64> = transaction
            .query_row(
                "SELECT state_version FROM plugin_host_scope_sets
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                |row| row.get(0),
            )
            .optional()?;
        if current_scope_version.map(|value| WireSequence::new(value as u64))
            != expected_scope_state_version
        {
            return Err(AppPersistenceError::Conflict);
        }
        for (host_id, capability) in host_scopes {
            if !grants
                .iter()
                .any(|(candidate, granted)| candidate == capability && *granted)
            {
                return Err(AppPersistenceError::InvalidInput(
                    "Host scope requires a granted plugin capability",
                ));
            }
            let exists: Option<i64> = transaction
                .query_row(
                    "SELECT 1 FROM hosts WHERE id = ?1",
                    [host_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Err(AppPersistenceError::InvalidInput(
                    "Host scope references an unknown Host",
                ));
            }
        }

        let grant_version = expected_grant_state_version
            .map(next_revision)
            .transpose()?
            .unwrap_or(1);
        let scope_version = expected_scope_state_version
            .map(next_revision)
            .transpose()?
            .unwrap_or(1);
        let updated_at_ms = unix_time_ms();
        let previous = transaction
            .prepare(
                "SELECT capability, granted, artifact_sha256, app_version_major,
                        app_version_minor, secure_surface_contract_revision
                 FROM plugin_capability_grants
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND major_version = ?3",
            )?
            .query_map(
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                |row| {
                    Ok((
                        plugin_capability_from_db(&row.get::<_, String>(0)?)?,
                        (
                            row.get::<_, bool>(1)?,
                            permission_binding_from_columns(row, 2)?,
                        ),
                    ))
                },
            )?
            .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
        transaction.execute(
            "DELETE FROM plugin_capability_grants
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?
            ],
        )?;
        for (capability, granted) in grants {
            let row_binding = if !special_plugin_capability(*capability)
                && previous
                    .get(capability)
                    .is_some_and(|(previous_granted, _)| previous_granted == granted)
            {
                previous
                    .get(capability)
                    .and_then(|(_, binding)| binding.as_ref())
            } else {
                Some(binding)
            };
            transaction.execute(
                "INSERT INTO plugin_capability_grants
                 (plugin_id, signer_fingerprint_sha256, major_version, capability,
                  granted, state_version, updated_at_ms, artifact_sha256,
                  app_version_major, app_version_minor, secure_surface_contract_revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?,
                    plugin_capability_to_db(*capability),
                    i64::from(*granted),
                    u64_to_i64(grant_version)?,
                    updated_at_ms,
                    row_binding.map(|binding| binding.artifact_sha256.as_str()),
                    row_binding
                        .map(|binding| u64_to_i64(binding.app_version_major))
                        .transpose()?,
                    row_binding
                        .map(|binding| u64_to_i64(binding.app_version_minor))
                        .transpose()?,
                    row_binding
                        .map(|binding| u64_to_i64(binding.secure_surface_contract_revision))
                        .transpose()?,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO plugin_host_scope_sets
             (plugin_id, signer_fingerprint_sha256, major_version, state_version, updated_at_ms,
              artifact_sha256, app_version_major, app_version_minor,
              secure_surface_contract_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(plugin_id, signer_fingerprint_sha256, major_version) DO UPDATE SET
               state_version = excluded.state_version, updated_at_ms = excluded.updated_at_ms,
               artifact_sha256 = excluded.artifact_sha256,
               app_version_major = excluded.app_version_major,
               app_version_minor = excluded.app_version_minor,
               secure_surface_contract_revision = excluded.secure_surface_contract_revision",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?,
                u64_to_i64(scope_version)?,
                updated_at_ms,
                binding.artifact_sha256,
                u64_to_i64(binding.app_version_major)?,
                u64_to_i64(binding.app_version_minor)?,
                u64_to_i64(binding.secure_surface_contract_revision)?,
            ],
        )?;
        transaction.execute(
            "DELETE FROM plugin_host_scope_grants
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?
            ],
        )?;
        for (host_id, capability) in host_scopes {
            transaction.execute(
                "INSERT INTO plugin_host_scope_grants
                 (plugin_id, signer_fingerprint_sha256, major_version, host_id, capability,
                  state_version, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?,
                    host_id.as_str(),
                    plugin_capability_to_db(*capability),
                    u64_to_i64(scope_version)?,
                    updated_at_ms,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO plugin_audit
             (plugin_id, operation_id, action, outcome, detail_code, created_at_ms)
             VALUES (?1, NULL, 'permission.special', 'committed', 'protected_window', ?2)",
            params![plugin_id.as_str(), updated_at_ms],
        )?;
        transaction.commit()?;

        Ok((
            grants
                .iter()
                .map(|(capability, granted)| PluginCapabilityGrantRecord {
                    plugin_id: plugin_id.clone(),
                    signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
                    major_version,
                    capability: *capability,
                    granted: *granted,
                    state_version: WireSequence::new(grant_version),
                    binding: if !special_plugin_capability(*capability)
                        && previous
                            .get(capability)
                            .is_some_and(|(previous_granted, _)| previous_granted == granted)
                    {
                        previous
                            .get(capability)
                            .and_then(|(_, binding)| binding.clone())
                    } else {
                        Some(binding.clone())
                    },
                })
                .collect(),
            host_scopes
                .iter()
                .map(|(host_id, capability)| PluginHostScopeGrantRecord {
                    plugin_id: plugin_id.clone(),
                    signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
                    major_version,
                    host_id: host_id.clone(),
                    capability: *capability,
                    state_version: WireSequence::new(scope_version),
                })
                .collect(),
        ))
    }

    pub fn plugin_host_scope_state_version(
        &self,
        plugin_id: &PluginId,
        signer_fingerprint_sha256: &str,
        major_version: u64,
    ) -> Result<Option<WireSequence>> {
        validate_digest(signer_fingerprint_sha256)?;
        let version = self
            .connection
            .query_row(
                "SELECT state_version FROM plugin_host_scope_sets
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        version
            .map(|value| {
                u64::try_from(value)
                    .map(WireSequence::new)
                    .map_err(|_| AppPersistenceError::InvalidStoredData)
            })
            .transpose()
    }

    pub fn plugin_host_scope_set(
        &self,
        plugin_id: &PluginId,
        signer_fingerprint_sha256: &str,
        major_version: u64,
    ) -> Result<Option<PluginHostScopeSetRecord>> {
        validate_digest(signer_fingerprint_sha256)?;
        self.connection
            .query_row(
                "SELECT state_version, artifact_sha256, app_version_major,
                        app_version_minor, secure_surface_contract_revision
                 FROM plugin_host_scope_sets
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                |row| {
                    Ok(PluginHostScopeSetRecord {
                        state_version: WireSequence::new(
                            u64::try_from(row.get::<_, i64>(0)?).map_err(invalid_column)?,
                        ),
                        binding: permission_binding_from_columns(row, 1)?,
                    })
                },
            )
            .optional()
            .map_err(AppPersistenceError::from)
    }

    pub fn list_plugin_host_scope_grants(
        &self,
        plugin_id: &PluginId,
        signer_fingerprint_sha256: &str,
        major_version: u64,
    ) -> Result<Vec<PluginHostScopeGrantRecord>> {
        validate_digest(signer_fingerprint_sha256)?;
        let mut statement = self.connection.prepare(
            "SELECT plugin_id, signer_fingerprint_sha256, major_version, host_id,
                    capability, state_version
             FROM plugin_host_scope_grants
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3
             ORDER BY host_id, capability",
        )?;
        statement
            .query_map(
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                |row| {
                    Ok(PluginHostScopeGrantRecord {
                        plugin_id: PluginId::parse(row.get::<_, String>(0)?)
                            .map_err(invalid_text_column)?,
                        signer_fingerprint_sha256: row.get(1)?,
                        major_version: u64::try_from(row.get::<_, i64>(2)?)
                            .map_err(invalid_column)?,
                        host_id: HostId::parse(row.get::<_, String>(3)?)
                            .map_err(invalid_text_column)?,
                        capability: plugin_capability_from_db(&row.get::<_, String>(4)?)?,
                        state_version: WireSequence::new(
                            u64::try_from(row.get::<_, i64>(5)?).map_err(invalid_column)?,
                        ),
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(AppPersistenceError::from)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn replace_plugin_host_scope_grants(
        &mut self,
        plugin_id: &PluginId,
        signer_fingerprint_sha256: &str,
        major_version: u64,
        expected_plugin_state_version: WireSequence,
        expected_scope_state_version: Option<WireSequence>,
        binding: &PluginPermissionBinding,
        grants: &[(HostId, PluginCapability)],
    ) -> Result<Vec<PluginHostScopeGrantRecord>> {
        validate_digest(signer_fingerprint_sha256)?;
        validate_plugin_permission_binding(binding)?;
        if grants.len() > 1_536 {
            return Err(AppPersistenceError::InvalidInput(
                "too many plugin Host scope grants",
            ));
        }
        let mut unique = BTreeSet::new();
        for (host_id, capability) in grants {
            if !host_scoped_capability(*capability)
                || !unique.insert((host_id.as_str().to_owned(), *capability))
            {
                return Err(AppPersistenceError::InvalidInput(
                    "invalid plugin Host scope grant",
                ));
            }
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (installed_signer, plugin_state_version): (String, i64) = transaction
            .query_row(
                "SELECT signer_fingerprint_sha256, state_version
                 FROM plugin_installations WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)?;
        if installed_signer != signer_fingerprint_sha256
            || plugin_state_version != u64_to_i64(expected_plugin_state_version.get())?
        {
            return Err(AppPersistenceError::Conflict);
        }
        let current_version: Option<i64> = transaction
            .query_row(
                "SELECT state_version FROM plugin_host_scope_sets
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?
                ],
                |row| row.get(0),
            )
            .optional()?;
        if current_version.map(|value| WireSequence::new(value as u64))
            != expected_scope_state_version
        {
            return Err(AppPersistenceError::Conflict);
        }
        for (_, capability) in grants {
            let granted: Option<i64> = transaction
                .query_row(
                    "SELECT granted FROM plugin_capability_grants
                     WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                       AND major_version = ?3 AND capability = ?4",
                    params![
                        plugin_id.as_str(),
                        signer_fingerprint_sha256,
                        u64_to_i64(major_version)?,
                        plugin_capability_to_db(*capability),
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            if granted != Some(1) {
                return Err(AppPersistenceError::InvalidInput(
                    "Host scope requires a granted plugin capability",
                ));
            }
        }
        let next_version = expected_scope_state_version
            .map(next_revision)
            .transpose()?
            .unwrap_or(1);
        let updated_at_ms = unix_time_ms();
        transaction.execute(
            "INSERT INTO plugin_host_scope_sets
             (plugin_id, signer_fingerprint_sha256, major_version, state_version, updated_at_ms,
              artifact_sha256, app_version_major, app_version_minor,
              secure_surface_contract_revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(plugin_id, signer_fingerprint_sha256, major_version) DO UPDATE SET
               state_version = excluded.state_version, updated_at_ms = excluded.updated_at_ms,
               artifact_sha256 = excluded.artifact_sha256,
               app_version_major = excluded.app_version_major,
               app_version_minor = excluded.app_version_minor,
               secure_surface_contract_revision = excluded.secure_surface_contract_revision",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?,
                u64_to_i64(next_version)?,
                updated_at_ms,
                binding.artifact_sha256,
                u64_to_i64(binding.app_version_major)?,
                u64_to_i64(binding.app_version_minor)?,
                u64_to_i64(binding.secure_surface_contract_revision)?,
            ],
        )?;
        transaction.execute(
            "DELETE FROM plugin_host_scope_grants
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND major_version = ?3",
            params![
                plugin_id.as_str(),
                signer_fingerprint_sha256,
                u64_to_i64(major_version)?
            ],
        )?;
        for (host_id, capability) in grants {
            transaction.execute(
                "INSERT INTO plugin_host_scope_grants
                 (plugin_id, signer_fingerprint_sha256, major_version, host_id, capability,
                  state_version, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    plugin_id.as_str(),
                    signer_fingerprint_sha256,
                    u64_to_i64(major_version)?,
                    host_id.as_str(),
                    plugin_capability_to_db(*capability),
                    u64_to_i64(next_version)?,
                    updated_at_ms,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(grants
            .iter()
            .map(|(host_id, capability)| PluginHostScopeGrantRecord {
                plugin_id: plugin_id.clone(),
                signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
                major_version,
                host_id: host_id.clone(),
                capability: *capability,
                state_version: WireSequence::new(next_version),
            })
            .collect())
    }
}

fn host_scoped_capability(capability: PluginCapability) -> bool {
    matches!(
        capability,
        PluginCapability::HostMetadataRead
            | PluginCapability::HostMutationPropose
            | PluginCapability::HostSessionRequest
    )
}

fn invalid_column(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn invalid_text_column(message: &'static str) -> rusqlite::Error {
    invalid_column(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}
