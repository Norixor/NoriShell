//! Bounded local command history, stored through the Vault when enabled.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use norishell_core_api::{
    CoreApiError, ErrorCategory, NativeTerminalHistoryClearRequest,
    NativeTerminalHistoryDeleteRequest, NativeTerminalHistoryEntry, NativeTerminalHistoryEntryId,
    NativeTerminalHistoryListRequest, NativeTerminalHistoryPauseRequest,
    NativeTerminalHistoryRecordRequest, NativeTerminalHistoryScope, NativeTerminalSettings,
    NativeTerminalSettingsGetRequest, NativeTerminalSettingsReplaceRequest,
    NativeTerminalSettingsSnapshot, RequestId, RetryStrategy, WireSequence,
};
use norishell_secret_vault::{
    CommandHistoryEntry, CommandHistoryPayload, MAX_COMMAND_HISTORY_TOTAL_BYTES,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{
    core_api_error::core_error,
    ssh_session_service::SshSessionService,
    time::unix_time_ms,
    vault_service::{VaultAvailability, VaultService},
};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

const SETTINGS_FILE_NAME: &str = "native-terminal-settings.json";
const COMMAND_MAX_BYTES: usize = 16 * 1024;
const HISTORY_QUERY_MAX: u16 = 2_000;
/// A user can submit commands much faster than an encrypted Vault write.
/// Keep at most one wake-up and one newest history snapshot waiting behind the
/// in-flight write; retaining one payload is sufficient because every write is
/// a complete bounded projection.
const HISTORY_PERSISTENCE_WAKE_CAPACITY: usize = 1;

#[derive(Clone)]
pub struct NativeTerminalService {
    inner: Arc<NativeTerminalInner>,
}

/// All material retained by the terminal runtime. Keeping it behind one Arc
/// lets the Vault observer hold only a Weak reference: the observer must never
/// retain encrypted history or the Vault's unlocked VMK after the desktop
/// services have been dropped.
struct NativeTerminalInner {
    state: Arc<Mutex<NativeTerminalState>>,
    settings_path: PathBuf,
    vault: VaultService,
    persistence: HistoryPersistenceQueue,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedSettings {
    settings: NativeTerminalSettings,
    settings_revision: u64,
}

struct NativeTerminalState {
    settings: NativeTerminalSettings,
    settings_revision: u64,
    history: Vec<NativeTerminalHistoryEntry>,
    /// Every mutation of the in-memory history projection has an increasing
    /// revision. Persistence tasks carry it so an older callback can
    /// never put a stale full payload behind a later clear/delete.
    history_epoch: u64,
    /// This is Core's synchronized view of encrypted-history accessibility.
    /// History operations synchronize Vault availability before taking this lock.
    vault_available: bool,
    vault_epoch: u64,
    persistence_failed: bool,
    /// Delete/clear remains in this state until its encrypted replacement is
    /// known durable. While set, capture is fail-closed and another destructive
    /// edit cannot acknowledge a snapshot that might later roll back.
    history_destructive_pending: bool,
}

#[derive(Clone)]
struct HistoryPersistenceQueue {
    pending: Arc<Mutex<HistoryPersistenceQueueState>>,
    wake_tx: mpsc::Sender<()>,
}

struct HistoryPersistenceQueueState {
    /// A complete snapshot replaces any older snapshot that has not started
    /// writing. This bounds both queued work and queued command strings.
    latest: Option<PendingHistoryPersistence>,
    highest_revision: u64,
    generation: u64,
}

struct PendingHistoryPersistence {
    revision: u64,
    vault_epoch: u64,
    generation: u64,
    history: CommandHistoryPayload,
    /// Destructive history edits wait for the merged write. One waiter is
    /// enough: additional edits receive a bounded backpressure failure and
    /// restore their optimistic local state instead of extending this queue.
    confirmation: Option<oneshot::Sender<bool>>,
}

struct PendingDestructiveHistoryWrite {
    old_history: Vec<NativeTerminalHistoryEntry>,
    epoch: u64,
    response: oneshot::Receiver<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryPersistenceEnqueueResult {
    Queued,
    Stale,
    Backpressured,
    Closed,
}

impl HistoryPersistenceQueue {
    fn start(state: Arc<Mutex<NativeTerminalState>>, vault: VaultService) -> Self {
        let pending = Arc::new(Mutex::new(HistoryPersistenceQueueState {
            latest: None,
            highest_revision: 0,
            generation: 0,
        }));
        let (wake_tx, mut wake_rx) = mpsc::channel(HISTORY_PERSISTENCE_WAKE_CAPACITY);
        let worker_pending = Arc::clone(&pending);
        tauri::async_runtime::spawn(async move {
            while wake_rx.recv().await.is_some() {
                loop {
                    let Some(task) = worker_pending
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .latest
                        .take()
                    else {
                        break;
                    };
                    let PendingHistoryPersistence {
                        revision,
                        vault_epoch,
                        generation,
                        history,
                        confirmation,
                    } = task;
                    let result = tokio::task::spawn_blocking({
                        let vault = vault.clone();
                        move || vault.replace_command_history_at_epoch(history, vault_epoch)
                    })
                    .await
                    .is_ok_and(|result| result.is_ok());
                    let generation_current = worker_pending
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .generation
                        == generation;
                    if generation_current {
                        let mut state = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        // A result for an older snapshot does not describe the
                        // latest history. The later revision remains queued and
                        // will publish its own persistence result.
                        if state.history_epoch == revision
                            && state.vault_epoch == vault_epoch
                            && state.persistence_failed != !result
                        {
                            state.persistence_failed = !result;
                        }
                    }
                    if let Some(confirmation) = confirmation {
                        let _ = confirmation.send(result && generation_current);
                    }
                }
            }
        });
        Self { pending, wake_tx }
    }

    /// Replace—not append—the pending full snapshot. A full history is
    /// bounded by settings, so this keeps the write queue bounded even if a
    /// commands arrive while the Vault is slow.
    fn enqueue(
        &self,
        revision: u64,
        vault_epoch: u64,
        history: CommandHistoryPayload,
        confirmation: Option<oneshot::Sender<bool>>,
    ) -> HistoryPersistenceEnqueueResult {
        if self.wake_tx.is_closed() {
            if let Some(confirmation) = confirmation {
                let _ = confirmation.send(false);
            }
            return HistoryPersistenceEnqueueResult::Closed;
        }
        let mut rejected_confirmation = None;
        let mut stale_confirmation = None;
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let generation = pending.generation;
            if revision < pending.highest_revision {
                stale_confirmation = confirmation;
            } else if confirmation.is_some()
                && pending
                    .latest
                    .as_ref()
                    .is_some_and(|latest| latest.confirmation.is_some())
            {
                // Do not replace a pending destructive snapshot if we cannot
                // also bind this request's acknowledgement to it. The caller
                // will receive `false` and restore its optimistic projection;
                // preserving the existing task prevents that rolled-back
                // change from being written later.
                rejected_confirmation = confirmation;
            } else {
                pending.highest_revision = revision;
                if let Some(latest) = pending.latest.as_mut() {
                    latest.revision = revision;
                    latest.vault_epoch = vault_epoch;
                    latest.generation = generation;
                    latest.history = history;
                    if latest.confirmation.is_none() {
                        latest.confirmation = confirmation;
                    } else {
                        rejected_confirmation = confirmation;
                    }
                } else {
                    pending.latest = Some(PendingHistoryPersistence {
                        revision,
                        vault_epoch,
                        generation,
                        history,
                        confirmation,
                    });
                }
            }
        }
        if let Some(confirmation) = stale_confirmation {
            let _ = confirmation.send(false);
            return HistoryPersistenceEnqueueResult::Stale;
        }
        if let Some(confirmation) = rejected_confirmation {
            let _ = confirmation.send(false);
            return HistoryPersistenceEnqueueResult::Backpressured;
        }
        match self.wake_tx.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {
                HistoryPersistenceEnqueueResult::Queued
            }
            Err(mpsc::error::TrySendError::Closed(())) => {
                let confirmation = self
                    .pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .latest
                    .take()
                    .and_then(|task| task.confirmation);
                if let Some(confirmation) = confirmation {
                    let _ = confirmation.send(false);
                }
                HistoryPersistenceEnqueueResult::Closed
            }
        }
    }

    /// Vault lock revokes pending encrypted writes. An in-flight task cannot
    /// be cancelled safely, but its generation no longer reports success or
    /// changes the current persistence state when it returns.
    fn invalidate(&self) {
        let confirmation = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pending.generation = pending.generation.saturating_add(1);
            pending.highest_revision = 0;
            pending.latest.take().and_then(|task| task.confirmation)
        };
        if let Some(confirmation) = confirmation {
            let _ = confirmation.send(false);
        }
    }
}

