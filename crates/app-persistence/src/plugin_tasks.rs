//! Durable, non-secret facts for Core-owned plugin workflow tasks.
//!
//! This module intentionally persists only identity, fixed workflow/step IDs,
//! safe method names and lifecycle facts. Call payloads, input JSON, replies,
//! URLs, commands, paths, errors and every kind of secret-derived value remain
//! in the owning Core process only.

use norishell_core_api::{
    PluginId, PluginWorkflowApiMethod, PluginWorkflowStepState, PluginWorkflowTaskId,
    PluginWorkflowTaskState, WireSequence, valid_plugin_workflow_id,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    AppPersistenceError, AppRepository, Result, next_revision, parse_id, read_wire_sequence,
    u64_to_i64, unix_time_ms,
};

pub const MAX_PLUGIN_WORKFLOW_TASKS_PER_PLUGIN: u32 = 4;
pub const MAX_PLUGIN_WORKFLOW_TASKS_TOTAL: u32 = 16;

const ACTIVE_TASK_STATES: &str =
    "'running', 'dispatching', 'needs_user_action', 'cancelling', 'cleanup_incomplete'";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginWorkflowTaskRecord {
    pub task_id: PluginWorkflowTaskId,
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
    pub package_sha256: String,
    pub workflow_id: String,
    pub state: PluginWorkflowTaskState,
    pub revision: WireSequence,
    pub current_step_id: Option<String>,
    pub outcome_unknown: bool,
    pub cleanup_incomplete: bool,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginWorkflowTaskStepRecord {
    pub task_id: PluginWorkflowTaskId,
    pub step_id: String,
    pub method: PluginWorkflowApiMethod,
    pub state: PluginWorkflowStepState,
    pub revision: WireSequence,
    pub outcome_unknown: bool,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

/// The identity values are all independently rechecked against the current
/// installation in the same SQLite transaction before the task row is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginWorkflowTaskCreate {
    pub task_id: PluginWorkflowTaskId,
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
    pub package_sha256: String,
    pub expected_installed_state_version: WireSequence,
    pub workflow_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginWorkflowTaskTransition {
    pub state: PluginWorkflowTaskState,
    pub outcome_unknown: bool,
    pub cleanup_incomplete: bool,
}

impl AppRepository {
    /// Creates a task only for the exact active package and a current
    /// installation revision. The global and per-plugin limits are checked in
    /// this same transaction so concurrent callers cannot over-admit tasks.
    pub fn create_plugin_workflow_task(
        &mut self,
        input: &PluginWorkflowTaskCreate,
    ) -> Result<PluginWorkflowTaskRecord> {
        validate_task_create(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let installation: Option<(String, String, i64, String)> = transaction
            .query_row(
                "SELECT signer_fingerprint_sha256, package_sha256, state_version, state
                 FROM plugin_installations WHERE plugin_id = ?1",
                [input.plugin_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let expected_installation = (
            input.signer_fingerprint_sha256.clone(),
            input.package_sha256.clone(),
            u64_to_i64(input.expected_installed_state_version.get())?,
            "enabled".to_owned(),
        );
        if installation != Some(expected_installation) {
            return Err(AppPersistenceError::Conflict);
        }
        let plugin_active_sql = format!(
            "SELECT COUNT(*) FROM plugin_workflow_tasks
             WHERE plugin_id = ?1 AND state IN ({ACTIVE_TASK_STATES})"
        );
        let plugin_active: i64 =
            transaction.query_row(&plugin_active_sql, [input.plugin_id.as_str()], |row| {
                row.get(0)
            })?;
        let total_active_sql = format!(
            "SELECT COUNT(*) FROM plugin_workflow_tasks WHERE state IN ({ACTIVE_TASK_STATES})"
        );
        let total_active: i64 = transaction.query_row(&total_active_sql, [], |row| row.get(0))?;
        if plugin_active >= i64::from(MAX_PLUGIN_WORKFLOW_TASKS_PER_PLUGIN)
            || total_active >= i64::from(MAX_PLUGIN_WORKFLOW_TASKS_TOTAL)
        {
            return Err(AppPersistenceError::Conflict);
        }
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO plugin_workflow_tasks
             (task_id, plugin_id, signer_fingerprint_sha256, package_sha256, workflow_id,
              state, current_step_id, outcome_unknown, cleanup_incomplete, state_version,
              created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', NULL, 0, 0, 1, ?6, ?6)",
            params![
                input.task_id.as_str(),
                input.plugin_id.as_str(),
                input.signer_fingerprint_sha256,
                input.package_sha256,
                input.workflow_id,
                now,
            ],
        )?;
        let record = get_plugin_workflow_task_transaction(&transaction, &input.task_id)?
            .ok_or(AppPersistenceError::Conflict)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn get_plugin_workflow_task(
        &self,
        task_id: &PluginWorkflowTaskId,
    ) -> Result<PluginWorkflowTaskRecord> {
        self.connection
            .query_row(
                TASK_SELECT_SQL,
                [task_id.as_str()],
                read_plugin_workflow_task,
            )
            .optional()?
            .ok_or(AppPersistenceError::NotFound)
    }

    pub fn list_plugin_workflow_tasks(
        &self,
        plugin_id: Option<&PluginId>,
    ) -> Result<Vec<PluginWorkflowTaskRecord>> {
        let mut records = Vec::new();
        if let Some(plugin_id) = plugin_id {
            let mut statement = self.connection.prepare(
                "SELECT task_id, plugin_id, signer_fingerprint_sha256, package_sha256, workflow_id,
                        state, state_version, current_step_id, outcome_unknown, cleanup_incomplete,
                        created_at_ms, updated_at_ms
                 FROM plugin_workflow_tasks WHERE plugin_id = ?1
                 ORDER BY updated_at_ms DESC, task_id DESC",
            )?;
            records.extend(
                statement
                    .query_map([plugin_id.as_str()], read_plugin_workflow_task)?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        } else {
            let mut statement = self.connection.prepare(
                "SELECT task_id, plugin_id, signer_fingerprint_sha256, package_sha256, workflow_id,
                        state, state_version, current_step_id, outcome_unknown, cleanup_incomplete,
                        created_at_ms, updated_at_ms
                 FROM plugin_workflow_tasks ORDER BY updated_at_ms DESC, task_id DESC",
            )?;
            records.extend(
                statement
                    .query_map([], read_plugin_workflow_task)?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }
        Ok(records)
    }

    pub fn list_plugin_workflow_task_steps(
        &self,
        task_id: &PluginWorkflowTaskId,
    ) -> Result<Vec<PluginWorkflowTaskStepRecord>> {
        list_plugin_workflow_task_steps_connection(&self.connection, task_id)
    }

    /// Starts (or explicitly resumes) one fixed step. The task and step
    /// transitions happen before the caller invokes a capability, which gives
    /// a crash boundary for an externally observable dispatch.
    pub fn begin_plugin_workflow_task_step(
        &mut self,
        task_id: &PluginWorkflowTaskId,
        expected_task_revision: WireSequence,
        step_id: &str,
        method: PluginWorkflowApiMethod,
        expected_step_revision: Option<WireSequence>,
    ) -> Result<(PluginWorkflowTaskRecord, PluginWorkflowTaskStepRecord)> {
        validate_identifier(step_id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = get_plugin_workflow_task_transaction(&transaction, task_id)?
            .ok_or(AppPersistenceError::NotFound)?;
        if task.revision != expected_task_revision
            || !matches!(
                task.state,
                PluginWorkflowTaskState::Running | PluginWorkflowTaskState::NeedsUserAction
            )
        {
            return Err(AppPersistenceError::Conflict);
        }
        let existing = get_plugin_workflow_task_step_transaction(&transaction, task_id, step_id)?;
        let now = unix_time_ms();
        let step = match existing {
            None if expected_step_revision.is_none() => {
                transaction.execute(
                    "INSERT INTO plugin_workflow_task_steps
                     (task_id, step_id, method, state, outcome_unknown, state_version,
                      created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, ?3, 'dispatching', 0, 1, ?4, ?4)",
                    params![
                        task_id.as_str(),
                        step_id,
                        workflow_method_to_db(method),
                        now
                    ],
                )?;
                get_plugin_workflow_task_step_transaction(&transaction, task_id, step_id)?
                    .ok_or(AppPersistenceError::Conflict)?
            }
            Some(existing)
                if expected_step_revision == Some(existing.revision)
                    && existing.method == method
                    && existing.state == PluginWorkflowStepState::NeedsUserAction =>
            {
                let next = next_revision(existing.revision)?;
                let changed = transaction.execute(
                    "UPDATE plugin_workflow_task_steps
                     SET state = 'dispatching', state_version = ?1, updated_at_ms = ?2
                     WHERE task_id = ?3 AND step_id = ?4 AND state_version = ?5",
                    params![
                        u64_to_i64(next)?,
                        now,
                        task_id.as_str(),
                        step_id,
                        u64_to_i64(existing.revision.get())?,
                    ],
                )?;
                if changed != 1 {
                    return Err(AppPersistenceError::Conflict);
                }
                get_plugin_workflow_task_step_transaction(&transaction, task_id, step_id)?
                    .ok_or(AppPersistenceError::Conflict)?
            }
            _ => return Err(AppPersistenceError::Conflict),
        };
        let next_task = next_revision(task.revision)?;
        let changed = transaction.execute(
            "UPDATE plugin_workflow_tasks
             SET state = 'dispatching', current_step_id = ?1, state_version = ?2,
                 updated_at_ms = ?3
             WHERE task_id = ?4 AND state_version = ?5",
            params![
                step_id,
                u64_to_i64(next_task)?,
                now,
                task_id.as_str(),
                u64_to_i64(task.revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let updated_task = get_plugin_workflow_task_transaction(&transaction, task_id)?
            .ok_or(AppPersistenceError::Conflict)?;
        transaction.commit()?;
        Ok((updated_task, step))
    }

    /// Saves the stable result fact before Core asks Wasm to advance. The
    /// reply itself is intentionally absent from this method and from SQLite.
    #[allow(clippy::too_many_arguments)]
    pub fn settle_plugin_workflow_task_step(
        &mut self,
        task_id: &PluginWorkflowTaskId,
        expected_task_revision: WireSequence,
        step_id: &str,
        expected_step_revision: WireSequence,
        step_state: PluginWorkflowStepState,
        task_state: PluginWorkflowTaskState,
        outcome_unknown: bool,
    ) -> Result<(PluginWorkflowTaskRecord, PluginWorkflowTaskStepRecord)> {
        validate_identifier(step_id)?;
        if !matches!(
            step_state,
            PluginWorkflowStepState::Succeeded
                | PluginWorkflowStepState::Failed
                | PluginWorkflowStepState::NeedsUserAction
                | PluginWorkflowStepState::OutcomeUnknown
                | PluginWorkflowStepState::Interrupted
        ) {
            return Err(AppPersistenceError::InvalidInput(
                "invalid workflow step settlement",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = get_plugin_workflow_task_transaction(&transaction, task_id)?
            .ok_or(AppPersistenceError::NotFound)?;
        let step = get_plugin_workflow_task_step_transaction(&transaction, task_id, step_id)?
            .ok_or(AppPersistenceError::NotFound)?;
        if task.revision != expected_task_revision
            || step.revision != expected_step_revision
            || step.state != PluginWorkflowStepState::Dispatching
            || !task_transition_allowed(task.state, task_state)
        {
            return Err(AppPersistenceError::Conflict);
        }
        let now = unix_time_ms();
        let next_step = next_revision(step.revision)?;
        let next_task = next_revision(task.revision)?;
        let changed_step = transaction.execute(
            "UPDATE plugin_workflow_task_steps
             SET state = ?1, outcome_unknown = ?2, state_version = ?3, updated_at_ms = ?4
             WHERE task_id = ?5 AND step_id = ?6 AND state_version = ?7",
            params![
                workflow_step_state_to_db(step_state),
                outcome_unknown,
                u64_to_i64(next_step)?,
                now,
                task_id.as_str(),
                step_id,
                u64_to_i64(step.revision.get())?,
            ],
        )?;
        let changed_task = transaction.execute(
            "UPDATE plugin_workflow_tasks
             SET state = ?1, outcome_unknown = ?2, state_version = ?3, updated_at_ms = ?4
             WHERE task_id = ?5 AND state_version = ?6",
            params![
                workflow_task_state_to_db(task_state),
                task.outcome_unknown || outcome_unknown,
                u64_to_i64(next_task)?,
                now,
                task_id.as_str(),
                u64_to_i64(task.revision.get())?,
            ],
        )?;
        if changed_step != 1 || changed_task != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let updated_task = get_plugin_workflow_task_transaction(&transaction, task_id)?
            .ok_or(AppPersistenceError::Conflict)?;
        let updated_step =
            get_plugin_workflow_task_step_transaction(&transaction, task_id, step_id)?
                .ok_or(AppPersistenceError::Conflict)?;
        transaction.commit()?;
        Ok((updated_task, updated_step))
    }

    pub fn transition_plugin_workflow_task(
        &mut self,
        task_id: &PluginWorkflowTaskId,
        expected_revision: WireSequence,
        transition: PluginWorkflowTaskTransition,
    ) -> Result<PluginWorkflowTaskRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = get_plugin_workflow_task_transaction(&transaction, task_id)?
            .ok_or(AppPersistenceError::NotFound)?;
        if task.revision != expected_revision
            || !task_transition_allowed(task.state, transition.state)
        {
            return Err(AppPersistenceError::Conflict);
        }
        let next = next_revision(task.revision)?;
        let changed = transaction.execute(
            "UPDATE plugin_workflow_tasks
             SET state = ?1, outcome_unknown = ?2, cleanup_incomplete = ?3,
                 state_version = ?4, updated_at_ms = ?5
             WHERE task_id = ?6 AND state_version = ?7",
            params![
                workflow_task_state_to_db(transition.state),
                task.outcome_unknown || transition.outcome_unknown,
                transition.cleanup_incomplete,
                u64_to_i64(next)?,
                unix_time_ms(),
                task_id.as_str(),
                u64_to_i64(task.revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let record = get_plugin_workflow_task_transaction(&transaction, task_id)?
            .ok_or(AppPersistenceError::Conflict)?;
        transaction.commit()?;
        Ok(record)
    }

    /// Startup recovery is deliberately reconciliation only. It never invokes
    /// Wasm, reuses an in-memory payload, or replays an external call.
    pub fn recover_plugin_workflow_tasks(&mut self) -> Result<Vec<PluginWorkflowTaskRecord>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = transaction.prepare(
            "SELECT task_id, plugin_id, signer_fingerprint_sha256, package_sha256, workflow_id,
                    state, state_version, current_step_id, outcome_unknown, cleanup_incomplete,
                    created_at_ms, updated_at_ms
             FROM plugin_workflow_tasks
             WHERE state IN ('running', 'dispatching', 'needs_user_action', 'cancelling')",
        )?;
        let active = statement
            .query_map([], read_plugin_workflow_task)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let now = unix_time_ms();
        for task in &active {
            let was_dispatching = task.state == PluginWorkflowTaskState::Dispatching;
            let changed = transaction.execute(
                "UPDATE plugin_workflow_tasks
                 SET state = 'interrupted', outcome_unknown = ?1, state_version = ?2,
                     updated_at_ms = ?3
                 WHERE task_id = ?4 AND state_version = ?5",
                params![
                    task.outcome_unknown || was_dispatching,
                    u64_to_i64(next_revision(task.revision)?)?,
                    now,
                    task.task_id.as_str(),
                    u64_to_i64(task.revision.get())?,
                ],
            )?;
            if changed != 1 {
                return Err(AppPersistenceError::Conflict);
            }
            transaction.execute(
                "UPDATE plugin_workflow_task_steps
                 SET state = 'interrupted',
                     outcome_unknown = CASE WHEN state = 'dispatching' THEN 1 ELSE outcome_unknown END,
                     state_version = state_version + 1, updated_at_ms = ?1
                 WHERE task_id = ?2 AND state IN ('dispatching', 'needs_user_action')",
                params![now, task.task_id.as_str()],
            )?;
        }
        let recovered = active
            .iter()
            .map(|task| get_plugin_workflow_task_transaction(&transaction, &task.task_id))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(AppPersistenceError::Conflict)?;
        transaction.commit()?;
        Ok(recovered)
    }
}

pub(super) fn migrate_v37_to_v38(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TABLE plugin_workflow_tasks (
           task_id TEXT PRIMARY KEY CHECK(length(task_id) = 36),
           plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
           signer_fingerprint_sha256 TEXT NOT NULL
             CHECK(length(signer_fingerprint_sha256) = 64
               AND signer_fingerprint_sha256 = lower(signer_fingerprint_sha256)
               AND signer_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'),
           package_sha256 TEXT NOT NULL
             CHECK(length(package_sha256) = 64 AND package_sha256 = lower(package_sha256)
               AND package_sha256 NOT GLOB '*[^0-9a-f]*'),
           workflow_id TEXT NOT NULL CHECK(length(workflow_id) BETWEEN 1 AND 64),
           state TEXT NOT NULL CHECK(state IN
             ('running', 'dispatching', 'needs_user_action', 'cancelling', 'cancelled',
              'completed', 'failed', 'interrupted', 'cleanup_incomplete')),
           current_step_id TEXT CHECK(current_step_id IS NULL
             OR length(current_step_id) BETWEEN 1 AND 64),
           outcome_unknown INTEGER NOT NULL CHECK(outcome_unknown IN (0, 1)),
           cleanup_incomplete INTEGER NOT NULL CHECK(cleanup_incomplete IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL
         ) STRICT;
         CREATE INDEX plugin_workflow_tasks_plugin_updated
           ON plugin_workflow_tasks(plugin_id, updated_at_ms DESC, task_id DESC);
         CREATE INDEX plugin_workflow_tasks_active
           ON plugin_workflow_tasks(state, plugin_id);
         CREATE TABLE plugin_workflow_task_steps (
           task_id TEXT NOT NULL REFERENCES plugin_workflow_tasks(task_id) ON DELETE CASCADE,
           step_id TEXT NOT NULL CHECK(length(step_id) BETWEEN 1 AND 64),
           method TEXT NOT NULL CHECK(method IN
             ('serial_devices', 'serial_open', 'serial_send', 'protocol_open', 'describe',
              'permissions', 'permission_request', 'permissions_forget', 'resources_list',
              'resource_close', 'subscription_start', 'timer_start', 'resource_events',
              'network_start', 'network_send', 'remote_exec_start', 'remote_exec_send',
              'process_start', 'process_send', 'sftp_open', 'sftp', 'file_pick', 'file',
              'credential', 'storage', 'terminal_request_input')),
           state TEXT NOT NULL CHECK(state IN
             ('dispatching', 'succeeded', 'failed', 'needs_user_action', 'outcome_unknown',
              'interrupted')),
           outcome_unknown INTEGER NOT NULL CHECK(outcome_unknown IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(task_id, step_id)
         ) STRICT;
         CREATE INDEX plugin_workflow_task_steps_task_updated
           ON plugin_workflow_task_steps(task_id, updated_at_ms DESC, step_id DESC);
         PRAGMA user_version = 38;
         COMMIT;",
    )?;
    Ok(())
}

/// Adds the guest-visible remembered-operation revoke method and the durable
/// Core-owned expiry point.  The operation table intentionally stores a
/// nullable absolute timestamp rather than a duration: reopening the database
/// must never extend an approval.
pub(super) fn migrate_v38_to_v39(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE plugin_operation_permissions ADD COLUMN expires_at_unix_ms INTEGER
           CHECK(expires_at_unix_ms IS NULL OR expires_at_unix_ms > 0);
         -- Historical file/device rows may contain a selected path or a
         -- platform device identity. Keep the binding/fingerprint intact but
         -- replace only the management label with a Core-generated opaque id.
         UPDATE plugin_operation_permissions
            SET target_label = CASE operation
              WHEN 'file_access' THEN 'Selected local file/directory · ' || substr(permission_id, 1, 8)
              WHEN 'serial_access' THEN 'Selected serial device · ' || substr(permission_id, 1, 8)
              ELSE target_label
            END
          WHERE operation IN ('file_access', 'serial_access');
         DROP INDEX plugin_workflow_task_steps_task_updated;
         ALTER TABLE plugin_workflow_task_steps
           RENAME TO plugin_workflow_task_steps_v38;
         CREATE TABLE plugin_workflow_task_steps (
           task_id TEXT NOT NULL REFERENCES plugin_workflow_tasks(task_id) ON DELETE CASCADE,
           step_id TEXT NOT NULL CHECK(length(step_id) BETWEEN 1 AND 64),
           method TEXT NOT NULL CHECK(method IN
             ('serial_devices', 'serial_open', 'serial_send', 'protocol_open', 'describe',
              'permissions', 'permission_request', 'permission_revoke', 'permissions_forget',
              'resources_list', 'resource_close', 'subscription_start', 'timer_start',
              'resource_events', 'network_start', 'network_send', 'remote_exec_start',
              'remote_exec_send', 'process_start', 'process_send', 'sftp_open', 'sftp',
              'file_pick', 'file', 'credential', 'storage', 'terminal_request_input')),
           state TEXT NOT NULL CHECK(state IN
             ('dispatching', 'succeeded', 'failed', 'needs_user_action', 'outcome_unknown',
              'interrupted')),
           outcome_unknown INTEGER NOT NULL CHECK(outcome_unknown IN (0, 1)),
           state_version INTEGER NOT NULL CHECK(state_version >= 1),
           created_at_ms INTEGER NOT NULL,
           updated_at_ms INTEGER NOT NULL,
           PRIMARY KEY(task_id, step_id)
         ) STRICT;
         INSERT INTO plugin_workflow_task_steps
           (task_id, step_id, method, state, outcome_unknown, state_version,
            created_at_ms, updated_at_ms)
         SELECT task_id, step_id, method, state, outcome_unknown, state_version,
                created_at_ms, updated_at_ms
         FROM plugin_workflow_task_steps_v38;
         DROP TABLE plugin_workflow_task_steps_v38;
         CREATE INDEX plugin_workflow_task_steps_task_updated
           ON plugin_workflow_task_steps(task_id, updated_at_ms DESC, step_id DESC);
         PRAGMA user_version = 39;
         COMMIT;",
    )?;
    Ok(())
}

const TASK_SELECT_SQL: &str =
    "SELECT task_id, plugin_id, signer_fingerprint_sha256, package_sha256, workflow_id,
    state, state_version, current_step_id, outcome_unknown, cleanup_incomplete,
    created_at_ms, updated_at_ms
 FROM plugin_workflow_tasks WHERE task_id = ?1";

fn get_plugin_workflow_task_transaction(
    transaction: &Transaction<'_>,
    task_id: &PluginWorkflowTaskId,
) -> Result<Option<PluginWorkflowTaskRecord>> {
    transaction
        .query_row(
            TASK_SELECT_SQL,
            [task_id.as_str()],
            read_plugin_workflow_task,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn get_plugin_workflow_task_step_transaction(
    transaction: &Transaction<'_>,
    task_id: &PluginWorkflowTaskId,
    step_id: &str,
) -> Result<Option<PluginWorkflowTaskStepRecord>> {
    transaction
        .query_row(
            "SELECT task_id, step_id, method, state, state_version, outcome_unknown,
                    created_at_ms, updated_at_ms
             FROM plugin_workflow_task_steps WHERE task_id = ?1 AND step_id = ?2",
            params![task_id.as_str(), step_id],
            read_plugin_workflow_task_step,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn list_plugin_workflow_task_steps_connection(
    connection: &Connection,
    task_id: &PluginWorkflowTaskId,
) -> Result<Vec<PluginWorkflowTaskStepRecord>> {
    let mut statement = connection.prepare(
        "SELECT task_id, step_id, method, state, state_version, outcome_unknown,
                created_at_ms, updated_at_ms
         FROM plugin_workflow_task_steps WHERE task_id = ?1
         ORDER BY created_at_ms, step_id",
    )?;
    statement
        .query_map([task_id.as_str()], read_plugin_workflow_task_step)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn read_plugin_workflow_task(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PluginWorkflowTaskRecord> {
    Ok(PluginWorkflowTaskRecord {
        task_id: parse_id(row.get::<_, String>(0)?, PluginWorkflowTaskId::parse)?,
        plugin_id: parse_id(row.get::<_, String>(1)?, PluginId::parse)?,
        signer_fingerprint_sha256: row.get(2)?,
        package_sha256: row.get(3)?,
        workflow_id: row.get(4)?,
        state: workflow_task_state_from_db(&row.get::<_, String>(5)?)?,
        revision: read_wire_sequence(row, 6)?,
        current_step_id: row.get(7)?,
        outcome_unknown: row.get(8)?,
        cleanup_incomplete: row.get(9)?,
        created_at_unix_ms: row.get(10)?,
        updated_at_unix_ms: row.get(11)?,
    })
}

fn read_plugin_workflow_task_step(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PluginWorkflowTaskStepRecord> {
    Ok(PluginWorkflowTaskStepRecord {
        task_id: parse_id(row.get::<_, String>(0)?, PluginWorkflowTaskId::parse)?,
        step_id: row.get(1)?,
        method: workflow_method_from_db(&row.get::<_, String>(2)?)?,
        state: workflow_step_state_from_db(&row.get::<_, String>(3)?)?,
        revision: read_wire_sequence(row, 4)?,
        outcome_unknown: row.get(5)?,
        created_at_unix_ms: row.get(6)?,
        updated_at_unix_ms: row.get(7)?,
    })
}

fn validate_task_create(input: &PluginWorkflowTaskCreate) -> Result<()> {
    validate_lower_hex(&input.signer_fingerprint_sha256)?;
    validate_lower_hex(&input.package_sha256)?;
    if input.expected_installed_state_version.get() == 0 {
        return Err(AppPersistenceError::InvalidInput(
            "invalid installed plugin revision",
        ));
    }
    validate_identifier(&input.workflow_id)
}

fn validate_identifier(value: &str) -> Result<()> {
    if valid_plugin_workflow_id(value) {
        Ok(())
    } else {
        Err(AppPersistenceError::InvalidInput(
            "invalid plugin workflow identifier",
        ))
    }
}

fn validate_lower_hex(value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(AppPersistenceError::InvalidInput(
            "invalid plugin workflow digest",
        ))
    }
}

fn task_transition_allowed(
    current: PluginWorkflowTaskState,
    next: PluginWorkflowTaskState,
) -> bool {
    use PluginWorkflowTaskState as State;
    matches!(
        (current, next),
        (State::Running, State::Dispatching)
            | (State::Running, State::NeedsUserAction)
            | (State::Running, State::Cancelling)
            | (State::Running, State::Completed)
            | (State::Running, State::Failed)
            | (State::Running, State::Interrupted)
            | (State::Running, State::CleanupIncomplete)
            | (State::Dispatching, State::Running)
            | (State::Dispatching, State::NeedsUserAction)
            | (State::Dispatching, State::Cancelling)
            | (State::Dispatching, State::Failed)
            | (State::Dispatching, State::Interrupted)
            | (State::Dispatching, State::CleanupIncomplete)
            | (State::NeedsUserAction, State::Dispatching)
            | (State::NeedsUserAction, State::Cancelling)
            | (State::NeedsUserAction, State::Failed)
            | (State::NeedsUserAction, State::Interrupted)
            | (State::Cancelling, State::Cancelled)
            | (State::Cancelling, State::Failed)
            | (State::Cancelling, State::CleanupIncomplete)
            | (State::CleanupIncomplete, State::Cancelling)
    )
}

fn workflow_task_state_to_db(value: PluginWorkflowTaskState) -> &'static str {
    match value {
        PluginWorkflowTaskState::Running => "running",
        PluginWorkflowTaskState::Dispatching => "dispatching",
        PluginWorkflowTaskState::NeedsUserAction => "needs_user_action",
        PluginWorkflowTaskState::Cancelling => "cancelling",
        PluginWorkflowTaskState::Cancelled => "cancelled",
        PluginWorkflowTaskState::Completed => "completed",
        PluginWorkflowTaskState::Failed => "failed",
        PluginWorkflowTaskState::Interrupted => "interrupted",
        PluginWorkflowTaskState::CleanupIncomplete => "cleanup_incomplete",
    }
}

fn workflow_task_state_from_db(value: &str) -> rusqlite::Result<PluginWorkflowTaskState> {
    match value {
        "running" => Ok(PluginWorkflowTaskState::Running),
        "dispatching" => Ok(PluginWorkflowTaskState::Dispatching),
        "needs_user_action" => Ok(PluginWorkflowTaskState::NeedsUserAction),
        "cancelling" => Ok(PluginWorkflowTaskState::Cancelling),
        "cancelled" => Ok(PluginWorkflowTaskState::Cancelled),
        "completed" => Ok(PluginWorkflowTaskState::Completed),
        "failed" => Ok(PluginWorkflowTaskState::Failed),
        "interrupted" => Ok(PluginWorkflowTaskState::Interrupted),
        "cleanup_incomplete" => Ok(PluginWorkflowTaskState::CleanupIncomplete),
        _ => Err(invalid_stored_workflow_value()),
    }
}

fn workflow_step_state_to_db(value: PluginWorkflowStepState) -> &'static str {
    match value {
        PluginWorkflowStepState::Dispatching => "dispatching",
        PluginWorkflowStepState::Succeeded => "succeeded",
        PluginWorkflowStepState::Failed => "failed",
        PluginWorkflowStepState::NeedsUserAction => "needs_user_action",
        PluginWorkflowStepState::OutcomeUnknown => "outcome_unknown",
        PluginWorkflowStepState::Interrupted => "interrupted",
    }
}

fn workflow_step_state_from_db(value: &str) -> rusqlite::Result<PluginWorkflowStepState> {
    match value {
        "dispatching" => Ok(PluginWorkflowStepState::Dispatching),
        "succeeded" => Ok(PluginWorkflowStepState::Succeeded),
        "failed" => Ok(PluginWorkflowStepState::Failed),
        "needs_user_action" => Ok(PluginWorkflowStepState::NeedsUserAction),
        "outcome_unknown" => Ok(PluginWorkflowStepState::OutcomeUnknown),
        "interrupted" => Ok(PluginWorkflowStepState::Interrupted),
        _ => Err(invalid_stored_workflow_value()),
    }
}

fn workflow_method_to_db(value: PluginWorkflowApiMethod) -> &'static str {
    match value {
        PluginWorkflowApiMethod::SerialDevices => "serial_devices",
        PluginWorkflowApiMethod::SerialOpen => "serial_open",
        PluginWorkflowApiMethod::SerialSend => "serial_send",
        PluginWorkflowApiMethod::ProtocolOpen => "protocol_open",
        PluginWorkflowApiMethod::Describe => "describe",
        PluginWorkflowApiMethod::Permissions => "permissions",
        PluginWorkflowApiMethod::PermissionRequest => "permission_request",
        PluginWorkflowApiMethod::PermissionRevoke => "permission_revoke",
        PluginWorkflowApiMethod::PermissionsForget => "permissions_forget",
        PluginWorkflowApiMethod::ResourcesList => "resources_list",
        PluginWorkflowApiMethod::ResourceClose => "resource_close",
        PluginWorkflowApiMethod::SubscriptionStart => "subscription_start",
        PluginWorkflowApiMethod::TimerStart => "timer_start",
        PluginWorkflowApiMethod::ResourceEvents => "resource_events",
        PluginWorkflowApiMethod::NetworkStart => "network_start",
        PluginWorkflowApiMethod::NetworkSend => "network_send",
        PluginWorkflowApiMethod::RemoteExecStart => "remote_exec_start",
        PluginWorkflowApiMethod::RemoteExecSend => "remote_exec_send",
        PluginWorkflowApiMethod::ProcessStart => "process_start",
        PluginWorkflowApiMethod::ProcessSend => "process_send",
        PluginWorkflowApiMethod::SftpOpen => "sftp_open",
        PluginWorkflowApiMethod::Sftp => "sftp",
        PluginWorkflowApiMethod::FilePick => "file_pick",
        PluginWorkflowApiMethod::File => "file",
        PluginWorkflowApiMethod::Credential => "credential",
        PluginWorkflowApiMethod::Storage => "storage",
        PluginWorkflowApiMethod::TerminalRequestInput => "terminal_request_input",
    }
}

fn workflow_method_from_db(value: &str) -> rusqlite::Result<PluginWorkflowApiMethod> {
    match value {
        "serial_devices" => Ok(PluginWorkflowApiMethod::SerialDevices),
        "serial_open" => Ok(PluginWorkflowApiMethod::SerialOpen),
        "serial_send" => Ok(PluginWorkflowApiMethod::SerialSend),
        "protocol_open" => Ok(PluginWorkflowApiMethod::ProtocolOpen),
        "describe" => Ok(PluginWorkflowApiMethod::Describe),
        "permissions" => Ok(PluginWorkflowApiMethod::Permissions),
        "permission_request" => Ok(PluginWorkflowApiMethod::PermissionRequest),
        "permission_revoke" => Ok(PluginWorkflowApiMethod::PermissionRevoke),
        "permissions_forget" => Ok(PluginWorkflowApiMethod::PermissionsForget),
        "resources_list" => Ok(PluginWorkflowApiMethod::ResourcesList),
        "resource_close" => Ok(PluginWorkflowApiMethod::ResourceClose),
        "subscription_start" => Ok(PluginWorkflowApiMethod::SubscriptionStart),
        "timer_start" => Ok(PluginWorkflowApiMethod::TimerStart),
        "resource_events" => Ok(PluginWorkflowApiMethod::ResourceEvents),
        "network_start" => Ok(PluginWorkflowApiMethod::NetworkStart),
        "network_send" => Ok(PluginWorkflowApiMethod::NetworkSend),
        "remote_exec_start" => Ok(PluginWorkflowApiMethod::RemoteExecStart),
        "remote_exec_send" => Ok(PluginWorkflowApiMethod::RemoteExecSend),
        "process_start" => Ok(PluginWorkflowApiMethod::ProcessStart),
        "process_send" => Ok(PluginWorkflowApiMethod::ProcessSend),
        "sftp_open" => Ok(PluginWorkflowApiMethod::SftpOpen),
        "sftp" => Ok(PluginWorkflowApiMethod::Sftp),
        "file_pick" => Ok(PluginWorkflowApiMethod::FilePick),
        "file" => Ok(PluginWorkflowApiMethod::File),
        "credential" => Ok(PluginWorkflowApiMethod::Credential),
        "storage" => Ok(PluginWorkflowApiMethod::Storage),
        "terminal_request_input" => Ok(PluginWorkflowApiMethod::TerminalRequestInput),
        _ => Err(invalid_stored_workflow_value()),
    }
}

fn invalid_stored_workflow_value() -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(AppPersistenceError::InvalidStoredData),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginInstalledRecord;
    use norishell_core_api::PluginInstallState;

    fn installed(plugin_id: PluginId) -> PluginInstalledRecord {
        PluginInstalledRecord {
            plugin_id,
            name: "Workflow fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: "a".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "b".repeat(64),
            capabilities: Vec::new(),
            state: PluginInstallState::Enabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    fn repository() -> (tempfile::TempDir, AppRepository, PluginId) {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository =
            AppRepository::open(directory.path().join("data.sqlite3")).expect("repository");
        let plugin_id = PluginId::parse("org.norishell.workflow-fixture").expect("plugin id");
        let plugin = installed(plugin_id.clone());
        repository
            .activate_plugin_installation(None, &plugin, 1, "", "", true, None)
            .expect("install plugin");
        (directory, repository, plugin_id)
    }

    fn create(plugin_id: PluginId) -> PluginWorkflowTaskCreate {
        PluginWorkflowTaskCreate {
            task_id: PluginWorkflowTaskId::new(),
            plugin_id,
            signer_fingerprint_sha256: "a".repeat(64),
            package_sha256: "b".repeat(64),
            expected_installed_state_version: WireSequence::new(1),
            workflow_id: "backup.daily".to_owned(),
        }
    }

    #[test]
    fn persists_only_safe_lifecycle_facts_and_recovers_dispatch_without_replay() {
        let (_directory, mut repository, plugin_id) = repository();
        let input = create(plugin_id);
        let created = repository
            .create_plugin_workflow_task(&input)
            .expect("create");
        let (dispatching, step) = repository
            .begin_plugin_workflow_task_step(
                &created.task_id,
                created.revision,
                "prepare",
                PluginWorkflowApiMethod::NetworkStart,
                None,
            )
            .expect("durable dispatch marker");
        assert_eq!(dispatching.state, PluginWorkflowTaskState::Dispatching);
        assert_eq!(step.state, PluginWorkflowStepState::Dispatching);
        let recovered = repository.recover_plugin_workflow_tasks().expect("recover");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, PluginWorkflowTaskState::Interrupted);
        assert!(recovered[0].outcome_unknown);
        let steps = repository
            .list_plugin_workflow_task_steps(&created.task_id)
            .expect("steps");
        assert_eq!(steps[0].state, PluginWorkflowStepState::Interrupted);
        assert!(steps[0].outcome_unknown);
        assert!(
            repository
                .begin_plugin_workflow_task_step(
                    &created.task_id,
                    recovered[0].revision,
                    "prepare",
                    PluginWorkflowApiMethod::NetworkStart,
                    Some(step.revision),
                )
                .is_err(),
            "interrupted tasks cannot resume"
        );

        let schema: String = repository
            .connection
            .query_row(
                "SELECT group_concat(sql, ' ') FROM sqlite_master
                 WHERE name IN ('plugin_workflow_tasks', 'plugin_workflow_task_steps')",
                [],
                |row| row.get(0),
            )
            .expect("schema");
        for forbidden in [
            "input_json",
            "payload",
            "command",
            "url",
            "body",
            "token",
            "error",
        ] {
            assert!(
                !schema.to_ascii_lowercase().contains(forbidden),
                "persisted {forbidden}"
            );
        }
    }

    #[test]
    fn needs_user_action_can_only_reenter_the_same_pending_step_once_per_dispatch() {
        let (_directory, mut repository, plugin_id) = repository();
        let created = repository
            .create_plugin_workflow_task(&create(plugin_id))
            .unwrap();
        let (dispatching, step) = repository
            .begin_plugin_workflow_task_step(
                &created.task_id,
                created.revision,
                "publish",
                PluginWorkflowApiMethod::NetworkStart,
                None,
            )
            .unwrap();
        let (needs_action, pending) = repository
            .settle_plugin_workflow_task_step(
                &created.task_id,
                dispatching.revision,
                "publish",
                step.revision,
                PluginWorkflowStepState::NeedsUserAction,
                PluginWorkflowTaskState::NeedsUserAction,
                false,
            )
            .unwrap();
        let (resumed, _) = repository
            .begin_plugin_workflow_task_step(
                &created.task_id,
                needs_action.revision,
                "publish",
                PluginWorkflowApiMethod::NetworkStart,
                Some(pending.revision),
            )
            .unwrap();
        assert_eq!(resumed.state, PluginWorkflowTaskState::Dispatching);
        assert!(
            repository
                .begin_plugin_workflow_task_step(
                    &created.task_id,
                    resumed.revision,
                    "different",
                    PluginWorkflowApiMethod::NetworkStart,
                    None,
                )
                .is_err()
        );
    }
}
