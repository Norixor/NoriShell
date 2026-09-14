//! Session-scoped shell integration for command-completion facts and bounded
//! command history. Shell output is parsed only as it passes from the owner
//! into the live terminal stream; neither renderer replay nor plugin output is
//! treated as a source of command facts.

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::{
    CoreApiError, ErrorCategory, NATIVE_TERMINAL_EVENT_SCHEMA_VERSION, NativeTerminalActivity,
    NativeTerminalCaptureFailureCode, NativeTerminalCaptureState, NativeTerminalCommandCompletion,
    NativeTerminalEnableRequest, NativeTerminalHistoryClearRequest,
    NativeTerminalHistoryDeleteRequest, NativeTerminalHistoryEntry, NativeTerminalHistoryEntryId,
    NativeTerminalHistoryListRequest, NativeTerminalHistoryPauseRequest,
    NativeTerminalHistoryScope, NativeTerminalInputFence, NativeTerminalSessionScope,
    NativeTerminalSessionStatus, NativeTerminalSettings, NativeTerminalSettingsGetRequest,
    NativeTerminalSettingsReplaceRequest, NativeTerminalSettingsSnapshot, NativeTerminalShellKind,
    NativeTerminalSnapshot, NativeTerminalSnapshotRequest, RequestId, RetryStrategy,
    SshSessionInputRequest, WireSequence,
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
    native_terminal_scripts::script_for,
    ssh_session_service::SshSessionService,
    time::unix_time_ms,
    vault_service::{VaultAvailability, VaultService},
};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

const SETTINGS_FILE_NAME: &str = "native-terminal-settings.json";
// A maximum raw UTF-8 command is 16 KiB; standard Base64 expands it to about
// 21.4 KiB before the fixed OSC header and terminator. Keep the frame bound
// above that valid protocol shape while still bounding malformed private OSC.
const OSC_FRAME_MAX_BYTES: usize = 24 * 1024;
const COMMAND_MAX_BYTES: usize = 16 * 1024;
const HISTORY_QUERY_MAX: u16 = 100;
const COMPLETION_REPLAY_LIMIT: usize = 256;
const NATIVE_TERMINAL_ENABLE_TIMEOUT: Duration = Duration::from_secs(15);
/// A terminal can complete commands much faster than an encrypted Vault write.
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
    snapshot_revision: u64,
    completion_cursor: u64,
    sessions: BTreeMap<NativeSessionKey, CaptureSession>,
    completions: VecDeque<NativeTerminalCommandCompletion>,
    history: Vec<NativeTerminalHistoryEntry>,
    /// Every mutation of the in-memory history projection has an increasing
    /// revision. Persistence tasks carry it so an older output callback can
    /// never put a stale full payload behind a later clear/delete.
    history_epoch: u64,
    /// This is Core's synchronized view of encrypted-history accessibility.
    /// Terminal output processing never queries the Vault while holding state.
    vault_available: bool,
    vault_epoch: u64,
    persistence_failed: bool,
    /// Delete/clear remains in this state until its encrypted replacement is
    /// known durable. While set, capture is fail-closed and another destructive
    /// edit cannot acknowledge a snapshot that might later roll back.
    history_destructive_pending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NativeSessionKey {
    Ssh {
        session_id: String,
        generation: u64,
        channel_id: String,
    },
    Local {
        session_id: String,
        generation: u64,
        pty_id: String,
    },
}

impl NativeSessionKey {
    fn from_scope(scope: &NativeTerminalSessionScope) -> Self {
        match scope {
            NativeTerminalSessionScope::Ssh {
                session_id,
                generation,
                channel_id,
                ..
            } => Self::Ssh {
                session_id: session_id.as_str().to_owned(),
                generation: generation.get(),
                channel_id: channel_id.as_str().to_owned(),
            },
            NativeTerminalSessionScope::Local {
                session_id,
                generation,
                pty_id,
                ..
            } => Self::Local {
                session_id: session_id.as_str().to_owned(),
                generation: generation.get(),
                pty_id: pty_id.as_str().to_owned(),
            },
        }
    }
}

struct CaptureSession {
    scope: NativeTerminalSessionScope,
    history_scope: Option<NativeTerminalHistoryScope>,
    shell_kind: NativeTerminalShellKind,
    nonce: String,
    /// This is fixed when the generated shell integration is written. It
    /// describes whether that script ever sends command text in private OSC,
    /// rather than whether a later settings change happens to retain it.
    capture_commands: bool,
    pending_deadline: Option<Instant>,
    capture_state: NativeTerminalCaptureState,
    failure_code: Option<NativeTerminalCaptureFailureCode>,
    activity: NativeTerminalActivity,
    prompt_observed: bool,
    prompt_sequence: u64,
    prompt_input_sequence: u64,
    prompt_input_epoch: Option<u64>,
    parser: PrivateOscParser,
    active: Option<ActiveCommand>,
}

struct ActiveCommand {
    command_id: u64,
    started_at: Instant,
    /// Command text is retained only when the capture setting is currently
    /// permitted. Completion notifications never receive it.
    command: Option<String>,
    background: bool,
}

pub(crate) struct PreparedNativeTerminalEnable {
    pub(crate) input_fence: NativeTerminalInputFence,
    pub(crate) scope: NativeTerminalSessionScope,
    pub(crate) shell_kind: NativeTerminalShellKind,
    pub(crate) nonce: String,
    pub(crate) capture_commands: bool,
    pub(crate) script_bytes: Vec<u8>,
}