impl NativeTerminalService {
    /// Compatibility construction for narrow test helpers that instantiate a
    /// terminal service without a desktop application directory. Production
    /// always uses `start` with the real app-data directory.
    #[cfg(test)]
    pub(crate) fn detached(vault: VaultService) -> Self {
        Self::start(
            std::env::temp_dir().join(format!("norishell-native-terminal-{}", Uuid::now_v7())),
            vault,
        )
    }

    pub fn start(app_data_directory: impl AsRef<Path>, vault: VaultService) -> Self {
        let settings_path = app_data_directory.as_ref().join(SETTINGS_FILE_NAME);
        let persisted = load_settings(&settings_path).unwrap_or_else(|| PersistedSettings {
            settings: NativeTerminalSettings::default(),
            settings_revision: 1,
        });
        let state = Arc::new(Mutex::new(NativeTerminalState {
            settings: persisted.settings,
            settings_revision: persisted.settings_revision.max(1),
            history: Vec::new(),
            history_epoch: 0,
            vault_available: false,
            vault_epoch: 0,
            persistence_failed: false,
            history_destructive_pending: false,
        }));
        let persistence = HistoryPersistenceQueue::start(Arc::clone(&state), vault.clone());
        let service = Self {
            inner: Arc::new(NativeTerminalInner {
                state,
                settings_path,
                vault,
                persistence,
            }),
        };
        service.sync_vault_availability();
        service
    }

    /// Creates a non-owning Vault availability callback. `VaultService` owns
    /// the callback, while the terminal runtime owns the Vault, so capturing a
    /// service clone here would form an Arc cycle and keep both history and
    /// unlocked Vault material alive after app shutdown.
    pub(crate) fn availability_observer(&self) -> Arc<dyn Fn(VaultAvailability) + Send + Sync> {
        let terminal = Arc::downgrade(&self.inner);
        Arc::new(move |availability| {
            let Some(inner) = terminal.upgrade() else {
                return;
            };
            NativeTerminalService { inner }.vault_availability_changed(availability);
        })
    }

    #[cfg(test)]
    fn replace_persistence_for_tests(&mut self, persistence: HistoryPersistenceQueue) {
        Arc::get_mut(&mut self.inner)
            .expect("test terminal service must not be cloned before replacing its worker")
            .persistence = persistence;
    }

    pub(crate) fn settings_snapshot(&self) -> NativeTerminalSettingsSnapshot {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        project_settings_snapshot(&state)
    }

    pub(crate) fn replace_settings(
        &self,
        request: NativeTerminalSettingsReplaceRequest,
    ) -> CoreResult<NativeTerminalSettingsSnapshot> {
        self.inner
            .vault
            .with_vault_availability(|availability, vault| {
                self.apply_vault_availability_under_guard(availability, vault);
                self.replace_settings_under_vault_guard(request)
            })
    }

