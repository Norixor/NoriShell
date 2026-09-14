//! Core-owned persistence for package-declared, non-secret plugin settings.

use norishell_core_api::{PluginId, WireSequence};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::{
    AppPersistenceError, AppRepository, Result, next_revision, parse_id, read_wire_sequence,
    u64_to_i64, unix_time_ms,
};

const MAX_PLUGIN_SETTINGS_SCHEMA_BYTES: usize = 32 * 1024;
const MAX_PLUGIN_SETTINGS_VALUES_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSettingsRecord {
    pub plugin_id: PluginId,
    pub package_sha256: String,
    pub schema_json: String,
    pub schema_sha256: String,
    pub values_json: String,
    pub revision: WireSequence,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct PluginSettingsInstall<'a> {
    pub schema_json: &'a str,
    pub schema_sha256: &'a str,
    pub values_json: &'a str,
    pub expected_previous_revision: Option<WireSequence>,
}

impl AppRepository {
    /// Returns retained settings too, including rows whose package is no
    /// longer installed. Callers must bind an active projection to the exact
    /// installation/package before exposing it to a plugin or renderer.
    pub fn get_plugin_settings(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<PluginSettingsRecord>> {
        self.connection
            .query_row(
                "SELECT plugin_id, package_sha256, schema_json, schema_sha256,
                        values_json, revision, updated_at_ms
                 FROM plugin_settings WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                read_plugin_settings,
            )
            .optional()
            .map_err(AppPersistenceError::from)
    }

    /// Replaces values only when the installation, package, schema and
    /// settings revision still match the renderer snapshot.
    pub fn replace_plugin_settings(
        &mut self,
        plugin_id: &PluginId,
        expected_package_sha256: &str,
        expected_installed_state_version: WireSequence,
        expected_schema_sha256: &str,
        expected_revision: WireSequence,
        values_json: &str,
    ) -> Result<PluginSettingsRecord> {
        validate_values_json(values_json)?;
        let next = next_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let installed: Option<(String, i64)> = transaction
            .query_row(
                "SELECT package_sha256, state_version FROM plugin_installations
                 WHERE plugin_id = ?1",
                [plugin_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if installed
            != Some((
                expected_package_sha256.to_owned(),
                u64_to_i64(expected_installed_state_version.get())?,
            ))
        {
            return Err(AppPersistenceError::Conflict);
        }
        let updated_at_unix_ms = unix_time_ms();
        let changed = transaction.execute(
            "UPDATE plugin_settings
             SET values_json = ?1, revision = ?2, updated_at_ms = ?3
             WHERE plugin_id = ?4 AND package_sha256 = ?5 AND schema_sha256 = ?6
               AND revision = ?7",
            params![
                values_json,
                u64_to_i64(next)?,
                updated_at_unix_ms,
                plugin_id.as_str(),
                expected_package_sha256,
                expected_schema_sha256,
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let record = get_plugin_settings_transaction(&transaction, plugin_id)?
            .ok_or(AppPersistenceError::Conflict)?;
        transaction.commit()?;
        Ok(record)
    }
}

pub(super) fn install_plugin_settings(
    transaction: &Transaction<'_>,
    plugin_id: &PluginId,
    package_sha256: &str,
    settings: PluginSettingsInstall<'_>,
    updated_at_unix_ms: i64,
) -> Result<()> {
    validate_hash(package_sha256)?;
    validate_hash(settings.schema_sha256)?;
    validate_schema_json(settings.schema_json)?;
    validate_values_json(settings.values_json)?;
    let current_revision: Option<i64> = transaction
        .query_row(
            "SELECT revision FROM plugin_settings WHERE plugin_id = ?1",
            [plugin_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    if current_revision.map(|value| WireSequence::new(value as u64))
        != settings.expected_previous_revision
    {
        return Err(AppPersistenceError::Conflict);
    }
    let next_revision = settings
        .expected_previous_revision
        .map(super::next_revision)
        .transpose()?
        .unwrap_or(1);
    transaction.execute(
        "INSERT INTO plugin_settings
         (plugin_id, package_sha256, schema_json, schema_sha256, values_json,
          revision, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(plugin_id) DO UPDATE SET
           package_sha256 = excluded.package_sha256,
           schema_json = excluded.schema_json,
           schema_sha256 = excluded.schema_sha256,
           values_json = excluded.values_json,
           revision = excluded.revision,
           updated_at_ms = excluded.updated_at_ms",
        params![
            plugin_id.as_str(),
            package_sha256,
            settings.schema_json,
            settings.schema_sha256,
            settings.values_json,
            u64_to_i64(next_revision)?,
            updated_at_unix_ms,
        ],
    )?;
    Ok(())
}

fn get_plugin_settings_transaction(
    transaction: &Transaction<'_>,
    plugin_id: &PluginId,
) -> Result<Option<PluginSettingsRecord>> {
    transaction
        .query_row(
            "SELECT plugin_id, package_sha256, schema_json, schema_sha256,
                    values_json, revision, updated_at_ms
             FROM plugin_settings WHERE plugin_id = ?1",
            [plugin_id.as_str()],
            read_plugin_settings,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn read_plugin_settings(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginSettingsRecord> {
    Ok(PluginSettingsRecord {
        plugin_id: parse_id(row.get::<_, String>(0)?, PluginId::parse)?,
        package_sha256: row.get(1)?,
        schema_json: row.get(2)?,
        schema_sha256: row.get(3)?,
        values_json: row.get(4)?,
        revision: read_wire_sequence(row, 5)?,
        updated_at_unix_ms: row.get(6)?,
    })
}

fn validate_hash(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(AppPersistenceError::InvalidInput(
            "plugin settings digest is invalid",
        ));
    }
    Ok(())
}

fn validate_schema_json(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_PLUGIN_SETTINGS_SCHEMA_BYTES
        || !serde_json::from_str::<serde_json::Value>(value).is_ok_and(|value| value.is_object())
    {
        return Err(AppPersistenceError::InvalidInput(
            "plugin settings schema must be a bounded JSON object",
        ));
    }
    Ok(())
}

fn validate_values_json(value: &str) -> Result<()> {
    if value.len() > MAX_PLUGIN_SETTINGS_VALUES_BYTES
        || !serde_json::from_str::<serde_json::Value>(value).is_ok_and(|value| value.is_object())
    {
        return Err(AppPersistenceError::InvalidInput(
            "plugin settings values must be a bounded JSON object",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{PluginInstallState, PluginOperationId, PluginOperationKind};

    use super::*;
    use crate::{PluginInstalledRecord, PluginOperationPhase};

    fn installed(
        plugin_id: PluginId,
        package_sha256: &str,
        revision: u64,
    ) -> PluginInstalledRecord {
        PluginInstalledRecord {
            plugin_id,
            name: "Settings fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: format!("1.0.{revision}"),
            package_sha256: package_sha256.to_owned(),
            capabilities: Vec::new(),
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(revision),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: revision as i64,
        }
    }

    #[test]
    fn settings_install_replace_and_uninstall_retention_are_revision_fenced() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let plugin_id = PluginId::parse("org.norishell.settings-fixture").expect("plugin id");
        let package_a = "a".repeat(64);
        let first = installed(plugin_id.clone(), &package_a, 1);
        repository
            .activate_plugin_installation_with_settings(
                None,
                &first,
                100,
                "",
                "",
                true,
                None,
                Some(PluginSettingsInstall {
                    schema_json: r#"{"schemaVersion":1,"fields":[]}"#,
                    schema_sha256: &"b".repeat(64),
                    values_json: r#"{"show":true}"#,
                    expected_previous_revision: None,
                }),
            )
            .expect("install settings");
        let initial = repository
            .get_plugin_settings(&plugin_id)
            .expect("read settings")
            .expect("settings row");
        assert_eq!(initial.revision, WireSequence::new(1));

        let replaced = repository
            .replace_plugin_settings(
                &plugin_id,
                &package_a,
                WireSequence::new(1),
                &initial.schema_sha256,
                initial.revision,
                r#"{"show":false}"#,
            )
            .expect("replace values");
        assert_eq!(replaced.revision, WireSequence::new(2));
        assert!(matches!(
            repository.replace_plugin_settings(
                &plugin_id,
                &package_a,
                WireSequence::new(1),
                &initial.schema_sha256,
                initial.revision,
                r#"{"show":true}"#,
            ),
            Err(AppPersistenceError::Conflict)
        ));

        let operation_id = PluginOperationId::new();
        let operation = repository
            .begin_plugin_operation(
                &operation_id,
                Some(&plugin_id),
                PluginOperationKind::Uninstall,
                "retain-settings-fixture",
                &"c".repeat(64),
                None,
                Some(&first.active_version),
            )
            .expect("begin uninstall");
        let committed = repository
            .uninstall_plugin(
                &plugin_id,
                WireSequence::new(1),
                &operation_id,
                operation.state_version,
                true,
            )
            .expect("retain uninstall");
        assert_eq!(committed.phase, PluginOperationPhase::DatabaseCommitted);
        assert!(
            repository
                .get_plugin_settings(&plugin_id)
                .expect("retained settings")
                .is_some()
        );

        repository
            .activate_plugin_installation_with_settings(
                None,
                &first,
                100,
                "",
                "",
                true,
                None,
                Some(PluginSettingsInstall {
                    schema_json: &initial.schema_json,
                    schema_sha256: &initial.schema_sha256,
                    values_json: &replaced.values_json,
                    expected_previous_revision: Some(replaced.revision),
                }),
            )
            .expect("reinstall retained settings");
        let delete_operation_id = PluginOperationId::new();
        let delete_operation = repository
            .begin_plugin_operation(
                &delete_operation_id,
                Some(&plugin_id),
                PluginOperationKind::Uninstall,
                "delete-settings-fixture",
                &"d".repeat(64),
                None,
                Some(&first.active_version),
            )
            .expect("begin delete-data uninstall");
        repository
            .uninstall_plugin(
                &plugin_id,
                WireSequence::new(1),
                &delete_operation_id,
                delete_operation.state_version,
                false,
            )
            .expect("delete-data uninstall");
        assert_eq!(
            repository
                .get_plugin_settings(&plugin_id)
                .expect("deleted settings"),
            None
        );
    }
}