impl PreparedNativeTerminalEnable {
    /// Quick Connect has no saved Host scope. Keep its integration useful for
    /// completion timing while ensuring its shell never emits command text
    /// that Core could not place in a host-owned encrypted history.
    pub(crate) fn disable_command_capture(&mut self) {
        if self.capture_commands {
            self.capture_commands = false;
            self.script_bytes = script_for(self.shell_kind, &self.nonce, false);
        }
    }

    pub(crate) fn ssh_input(&self, request_id: RequestId) -> Option<SshSessionInputRequest> {
        let NativeTerminalInputFence::Ssh(fence) = &self.input_fence else {
            return None;
        };
        Some(SshSessionInputRequest {
            meta: norishell_core_api::RequestMeta { request_id },
            session_id: fence.session_id.clone(),
            expected_generation: fence.expected_generation,
            channel_id: fence.channel_id.clone(),
            attachment_id: fence.attachment_id.clone(),
            view_id: fence.view_id.clone(),
            focus_epoch: fence.focus_epoch,
            lease_id: fence.lease_id.clone(),
            input_epoch: fence.input_epoch,
            client_seq: fence.client_seq,
            bytes: self.script_bytes.clone(),
        })
    }

    pub(crate) fn local_input(
        &self,
        request_id: RequestId,
    ) -> Option<norishell_core_api::LocalSessionInputRequest> {
        let NativeTerminalInputFence::Local(fence) = &self.input_fence else {
            return None;
        };
        Some(norishell_core_api::LocalSessionInputRequest {
            meta: norishell_core_api::RequestMeta { request_id },
            session_id: fence.session_id.clone(),
            expected_generation: fence.expected_generation,
            expected_state_revision: fence.expected_state_revision,
            pty_id: fence.pty_id.clone(),
            attachment_id: fence.attachment_id.clone(),
            view_id: fence.view_id.clone(),
            lease_id: fence.lease_id.clone(),
            focus_epoch: fence.focus_epoch,
            input_epoch: fence.input_epoch,
            client_seq: fence.client_seq,
            bytes: self.script_bytes.clone(),
        })
    }
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
                            state.snapshot_revision = state.snapshot_revision.saturating_add(1);
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
    /// fast shell emits completions while the Vault is slow.
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
            snapshot_revision: 1,
            completion_cursor: 0,
            sessions: BTreeMap::new(),
            completions: VecDeque::new(),
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

    pub(crate) fn prepare_enable(
        &self,
        request: &NativeTerminalEnableRequest,
    ) -> CoreResult<PreparedNativeTerminalEnable> {
        if !request.confirmed_empty_prompt {
            return Err(native_validation_error(
                request.meta.request_id.clone(),
                "native_terminal.prompt_not_confirmed",
            ));
        }
        let scope = scope_from_fence(&request.input_fence);
        let nonce = Uuid::now_v7().to_string();
        // The shell script cannot safely change its private-output shape from
        // an unrelated settings update. Snapshot the current permission while
        // building the Core-owned bytes, then report that same mode for this
        // session after the actor has accepted the exact fence.
        let capture_commands = self.history_capture_allowed();
        let script = script_for(request.shell_kind, &nonce, capture_commands);
        Ok(PreparedNativeTerminalEnable {
            input_fence: request.input_fence.clone(),
            scope,
            shell_kind: request.shell_kind,
            nonce,
            capture_commands,
            script_bytes: script,
        })
    }