    fn replace_settings_under_vault_guard(
        &self,
        request: NativeTerminalSettingsReplaceRequest,
    ) -> CoreResult<NativeTerminalSettingsSnapshot> {
        validate_settings(&request.settings)
            .map_err(|code| native_validation_error(request.meta.request_id.clone(), code))?;
        let mut invalidate_persistence = false;
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if request.expected_settings_revision.get() != state.settings_revision {
                return Err(core_error(
                    request.meta.request_id,
                    "native_terminal.settings_conflict",
                    ErrorCategory::Conflict,
                    RetryStrategy::RefreshSnapshot,
                    "errors.nativeTerminal.settingsConflict",
                ));
            }
            if state.history_destructive_pending {
                return Err(native_unavailable_error(
                    request.meta.request_id,
                    "native_terminal.history_mutation_in_progress",
                ));
            }
            let previous = state.settings.clone();
            if !previous.persist_encrypted
                && request.settings.persist_encrypted
                && !state.vault_available
            {
                return Err(native_unavailable_error(
                    request.meta.request_id,
                    "native_terminal.history_vault_locked",
                ));
            }
            let next_revision = state.settings_revision.saturating_add(1);
            write_settings(
                &self.inner.settings_path,
                &PersistedSettings {
                    settings: request.settings.clone(),
                    settings_revision: next_revision,
                },
            )
            .map_err(|_| {
                native_unavailable_error(
                    request.meta.request_id.clone(),
                    "native_terminal.settings_write_failed",
                )
            })?;
            state.settings = request.settings;
            state.settings_revision = next_revision;
            let settings = state.settings.clone();
            let persist_became_enabled = !previous.persist_encrypted && settings.persist_encrypted;
            if prune_history(&mut state.history, &settings) || persist_became_enabled {
                state.history_epoch = state.history_epoch.saturating_add(1);
                if encrypted_persistence_available(&state) {
                    let _ = self.inner.persistence.enqueue(
                        state.history_epoch,
                        state.vault_epoch,
                        history_payload(&state.history),
                        None,
                    );
                }
            }
            if previous.persist_encrypted && !state.settings.persist_encrypted {
                invalidate_persistence = true;
                state.persistence_failed = false;
            }
        }
        if invalidate_persistence {
            self.inner.persistence.invalidate();
        }
        Ok(self.settings_snapshot())
    }

    pub(crate) fn history_list(
        &self,
        request: NativeTerminalHistoryListRequest,
    ) -> CoreResult<Vec<NativeTerminalHistoryEntry>> {
        if request.limit == 0 || request.limit > HISTORY_QUERY_MAX {
            return Err(native_validation_error(
                request.meta.request_id,
                "native_terminal.history_limit_invalid",
            ));
        }
        if request.query.len() > 256 || request.query.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(native_validation_error(
                request.meta.request_id,
                "native_terminal.history_query_invalid",
            ));
        }
        let query = request.query.trim().to_lowercase();
        self.inner
            .vault
            .with_vault_availability(|availability, vault| {
                self.apply_vault_availability_under_guard(availability, vault);
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !history_visible_for_state(&state) {
                    return Ok(Vec::new());
                }
                Ok(state
                    .history
                    .iter()
                    .rev()
                    .filter(|entry| {
                        request
                            .scope
                            .as_ref()
                            .is_none_or(|scope| &entry.scope == scope)
                    })
                    .filter(|entry| {
                        query.is_empty() || entry.command.to_lowercase().contains(&query)
                    })
                    .take(usize::from(request.limit))
                    .cloned()
                    .collect())
            })
    }

    pub(crate) fn record_history(
        &self,
        scope: NativeTerminalHistoryScope,
        command: String,
    ) -> CoreResult<bool> {
        if !should_store_history(&command) {
            return Ok(false);
        }
        self.inner
            .vault
            .with_vault_availability(|availability, vault| {
                self.apply_vault_availability_under_guard(availability, vault);
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !history_capture_allowed_for_state(&state) || state.persistence_failed {
                    return Ok(false);
                }
                let previous = state.history.clone();
                state.history.push(NativeTerminalHistoryEntry {
                    entry_id: NativeTerminalHistoryEntryId::new(),
                    scope,
                    command,
                    completed_at_unix_ms: unix_time_ms(),
                    elapsed_millis: 0,
                    exit_code: None,
                });
                let settings = state.settings.clone();
                prune_history(&mut state.history, &settings);
                state.history_epoch = state.history_epoch.saturating_add(1);
                if encrypted_persistence_available(&state)
                    && !matches!(
                        self.inner.persistence.enqueue(
                            state.history_epoch,
                            state.vault_epoch,
                            history_payload(&state.history),
                            None
                        ),
                        HistoryPersistenceEnqueueResult::Queued
                    )
                {
                    state.history = previous;
                    state.history_epoch = state.history_epoch.saturating_add(1);
                    state.persistence_failed = true;
                    return Ok(false);
                }
                Ok(true)
            })
    }

    /// Begins an optimistic delete/clear while the caller holds `state` in the
    /// Vault -> Native lock order. Encrypted mutations remain gated until the
    /// exact replacement is durable. If queue admission fails, rollback occurs
    /// before releasing `state`, so output cannot persist an unacknowledged
    /// deletion in a newer snapshot.
    fn begin_destructive_history_write_locked(
        &self,
        state: &mut NativeTerminalState,
        old_history: Vec<NativeTerminalHistoryEntry>,
    ) -> Result<Option<PendingDestructiveHistoryWrite>, HistoryPersistenceEnqueueResult> {
        state.history_epoch = state.history_epoch.saturating_add(1);
        state.history_destructive_pending = true;
        if !encrypted_persistence_available(state) {
            state.history_destructive_pending = false;
            return Ok(None);
        }

        let (confirmation, response) = oneshot::channel();
        let epoch = state.history_epoch;
        match self.inner.persistence.enqueue(
            epoch,
            state.vault_epoch,
            history_payload(&state.history),
            Some(confirmation),
        ) {
            HistoryPersistenceEnqueueResult::Queued => Ok(Some(PendingDestructiveHistoryWrite {
                old_history,
                epoch,
                response,
            })),
            outcome => {
                rollback_destructive_history_locked(state, old_history);
                if matches!(outcome, HistoryPersistenceEnqueueResult::Closed) {
                    state.persistence_failed = true;
                }
                Err(outcome)
            }
        }
    }

    async fn finish_destructive_history_write(
        &self,
        pending: PendingDestructiveHistoryWrite,
    ) -> bool {
        let persisted = pending.response.await.unwrap_or(false);
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.history_destructive_pending = false;
        if !persisted && state.history_epoch == pending.epoch {
            // The capture gate prevents another output mutation while this
            // write is pending. A changed epoch therefore means Vault lock or
            // poison already revoked the cache, which must remain empty.
            rollback_destructive_history_locked(&mut state, pending.old_history);
        }
        persisted
    }

    pub(crate) async fn history_delete(
        &self,
        request: NativeTerminalHistoryDeleteRequest,
    ) -> CoreResult<()> {
        let request_id = request.meta.request_id;
        let entry_id = request.entry_id;
        let pending = self
            .inner
            .vault
            .with_vault_availability(|availability, vault| {
                self.apply_vault_availability_under_guard(availability, vault);
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.history_destructive_pending {
                    return Err(native_unavailable_error(
                        request_id.clone(),
                        "native_terminal.history_mutation_in_progress",
                    ));
                }
                if encrypted_history_locked(&state) {
                    return Err(native_unavailable_error(
                        request_id.clone(),
                        "native_terminal.history_vault_locked",
                    ));
                }
                let Some(index) = state
                    .history
                    .iter()
                    .position(|entry| entry.entry_id == entry_id)
                else {
                    return Err(core_error(
                        request_id.clone(),
                        "native_terminal.history_not_found",
                        ErrorCategory::Validation,
                        RetryStrategy::RefreshSnapshot,
                        "errors.nativeTerminal.historyNotFound",
                    ));
                };
                let old_history = state.history.clone();
                state.history.remove(index);
                self.begin_destructive_history_write_locked(&mut state, old_history)
                    .map_err(|_| {
                        native_unavailable_error(
                            request_id.clone(),
                            "native_terminal.history_persist_unavailable",
                        )
                    })
            })?;
        if let Some(pending) = pending
            && !self.finish_destructive_history_write(pending).await
        {
            return Err(native_unavailable_error(
                request_id,
                "native_terminal.history_persist_failed",
            ));
        }
        Ok(())
    }

    pub(crate) async fn history_clear(
        &self,
        request: NativeTerminalHistoryClearRequest,
    ) -> CoreResult<()> {
        let request_id = request.meta.request_id;
        let scope = request.scope;
        let pending = self
            .inner
            .vault
            .with_vault_availability(|availability, vault| {
                self.apply_vault_availability_under_guard(availability, vault);
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.history_destructive_pending {
                    return Err(native_unavailable_error(
                        request_id.clone(),
                        "native_terminal.history_mutation_in_progress",
                    ));
                }
                if encrypted_history_locked(&state) {
                    return Err(native_unavailable_error(
                        request_id.clone(),
                        "native_terminal.history_vault_locked",
                    ));
                }
                let old_history = state.history.clone();
                state
                    .history
                    .retain(|entry| scope.as_ref().is_some_and(|scope| &entry.scope != scope));
                if old_history.len() == state.history.len() {
                    return Ok(None);
                }
                self.begin_destructive_history_write_locked(&mut state, old_history)
                    .map_err(|_| {
                        native_unavailable_error(
                            request_id.clone(),
                            "native_terminal.history_persist_unavailable",
                        )
                    })
            })?;
        if let Some(pending) = pending
            && !self.finish_destructive_history_write(pending).await
        {
            return Err(native_unavailable_error(
                request_id,
                "native_terminal.history_persist_failed",
            ));
        }
        Ok(())
    }

    pub(crate) fn history_pause(
        &self,
        request: NativeTerminalHistoryPauseRequest,
    ) -> CoreResult<NativeTerminalSettingsSnapshot> {
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.settings.history_paused != request.paused {
                let previous = state.settings.clone();
                state.settings.history_paused = request.paused;
                let next_revision = state.settings_revision.saturating_add(1);
                if write_settings(
                    &self.inner.settings_path,
                    &PersistedSettings {
                        settings: state.settings.clone(),
                        settings_revision: next_revision,
                    },
                )
                .is_err()
                {
                    state.settings = previous;
                    return Err(native_unavailable_error(
                        request.meta.request_id,
                        "native_terminal.settings_write_failed",
                    ));
                }
                state.settings_revision = next_revision;
            }
        }
        Ok(self.settings_snapshot())
    }

    /// Synchronize dependent history state while `VaultService` holds its
    /// actual availability lock. This is the only path that makes an
    /// encrypted-history cache readable; a callback's cached bool is never a
    /// permission decision on its own.
    fn sync_vault_availability(&self) {
        self.inner
            .vault
            .with_vault_availability(|availability, vault| {
                self.apply_vault_availability_under_guard(availability, vault);
            });
    }

    /// Called by the service-level Vault observer. Recheck through the Vault
    /// gate rather than trusting a delayed callback: a later lock/poison event
    /// may already have advanced the availability epoch.
    pub(crate) fn vault_availability_changed(&self, _observed: VaultAvailability) {
        self.sync_vault_availability();
    }

    fn apply_vault_availability_under_guard(
        &self,
        availability: VaultAvailability,
        vault: Option<&norishell_secret_vault::UnlockedVault>,
    ) {
        let invalidate_persistence = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if availability.epoch < state.vault_epoch {
                return;
            }
            if availability.available {
                if state.vault_epoch == availability.epoch && state.vault_available {
                    return;
                }
                state.vault_epoch = availability.epoch;
                state.vault_available = true;
                if state.settings.persist_encrypted {
                    let history = vault
                        .expect("available Vault supplies history")
                        .command_history()
                        .expect("usable Vault must expose command history");
                    replace_history_from_vault(&mut state, history);
                }
                state.persistence_failed = false;
                false
            } else {
                if state.vault_epoch == availability.epoch && !state.vault_available {
                    return;
                }
                state.vault_epoch = availability.epoch;
                state.vault_available = false;
                if state.settings.persist_encrypted {
                    clear_encrypted_history_cache(&mut state);
                    state.persistence_failed = false;
                    true
                } else {
                    false
                }
            }
        };
        if invalidate_persistence {
            self.inner.persistence.invalidate();
        }
    }
}

