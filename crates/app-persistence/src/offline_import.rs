//! Host-owned offline import journal, independent of every plugin lifecycle.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineImportPlan {
    pub attempt_id: OperationId,
    pub archive_sha256: String,
    pub plan_sha256: String,
    pub identities: Vec<SshSyncRestoreIdentityInput>,
    pub credentials: Vec<SshSyncRestoreCredentialInput>,
    pub hosts: Vec<SshSyncRestoreHostInput>,
    pub desktop_profiles: Vec<SshSyncRestoreDesktopProfileInput>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct OfflineImportSaga {
    pub attempt_id: OperationId,
    pub archive_sha256: String,
    pub plan_sha256: String,
    pub secret_ref_ids: Vec<SecretRefId>,
}

impl AppRepository {
    /// Enumerates every local Vault ID retained by SQLite, including staged,
    /// cleanup, and dormant sync records. Import callers must avoid minting any
    /// of these IDs even when a record is not currently user-visible.
    pub fn reserved_vault_secret_ref_ids(&self) -> Result<Vec<SecretRefId>> {
        let transaction = self.connection.unchecked_transaction()?;
        let refs = reserved_vault_secret_ref_ids(&transaction)?;
        transaction.commit()?;
        Ok(refs)
    }

    /// Caller must already hold its Vault lifecycle lock. The callback must not
    /// reenter this repository or acquire the Vault lock again. No SQLite data
    /// is changed here: the callback's durable result remains independent of
    /// this guard, including if releasing the transaction subsequently fails.
    pub fn with_fresh_database_guard<T, E>(
        &mut self,
        action: impl FnOnce() -> std::result::Result<T, E>,
    ) -> Result<std::result::Result<T, E>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fresh_database(&transaction)?;
        let result = action();
        transaction.rollback()?;
        Ok(result)
    }

    pub fn preview_offline_import(
        &mut self,
        plan: &OfflineImportPlan,
    ) -> Result<SshSyncRestorePreview> {
        let plan = normalized_offline_restore_plan(plan, None)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let preview = preview_restore_plan(&transaction, &plan)?;
        transaction.commit()?;
        Ok(preview)
    }

    /// The journal is durable before the caller inserts any Vault material.
    pub fn begin_offline_import(
        &mut self,
        plan: &OfflineImportPlan,
        expected_fence: &SshSyncChangeFence,
    ) -> Result<(OfflineImportSaga, SshSyncChangeFence)> {
        let plan = normalized_offline_restore_plan(plan, None)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ssh_sync_change_fence(&transaction, expected_fence)?;
        let preview = preview_restore_plan(&transaction, &plan)?;
        if preview.conflict_count != 0 {
            return Err(AppPersistenceError::Conflict);
        }
        let saga = OfflineImportSaga {
            attempt_id: plan.attempt_id,
            archive_sha256: plan.bundle_sha256,
            plan_sha256: plan.plan_sha256,
            secret_ref_ids: preview.new_secret_ref_ids,
        };
        if let Some(existing) = read_saga(&transaction, &saga.attempt_id)? {
            if existing != saga {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
        } else {
            transaction.execute(
                "INSERT INTO offline_import_sagas (attempt_id, archive_sha256, plan_sha256, secret_ref_ids_json) VALUES (?1, ?2, ?3, ?4)",
                params![saga.attempt_id.as_str(), saga.archive_sha256, saga.plan_sha256, encode_secret_ref_ids(&saga.secret_ref_ids)?],
            )?;
        }
        let fence = current_ssh_sync_change_fence(&transaction)?;
        transaction.commit()?;
        Ok((saga, fence))
    }

    /// All metadata and journal removal commit together. A remaining journal
    /// always denotes unpublished material, never an already-visible import.
    pub fn commit_offline_import(
        &mut self,
        plan: &OfflineImportPlan,
        expected_fence: &SshSyncChangeFence,
    ) -> Result<SshSyncRestoreCommitResult> {
        let plan = normalized_offline_restore_plan(plan, None)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ssh_sync_change_fence(&transaction, expected_fence)?;
        let saga =
            read_saga(&transaction, &plan.attempt_id)?.ok_or(AppPersistenceError::NotFound)?;
        let preview = preview_restore_plan(&transaction, &plan)?;
        if preview.conflict_count != 0 {
            return Err(AppPersistenceError::Conflict);
        }
        if saga.archive_sha256 != plan.bundle_sha256
            || saga.plan_sha256 != plan.plan_sha256
            || saga.secret_ref_ids != preview.new_secret_ref_ids
        {
            return Err(AppPersistenceError::IdempotencyConflict);
        }
        publish_restore_plan(&transaction, &plan, unix_time_ms())?;
        transaction.execute(
            "DELETE FROM offline_import_sagas WHERE attempt_id = ?1",
            [plan.attempt_id.as_str()],
        )?;
        let result = SshSyncRestoreCommitResult {
            created_count: preview.create_count,
            already_applied_count: preview.already_applied_count,
        };
        if transaction.commit().is_err() {
            // Never discard new Vault refs when the metadata commit may have succeeded.
            let confirmed = (|| -> Result<bool> {
                let preview = preview_restore_plan(&self.connection, &plan)?;
                Ok(read_saga(&self.connection, &plan.attempt_id)?.is_none()
                    && preview.create_count == 0
                    && preview.conflict_count == 0)
            })();
            if !matches!(confirmed, Ok(true)) {
                return Err(AppPersistenceError::RestoreCommitUnknown);
            }
        }
        Ok(result)
    }

    pub fn pending_offline_imports(&self) -> Result<Vec<OfflineImportSaga>> {
        let mut statement = self.connection.prepare("SELECT attempt_id, archive_sha256, plan_sha256, secret_ref_ids_json FROM offline_import_sagas ORDER BY attempt_id")?;
        statement
            .query_map([], read_row)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Caller must first remove every listed Vault ref. Does not touch plugins.
    pub fn finish_offline_import_cleanup(&mut self, saga: &OfflineImportSaga) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = read_saga(&transaction, &saga.attempt_id)? {
            if existing != *saga {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            transaction.execute(
                "DELETE FROM offline_import_sagas WHERE attempt_id = ?1",
                [saga.attempt_id.as_str()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

const RESERVED_SECRET_REF_COLUMNS: &[(&str, &str)] = &[
    ("credential_secret_slots", "secret_ref_id"),
    ("host_login_automation_steps", "secret_ref_id"),
    ("plugin_credentials", "secret_ref_id"),
    ("login_automation_secret_stages", "secret_ref_id"),
    ("host_create_password_stages", "secret_ref_id"),
    ("offline_import_sagas", "secret_ref_ids_json"),
    ("ssh_sync_restore_sagas", "secret_ref_ids_json"),
    ("ssh_sync_profile_states", "sync_key_secret_ref_id"),
    ("ssh_sync_vault_gc_queue", "secret_ref_id"),
    ("ssh_sync_plugin_delete_refs", "secret_ref_id"),
    ("ssh_sync_owned_delta_operations", "gc_ref_ids_json"),
];

fn reserved_vault_secret_ref_ids(connection: &Connection) -> Result<Vec<SecretRefId>> {
    let schema_version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if schema_version != SCHEMA_VERSION {
        return Err(AppPersistenceError::UnsupportedSchema(schema_version));
    }
    let mut expected = RESERVED_SECRET_REF_COLUMNS
        .iter()
        .map(|(table, column)| (table.to_string(), column.to_string()))
        .collect::<BTreeSet<_>>();
    let tables = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND substr(name, 1, 7) <> 'sqlite_'",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut has_secret_mapping_columns = false;
    for table in &tables {
        let quoted_table = table.replace('"', "\"\"");
        let mut statement =
            connection.prepare(&format!("PRAGMA table_xinfo(\"{quoted_table}\")"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if table == "ssh_sync_object_mappings" {
            has_secret_mapping_columns = columns.iter().any(|column| column == "object_kind")
                && columns.iter().any(|column| column == "local_object_id");
        }
        for column in columns {
            if expected.remove(&(table.clone(), column.clone())) {
                continue;
            }
            let lower = column.to_ascii_lowercase();
            if lower.contains("secret_ref")
                || (lower.contains("gc") && lower.ends_with("_ref_ids_json"))
            {
                return Err(AppPersistenceError::InvalidStoredData);
            }
        }
    }
    if !expected.is_empty() || !has_secret_mapping_columns {
        return Err(AppPersistenceError::InvalidStoredData);
    }

    let mut ids = BTreeSet::new();
    for &(table, column) in RESERVED_SECRET_REF_COLUMNS {
        let mut statement = connection.prepare(&format!("SELECT {column} FROM {table}"))?;
        let values = statement.query_map([], |row| row.get::<_, Option<String>>(0))?;
        for value in values {
            let Some(value) = value? else { continue };
            if column.ends_with("_json") {
                let parsed = serde_json::from_str::<Vec<String>>(&value)
                    .map_err(|_| AppPersistenceError::InvalidStoredData)?;
                for value in parsed {
                    let id = SecretRefId::parse(value)
                        .map_err(|_| AppPersistenceError::InvalidStoredData)?;
                    ids.insert(id.as_str().to_owned());
                }
            } else {
                let id = SecretRefId::parse(value)
                    .map_err(|_| AppPersistenceError::InvalidStoredData)?;
                ids.insert(id.as_str().to_owned());
            }
        }
    }
    let mut statement = connection.prepare(
        "SELECT local_object_id FROM ssh_sync_object_mappings WHERE object_kind = 'secret'",
    )?;
    let mapping_refs = statement.query_map([], |row| row.get::<_, String>(0))?;
    for value in mapping_refs {
        let id = SecretRefId::parse(value?).map_err(|_| AppPersistenceError::InvalidStoredData)?;
        ids.insert(id.as_str().to_owned());
    }
    ids.into_iter()
        .map(|value| SecretRefId::parse(value).map_err(|_| AppPersistenceError::InvalidStoredData))
        .collect()
}

fn require_fresh_database(connection: &Connection) -> Result<()> {
    // Enumerating the schema fails closed when a future feature adds a table
    // containing references, recovery work, or other user data.
    let tables = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND substr(name, 1, 7) <> 'sqlite_'",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for table in tables {
        match table.as_str() {
            // These settings have no user-data or Vault references.
            "desktop_preferences" => continue,
            "ssh_sync_business_generation" => {
                current_ssh_sync_change_fence(connection)?;
            }
            "terminal_workspace_layout" => {
                let json: String = connection.query_row(
                    "SELECT layout_json FROM terminal_workspace_layout WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )?;
                let layout: TerminalWorkspaceLayout = serde_json::from_str(&json)
                    .map_err(|_| AppPersistenceError::InvalidStoredData)?;
                layout
                    .validate()
                    .map_err(|_| AppPersistenceError::InvalidStoredData)?;
                if layout.tabs.iter().flat_map(|tab| &tab.panes).any(|pane| {
                    !matches!(
                        pane,
                        norishell_core_api::TerminalWorkspacePane::Launcher { .. }
                    )
                }) {
                    return Err(AppPersistenceError::DatabaseNotFresh);
                }
            }
            _ => {
                let quoted_table = table.replace('"', "\"\"");
                let has_rows: bool = connection.query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM \"{quoted_table}\" LIMIT 1)"),
                    [],
                    |row| row.get(0),
                )?;
                if has_rows {
                    return Err(AppPersistenceError::DatabaseNotFresh);
                }
            }
        }
    }
    Ok(())
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OfflineImportSaga> {
    let refs: String = row.get(3)?;
    let secret_ref_ids = serde_json::from_str(&refs).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(OfflineImportSaga {
        attempt_id: parse_id(row.get::<_, String>(0)?, OperationId::parse)?,
        archive_sha256: row.get(1)?,
        plan_sha256: row.get(2)?,
        secret_ref_ids,
    })
}

fn read_saga(connection: &Connection, id: &OperationId) -> Result<Option<OfflineImportSaga>> {
    connection.query_row("SELECT attempt_id, archive_sha256, plan_sha256, secret_ref_ids_json FROM offline_import_sagas WHERE attempt_id = ?1", [id.as_str()], read_row).optional().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn reserved_refs_include_active_dormant_and_pending_cleanup_records() {
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("refs.sqlite")).unwrap();
        assert!(
            repository
                .reserved_vault_secret_ref_ids()
                .unwrap()
                .is_empty()
        );
        let active = SecretRefId::new();
        let staged = SecretRefId::new();
        let pending = SecretRefId::new();
        let gc = SecretRefId::new();
        let mapped = SecretRefId::new();
        let identity = repository.create_identity("Identity", None).unwrap();
        let credential = CredentialRefId::new();
        repository
            .connection
            .execute(
                "INSERT INTO credential_refs
             (id, identity_id, kind, priority, label, state_version, created_at_ms,
              updated_at_ms, import_state)
             VALUES (?1, ?2, 'password', 0, 'Password', 1, 0, 0, 'ready')",
                params![credential.as_str(), identity.identity_id.as_str()],
            )
            .unwrap();
        repository
            .connection
            .execute_batch("PRAGMA foreign_keys = OFF")
            .unwrap();
        repository.connection.execute(
            "INSERT INTO credential_secret_slots (credential_ref_id, slot_index, slot_kind, secret_ref_id, label)
             VALUES (?1, 0, 'password', ?2, 'Password')",
            params![credential.as_str(), active.as_str()],
        ).unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO host_create_password_stages
             (stage_id, secret_ref_id, operation_id, idempotency_key, identity_label,
              credential_label, expires_at_ms, state, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 'retired', 'Identity', 'Password', 0, 'consumed', 0, 0)",
                params![
                    HostCreatePasswordStageId::new().as_str(),
                    staged.as_str(),
                    OperationId::new().as_str()
                ],
            )
            .unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO offline_import_sagas
             (attempt_id, archive_sha256, plan_sha256, secret_ref_ids_json)
             VALUES (?1, ?2, ?2, ?3)",
                params![
                    OperationId::new().as_str(),
                    "a".repeat(64),
                    serde_json::to_string(&vec![pending.as_str(), active.as_str()]).unwrap()
                ],
            )
            .unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO ssh_sync_owned_delta_operations
             (plugin_id, signer_fingerprint_sha256, profile_id, attempt_id, delta_sha256,
              created_count, updated_count, deleted_count, gc_ref_ids_json, committed_at_ms)
             VALUES ('plugin', ?1, 'profile', ?2, ?3, 0, 0, 0, ?4, 0)",
                params![
                    "b".repeat(64),
                    OperationId::new().as_str(),
                    "c".repeat(64),
                    serde_json::to_string(&vec![gc.as_str()]).unwrap()
                ],
            )
            .unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO ssh_sync_object_mappings
             (plugin_id, signer_fingerprint_sha256, profile_id, object_kind,
              portable_object_id, local_object_id, created_at_ms, updated_at_ms)
             VALUES ('plugin', ?1, 'profile', 'secret', ?2, ?3, 0, 0)",
                params!["b".repeat(64), SecretRefId::new().as_str(), mapped.as_str()],
            )
            .unwrap();
        repository
            .connection
            .execute_batch("PRAGMA foreign_keys = ON")
            .unwrap();
        let actual = repository.reserved_vault_secret_ref_ids().unwrap();
        let mut expected = vec![active, staged, pending, gc, mapped];
        expected.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        assert_eq!(actual, expected);
    }

    #[test]
    fn reserved_refs_reject_malformed_values_and_unrecognized_future_columns() {
        let directory = tempfile::tempdir().unwrap();
        let repository = AppRepository::open(directory.path().join("refs.sqlite")).unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO offline_import_sagas
             (attempt_id, archive_sha256, plan_sha256, secret_ref_ids_json)
             VALUES (?1, ?2, ?2, '[1]')",
                params![OperationId::new().as_str(), "a".repeat(64)],
            )
            .unwrap();
        assert!(matches!(
            repository.reserved_vault_secret_ref_ids(),
            Err(AppPersistenceError::InvalidStoredData)
        ));
        repository
            .connection
            .execute("DELETE FROM offline_import_sagas", [])
            .unwrap();
        repository.connection.execute(
            "INSERT INTO host_create_password_stages
             (stage_id, secret_ref_id, operation_id, idempotency_key, identity_label,
              credential_label, expires_at_ms, state, created_at_ms, updated_at_ms)
             VALUES (?1, 'invalid-id', ?2, 'invalid', 'Identity', 'Password', 0, 'cancelled', 0, 0)",
            params![HostCreatePasswordStageId::new().as_str(), OperationId::new().as_str()],
        ).unwrap();
        assert!(matches!(
            repository.reserved_vault_secret_ref_ids(),
            Err(AppPersistenceError::InvalidStoredData)
        ));
        repository
            .connection
            .execute("DELETE FROM host_create_password_stages", [])
            .unwrap();
        repository
            .connection
            .execute_batch("ALTER TABLE offline_import_sagas ADD COLUMN future_secret_ref_id TEXT")
            .unwrap();
        assert!(matches!(
            repository.reserved_vault_secret_ref_ids(),
            Err(AppPersistenceError::InvalidStoredData)
        ));
    }

    #[test]
    fn fresh_database_guard_holds_writer_exclusion_and_preserves_callback_errors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fresh.sqlite");
        let mut repository = AppRepository::open(&path).unwrap();
        let competing = AppRepository::open(&path).unwrap();
        competing
            .connection
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        let value = repository
            .with_fresh_database_guard(|| {
                let error = competing
                    .connection
                    .execute("UPDATE desktop_preferences SET revision = revision + 1", [])
                    .unwrap_err();
                assert_eq!(
                    error.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
                Ok::<_, &'static str>(17)
            })
            .unwrap()
            .unwrap();
        assert_eq!(value, 17);
        let result = repository.with_fresh_database_guard(|| Err::<(), _>("callback failed"));
        assert_eq!(result.unwrap(), Err("callback failed"));
        competing
            .connection
            .execute("UPDATE desktop_preferences SET revision = revision + 1", [])
            .unwrap();
        repository
            .with_fresh_database_guard(|| Ok::<_, ()>(()))
            .unwrap()
            .unwrap();
    }

    #[test]
    fn fresh_database_guard_rejects_user_data_and_future_tables_without_running_callback() {
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("fresh.sqlite")).unwrap();
        let called = Cell::new(false);
        repository
            .create_identity("Existing identity", None)
            .unwrap();
        assert!(matches!(
            repository.with_fresh_database_guard(|| {
                called.set(true);
                Ok::<_, ()>(())
            }),
            Err(AppPersistenceError::DatabaseNotFresh)
        ));
        assert!(!called.get());
        repository
            .connection
            .execute("DELETE FROM identities", [])
            .unwrap();
        repository.connection.execute_batch(
            "CREATE TABLE \"future\"\"refs\" (id TEXT); INSERT INTO \"future\"\"refs\" VALUES ('opaque-ref');",
        ).unwrap();
        assert!(matches!(
            repository.with_fresh_database_guard(|| Ok::<_, ()>(())),
            Err(AppPersistenceError::DatabaseNotFresh)
        ));
    }

    #[test]
    fn fresh_database_guard_rejects_pending_offline_journal_even_without_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("fresh.sqlite")).unwrap();
        repository
            .connection
            .execute(
                "INSERT INTO offline_import_sagas VALUES (?1, ?2, ?2, '[]')",
                params![OperationId::new().as_str(), "a".repeat(64)],
            )
            .unwrap();
        assert!(matches!(
            repository.with_fresh_database_guard(|| Ok::<_, ()>(())),
            Err(AppPersistenceError::DatabaseNotFresh)
        ));
    }

    #[test]
    fn fresh_database_guard_accepts_empty_launcher_but_rejects_terminal_history() {
        use norishell_core_api::{
            TerminalWorkspaceLayoutNode, TerminalWorkspacePane, TerminalWorkspaceTab,
        };
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("fresh.sqlite")).unwrap();
        let mut layout = TerminalWorkspaceLayout {
            schema_version: 1,
            active_tab_id: Some("tab".to_owned()),
            tabs: vec![TerminalWorkspaceTab {
                tab_id: "tab".to_owned(),
                layout: TerminalWorkspaceLayoutNode::Pane {
                    pane_id: "pane".to_owned(),
                    terminal_id: "pane".to_owned(),
                },
                active_pane_id: "pane".to_owned(),
                panes: vec![TerminalWorkspacePane::Launcher {
                    pane_id: "pane".to_owned(),
                    label: "New terminal".to_owned(),
                }],
            }],
        };
        layout.validate().unwrap();
        repository
            .connection
            .execute(
                "UPDATE terminal_workspace_layout SET layout_json = ?1",
                [serde_json::to_string(&layout).unwrap()],
            )
            .unwrap();
        repository
            .with_fresh_database_guard(|| Ok::<_, ()>(()))
            .unwrap()
            .unwrap();
        layout.tabs[0].panes[0] = TerminalWorkspacePane::Local {
            pane_id: "pane".to_owned(),
            label: "Local shell".to_owned(),
        };
        repository
            .connection
            .execute(
                "UPDATE terminal_workspace_layout SET layout_json = ?1",
                [serde_json::to_string(&layout).unwrap()],
            )
            .unwrap();
        assert!(matches!(
            repository.with_fresh_database_guard(|| Ok::<_, ()>(())),
            Err(AppPersistenceError::DatabaseNotFresh)
        ));
    }
}