    /// Called inside the SSH/Local actor's focus-broker ordering domain, after
    /// it has accepted the exact writer fence and before it waits for writer
    /// acknowledgement. A later ready OSC is only an integration hint.
    pub(crate) fn mark_pending(
        &self,
        prepared: &PreparedNativeTerminalEnable,
        history_scope: Option<NativeTerminalHistoryScope>,
    ) {
        let deadline = Instant::now() + NATIVE_TERMINAL_ENABLE_TIMEOUT;
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = NativeSessionKey::from_scope(&prepared.scope);
        let previous_prompt_sequence = state
            .sessions
            .get(&key)
            .map_or(0, |session| session.prompt_sequence);
        state.sessions.insert(
            key,
            CaptureSession {
                scope: prepared.scope.clone(),
                history_scope,
                shell_kind: prepared.shell_kind,
                nonce: prepared.nonce.clone(),
                capture_commands: prepared.capture_commands,
                pending_deadline: Some(deadline),
                capture_state: NativeTerminalCaptureState::Pending,
                failure_code: None,
                activity: NativeTerminalActivity::Prompt,
                prompt_observed: false,
                prompt_sequence: previous_prompt_sequence,
                prompt_input_sequence: 0,
                prompt_input_epoch: None,
                parser: PrivateOscParser::default(),
                active: None,
            },
        );
        state.snapshot_revision = state.snapshot_revision.saturating_add(1);
        drop(state);
        let service = self.clone();
        let scope = prepared.scope.clone();
        let nonce = prepared.nonce.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(NATIVE_TERMINAL_ENABLE_TIMEOUT).await;
            service.mark_enable_timed_out(&scope, &nonce);
        });
    }

    pub(crate) fn mark_enable_failed(&self, scope: &NativeTerminalSessionScope, nonce: &str) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = state.sessions.get_mut(&NativeSessionKey::from_scope(scope)) else {
            return;
        };
        if session.nonce != nonce {
            return;
        }
        session.capture_state = NativeTerminalCaptureState::Failed;
        session.failure_code = Some(NativeTerminalCaptureFailureCode::WriterRejected);
        session.pending_deadline = None;
        session.active = None;
        state.snapshot_revision = state.snapshot_revision.saturating_add(1);
    }

    fn mark_enable_timed_out(&self, scope: &NativeTerminalSessionScope, nonce: &str) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = state.sessions.get_mut(&NativeSessionKey::from_scope(scope)) else {
            return;
        };
        if session.nonce != nonce
            || !matches!(session.capture_state, NativeTerminalCaptureState::Pending)
            || session
                .pending_deadline
                .is_none_or(|deadline| deadline > Instant::now())
        {
            return;
        }
        session.capture_state = NativeTerminalCaptureState::Failed;
        session.failure_code = Some(NativeTerminalCaptureFailureCode::EnableTimedOut);
        session.pending_deadline = None;
        session.active = None;
        state.snapshot_revision = state.snapshot_revision.saturating_add(1);
    }

    pub(crate) fn status_for(
        &self,
        scope: &NativeTerminalSessionScope,
    ) -> Option<NativeTerminalSessionStatus> {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .sessions
            .get(&NativeSessionKey::from_scope(scope))
            .map(|session| project_session(&state, session))
    }

    pub(crate) fn filter_ssh_output(
        &self,
        session_id: &norishell_core_api::SshSessionId,
        generation: WireSequence,
        channel_id: &norishell_core_api::SshChannelId,
        prompt_input_sequence: WireSequence,
        prompt_input_epoch: Option<WireSequence>,
        bytes: Vec<u8>,
    ) -> Vec<u8> {
        let key = NativeSessionKey::Ssh {
            session_id: session_id.as_str().to_owned(),
            generation: generation.get(),
            channel_id: channel_id.as_str().to_owned(),
        };
        self.filter_output(key, prompt_input_sequence, prompt_input_epoch, bytes)
    }

    pub(crate) fn filter_local_output(
        &self,
        session_id: &norishell_core_api::LocalSessionId,
        generation: WireSequence,
        pty_id: &norishell_core_api::LocalPtyId,
        prompt_input_sequence: WireSequence,
        prompt_input_epoch: Option<WireSequence>,
        bytes: Vec<u8>,
    ) -> Vec<u8> {
        let key = NativeSessionKey::Local {
            session_id: session_id.as_str().to_owned(),
            generation: generation.get(),
            pty_id: pty_id.as_str().to_owned(),
        };
        self.filter_output(key, prompt_input_sequence, prompt_input_epoch, bytes)
    }

    fn filter_output(
        &self,
        key: NativeSessionKey,
        prompt_input_sequence: WireSequence,
        prompt_input_epoch: Option<WireSequence>,
        bytes: Vec<u8>,
    ) -> Vec<u8> {
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(session) = state.sessions.get(&key) else {
                // Most terminal output has no native integration registered;
                // preserve the normal SSH/local fast path without touching
                // the Vault mutex.
                return bytes;
            };
            let encrypted_capture_active = state.settings.persist_encrypted
                && state.settings.history_enabled
                && !state.settings.history_paused
                && !state.history_destructive_pending
                && session.capture_commands;
            if !encrypted_capture_active {
                return self.filter_output_with_state_locked(
                    &mut state,
                    key,
                    prompt_input_sequence,
                    prompt_input_epoch,
                    bytes,
                );
            }
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
                self.filter_output_with_state_locked(
                    &mut state,
                    key,
                    prompt_input_sequence,
                    prompt_input_epoch,
                    bytes,
                )
            })
    }

    fn filter_output_with_state_locked(
        &self,
        state: &mut NativeTerminalState,
        key: NativeSessionKey,
        prompt_input_sequence: WireSequence,
        prompt_input_epoch: Option<WireSequence>,
        bytes: Vec<u8>,
    ) -> Vec<u8> {
        let history_capture_allowed = history_capture_allowed_for_state(state);
        let Some(session) = state.sessions.get_mut(&key) else {
            return bytes;
        };
        let (filtered, frames) = session.parser.push(&bytes);
        let mut completions = Vec::new();
        let mut changed = false;
        for frame in frames {
            let (did_change, completion) = apply_private_osc(
                session,
                &frame,
                history_capture_allowed,
                prompt_input_sequence,
                prompt_input_epoch,
            );
            changed |= did_change;
            if let Some(completion) = completion {
                completions.push(completion);
            }
        }
        let mut history_changed = false;
        for completion in completions {
            state.completion_cursor = state.completion_cursor.saturating_add(1);
            let cursor = WireSequence::new(state.completion_cursor);
            let event = NativeTerminalCommandCompletion {
                cursor,
                event_id: norishell_core_api::NativeTerminalEventId::new(),
                session: completion.scope.clone(),
                elapsed_millis: completion.elapsed_millis,
                exit_code: completion.exit_code,
                completed_at_unix_ms: completion.completed_at_unix_ms,
            };
            state.completions.push_back(event);
            while state.completions.len() > COMPLETION_REPLAY_LIMIT {
                state.completions.pop_front();
            }
            if let (Some(command), Some(scope)) = (completion.command, completion.history_scope)
                && should_store_history(&command)
                && history_capture_allowed
            {
                state.history.push(NativeTerminalHistoryEntry {
                    entry_id: NativeTerminalHistoryEntryId::new(),
                    scope,
                    command,
                    completed_at_unix_ms: completion.completed_at_unix_ms,
                    elapsed_millis: completion.elapsed_millis,
                    exit_code: completion.exit_code,
                });
                let settings = state.settings.clone();
                prune_history(&mut state.history, &settings);
                state.history_epoch = state.history_epoch.saturating_add(1);
                history_changed = true;
            }
            changed = true;
        }
        if changed {
            state.snapshot_revision = state.snapshot_revision.saturating_add(1);
        }
        if history_changed && encrypted_persistence_available(state) {
            let _ = self.inner.persistence.enqueue(
                state.history_epoch,
                state.vault_epoch,
                history_payload(&state.history),
                None,
            );
        }
        filtered
    }

    pub(crate) fn clear_ssh_session(
        &self,
        session_id: &norishell_core_api::SshSessionId,
        generation: WireSequence,
    ) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = state.sessions.len();
        state.sessions.retain(|key, _| {
            !matches!(
                key,
                NativeSessionKey::Ssh {
                    session_id: candidate,
                    generation: candidate_generation,
                    ..
                } if candidate == session_id.as_str() && *candidate_generation == generation.get()
            )
        });
        if state.sessions.len() != before {
            state.snapshot_revision = state.snapshot_revision.saturating_add(1);
        }
    }

    pub(crate) fn clear_local_session(
        &self,
        session_id: &norishell_core_api::LocalSessionId,
        generation: WireSequence,
    ) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = state.sessions.len();
        state.sessions.retain(|key, _| {
            !matches!(
                key,
                NativeSessionKey::Local {
                    session_id: candidate,
                    generation: candidate_generation,
                    ..
                } if candidate == session_id.as_str() && *candidate_generation == generation.get()
            )
        });
        if state.sessions.len() != before {
            state.snapshot_revision = state.snapshot_revision.saturating_add(1);
        }
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
            let capture_was_allowed = history_capture_allowed_for_state(&state);
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
            if capture_was_allowed && !history_capture_allowed_for_state(&state) {
                revoke_active_history_commands(&mut state);
            }
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
            state.snapshot_revision = state.snapshot_revision.saturating_add(1);
        }
        if invalidate_persistence {
            self.inner.persistence.invalidate();
        }
        Ok(self.settings_snapshot())
    }

    pub(crate) fn snapshot(
        &self,
        request: NativeTerminalSnapshotRequest,
    ) -> NativeTerminalSnapshot {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let after = request.after_completion_cursor.map_or(0, WireSequence::get);
        NativeTerminalSnapshot {
            schema_version: NATIVE_TERMINAL_EVENT_SCHEMA_VERSION,
            snapshot_revision: WireSequence::new(state.snapshot_revision),
            history_paused: state.settings.history_paused,
            history_persistence_failed: state.persistence_failed,
            completion_cursor: WireSequence::new(state.completion_cursor),
            sessions: state
                .sessions
                .values()
                .map(|session| project_session(&state, session))
                .collect(),
            completions: state
                .completions
                .iter()
                .filter(|completion| completion.cursor.get() > after)
                .cloned()
                .collect(),
        }
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
        state.snapshot_revision = state.snapshot_revision.saturating_add(1);
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
        } else {
            state.snapshot_revision = state.snapshot_revision.saturating_add(1);
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
                let capture_was_allowed = history_capture_allowed_for_state(&state);
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
                if capture_was_allowed && !history_capture_allowed_for_state(&state) {
                    revoke_active_history_commands(&mut state);
                }
            }
            state.snapshot_revision = state.snapshot_revision.saturating_add(1);
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
                state.snapshot_revision = state.snapshot_revision.saturating_add(1);
                false
            } else {
                if state.vault_epoch == availability.epoch && !state.vault_available {
                    return;
                }
                let was_available = state.vault_available;
                let persistence_failed = state.persistence_failed;
                state.vault_epoch = availability.epoch;
                state.vault_available = false;
                if state.settings.persist_encrypted {
                    revoke_active_history_commands(&mut state);
                    let cleared = clear_encrypted_history_cache(&mut state);
                    state.persistence_failed = false;
                    if !cleared && (was_available || persistence_failed) {
                        state.snapshot_revision = state.snapshot_revision.saturating_add(1);
                    }
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

    fn history_capture_allowed(&self) -> bool {
        {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.settings.persist_encrypted
                || !state.settings.history_enabled
                || state.settings.history_paused
                || state.history_destructive_pending
            {
                return history_capture_allowed_for_state(&state);
            }
        }
        self.inner
            .vault
            .with_vault_availability(|availability, vault| {
                self.apply_vault_availability_under_guard(availability, vault);
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                history_capture_allowed_for_state(&state)
            })
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
pub async fn native_terminal_enable(
    request: NativeTerminalEnableRequest,
    service: State<'_, NativeTerminalService>,
    sessions: State<'_, SshSessionService>,
) -> CoreResult<NativeTerminalSessionStatus> {
    let request_id = request.meta.request_id.clone();
    let prepared = service.prepare_enable(&request)?;
    let scope = prepared.scope.clone();
    match &prepared.input_fence {
        NativeTerminalInputFence::Ssh(_) => {
            sessions
                .native_terminal_enable_ssh(request_id.clone(), prepared)
                .await?;
        }
        NativeTerminalInputFence::Local(_) => {
            sessions
                .native_terminal_enable_local(request_id.clone(), prepared)
                .await?;
        }
    }
    service.status_for(&scope).ok_or_else(|| {
        native_unavailable_error(request_id, "native_terminal.enable_state_unavailable")
    })
}

#[tauri::command]
pub fn native_terminal_snapshot(
    request: NativeTerminalSnapshotRequest,
    service: State<'_, NativeTerminalService>,
) -> CoreResult<NativeTerminalSnapshot> {
    Ok(service.snapshot(request))
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

fn scope_from_fence(fence: &NativeTerminalInputFence) -> NativeTerminalSessionScope {
    match fence {
        NativeTerminalInputFence::Ssh(fence) => NativeTerminalSessionScope::Ssh {
            session_id: fence.session_id.clone(),
            generation: fence.expected_generation,
            channel_id: fence.channel_id.clone(),
            pane_id: fence.view_id.clone(),
        },
        NativeTerminalInputFence::Local(fence) => NativeTerminalSessionScope::Local {
            session_id: fence.session_id.clone(),
            generation: fence.expected_generation,
            pty_id: fence.pty_id.clone(),
            pane_id: fence.view_id.clone(),
        },
    }
}

fn project_session(
    state: &NativeTerminalState,
    session: &CaptureSession,
) -> NativeTerminalSessionStatus {
    NativeTerminalSessionStatus {
        session: session.scope.clone(),
        shell_kind: session.shell_kind,
        capture_state: session.capture_state,
        failure_code: session.failure_code,
        activity: session.activity,
        captures_command: matches!(session.capture_state, NativeTerminalCaptureState::Ready)
            && session.capture_commands,
        history_paused: state.settings.history_paused,
        prompt_observed: session.prompt_observed,
        prompt_sequence: WireSequence::new(session.prompt_sequence),
        prompt_input_sequence: WireSequence::new(session.prompt_input_sequence),
        prompt_input_epoch: session.prompt_input_epoch.map(WireSequence::new),
        completion_cursor: WireSequence::new(state.completion_cursor),
    }
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

fn revoke_active_history_commands(state: &mut NativeTerminalState) {
    for session in state.sessions.values_mut() {
        if let Some(active) = session.active.as_mut() {
            active.command = None;
        }
    }
}

/// Restores an optimistic destructive edit before another history capture can
/// observe the projection. Callers already hold `NativeTerminalState`.
fn rollback_destructive_history_locked(
    state: &mut NativeTerminalState,
    history: Vec<NativeTerminalHistoryEntry>,
) {
    state.history = history;
    state.history_epoch = state.history_epoch.saturating_add(1);
    state.snapshot_revision = state.snapshot_revision.saturating_add(1);
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
    state.snapshot_revision = state.snapshot_revision.saturating_add(1);
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
        "password=",
        "passwd=",
        "token=",
        "secret=",
        "api_key=",
        "apikey=",
        "authorization:",
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

struct CompletionCandidate {
    scope: NativeTerminalSessionScope,
    history_scope: Option<NativeTerminalHistoryScope>,
    command: Option<String>,
    elapsed_millis: u64,
    exit_code: Option<i32>,
    completed_at_unix_ms: i64,
}

fn apply_private_osc(
    session: &mut CaptureSession,
    frame: &[u8],
    history_capture_allowed: bool,
    prompt_input_sequence: WireSequence,
    prompt_input_epoch: Option<WireSequence>,
) -> (bool, Option<CompletionCandidate>) {
    let Ok(frame) = std::str::from_utf8(frame) else {
        return protocol_failure(session);
    };
    let fields = frame.split(';').collect::<Vec<_>>();
    if fields.len() != 7
        || fields[0] != "6973"
        || fields[1] != "NoriShell"
        || fields[2] != "1"
        || fields[3] != session.nonce
    {
        return (false, None);
    }
    // A late frame from a bootstrap that already timed out or reported an
    // unsupported hook must still be removed from terminal output, but it can
    // never revive or overwrite that terminal fact.
    if matches!(
        session.capture_state,
        NativeTerminalCaptureState::Failed | NativeTerminalCaptureState::Unsupported
    ) {
        return (true, None);
    }
    let command_id = fields[5].parse::<u64>().ok();
    match fields[4] {
        "ready"
            if matches!(session.capture_state, NativeTerminalCaptureState::Pending)
                && command_id == Some(0)
                && fields[6] == "-" =>
        {
            session.capture_state = NativeTerminalCaptureState::Ready;
            session.failure_code = None;
            session.pending_deadline = None;
            session.activity = NativeTerminalActivity::Prompt;
            session.prompt_observed = true;
            session.prompt_sequence = session.prompt_sequence.saturating_add(1);
            session.prompt_input_sequence = prompt_input_sequence.get();
            session.prompt_input_epoch = prompt_input_epoch.map(WireSequence::get);
            (true, None)
        }
        "unsupported"
            if matches!(session.capture_state, NativeTerminalCaptureState::Pending)
                && command_id == Some(0) =>
        {
            session.capture_state = NativeTerminalCaptureState::Unsupported;
            session.failure_code = Some(NativeTerminalCaptureFailureCode::ShellUnsupported);
            session.pending_deadline = None;
            session.active = None;
            (true, None)
        }
        "start" if matches!(session.capture_state, NativeTerminalCaptureState::Ready) => {
            let Some(command_id) = command_id else {
                return protocol_failure(session);
            };
            if session.active.is_some() {
                return (false, None);
            }
            let captured_command = if session.capture_commands && fields[6] != "-" {
                // A session that was installed in capture mode can outlive a
                // pause, a setting change, or a Vault lock. Once capture is
                // revoked, keep the encoded OSC payload opaque: decoding it
                // would recreate command text in Core after the user asked us
                // to stop collecting it. The bounded parser still removes the
                // private frame and start/end timing remains available.
                if !history_capture_allowed {
                    None
                } else {
                    let Ok(command_bytes) = BASE64.decode(fields[6]) else {
                        return protocol_failure(session);
                    };
                    let Ok(command) = String::from_utf8(command_bytes) else {
                        return protocol_failure(session);
                    };
                    if command.is_empty()
                        || command.len() > COMMAND_MAX_BYTES
                        || command.bytes().any(|byte| byte.is_ascii_control())
                    {
                        return protocol_failure(session);
                    }
                    Some(command)
                }
            } else if !session.capture_commands {
                // Notification-only integration still needs a start/end pair
                // for elapsed-time facts, but its OSC must not contain the
                // command text (encoded or otherwise).
                if fields[6] != "-" {
                    return protocol_failure(session);
                }
                None
            } else {
                // A history-enabled script uses the same no-text sentinel for
                // a leading-blank, known-secret, or oversized command. Keep
                // completion timing without ever decoding a command payload.
                None
            };
            let background = captured_command
                .as_ref()
                .is_some_and(|command| command.trim_end().ends_with('&'));
            let retained_command = captured_command
                .filter(|command| history_capture_allowed && should_store_history(command));
            session.active = Some(ActiveCommand {
                command_id,
                started_at: Instant::now(),
                command: retained_command,
                background,
            });
            session.activity = NativeTerminalActivity::Executing;
            (true, None)
        }
        "end" => {
            let Some(command_id) = command_id else {
                return protocol_failure(session);
            };
            let Some(active) = session.active.take() else {
                return (false, None);
            };
            if active.command_id != command_id {
                session.active = Some(active);
                return (false, None);
            }
            let exit_code = if fields[6] == "unknown" {
                None
            } else {
                match fields[6].parse::<i32>() {
                    Ok(value) => Some(value),
                    Err(_) => return protocol_failure(session),
                }
            };
            session.activity = NativeTerminalActivity::Prompt;
            let elapsed_millis =
                u64::try_from(active.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            let history_scope = active.history_allowed(session);
            let command = if active.background {
                None
            } else {
                active.command
            };
            (
                true,
                Some(CompletionCandidate {
                    scope: session.scope.clone(),
                    history_scope,
                    command,
                    elapsed_millis,
                    exit_code,
                    completed_at_unix_ms: unix_time_ms(),
                }),
            )
        }
        "prompt" if command_id == Some(0) && fields[6] == "-" => {
            session.activity = NativeTerminalActivity::Prompt;
            session.prompt_observed = true;
            session.prompt_sequence = session.prompt_sequence.saturating_add(1);
            session.prompt_input_sequence = prompt_input_sequence.get();
            session.prompt_input_epoch = prompt_input_epoch.map(WireSequence::get);
            (true, None)
        }
        _ => protocol_failure(session),
    }
}

impl ActiveCommand {
    fn history_allowed(&self, session: &CaptureSession) -> Option<NativeTerminalHistoryScope> {
        self.command.as_ref().and(session.history_scope.clone())
    }
}

fn protocol_failure(session: &mut CaptureSession) -> (bool, Option<CompletionCandidate>) {
    session.capture_state = NativeTerminalCaptureState::Failed;
    session.failure_code = Some(NativeTerminalCaptureFailureCode::ProtocolViolation);
    session.pending_deadline = None;
    session.active = None;
    (true, None)
}

#[derive(Default)]
struct PrivateOscParser {
    state: OscParserState,
}

#[derive(Default)]
enum OscParserState {
    #[default]
    Ground,
    Escape,
    Osc(Vec<u8>),
    /// The prefix already proved this is a NoriShell-private OSC. Drop its
    /// bytes until BEL/ST no matter how long it is; otherwise a malicious or
    /// malformed overlong payload could leak its tail to xterm or plugins.
    PrivateDiscard {
        previous_escape: bool,
    },
}

impl PrivateOscParser {
    fn push(&mut self, input: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut output = Vec::with_capacity(input.len());
        let mut frames = Vec::new();
        for byte in input.iter().copied() {
            match &mut self.state {
                OscParserState::Ground => {
                    if byte == 0x1b {
                        self.state = OscParserState::Escape;
                    } else {
                        output.push(byte);
                    }
                }
                OscParserState::Escape => {
                    if byte == b']' {
                        self.state = OscParserState::Osc(vec![0x1b, b']']);
                    } else {
                        output.push(0x1b);
                        if byte == 0x1b {
                            self.state = OscParserState::Escape;
                        } else {
                            output.push(byte);
                            self.state = OscParserState::Ground;
                        }
                    }
                }
                OscParserState::Osc(buffer) => {
                    buffer.push(byte);
                    let terminated = byte == 0x07
                        || (byte == b'\\' && buffer.len() >= 2 && buffer[buffer.len() - 2] == 0x1b);
                    if terminated {
                        let buffer = std::mem::take(buffer);
                        if is_private_osc(&buffer) {
                            frames.push(osc_contents(&buffer).to_vec());
                        } else {
                            output.extend(buffer);
                        }
                        self.state = OscParserState::Ground;
                    } else if buffer.len() > OSC_FRAME_MAX_BYTES {
                        if is_private_osc(buffer) {
                            self.state = OscParserState::PrivateDiscard {
                                previous_escape: byte == 0x1b,
                            };
                        } else {
                            output.extend(std::mem::take(buffer));
                            self.state = OscParserState::Ground;
                        }
                    }
                }
                OscParserState::PrivateDiscard { previous_escape } => {
                    let terminated = byte == 0x07 || (*previous_escape && byte == b'\\');
                    *previous_escape = byte == 0x1b;
                    if terminated {
                        self.state = OscParserState::Ground;
                    }
                }
            }
        }
        (output, frames)
    }
}

fn is_private_osc(buffer: &[u8]) -> bool {
    osc_contents(buffer).starts_with(b"6973;NoriShell;1;")
}

fn osc_contents(buffer: &[u8]) -> &[u8] {
    let start = buffer
        .get(0..2)
        .is_some_and(|prefix| prefix == [0x1b, b']']) as usize
        * 2;
    let end = if buffer.last() == Some(&0x07) {
        buffer.len().saturating_sub(1)
    } else if buffer.len() >= 2 && buffer[buffer.len() - 2..] == [0x1b, b'\\'] {
        buffer.len().saturating_sub(2)
    } else {
        buffer.len()
    };
    buffer.get(start..end).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture_session(capture_commands: bool) -> CaptureSession {
        CaptureSession {
            scope: NativeTerminalSessionScope::Local {
                session_id: norishell_core_api::LocalSessionId::new(),
                generation: WireSequence::new(1),
                pty_id: norishell_core_api::LocalPtyId::new(),
                pane_id: norishell_core_api::LocalViewId::new(),
            },
            history_scope: Some(NativeTerminalHistoryScope::Local),
            shell_kind: NativeTerminalShellKind::Bash,
            nonce: "nonce".to_owned(),
            capture_commands,
            pending_deadline: None,
            capture_state: NativeTerminalCaptureState::Ready,
            failure_code: None,
            activity: NativeTerminalActivity::Prompt,
            prompt_observed: true,
            prompt_sequence: 1,
            prompt_input_sequence: 0,
            prompt_input_epoch: None,
            parser: PrivateOscParser::default(),
            active: None,
        }
    }

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
            snapshot_revision: 1,
            completion_cursor: 0,
            sessions: BTreeMap::new(),
            completions: VecDeque::new(),
            history: vec![history_entry("plain command")],
            history_epoch: 0,
            vault_available: true,
            vault_epoch: 0,
            persistence_failed: false,
            history_destructive_pending: false,
        }
    }

    #[test]
    fn private_osc_is_stripped_across_live_frame_boundaries() {
        let mut parser = PrivateOscParser::default();
        let (first_output, first_frames) = parser.push(b"before\x1b]6973;NoriShell;1;nonce;sta");
        assert_eq!(first_output, b"before");
        assert!(first_frames.is_empty());

        let (second_output, second_frames) = parser.push(b"rt;1;ZWNobyBoaQ==\x07after");
        assert_eq!(second_output, b"after");
        assert_eq!(
            second_frames,
            vec![b"6973;NoriShell;1;nonce;start;1;ZWNobyBoaQ=="]
        );
    }

    #[test]
    fn foreign_osc_and_other_vt_bytes_remain_verbatim() {
        let mut parser = PrivateOscParser::default();
        let bytes = b"\x1b[31mred\x1b]0;window title\x07\x1b[0m";
        let (output, frames) = parser.push(bytes);
        assert_eq!(output, bytes);
        assert!(frames.is_empty());
    }

    #[test]
    fn malformed_private_osc_does_not_leak_back_into_output() {
        let mut parser = PrivateOscParser::default();
        let (output, frames) = parser.push(b"a\x1b]6973;NoriShell;1;wrong;bad\x07b");
        assert_eq!(output, b"ab");
        assert_eq!(frames, vec![b"6973;NoriShell;1;wrong;bad"]);
    }

    #[test]
    fn oversized_private_osc_is_bounded_and_the_next_text_survives() {
        let mut parser = PrivateOscParser::default();
        let mut input = b"x\x1b]6973;NoriShell;1;nonce;start;1;".to_vec();
        input.extend(std::iter::repeat_n(b'a', super::OSC_FRAME_MAX_BYTES + 16));
        input.push(0x07);
        input.extend_from_slice(b"y");
        let (output, frames) = parser.push(&input);
        assert_eq!(output, b"xy");
        assert!(frames.is_empty());
    }

    #[test]
    fn largest_valid_command_frame_is_not_mistaken_for_an_oversized_private_osc() {
        let command = "a".repeat(COMMAND_MAX_BYTES);
        let encoded = BASE64.encode(command.as_bytes());
        let frame = format!("\x1b]6973;NoriShell;1;nonce;start;1;{encoded}\x07");
        assert!(frame.len() <= OSC_FRAME_MAX_BYTES);

        let mut parser = PrivateOscParser::default();
        let (output, frames) = parser.push(frame.as_bytes());
        assert!(output.is_empty());
        assert_eq!(frames.len(), 1);

        let mut session = capture_session(true);
        let (changed, completion) =
            apply_private_osc(&mut session, &frames[0], true, WireSequence::new(0), None);
        assert!(changed);
        assert!(completion.is_none());
        assert_eq!(
            session
                .active
                .as_ref()
                .and_then(|active| active.command.as_deref()),
            Some(command.as_str())
        );
    }

    #[test]
    fn unterminated_oversized_private_osc_never_releases_its_tail() {
        let mut parser = PrivateOscParser::default();
        let mut input = b"x\x1b]6973;NoriShell;1;nonce;start;1;".to_vec();
        input.extend(std::iter::repeat_n(b'a', super::OSC_FRAME_MAX_BYTES + 16));
        input.extend_from_slice(b"must-not-leak");
        let (output, frames) = parser.push(&input);
        assert_eq!(output, b"x");
        assert!(frames.is_empty());

        let (output, frames) = parser.push(b"\x07after");
        assert_eq!(output, b"after");
        assert!(frames.is_empty());
    }

    #[test]
    fn notification_only_start_frame_never_retains_or_accepts_command_text() {
        let mut session = capture_session(false);
        let (changed, completion) = apply_private_osc(
            &mut session,
            b"6973;NoriShell;1;nonce;start;1;-",
            false,
            WireSequence::new(0),
            None,
        );
        assert!(changed);
        assert!(completion.is_none());
        assert!(
            session
                .active
                .as_ref()
                .is_some_and(|active| active.command.is_none())
        );

        let (_, completion) = apply_private_osc(
            &mut session,
            b"6973;NoriShell;1;nonce;end;1;0",
            false,
            WireSequence::new(0),
            None,
        );
        assert!(completion.is_some_and(|completion| completion.command.is_none()));

        let mut invalid_session = capture_session(false);
        let (changed, completion) = apply_private_osc(
            &mut invalid_session,
            b"6973;NoriShell;1;nonce;start;1;ZWNobyBzZWNyZXQ=",
            false,
            WireSequence::new(0),
            None,
        );
        assert!(changed);
        assert!(completion.is_none());
        assert_eq!(
            invalid_session.capture_state,
            NativeTerminalCaptureState::Failed
        );
    }

    #[test]
    fn history_mode_no_text_sentinel_preserves_completion_timing() {
        let mut session = capture_session(true);
        let (changed, completion) = apply_private_osc(
            &mut session,
            b"6973;NoriShell;1;nonce;start;1;-",
            true,
            WireSequence::new(0),
            None,
        );
        assert!(changed);
        assert!(completion.is_none());
        assert!(
            session
                .active
                .as_ref()
                .is_some_and(|active| active.command.is_none())
        );
        let (_, completion) = apply_private_osc(
            &mut session,
            b"6973;NoriShell;1;nonce;end;1;0",
            true,
            WireSequence::new(0),
            None,
        );
        assert!(completion.is_some_and(|completion| completion.command.is_none()));
    }

    #[test]
    fn revoked_history_mode_keeps_command_payload_opaque() {
        let mut session = capture_session(true);
        // This is deliberately malformed Base64. Once capture is revoked it
        // must neither be decoded nor turn the session into a protocol
        // failure; the private bytes are simply stripped from live output.
        let (changed, completion) = apply_private_osc(
            &mut session,
            b"6973;NoriShell;1;nonce;start;1;not-base64!",
            false,
            WireSequence::new(0),
            None,
        );
        assert!(changed);
        assert!(completion.is_none());
        assert_eq!(session.capture_state, NativeTerminalCaptureState::Ready);
        assert!(
            session
                .active
                .as_ref()
                .is_some_and(|active| active.command.is_none())
        );
    }

    #[test]
    fn vault_lock_clears_only_the_encrypted_history_projection() {
        let mut state = state_with_encrypted_history();
        assert!(clear_encrypted_history_cache(&mut state));
        assert!(state.history.is_empty());
        assert_eq!(state.history_epoch, 1);
        assert_eq!(state.snapshot_revision, 2);
        assert!(!clear_encrypted_history_cache(&mut state));
        assert_eq!(state.history_epoch, 1);
        assert_eq!(state.snapshot_revision, 2);
    }

    #[test]
    fn pause_hides_history_and_revokes_active_capture_without_erasing_cache() {
        let mut state = state_with_encrypted_history();
        state.settings.history_enabled = true;
        let mut session = capture_session(true);
        session.active = Some(ActiveCommand {
            command_id: 1,
            started_at: Instant::now(),
            command: Some("git status".to_owned()),
            background: false,
        });
        state
            .sessions
            .insert(NativeSessionKey::from_scope(&session.scope), session);

        state.settings.history_paused = true;
        assert!(!history_visible_for_state(&state));
        assert!(!history_capture_allowed_for_state(&state));
        revoke_active_history_commands(&mut state);

        assert_eq!(state.history.len(), 1);
        assert!(
            state
                .sessions
                .values()
                .next()
                .and_then(|session| session.active.as_ref())
                .is_some_and(|active| active.command.is_none())
        );

        state.settings.history_paused = false;
        assert!(history_visible_for_state(&state));
        assert_eq!(state.history[0].command, "plain command");
    }

    #[test]
    fn paused_projection_is_distinct_from_history_availability() {
        let mut state = state_with_encrypted_history();
        state.settings.history_enabled = true;
        state.vault_available = false;
        let session = capture_session(true);

        assert!(!history_visible_for_state(&state));
        assert!(!state.settings.history_paused);
        assert!(!project_session(&state, &session).history_paused);

        state.settings.history_paused = true;
        assert!(project_session(&state, &session).history_paused);
    }

    #[test]
    fn enabling_encrypted_persistence_requires_an_unlocked_vault() {
        let directory = tempfile::tempdir().expect("temporary native-terminal directory");
        let vault = VaultService::start(directory.path().join("vault"));
        let service = NativeTerminalService::start(directory.path().join("settings"), vault);
        let result = service.replace_settings(NativeTerminalSettingsReplaceRequest {
            meta: norishell_core_api::RequestMeta {
                request_id: RequestId::new(),
            },
            expected_settings_revision: WireSequence::new(1),
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
    fn history_eligibility_honors_leading_whitespace_and_secret_boundaries() {
        assert!(should_store_history("git status"));
        assert!(!should_store_history(" git status"));
        assert!(!should_store_history("\u{2003}git status"));
        assert!(!should_store_history("curl -H 'Authorization: secret'"));
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