#[tauri::command]
pub fn native_terminal_settings_get(
    request: NativeTerminalSettingsGetRequest,
    service: State<'_, NativeTerminalService>,
) -> CoreResult<NativeTerminalSettingsSnapshot> {
    let _request_id = request.meta.request_id;
    Ok(service.settings_snapshot())
}

#[tauri::command]
pub fn native_terminal_settings_replace(
    request: NativeTerminalSettingsReplaceRequest,
    service: State<'_, NativeTerminalService>,
) -> CoreResult<NativeTerminalSettingsSnapshot> {
    service.replace_settings(request)
}

#[tauri::command]
pub async fn native_terminal_history_record(
    request: NativeTerminalHistoryRecordRequest,
    sessions: State<'_, SshSessionService>,
) -> CoreResult<bool> {
    sessions.native_terminal_history_record(request).await
}

#[tauri::command]
pub fn native_terminal_history_list(
    request: NativeTerminalHistoryListRequest,
    service: State<'_, NativeTerminalService>,
) -> CoreResult<Vec<NativeTerminalHistoryEntry>> {
    service.history_list(request)
}

#[tauri::command]
pub async fn native_terminal_history_delete(
    request: NativeTerminalHistoryDeleteRequest,
    service: State<'_, NativeTerminalService>,
) -> CoreResult<()> {
    service.history_delete(request).await
}

#[tauri::command]
pub async fn native_terminal_history_clear(
    request: NativeTerminalHistoryClearRequest,
    service: State<'_, NativeTerminalService>,
) -> CoreResult<()> {
    service.history_clear(request).await
}

#[tauri::command]
pub fn native_terminal_history_pause(
    request: NativeTerminalHistoryPauseRequest,
    service: State<'_, NativeTerminalService>,
) -> CoreResult<NativeTerminalSettingsSnapshot> {
    service.history_pause(request)
}

fn project_settings_snapshot(state: &NativeTerminalState) -> NativeTerminalSettingsSnapshot {
    NativeTerminalSettingsSnapshot {
        settings: state.settings.clone(),
        settings_revision: WireSequence::new(state.settings_revision),
        history_available: history_readable_for_state(state),
        history_persistence_failed: state.persistence_failed,
    }
}

/// Whether history data is currently safe for Core to read. This deliberately
/// does not include the user capture/pause preference: pausing hides history
/// from terminal UI but preserves it so the user can resume without loss.
fn history_readable_for_state(state: &NativeTerminalState) -> bool {
    !state.settings.persist_encrypted || state.vault_available
}

fn history_capture_allowed_for_state(state: &NativeTerminalState) -> bool {
    state.settings.history_enabled
        && !state.settings.history_paused
        && !state.history_destructive_pending
        && history_readable_for_state(state)
}

fn history_visible_for_state(state: &NativeTerminalState) -> bool {
    state.settings.history_enabled
        && !state.settings.history_paused
        && history_readable_for_state(state)
}

fn encrypted_persistence_available(state: &NativeTerminalState) -> bool {
    state.settings.persist_encrypted && state.vault_available
}

fn encrypted_history_locked(state: &NativeTerminalState) -> bool {
    state.settings.persist_encrypted && !state.vault_available
}

/// Restores an optimistic destructive edit before another history capture can
/// observe the projection. Callers already hold `NativeTerminalState`.
fn rollback_destructive_history_locked(
    state: &mut NativeTerminalState,
    history: Vec<NativeTerminalHistoryEntry>,
) {
    state.history = history;
    state.history_epoch = state.history_epoch.saturating_add(1);
    state.history_destructive_pending = false;
}

/// Vault lock is deliberately narrower than terminal teardown: it only
/// removes the Core-readable encrypted-history cache and advances the
/// projection. Existing SSH/Local sessions remain untouched.
fn clear_encrypted_history_cache(state: &mut NativeTerminalState) -> bool {
    if !state.settings.persist_encrypted || state.history.is_empty() {
        return false;
    }
    state.history.clear();
    state.history_epoch = state.history_epoch.saturating_add(1);
    true
}

fn replace_history_from_vault(state: &mut NativeTerminalState, history: CommandHistoryPayload) {
    let mut history = history;
    state.history = std::mem::take(&mut history.entries)
        .into_iter()
        .filter_map(history_entry_from_vault)
        .collect();
    let settings = state.settings.clone();
    prune_history(&mut state.history, &settings);
    state.history_epoch = state.history_epoch.saturating_add(1);
}

fn validate_settings(settings: &NativeTerminalSettings) -> Result<(), &'static str> {
    if !(100..=2_000).contains(&settings.history_max_entries) {
        return Err("native_terminal.history_max_entries_invalid");
    }
    if !(1..=90).contains(&settings.history_retention_days) {
        return Err("native_terminal.history_retention_days_invalid");
    }
    if !(1..=3_600).contains(&settings.notification_threshold_seconds) {
        return Err("native_terminal.notification_threshold_invalid");
    }
    Ok(())
}

fn should_store_history(command: &str) -> bool {
    if command.is_empty()
        || command.chars().count() < 10
        || command.len() > COMMAND_MAX_BYTES
        || command.bytes().any(|byte| byte.is_ascii_control())
        // A leading blank is a familiar shell convention for keeping a
        // sensitive one-off command out of history. Honor it before the
        // bounded keyword filter, including non-ASCII whitespace.
        || command.chars().next().is_some_and(char::is_whitespace)
    {
        return false;
    }
    let lower = command.to_ascii_lowercase();
    ![
        "password",
        "passwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "authorization",
        "sshpass",
        "curl -u ",
        "curl --user ",
        "begin private key",
        "aws_secret_access_key",
        "ghp_",
        "github_pat_",
        "sk-",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn prune_history(
    history: &mut Vec<NativeTerminalHistoryEntry>,
    settings: &NativeTerminalSettings,
) -> bool {
    let original_len = history.len();
    let earliest = unix_time_ms().saturating_sub(
        i64::from(settings.history_retention_days).saturating_mul(24 * 60 * 60 * 1_000),
    );
    history.retain(|entry| entry.completed_at_unix_ms >= earliest);
    history.sort_by_key(|entry| {
        (
            entry.completed_at_unix_ms,
            entry.entry_id.as_str().to_owned(),
        )
    });
    let maximum = usize::from(settings.history_max_entries);
    if history.len() > maximum {
        history.drain(..history.len().saturating_sub(maximum));
    }
    let mut total_bytes = history
        .iter()
        .map(history_entry_storage_bytes)
        .sum::<usize>();
    let mut first_kept = 0;
    while total_bytes > MAX_COMMAND_HISTORY_TOTAL_BYTES && first_kept < history.len() {
        total_bytes = total_bytes.saturating_sub(history_entry_storage_bytes(&history[first_kept]));
        first_kept = first_kept.saturating_add(1);
    }
    if first_kept > 0 {
        history.drain(..first_kept);
    }
    original_len != history.len()
}

fn history_entry_storage_bytes(entry: &NativeTerminalHistoryEntry) -> usize {
    let scope_bytes = match &entry.scope {
        NativeTerminalHistoryScope::Host { host_id } => 5 + host_id.as_str().len(),
        NativeTerminalHistoryScope::Local => "local".len(),
    };
    scope_bytes.saturating_add(entry.command.len())
}

fn history_payload(history: &[NativeTerminalHistoryEntry]) -> CommandHistoryPayload {
    CommandHistoryPayload {
        schema_version: norishell_secret_vault::COMMAND_HISTORY_SCHEMA_VERSION,
        entries: history
            .iter()
            .filter_map(|entry| {
                Some(CommandHistoryEntry {
                    entry_id: Uuid::parse_str(entry.entry_id.as_str()).ok()?,
                    scope: history_scope_to_storage(&entry.scope),
                    command: entry.command.clone(),
                    completed_at_unix_ms: entry.completed_at_unix_ms,
                    elapsed_millis: entry.elapsed_millis,
                    exit_code: entry.exit_code,
                })
            })
            .collect(),
    }
}

fn history_entry_from_vault(mut entry: CommandHistoryEntry) -> Option<NativeTerminalHistoryEntry> {
    let entry_id = NativeTerminalHistoryEntryId::parse(entry.entry_id.to_string()).ok()?;
    let scope = history_scope_from_storage(&entry.scope)?;
    // CommandHistoryEntry zeroizes on Drop, so move the command out only by
    // replacing the field. This preserves the no-extra-copy Vault handoff.
    let command = std::mem::take(&mut entry.command);
    Some(NativeTerminalHistoryEntry {
        entry_id,
        scope,
        command,
        completed_at_unix_ms: entry.completed_at_unix_ms,
        elapsed_millis: entry.elapsed_millis,
        exit_code: entry.exit_code,
    })
}

fn history_scope_to_storage(scope: &NativeTerminalHistoryScope) -> String {
    match scope {
        NativeTerminalHistoryScope::Host { host_id } => format!("host:{}", host_id.as_str()),
        NativeTerminalHistoryScope::Local => "local".to_owned(),
    }
}

fn history_scope_from_storage(value: &str) -> Option<NativeTerminalHistoryScope> {
    if value == "local" {
        return Some(NativeTerminalHistoryScope::Local);
    }
    let host_id = value.strip_prefix("host:")?;
    Some(NativeTerminalHistoryScope::Host {
        host_id: norishell_core_api::HostId::parse(host_id).ok()?,
    })
}

fn load_settings(path: &Path) -> Option<PersistedSettings> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() > 16 * 1024 {
        return None;
    }
    let persisted: PersistedSettings = serde_json::from_slice(&bytes).ok()?;
    validate_settings(&persisted.settings).ok()?;
    Some(persisted)
}

fn write_settings(path: &Path, settings: &PersistedSettings) -> std::io::Result<()> {
    let encoded = serde_json::to_vec(settings).map_err(std::io::Error::other)?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("native terminal settings path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".native-terminal-settings-{}.tmp", Uuid::now_v7()));
    fs::write(&temporary, encoded)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)
}

fn native_validation_error(request_id: RequestId, code: &str) -> Box<CoreApiError> {
    core_error(
        request_id,
        code,
        ErrorCategory::Validation,
        RetryStrategy::Never,
        "errors.nativeTerminal.validation",
    )
}

fn native_unavailable_error(request_id: RequestId, code: &str) -> Box<CoreApiError> {
    core_error(
        request_id,
        code,
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.nativeTerminal.unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn history_payload_with_command(command: &str) -> CommandHistoryPayload {
        CommandHistoryPayload {
            schema_version: norishell_secret_vault::COMMAND_HISTORY_SCHEMA_VERSION,
            entries: vec![CommandHistoryEntry {
                entry_id: Uuid::now_v7(),
                scope: "local".to_owned(),
                command: command.to_owned(),
                completed_at_unix_ms: 1,
                elapsed_millis: 1,
                exit_code: Some(0),
            }],
        }
    }

    fn history_entry(command: &str) -> NativeTerminalHistoryEntry {
        NativeTerminalHistoryEntry {
            entry_id: NativeTerminalHistoryEntryId::new(),
            scope: NativeTerminalHistoryScope::Local,
            command: command.to_owned(),
            completed_at_unix_ms: 1,
            elapsed_millis: 1,
            exit_code: Some(0),
        }
    }

    fn state_with_encrypted_history() -> NativeTerminalState {
        NativeTerminalState {
            settings: NativeTerminalSettings {
                persist_encrypted: true,
                ..NativeTerminalSettings::default()
            },
            settings_revision: 1,
            history: vec![history_entry("plain command")],
            history_epoch: 0,
            vault_available: true,
            vault_epoch: 0,
            persistence_failed: false,
            history_destructive_pending: false,
        }
    }

    #[test]
    fn vault_lock_clears_only_the_encrypted_history_projection() {
        let mut state = state_with_encrypted_history();
        assert!(clear_encrypted_history_cache(&mut state));
        assert!(state.history.is_empty());
        assert_eq!(state.history_epoch, 1);
        assert!(!clear_encrypted_history_cache(&mut state));
        assert_eq!(state.history_epoch, 1);
    }

    #[test]
    fn enabling_encrypted_persistence_requires_an_unlocked_vault() {
        let directory = tempfile::tempdir().expect("temporary native-terminal directory");
        let vault = VaultService::start(directory.path().join("vault"));
        let service = NativeTerminalService::start(directory.path().join("settings"), vault);
        let disabled = service
            .replace_settings(NativeTerminalSettingsReplaceRequest {
                meta: norishell_core_api::RequestMeta {
                    request_id: RequestId::new(),
                },
                expected_settings_revision: WireSequence::new(1),
                settings: NativeTerminalSettings {
                    persist_encrypted: false,
                    ..NativeTerminalSettings::default()
                },
            })
            .expect("turn off default encrypted persistence");
        let result = service.replace_settings(NativeTerminalSettingsReplaceRequest {
            meta: norishell_core_api::RequestMeta {
                request_id: RequestId::new(),
            },
            expected_settings_revision: disabled.settings_revision,
            settings: NativeTerminalSettings {
                history_enabled: true,
                persist_encrypted: true,
                ..NativeTerminalSettings::default()
            },
        });

        assert_eq!(
            result
                .expect_err("locked Vault rejects encrypted history")
                .code,
            "native_terminal.history_vault_locked"
        );
    }

    #[test]
    fn vault_observer_does_not_retain_the_native_terminal_runtime() {
        let directory = tempfile::tempdir().expect("temporary native-terminal directory");
        let vault = VaultService::start(directory.path().join("vault"));
        let service =
            NativeTerminalService::start(directory.path().join("settings"), vault.clone());
        let weak_runtime = Arc::downgrade(&service.inner);
        vault.set_availability_observer(service.availability_observer());

        drop(service);

        assert!(
            weak_runtime.upgrade().is_none(),
            "Vault's callback must not form a strong Vault -> observer -> terminal cycle"
        );
    }

    #[test]
    fn vault_merge_reloads_encrypted_history_and_rejects_stale_persistence() {
        const PASSWORD: &[u8] = b"correct horse battery staple";
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = VaultService::start(directory.path().join("source-vault"));
        source
            .create_for_tests(PASSWORD)
            .expect("create source Vault");
        let source_epoch = source.with_vault_availability(|availability, _| availability.epoch);
        let mut imported = history_payload_with_command("imported command");
        imported.entries[0].completed_at_unix_ms = unix_time_ms();
        source
            .replace_command_history_at_epoch(imported, source_epoch)
            .expect("save source history");
        let envelope = source
            .export_encrypted_envelope()
            .expect("export source Vault");

        let target = VaultService::start(directory.path().join("target-vault"));
        target
            .create_for_tests(PASSWORD)
            .expect("create target Vault");
        let old_epoch = target.with_vault_availability(|availability, _| availability.epoch);
        let mut existing = history_payload_with_command("existing command");
        existing.entries[0].completed_at_unix_ms = unix_time_ms();
        target
            .replace_command_history_at_epoch(existing.clone(), old_epoch)
            .expect("save target history");

        let settings_directory = directory.path().join("settings");
        write_settings(
            &settings_directory.join(SETTINGS_FILE_NAME),
            &PersistedSettings {
                settings: NativeTerminalSettings {
                    history_enabled: true,
                    persist_encrypted: true,
                    ..NativeTerminalSettings::default()
                },
                settings_revision: 1,
            },
        )
        .expect("enable encrypted history");
        let terminal = NativeTerminalService::start(&settings_directory, target.clone());
        target.set_availability_observer(terminal.availability_observer());
        assert_eq!(terminal.inner.state.lock().expect("state").history.len(), 1);

        target
            .merge_encrypted_envelope(&envelope, PASSWORD, &[])
            .expect("merge backup Vault");
        let history = terminal.inner.state.lock().expect("state").history.clone();
        assert_eq!(history.len(), 2);
        assert!(
            history
                .iter()
                .any(|entry| entry.command == "existing command")
        );
        assert!(
            history
                .iter()
                .any(|entry| entry.command == "imported command")
        );
        assert!(matches!(
            target.replace_command_history_at_epoch(existing, old_epoch),
            Err(crate::vault_service::VaultServiceError::AvailabilityChanged)
        ));
    }

    #[test]
    fn history_eligibility_honors_leading_whitespace_and_secret_boundaries() {
        assert!(should_store_history("git status"));
        assert!(should_store_history("cd /www/wwwroot/NorixorAI/"));
        assert!(!should_store_history("cd /www"));
        assert!(!should_store_history(" git status"));
        assert!(!should_store_history("\u{2003}git status"));
        assert!(!should_store_history("curl -H 'Authorization: secret'"));
        assert!(!should_store_history("login --token abc"));
        assert!(!should_store_history("mysql --password abc"));
        assert!(!should_store_history("sshpass -p abc ssh host"));
    }

    #[test]
    fn history_persistence_queue_coalesces_to_one_latest_payload() {
        let pending = Arc::new(Mutex::new(HistoryPersistenceQueueState {
            latest: None,
            highest_revision: 0,
            generation: 0,
        }));
        let (wake_tx, mut wake_rx) = mpsc::channel(HISTORY_PERSISTENCE_WAKE_CAPACITY);
        let queue = HistoryPersistenceQueue {
            pending: Arc::clone(&pending),
            wake_tx,
        };

        assert_eq!(
            queue.enqueue(1, 1, history_payload_with_command("first"), None),
            HistoryPersistenceEnqueueResult::Queued
        );
        assert_eq!(
            queue.enqueue(2, 1, history_payload_with_command("second"), None),
            HistoryPersistenceEnqueueResult::Queued
        );
        assert!(wake_rx.try_recv().is_ok());
        assert!(wake_rx.try_recv().is_err());

        let task = pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .take()
            .expect("one coalesced persistence task");
        assert_eq!(task.history.entries.len(), 1);
        assert_eq!(task.history.entries[0].command, "second");
        assert_eq!(task.revision, 2);
        assert_eq!(task.vault_epoch, 1);
        assert!(task.confirmation.is_none());
    }

    #[test]
    fn history_persistence_queue_rejects_a_stale_payload_after_clear_revision() {
        let pending = Arc::new(Mutex::new(HistoryPersistenceQueueState {
            latest: None,
            highest_revision: 0,
            generation: 0,
        }));
        let (wake_tx, mut wake_rx) = mpsc::channel(HISTORY_PERSISTENCE_WAKE_CAPACITY);
        let queue = HistoryPersistenceQueue {
            pending: Arc::clone(&pending),
            wake_tx,
        };
        assert_eq!(
            queue.enqueue(9, 1, history_payload_with_command("cleared"), None),
            HistoryPersistenceEnqueueResult::Queued
        );
        let (confirmation, mut response) = oneshot::channel();
        assert_eq!(
            queue.enqueue(
                8,
                1,
                history_payload_with_command("stale"),
                Some(confirmation)
            ),
            HistoryPersistenceEnqueueResult::Stale
        );
        assert_eq!(response.try_recv(), Ok(false));
        assert!(wake_rx.try_recv().is_ok());
        let task = pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .take()
            .expect("newer clear snapshot remains queued");
        assert_eq!(task.revision, 9);
        assert_eq!(task.history.entries[0].command, "cleared");
    }

    #[test]
    fn history_persistence_queue_rejects_second_destructive_snapshot_without_replacing_first() {
        let pending = Arc::new(Mutex::new(HistoryPersistenceQueueState {
            latest: None,
            highest_revision: 0,
            generation: 0,
        }));
        let (wake_tx, mut wake_rx) = mpsc::channel(HISTORY_PERSISTENCE_WAKE_CAPACITY);
        let queue = HistoryPersistenceQueue {
            pending: Arc::clone(&pending),
            wake_tx,
        };
        let (first_confirmation, mut first_response) = oneshot::channel();
        assert_eq!(
            queue.enqueue(
                1,
                1,
                history_payload_with_command("first"),
                Some(first_confirmation)
            ),
            HistoryPersistenceEnqueueResult::Queued
        );
        let (second_confirmation, mut second_response) = oneshot::channel();
        assert_eq!(
            queue.enqueue(
                2,
                1,
                history_payload_with_command("second"),
                Some(second_confirmation)
            ),
            HistoryPersistenceEnqueueResult::Backpressured
        );
        assert_eq!(second_response.try_recv(), Ok(false));
        assert!(matches!(
            first_response.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(wake_rx.try_recv().is_ok());

        let task = pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .take()
            .expect("one pending persistence task");
        assert_eq!(task.history.entries[0].command, "first");
        assert!(task.confirmation.is_some());
    }

    #[tokio::test]
    async fn pending_encrypted_clear_blocks_a_second_empty_clear_until_durable() {
        const PASSWORD: &[u8] = b"correct horse battery staple";
        let directory = tempfile::tempdir().expect("temporary native-terminal directory");
        let vault = VaultService::start(directory.path().join("vault"));
        vault.create_for_tests(PASSWORD).expect("create test Vault");
        let vault_epoch = vault.with_vault_availability(|availability, _| availability.epoch);
        let mut history = history_payload_with_command("persisted command");
        // Keep this concurrency fixture inside retention; an expired entry
        // is correctly removed during startup hydration before clear runs.
        history.entries[0].completed_at_unix_ms = unix_time_ms();
        vault
            .replace_command_history_at_epoch(history, vault_epoch)
            .expect("seed encrypted history");

        let settings_directory = directory.path().join("settings");
        write_settings(
            &settings_directory.join(SETTINGS_FILE_NAME),
            &PersistedSettings {
                settings: NativeTerminalSettings {
                    history_enabled: true,
                    persist_encrypted: true,
                    ..NativeTerminalSettings::default()
                },
                settings_revision: 1,
            },
        )
        .expect("persist encrypted-history settings before service startup");
        let mut service = NativeTerminalService::start(&settings_directory, vault);
        assert_eq!(
            service
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .history
                .len(),
            1,
            "startup availability synchronization must hydrate encrypted history"
        );
        // Replace the worker with a manually held queue so the first clear
        // reaches its durable wait point deterministically.
        let pending = Arc::new(Mutex::new(HistoryPersistenceQueueState {
            latest: None,
            highest_revision: 0,
            generation: 0,
        }));
        let (wake_tx, mut wake_rx) = mpsc::channel(HISTORY_PERSISTENCE_WAKE_CAPACITY);
        service.replace_persistence_for_tests(HistoryPersistenceQueue {
            pending: Arc::clone(&pending),
            wake_tx,
        });
        let first_service = service.clone();
        let first = tokio::spawn(async move {
            first_service
                .history_clear(NativeTerminalHistoryClearRequest {
                    meta: norishell_core_api::RequestMeta {
                        request_id: RequestId::new(),
                    },
                    scope: None,
                })
                .await
        });
        // A queue wake is emitted only after `history_clear` has accepted the
        // exact encrypted replacement. It is a deterministic progress fence,
        // unlike repeatedly yielding and hoping the spawned task ran first.
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), wake_rx.recv())
                .await
                .expect("first clear reaches its encrypted persistence fence"),
            Some(())
        );
        assert!(
            service
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .history_destructive_pending
        );

        let second = service
            .history_clear(NativeTerminalHistoryClearRequest {
                meta: norishell_core_api::RequestMeta {
                    request_id: RequestId::new(),
                },
                scope: None,
            })
            .await
            .expect_err("second clear cannot acknowledge the first pending replacement");
        assert_eq!(second.code, "native_terminal.history_mutation_in_progress");
        let confirmation = pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .take()
            .and_then(|task| task.confirmation)
            .expect("pending clear owns its durable confirmation");
        assert!(confirmation.send(true).is_ok());
        first
            .await
            .expect("first clear task joins")
            .expect("durable clear succeeds");
        assert!(
            !service
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .history_destructive_pending
        );
    }
}
