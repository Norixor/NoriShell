use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Read},
    sync::{Arc, Mutex, mpsc as std_mpsc},
    time::Duration,
};

use norishell_core_api::{
    CoreApiError, ErrorCategory, LOCAL_TERMINAL_EVENT_SCHEMA_VERSION,
    LOCAL_TERMINAL_OUTPUT_FRAME_MAX_BYTES, LocalAttachAttemptId, LocalAttachmentId,
    LocalInputLeaseId, LocalOpenAttemptId, LocalPtyId, LocalSessionAttachRequest,
    LocalSessionAttachResponse, LocalSessionAttachment, LocalSessionAttachmentChange,
    LocalSessionAttachmentHeartbeatRequest, LocalSessionDetachIntent, LocalSessionDetachRequest,
    LocalSessionDetachResult, LocalSessionDetails, LocalSessionEvent, LocalSessionEventPayload,
    LocalSessionExit, LocalSessionFailureCode, LocalSessionFailureReason, LocalSessionGetRequest,
    LocalSessionInputLease, LocalSessionInputLeaseChange, LocalSessionInputLeaseRenewRequest,
    LocalSessionInputRequest, LocalSessionLastDetachAction, LocalSessionOpenRequest,
    LocalSessionOpenResponse, LocalSessionOutputFrame, LocalSessionOutputGap,
    LocalSessionOutputGapReason, LocalSessionOutputItem, LocalSessionResizeRequest,
    LocalSessionSnapshot, LocalSessionState, LocalSessionSummary, LocalSessionTerminateRequest,
    LocalTerminalInputFocusTarget, RequestId, RetryStrategy, WireSequence,
};
use tauri::ipc::Channel;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    local_terminal_platform::{
        LocalPtyExit, LocalPtyMetadata, LocalPtyProcess, LocalPtySpawnError, LocalPtySpawnFailure,
        LocalPtyWriter,
    },
    native_terminal::NativeTerminalService,
    ssh_operation_ledger::{SshOperationLedger, SshOperationLookup, StoredSshOperationResult},
};

use super::{
    ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS, ActorResult, CONTROL_OPERATION_LEDGER_CAPACITY,
    INPUT_LEASE_MILLIS, Message, OUTPUT_RING_MAX_BYTES, PluginSessionMetadataEvent, unix_time_ms,
};

const LOCAL_PROCESS_MAILBOX_CAPACITY: usize = 128;
const LOCAL_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);

enum LocalProcessOwnerSpawnError {
    SpawnFailed(io::Error),
    CleanupFailed {
        source: io::Error,
        process: LocalPtyProcess,
    },
}

#[derive(Clone, PartialEq, Eq)]
enum LocalOperationFingerprint {
    Open {
        open_attempt_id: LocalOpenAttemptId,
        attach_attempt_id: LocalAttachAttemptId,
        view_id: norishell_core_api::LocalViewId,
        rows: u16,
        cols: u16,
    },
    Attach {
        session_id: norishell_core_api::LocalSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
        attach_attempt_id: LocalAttachAttemptId,
        view_id: norishell_core_api::LocalViewId,
        after_output_seq: Option<WireSequence>,
    },
    Detach {
        session_id: norishell_core_api::LocalSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
        attachment_id: LocalAttachmentId,
        view_id: norishell_core_api::LocalViewId,
        intent: LocalSessionDetachIntent,
        confirmation: Option<norishell_core_api::LocalSessionLastDetachConfirmation>,
    },
    Terminate {
        session_id: norishell_core_api::LocalSessionId,
        expected_generation: WireSequence,
        expected_state_revision: WireSequence,
    },
}

impl From<&LocalSessionOpenRequest> for LocalOperationFingerprint {
    fn from(request: &LocalSessionOpenRequest) -> Self {
        Self::Open {
            open_attempt_id: request.open_attempt_id.clone(),
            attach_attempt_id: request.attach_attempt_id.clone(),
            view_id: request.view_id.clone(),
            rows: request.rows,
            cols: request.cols,
        }
    }
}

impl From<&LocalSessionAttachRequest> for LocalOperationFingerprint {
    fn from(request: &LocalSessionAttachRequest) -> Self {
        Self::Attach {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            attach_attempt_id: request.attach_attempt_id.clone(),
            view_id: request.view_id.clone(),
            after_output_seq: request.after_output_seq,
        }
    }
}

impl From<&LocalSessionDetachRequest> for LocalOperationFingerprint {
    fn from(request: &LocalSessionDetachRequest) -> Self {
        Self::Detach {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            attachment_id: request.attachment_id.clone(),
            view_id: request.view_id.clone(),
            intent: request.intent,
            confirmation: request.confirmation.clone(),
        }
    }
}

impl From<&LocalSessionTerminateRequest> for LocalOperationFingerprint {
    fn from(request: &LocalSessionTerminateRequest) -> Self {
        Self::Terminate {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
        }
    }
}

pub(super) enum LocalProcessCommand {
    Input {
        bytes: Vec<u8>,
        completion: oneshot::Sender<bool>,
    },
    Resize {
        rows: u16,
        cols: u16,
        completion: oneshot::Sender<bool>,
    },
    Terminate,
    FailAndTerminate(LocalSessionFailureReason),
}

enum LocalWriterCommand {
    Input {
        bytes: Vec<u8>,
        completion: oneshot::Sender<bool>,
    },
}

struct LocalAttachmentRecord {
    summary: LocalSessionAttachment,
    events: Channel<LocalSessionEvent>,
    last_seen_at_unix_ms: i64,
}

struct LocalSessionRecord {
    summary: LocalSessionSummary,
    attachments: BTreeMap<String, LocalAttachmentRecord>,
    input_lease: Option<LocalSessionInputLease>,
    next_input_epoch: u64,
    last_client_seq: u64,
    last_resize_seq: u64,
    next_output_seq: u64,
    output_ring: VecDeque<LocalSessionOutputItem>,
    output_ring_bytes: usize,
    process: Option<std_mpsc::SyncSender<LocalProcessCommand>>,
    orphan_process: Option<LocalPtyProcess>,
    input_write_poisoned: bool,
    // Owner exit and reader output race through separate threads. Publish the
    // terminal exit only after the reader has queued every preceding byte.
    pending_exit: Option<LocalSessionExit>,
    reader_drained: bool,
    stop_on_ready: bool,
    detach_after_exit: Option<LocalAttachmentId>,
    plugin_metadata_events: broadcast::Sender<PluginSessionMetadataEvent>,
}

pub(super) struct LocalSessions {
    sessions: BTreeMap<String, LocalSessionRecord>,
    snapshot_revision: u64,
    open_operations: SshOperationLedger<
        LocalOperationFingerprint,
        StoredSshOperationResult<LocalSessionOpenResponse>,
    >,
    attach_operations: SshOperationLedger<
        LocalOperationFingerprint,
        StoredSshOperationResult<LocalSessionAttachResponse>,
    >,
    detach_operations: SshOperationLedger<
        LocalOperationFingerprint,
        StoredSshOperationResult<LocalSessionDetachResult>,
    >,
    terminate_operations: SshOperationLedger<
        LocalOperationFingerprint,
        StoredSshOperationResult<LocalSessionSummary>,
    >,
    live_sessions: Arc<Mutex<BTreeSet<String>>>,
    native_terminal: NativeTerminalService,
    plugin_metadata_events: broadcast::Sender<PluginSessionMetadataEvent>,
}

impl LocalSessions {
    pub(super) fn new(
        live_sessions: Arc<Mutex<BTreeSet<String>>>,
        native_terminal: NativeTerminalService,
        plugin_metadata_events: broadcast::Sender<PluginSessionMetadataEvent>,
    ) -> Self {
        Self {
            sessions: BTreeMap::new(),
            snapshot_revision: 0,
            open_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            attach_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            detach_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            terminate_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            live_sessions,
            native_terminal,
            plugin_metadata_events,
        }
    }

    #[cfg(test)]
    pub(super) fn insert_resize_test_session(
        &mut self,
        process: std_mpsc::SyncSender<LocalProcessCommand>,
        focus_epoch: u64,
    ) -> (LocalSessionResizeRequest, LocalTerminalInputFocusTarget) {
        let session_id = norishell_core_api::LocalSessionId::new();
        let attachment_id = LocalAttachmentId::new();
        let attach_attempt_id = LocalAttachAttemptId::new();
        let view_id = norishell_core_api::LocalViewId::new();
        let pty_id = LocalPtyId::new();
        let lease_id = LocalInputLeaseId::new();
        let generation = WireSequence::new(1);
        let state_revision = WireSequence::new(2);
        let attachment_revision = WireSequence::new(1);
        let now = unix_time_ms();
        let attachment = LocalSessionAttachment {
            attachment_id: attachment_id.clone(),
            attach_attempt_id,
            session_id: session_id.clone(),
            generation,
            pty_id: Some(pty_id.clone()),
            view_id: view_id.clone(),
            state_revision,
            attachment_revision,
            attached_at_unix_ms: now,
        };
        let lease = LocalSessionInputLease {
            lease_id: lease_id.clone(),
            session_id: session_id.clone(),
            generation,
            attachment_id: attachment_id.clone(),
            view_id: view_id.clone(),
            focus_epoch: WireSequence::new(focus_epoch),
            input_epoch: WireSequence::new(1),
            expires_at_unix_ms: now.saturating_add(INPUT_LEASE_MILLIS),
        };
        self.sessions.insert(
            session_id.as_str().to_owned(),
            LocalSessionRecord {
                summary: LocalSessionSummary {
                    session_id: session_id.clone(),
                    open_attempt_id: LocalOpenAttemptId::new(),
                    shell_name: "test-shell".to_owned(),
                    generation,
                    state_revision,
                    attachment_revision,
                    event_seq: WireSequence::new(0),
                    pty_id: Some(pty_id.clone()),
                    state: LocalSessionState::Running,
                    exit: None,
                    failure_reason: None,
                    attachment_count: 1,
                    created_at_unix_ms: now,
                    updated_at_unix_ms: now,
                },
                attachments: BTreeMap::from([(
                    attachment_id.as_str().to_owned(),
                    LocalAttachmentRecord {
                        summary: attachment,
                        events: Channel::new(|_| Ok(())),
                        last_seen_at_unix_ms: now,
                    },
                )]),
                input_lease: Some(lease),
                next_input_epoch: 1,
                last_client_seq: 0,
                last_resize_seq: 0,
                next_output_seq: 1,
                output_ring: VecDeque::new(),
                output_ring_bytes: 0,
                process: Some(process),
                orphan_process: None,
                input_write_poisoned: false,
                pending_exit: None,
                reader_drained: false,
                stop_on_ready: false,
                detach_after_exit: None,
                plugin_metadata_events: self.plugin_metadata_events.clone(),
            },
        );
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.as_str().to_owned());

        let target = LocalTerminalInputFocusTarget {
            session_id: session_id.clone(),
            expected_generation: generation,
            expected_state_revision: state_revision,
            pty_id: pty_id.clone(),
            attachment_id: attachment_id.clone(),
            view_id: view_id.clone(),
        };
        let request = LocalSessionResizeRequest {
            meta: norishell_core_api::RequestMeta {
                request_id: RequestId::new(),
            },
            session_id,
            expected_generation: generation,
            expected_state_revision: state_revision,
            pty_id,
            attachment_id,
            view_id,
            lease_id,
            focus_epoch: WireSequence::new(focus_epoch),
            input_epoch: WireSequence::new(1),
            resize_seq: WireSequence::new(1),
            rows: 43,
            cols: 133,
        };
        (request, target)
    }

    #[cfg(test)]
    pub(super) fn resize_seq_for_test(&self, session_id: &str) -> Option<u64> {
        self.sessions
            .get(session_id)
            .map(|record| record.last_resize_seq)
    }

    pub(super) fn snapshot(&self) -> LocalSessionSnapshot {
        LocalSessionSnapshot {
            snapshot_revision: WireSequence::new(self.snapshot_revision),
            sessions: self
                .sessions
                .values()
                .map(|record| record.summary.clone())
                .collect(),
        }
    }

    pub(super) fn live_sessions_empty(&self) -> bool {
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub(super) fn shutdown_all(&mut self) -> io::Result<()> {
        let live_session_ids = self
            .live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut first_error = None;
        for session_id in live_session_ids {
            let mut remove_live = false;
            let Some(record) = self.sessions.get_mut(&session_id) else {
                self.remove_live(&session_id);
                continue;
            };
            record.input_lease = None;
            record.detach_after_exit = None;
            if let Some(process) = record.orphan_process.as_mut() {
                match process.terminate() {
                    Ok(exit) => {
                        record.orphan_process = None;
                        match exit_to_wire(exit) {
                            Ok(exit) => finalize_exit(record, exit),
                            Err(reason) => transition_record(
                                record,
                                LocalSessionState::Failed,
                                None,
                                Some(reason),
                            ),
                        }
                        remove_live = true;
                    }
                    Err(error) => {
                        first_error.get_or_insert(error);
                        transition_record(
                            record,
                            LocalSessionState::Failed,
                            None,
                            Some(failure(
                                LocalSessionFailureCode::ProcessCleanupFailed,
                                "errors.localTerminal.processCleanupFailed",
                                None,
                            )),
                        );
                    }
                }
            } else if let Some(process) = &record.process {
                if let Err(error) = process.try_send(LocalProcessCommand::Terminate) {
                    first_error.get_or_insert_with(|| {
                        io::Error::other(format!(
                            "local PTY shutdown command was not accepted: {error}"
                        ))
                    });
                } else {
                    transition_record(record, LocalSessionState::Stopping, None, None);
                }
            } else {
                record.stop_on_ready = true;
                transition_record(record, LocalSessionState::Stopping, None, None);
            }
            if remove_live {
                self.remove_live(&session_id);
            }
        }
        self.bump_snapshot();
        first_error.map_or(Ok(()), Err)
    }

    pub(super) fn get(&self, request: LocalSessionGetRequest) -> ActorResult<LocalSessionDetails> {
        let record = self
            .sessions
            .get(request.session_id.as_str())
            .ok_or_else(|| local_not_found(request.meta.request_id))?;
        Ok(details_from(record))
    }

    pub(super) fn open(
        &mut self,
        request: LocalSessionOpenRequest,
        events: Channel<LocalSessionEvent>,
        actor_tx: mpsc::Sender<Message>,
    ) -> ActorResult<LocalSessionOpenResponse> {
        if request.idempotency_key.trim().is_empty() || request.rows == 0 || request.cols == 0 {
            return Err(local_validation(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = LocalOperationFingerprint::from(&request);
        if let Some(result) = replay_operation(
            &self.open_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }

        let result = self.open_once(request, events, actor_tx);
        record_operation(
            &mut self.open_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn open_once(
        &mut self,
        request: LocalSessionOpenRequest,
        events: Channel<LocalSessionEvent>,
        actor_tx: mpsc::Sender<Message>,
    ) -> ActorResult<LocalSessionOpenResponse> {
        let request_id = request.meta.request_id.clone();
        let session_id = norishell_core_api::LocalSessionId::new();
        let attachment_id = LocalAttachmentId::new();
        let generation = WireSequence::new(1);
        let now = unix_time_ms();
        let summary = LocalSessionSummary {
            session_id: session_id.clone(),
            open_attempt_id: request.open_attempt_id,
            shell_name: String::new(),
            generation,
            state_revision: WireSequence::new(1),
            attachment_revision: WireSequence::new(1),
            event_seq: WireSequence::new(0),
            pty_id: None,
            state: LocalSessionState::Starting,
            exit: None,
            failure_reason: None,
            attachment_count: 1,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let attachment = LocalSessionAttachment {
            attachment_id: attachment_id.clone(),
            attach_attempt_id: request.attach_attempt_id,
            session_id: session_id.clone(),
            generation,
            pty_id: None,
            view_id: request.view_id,
            state_revision: summary.state_revision,
            attachment_revision: summary.attachment_revision,
            attached_at_unix_ms: now,
        };
        self.sessions.insert(
            session_id.as_str().to_owned(),
            LocalSessionRecord {
                summary: summary.clone(),
                attachments: BTreeMap::from([(
                    attachment_id.as_str().to_owned(),
                    LocalAttachmentRecord {
                        summary: attachment.clone(),
                        events,
                        last_seen_at_unix_ms: now,
                    },
                )]),
                input_lease: None,
                next_input_epoch: 0,
                last_client_seq: 0,
                last_resize_seq: 0,
                next_output_seq: 1,
                output_ring: VecDeque::new(),
                output_ring_bytes: 0,
                process: None,
                orphan_process: None,
                input_write_poisoned: false,
                pending_exit: None,
                reader_drained: false,
                stop_on_ready: false,
                detach_after_exit: None,
                plugin_metadata_events: self.plugin_metadata_events.clone(),
            },
        );
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.as_str().to_owned());
        self.bump_snapshot();

        let session_id_text = session_id.as_str().to_owned();
        let rows = request.rows;
        let cols = request.cols;
        if std::thread::Builder::new()
            .name(format!("local-pty-spawn-{}", session_id.as_str()))
            .spawn(move || match LocalPtyProcess::spawn(rows, cols) {
                Ok((metadata, process)) => {
                    let _ = actor_tx.blocking_send(Message::LocalProcessReady {
                        session_id: session_id_text,
                        generation: 1,
                        metadata,
                        process,
                    });
                }
                Err(error) => {
                    let _ = actor_tx.blocking_send(Message::LocalProcessFailed {
                        session_id: session_id_text,
                        generation: 1,
                        failure: spawn_failure(error),
                    });
                }
            })
            .is_err()
        {
            self.sessions.remove(session_id.as_str());
            self.remove_live(session_id.as_str());
            self.bump_snapshot();
            return Err(local_unavailable(request_id));
        }

        Ok(LocalSessionOpenResponse {
            session: summary,
            attachment,
        })
    }

    pub(super) fn attach(
        &mut self,
        request: LocalSessionAttachRequest,
        events: Channel<LocalSessionEvent>,
    ) -> ActorResult<LocalSessionAttachResponse> {
        if request.idempotency_key.trim().is_empty() {
            return Err(local_validation(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = LocalOperationFingerprint::from(&request);
        if let Some(result) = replay_operation(
            &self.attach_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.attach_once(request, events);
        record_operation(
            &mut self.attach_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn attach_once(
        &mut self,
        request: LocalSessionAttachRequest,
        events: Channel<LocalSessionEvent>,
    ) -> ActorResult<LocalSessionAttachResponse> {
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| local_not_found(request.meta.request_id.clone()))?;
        if record.summary.generation != request.expected_generation
            || record.summary.state_revision != request.expected_state_revision
        {
            return Err(local_conflict(request.meta.request_id));
        }
        if record
            .attachments
            .values()
            .any(|attachment| attachment.summary.attach_attempt_id == request.attach_attempt_id)
        {
            return Err(local_conflict(request.meta.request_id));
        }
        let now = unix_time_ms();
        let replaced = record
            .attachments
            .iter()
            .find(|(_, attachment)| attachment.summary.view_id == request.view_id)
            .map(|(key, _)| key.clone())
            .and_then(|key| record.attachments.remove(&key));
        if let Some(replaced) = &replaced
            && record
                .input_lease
                .as_ref()
                .is_some_and(|lease| lease.attachment_id == replaced.summary.attachment_id)
        {
            record.input_lease = None;
            emit_payload(
                record,
                LocalSessionEventPayload::InputLeaseChanged {
                    change: LocalSessionInputLeaseChange::Released,
                    lease: None,
                },
            );
        }
        let detached_revision = replaced
            .as_ref()
            .map(|_| WireSequence::new(record.summary.attachment_revision.get().saturating_add(1)));
        let next_revision = detached_revision
            .unwrap_or(record.summary.attachment_revision)
            .get()
            .saturating_add(1);
        let attachment = LocalSessionAttachment {
            attachment_id: LocalAttachmentId::new(),
            attach_attempt_id: request.attach_attempt_id,
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            pty_id: record.summary.pty_id.clone(),
            view_id: request.view_id,
            state_revision: record.summary.state_revision,
            attachment_revision: WireSequence::new(next_revision),
            attached_at_unix_ms: now,
        };
        record.attachments.insert(
            attachment.attachment_id.as_str().to_owned(),
            LocalAttachmentRecord {
                summary: attachment.clone(),
                events,
                last_seen_at_unix_ms: now,
            },
        );
        record.summary.attachment_revision = WireSequence::new(next_revision);
        record.summary.attachment_count = record.attachments.len() as u32;
        let replay = replay_after(record, request.after_output_seq);
        if let (Some(old), Some(revision)) = (replaced, detached_revision) {
            emit_payload(
                record,
                LocalSessionEventPayload::AttachmentChanged {
                    change: LocalSessionAttachmentChange::Detached,
                    attachment_revision: revision,
                    attachment: old.summary,
                },
            );
        }
        emit_payload(
            record,
            LocalSessionEventPayload::AttachmentChanged {
                change: LocalSessionAttachmentChange::Attached,
                attachment_revision: WireSequence::new(next_revision),
                attachment: attachment.clone(),
            },
        );
        let response = LocalSessionAttachResponse {
            state_revision: record.summary.state_revision,
            attachment_revision: record.summary.attachment_revision,
            attachment,
            replay,
        };
        self.bump_snapshot();
        Ok(response)
    }

    pub(super) fn attachment_heartbeat(
        &mut self,
        request: LocalSessionAttachmentHeartbeatRequest,
    ) -> ActorResult<LocalSessionAttachment> {
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| local_not_found(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        let attachment = record
            .attachments
            .get_mut(request.attachment_id.as_str())
            .expect("validated local attachment");
        if attachment.summary.attachment_revision != request.expected_attachment_revision {
            return Err(local_conflict(request.meta.request_id));
        }
        attachment.last_seen_at_unix_ms = unix_time_ms();
        Ok(attachment.summary.clone())
    }

    pub(super) fn detach(
        &mut self,
        request: LocalSessionDetachRequest,
    ) -> ActorResult<LocalSessionDetachResult> {
        if request.idempotency_key.trim().is_empty() {
            return Err(local_validation(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = LocalOperationFingerprint::from(&request);
        if let Some(result) = replay_operation(
            &self.detach_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.detach_once(request);
        record_operation(
            &mut self.detach_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn detach_once(
        &mut self,
        request: LocalSessionDetachRequest,
    ) -> ActorResult<LocalSessionDetachResult> {
        let session_id = request.session_id.as_str().to_owned();
        let record = self
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| local_not_found(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        if record.summary.state_revision != request.expected_state_revision {
            return Err(local_conflict(request.meta.request_id));
        }
        if request.intent == LocalSessionDetachIntent::RendererUnavailable {
            if request.confirmation.is_some() {
                return Err(local_validation(request.meta.request_id));
            }
            remove_attachment(record, &request.attachment_id)
                .ok_or_else(|| local_not_found(request.meta.request_id.clone()))?;
            let result = LocalSessionDetachResult::Detached {
                session: record.summary.clone(),
                remaining_attachment_count: record.summary.attachment_count,
            };
            self.bump_snapshot();
            return Ok(result);
        }

        let is_last = record.attachments.len() == 1;
        let active = matches!(
            record.summary.state,
            LocalSessionState::Starting | LocalSessionState::Running | LocalSessionState::Stopping
        );
        if is_last && active && request.confirmation.is_none() {
            return Ok(LocalSessionDetachResult::ConfirmationRequired {
                session: record.summary.clone(),
                expected_state_revision: record.summary.state_revision,
                expected_attachment_revision: record.summary.attachment_revision,
            });
        }
        if let Some(confirmation) = &request.confirmation {
            if confirmation.expected_state_revision != record.summary.state_revision
                || confirmation.expected_attachment_revision != record.summary.attachment_revision
            {
                return Err(local_conflict(request.meta.request_id));
            }
            if confirmation.action == LocalSessionLastDetachAction::KeepAttached {
                return Ok(LocalSessionDetachResult::KeptAttached {
                    session: record.summary.clone(),
                });
            }
            record.detach_after_exit = Some(request.attachment_id.clone());
            if let Some(process) = &record.process {
                process
                    .try_send(LocalProcessCommand::Terminate)
                    .map_err(|_| local_unavailable(request.meta.request_id.clone()))?;
            } else {
                record.stop_on_ready = true;
            }
            transition_record(record, LocalSessionState::Stopping, None, None);
            let result = LocalSessionDetachResult::Stopping {
                session: record.summary.clone(),
            };
            self.bump_snapshot();
            return Ok(result);
        }

        remove_attachment(record, &request.attachment_id)
            .ok_or_else(|| local_not_found(request.meta.request_id.clone()))?;
        let result = LocalSessionDetachResult::Detached {
            session: record.summary.clone(),
            remaining_attachment_count: record.summary.attachment_count,
        };
        self.bump_snapshot();
        Ok(result)
    }

    pub(super) fn terminate(
        &mut self,
        request: LocalSessionTerminateRequest,
    ) -> ActorResult<LocalSessionSummary> {
        if request.idempotency_key.trim().is_empty() {
            return Err(local_validation(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = LocalOperationFingerprint::from(&request);
        if let Some(result) = replay_operation(
            &self.terminate_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.terminate_once(request);
        record_operation(
            &mut self.terminate_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn terminate_once(
        &mut self,
        request: LocalSessionTerminateRequest,
    ) -> ActorResult<LocalSessionSummary> {
        let session_id = request.session_id.as_str().to_owned();
        let (summary, changed, exited) = {
            let record = self
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| local_not_found(request.meta.request_id.clone()))?;
            if record.summary.generation != request.expected_generation
                || record.summary.state_revision != request.expected_state_revision
            {
                return Err(local_conflict(request.meta.request_id));
            }
            if matches!(
                record.summary.state,
                LocalSessionState::Exited | LocalSessionState::Closed
            ) {
                return Ok(record.summary.clone());
            }
            if record.orphan_process.is_some() {
                match terminate_orphan_process(&mut record.orphan_process) {
                    Ok(exit) => {
                        record.stop_on_ready = false;
                        let exit = exit_to_wire(exit).map_err(|reason| {
                            transition_record(
                                record,
                                LocalSessionState::Failed,
                                None,
                                Some(reason),
                            );
                            local_unavailable(request.meta.request_id.clone())
                        })?;
                        finalize_exit(record, exit);
                        (record.summary.clone(), true, true)
                    }
                    Err(error) => {
                        transition_record(
                            record,
                            LocalSessionState::Failed,
                            None,
                            Some(failure(
                                LocalSessionFailureCode::ProcessCleanupFailed,
                                "errors.localTerminal.processCleanupFailed",
                                Some(error),
                            )),
                        );
                        (record.summary.clone(), true, false)
                    }
                }
            } else {
                let changed = record.summary.state != LocalSessionState::Stopping;
                if changed {
                    if let Some(process) = &record.process {
                        process
                            .try_send(LocalProcessCommand::Terminate)
                            .map_err(|_| local_unavailable(request.meta.request_id.clone()))?;
                    } else {
                        record.stop_on_ready = true;
                    }
                    transition_record(record, LocalSessionState::Stopping, None, None);
                }
                (record.summary.clone(), changed, false)
            }
        };
        if exited {
            self.remove_live(&session_id);
        }
        if changed {
            self.bump_snapshot();
        }
        Ok(summary)
    }

    pub(super) fn same_view_target(
        &self,
        request: &LocalSessionAttachRequest,
        target: &LocalTerminalInputFocusTarget,
    ) -> bool {
        target.session_id == request.session_id && target.view_id == request.view_id
    }

    pub(super) fn detach_target(
        &self,
        request: &LocalSessionDetachRequest,
        target: &LocalTerminalInputFocusTarget,
    ) -> bool {
        target.session_id == request.session_id
            && target.attachment_id == request.attachment_id
            && target.view_id == request.view_id
    }

    pub(super) fn validate_focus_target(
        &self,
        target: &LocalTerminalInputFocusTarget,
        request_id: &RequestId,
    ) -> ActorResult<()> {
        let record = self
            .sessions
            .get(target.session_id.as_str())
            .ok_or_else(|| local_not_found(request_id.clone()))?;
        validate_attachment(
            record,
            target.expected_generation,
            &target.attachment_id,
            &target.view_id,
            request_id,
        )?;
        if record.summary.state != LocalSessionState::Running
            || record.summary.state_revision != target.expected_state_revision
            || record.summary.pty_id.as_ref() != Some(&target.pty_id)
        {
            return Err(local_conflict(request_id.clone()));
        }
        Ok(())
    }

    pub(super) fn acquire_focus(
        &mut self,
        target: &LocalTerminalInputFocusTarget,
        focus_epoch: WireSequence,
    ) -> LocalSessionInputLease {
        let record = self
            .sessions
            .get_mut(target.session_id.as_str())
            .expect("local focus target was validated");
        record.next_input_epoch = record.next_input_epoch.saturating_add(1);
        let lease = LocalSessionInputLease {
            lease_id: LocalInputLeaseId::new(),
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            attachment_id: target.attachment_id.clone(),
            view_id: target.view_id.clone(),
            focus_epoch,
            input_epoch: WireSequence::new(record.next_input_epoch),
            expires_at_unix_ms: unix_time_ms().saturating_add(INPUT_LEASE_MILLIS),
        };
        record.input_lease = Some(lease.clone());
        record.last_client_seq = 0;
        record.last_resize_seq = 0;
        emit_payload(
            record,
            LocalSessionEventPayload::InputLeaseChanged {
                change: LocalSessionInputLeaseChange::Acquired,
                lease: Some(lease.clone()),
            },
        );
        lease
    }

    pub(super) fn focused_lease(
        &self,
        target: &LocalTerminalInputFocusTarget,
        focus_epoch: u64,
    ) -> Option<LocalSessionInputLease> {
        self.sessions
            .get(target.session_id.as_str())
            .and_then(|record| record.input_lease.clone())
            .filter(|lease| lease.focus_epoch.get() == focus_epoch)
    }

    pub(super) fn revoke_focus(&mut self, target: &LocalTerminalInputFocusTarget) {
        let Some(record) = self.sessions.get_mut(target.session_id.as_str()) else {
            return;
        };
        if record.input_lease.take().is_some() {
            emit_payload(
                record,
                LocalSessionEventPayload::InputLeaseChanged {
                    change: LocalSessionInputLeaseChange::Released,
                    lease: None,
                },
            );
        }
    }

    pub(super) fn lease_renew(
        &mut self,
        request: LocalSessionInputLeaseRenewRequest,
        target: &LocalTerminalInputFocusTarget,
        current_focus_epoch: u64,
    ) -> ActorResult<LocalSessionInputLease> {
        self.validate_focus_fence(
            &request.session_id,
            request.expected_generation,
            request.expected_state_revision,
            &request.pty_id,
            &request.attachment_id,
            &request.view_id,
            request.focus_epoch,
            target,
            current_focus_epoch,
            &request.meta.request_id,
        )?;
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .expect("validated local focus record");
        validate_lease(
            record,
            &request.lease_id,
            request.input_epoch,
            &request.meta.request_id,
            false,
        )?;
        let lease = record.input_lease.as_mut().expect("validated local lease");
        lease.expires_at_unix_ms = unix_time_ms().saturating_add(INPUT_LEASE_MILLIS);
        let lease = lease.clone();
        emit_payload(
            record,
            LocalSessionEventPayload::InputLeaseChanged {
                change: LocalSessionInputLeaseChange::Renewed,
                lease: Some(lease.clone()),
            },
        );
        Ok(lease)
    }

    pub(super) fn input(
        &mut self,
        request: LocalSessionInputRequest,
        target: &LocalTerminalInputFocusTarget,
        current_focus_epoch: u64,
    ) -> ActorResult<oneshot::Receiver<bool>> {
        if request.bytes.is_empty() || request.bytes.len() > LOCAL_TERMINAL_OUTPUT_FRAME_MAX_BYTES {
            return Err(local_validation(request.meta.request_id));
        }
        self.validate_focus_fence(
            &request.session_id,
            request.expected_generation,
            request.expected_state_revision,
            &request.pty_id,
            &request.attachment_id,
            &request.view_id,
            request.focus_epoch,
            target,
            current_focus_epoch,
            &request.meta.request_id,
        )?;
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .expect("validated local input record");
        validate_lease(
            record,
            &request.lease_id,
            request.input_epoch,
            &request.meta.request_id,
            true,
        )?;
        if request.client_seq.get() <= record.last_client_seq {
            return Err(local_conflict(request.meta.request_id));
        }
        let (completion, completed) = oneshot::channel();
        record
            .process
            .as_ref()
            .ok_or_else(|| local_unavailable(request.meta.request_id.clone()))?
            .try_send(LocalProcessCommand::Input {
                bytes: request.bytes,
                completion,
            })
            .map_err(|_| local_unavailable(request.meta.request_id))?;
        record.last_client_seq = request.client_seq.get();
        Ok(completed)
    }

    /// Marks an acknowledged write failure or delivery-uncertain timeout as a
    /// terminal generation failure immediately. The PTY owner remains tracked
    /// until cleanup is confirmed, so application exit cannot hide a process
    /// whose write result is unknown.
    pub(super) fn poison_input(&mut self, session_id: &str, generation: u64) -> bool {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return false;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != LocalSessionState::Running
        {
            return false;
        }
        let reason = failure(
            LocalSessionFailureCode::PtyWriteFailed,
            "errors.localTerminal.ptyWriteFailed",
            None,
        );
        if let Some(process) = &record.process {
            let _ = process.try_send(LocalProcessCommand::FailAndTerminate(reason.clone()));
        }
        record.pending_exit = None;
        record.input_lease = None;
        record.input_write_poisoned = true;
        transition_record(record, LocalSessionState::Failed, None, Some(reason));
        self.bump_snapshot();
        true
    }

    pub(super) fn resize(
        &mut self,
        request: LocalSessionResizeRequest,
        target: &LocalTerminalInputFocusTarget,
        current_focus_epoch: u64,
    ) -> ActorResult<oneshot::Receiver<bool>> {
        if request.rows == 0 || request.cols == 0 {
            return Err(local_validation(request.meta.request_id));
        }
        self.validate_focus_fence(
            &request.session_id,
            request.expected_generation,
            request.expected_state_revision,
            &request.pty_id,
            &request.attachment_id,
            &request.view_id,
            request.focus_epoch,
            target,
            current_focus_epoch,
            &request.meta.request_id,
        )?;
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .expect("validated local resize record");
        validate_lease(
            record,
            &request.lease_id,
            request.input_epoch,
            &request.meta.request_id,
            true,
        )?;
        if request.resize_seq.get() <= record.last_resize_seq {
            return Err(local_conflict(request.meta.request_id));
        }
        let (completion, completed) = oneshot::channel();
        record
            .process
            .as_ref()
            .ok_or_else(|| local_unavailable(request.meta.request_id.clone()))?
            .try_send(LocalProcessCommand::Resize {
                rows: request.rows,
                cols: request.cols,
                completion,
            })
            .map_err(|_| local_unavailable(request.meta.request_id))?;
        Ok(completed)
    }

    pub(super) fn commit_resize(
        &mut self,
        session_id: &str,
        generation: u64,
        resize_seq: u64,
    ) -> bool {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return false;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != LocalSessionState::Running
            || resize_seq <= record.last_resize_seq
        {
            return false;
        }
        record.last_resize_seq = resize_seq;
        true
    }

    pub(super) fn poison_resize(&mut self, session_id: &str, generation: u64) -> bool {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return false;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != LocalSessionState::Running
        {
            return false;
        }
        let reason = failure(
            LocalSessionFailureCode::PtyResizeFailed,
            "errors.localTerminal.ptyResizeFailed",
            None,
        );
        if let Some(process) = &record.process {
            let _ = process.try_send(LocalProcessCommand::FailAndTerminate(reason.clone()));
        }
        record.pending_exit = None;
        record.input_lease = None;
        record.input_write_poisoned = true;
        transition_record(record, LocalSessionState::Failed, None, Some(reason));
        self.bump_snapshot();
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_focus_fence(
        &self,
        session_id: &norishell_core_api::LocalSessionId,
        generation: WireSequence,
        state_revision: WireSequence,
        pty_id: &LocalPtyId,
        attachment_id: &LocalAttachmentId,
        view_id: &norishell_core_api::LocalViewId,
        focus_epoch: WireSequence,
        target: &LocalTerminalInputFocusTarget,
        current_focus_epoch: u64,
        request_id: &RequestId,
    ) -> ActorResult<()> {
        if focus_epoch.get() != current_focus_epoch
            || target.session_id != *session_id
            || target.expected_generation != generation
            || target.expected_state_revision != state_revision
            || target.pty_id != *pty_id
            || target.attachment_id != *attachment_id
            || target.view_id != *view_id
        {
            return Err(local_stale_focus(request_id.clone()));
        }
        self.validate_focus_target(target, request_id)
            .map_err(|_| local_stale_focus(request_id.clone()))
    }

    pub(super) fn focus_is_stale(
        &self,
        target: &LocalTerminalInputFocusTarget,
        now_unix_ms: i64,
    ) -> bool {
        self.sessions
            .get(target.session_id.as_str())
            .is_none_or(|record| {
                record.input_lease.as_ref().is_none_or(|lease| {
                    lease.expires_at_unix_ms <= now_unix_ms
                        || record
                            .attachments
                            .get(target.attachment_id.as_str())
                            .is_none_or(|attachment| {
                                now_unix_ms.saturating_sub(attachment.last_seen_at_unix_ms)
                                    >= ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS
                            })
                })
            })
    }

    pub(super) fn reap_stale_attachments(&mut self, now_unix_ms: i64) {
        let mut changed = false;
        for record in self.sessions.values_mut() {
            if record
                .input_lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_unix_ms <= now_unix_ms)
            {
                record.input_lease = None;
                emit_payload(
                    record,
                    LocalSessionEventPayload::InputLeaseChanged {
                        change: LocalSessionInputLeaseChange::Released,
                        lease: None,
                    },
                );
            }
            let stale = record
                .attachments
                .values()
                .filter(|attachment| {
                    now_unix_ms.saturating_sub(attachment.last_seen_at_unix_ms)
                        >= ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS
                })
                .map(|attachment| attachment.summary.attachment_id.clone())
                .collect::<Vec<_>>();
            for attachment_id in stale {
                changed |= remove_attachment(record, &attachment_id).is_some();
            }
        }
        if changed {
            self.bump_snapshot();
        }
    }

    pub(super) fn process_ready(
        &mut self,
        session_id: String,
        generation: u64,
        metadata: LocalPtyMetadata,
        mut process: LocalPtyProcess,
        actor_tx: mpsc::Sender<Message>,
    ) -> bool {
        let valid = self.sessions.get(&session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && matches!(
                    record.summary.state,
                    LocalSessionState::Starting | LocalSessionState::Stopping
                )
        });
        if !valid {
            std::thread::spawn(move || {
                let _ = process.terminate();
            });
            return false;
        }
        let reader = match process.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                let reason = failure(
                    LocalSessionFailureCode::PtyReadFailed,
                    "errors.localTerminal.ptyReadFailed",
                    Some(error),
                );
                let (commands, command_rx) =
                    std_mpsc::sync_channel::<LocalProcessCommand>(LOCAL_PROCESS_MAILBOX_CAPACITY);
                {
                    let record = self
                        .sessions
                        .get_mut(&session_id)
                        .expect("validated local session");
                    record.process = Some(commands.clone());
                    // Reader creation failed before any byte could be consumed,
                    // so an eventual owner exit needs no additional drain fact.
                    record.reader_drained = true;
                }
                if let Err(error) = spawn_process_owner(
                    actor_tx,
                    session_id.clone(),
                    generation,
                    process,
                    command_rx,
                ) {
                    self.process_owner_spawn_failed(&session_id, generation, error);
                    return true;
                }
                let _ = commands.try_send(LocalProcessCommand::FailAndTerminate(reason));
                return true;
            }
        };
        let pty_id = LocalPtyId::new();
        let (commands, command_rx) =
            std_mpsc::sync_channel::<LocalProcessCommand>(LOCAL_PROCESS_MAILBOX_CAPACITY);
        let stop_on_ready;
        {
            let record = self
                .sessions
                .get_mut(&session_id)
                .expect("validated local session");
            record.summary.shell_name = metadata.shell_name;
            record.summary.pty_id = Some(pty_id.clone());
            record.process = Some(commands.clone());
            let next_attachment_revision =
                record.summary.attachment_revision.get().saturating_add(1);
            record.summary.attachment_revision = WireSequence::new(next_attachment_revision);
            for attachment in record.attachments.values_mut() {
                attachment.summary.pty_id = Some(pty_id.clone());
                attachment.summary.state_revision =
                    WireSequence::new(record.summary.state_revision.get().saturating_add(1));
                attachment.summary.attachment_revision =
                    WireSequence::new(next_attachment_revision);
            }
            stop_on_ready = record.stop_on_ready;
            if !stop_on_ready {
                transition_record(record, LocalSessionState::Running, None, None);
            }
            let attachments = record
                .attachments
                .values()
                .map(|attachment| attachment.summary.clone())
                .collect::<Vec<_>>();
            for attachment in attachments {
                emit_payload(
                    record,
                    LocalSessionEventPayload::AttachmentChanged {
                        change: LocalSessionAttachmentChange::Attached,
                        attachment_revision: WireSequence::new(next_attachment_revision),
                        attachment,
                    },
                );
            }
        }
        if let Err(error) = spawn_process_owner(
            actor_tx.clone(),
            session_id.clone(),
            generation,
            process,
            command_rx,
        ) {
            self.process_owner_spawn_failed(&session_id, generation, error);
            return true;
        }
        self.bump_snapshot();
        if let Err(error) = spawn_reader(
            actor_tx,
            commands.clone(),
            session_id.clone(),
            generation,
            reader,
        ) {
            let _ = commands.try_send(LocalProcessCommand::FailAndTerminate(failure(
                LocalSessionFailureCode::PtyReadFailed,
                "errors.localTerminal.ptyReadFailed",
                Some(error),
            )));
        }
        if stop_on_ready {
            let _ = commands.try_send(LocalProcessCommand::Terminate);
        }
        true
    }

    pub(super) fn output_can_progress_during_ack(&self, session_id: &str, generation: u64) -> bool {
        self.sessions.get(session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && matches!(
                    record.summary.state,
                    LocalSessionState::Running | LocalSessionState::Stopping
                )
        })
    }

    pub(super) fn process_output(&mut self, session_id: &str, generation: u64, bytes: Vec<u8>) {
        let Some((wire_session_id, pty_id, prompt_input_sequence, prompt_input_epoch)) =
            self.sessions.get(session_id).and_then(|record| {
                Some((
                    record.summary.session_id.clone(),
                    record.summary.pty_id.clone()?,
                    WireSequence::new(record.last_client_seq),
                    record.input_lease.as_ref().map(|lease| lease.input_epoch),
                ))
            })
        else {
            return;
        };
        let bytes = self.native_terminal.filter_local_output(
            &wire_session_id,
            WireSequence::new(generation),
            &pty_id,
            prompt_input_sequence,
            prompt_input_epoch,
            bytes,
        );
        if bytes.is_empty() {
            return;
        }
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation.get() != generation
            || !matches!(
                record.summary.state,
                LocalSessionState::Running | LocalSessionState::Stopping
            )
        {
            return;
        }
        for chunk in bytes.chunks(LOCAL_TERMINAL_OUTPUT_FRAME_MAX_BYTES) {
            let frame = LocalSessionOutputFrame {
                session_id: record.summary.session_id.clone(),
                generation: record.summary.generation,
                pty_id: pty_id.clone(),
                output_seq: WireSequence::new(record.next_output_seq),
                bytes: chunk.to_vec(),
            };
            record.next_output_seq = record.next_output_seq.saturating_add(1);
            record.output_ring_bytes = record.output_ring_bytes.saturating_add(frame.bytes.len());
            record
                .output_ring
                .push_back(LocalSessionOutputItem::Frame(frame.clone()));
            emit_payload(record, LocalSessionEventPayload::OutputFrame { frame });
        }
        trim_output_ring(record);
    }

    pub(super) fn process_exited(
        &mut self,
        session_id: &str,
        generation: u64,
        exit: LocalPtyExit,
    ) -> bool {
        let finalized = {
            let Some(record) = self.sessions.get_mut(session_id) else {
                return false;
            };
            if record.summary.generation.get() == generation
                && record.summary.state == LocalSessionState::Failed
                && record.input_write_poisoned
            {
                record.process = None;
                record.orphan_process = None;
                record.input_write_poisoned = false;
                self.remove_live(session_id);
                self.bump_snapshot();
                return true;
            }
            if record.summary.generation.get() != generation
                || record.pending_exit.is_some()
                || matches!(
                    record.summary.state,
                    LocalSessionState::Exited
                        | LocalSessionState::Failed
                        | LocalSessionState::Closed
                )
            {
                return false;
            }
            record.process = None;
            record.orphan_process = None;
            record.input_write_poisoned = false;
            record.input_lease = None;
            let exit = match exit_to_wire(exit) {
                Ok(exit) => exit,
                Err(reason) => {
                    transition_record(record, LocalSessionState::Failed, None, Some(reason));
                    self.remove_live(session_id);
                    self.bump_snapshot();
                    return true;
                }
            };
            if record.reader_drained {
                finalize_exit(record, exit);
                true
            } else {
                record.pending_exit = Some(exit);
                false
            }
        };
        if finalized {
            self.remove_live(session_id);
            self.bump_snapshot();
        }
        true
    }

    pub(super) fn process_output_drained(&mut self, session_id: &str, generation: u64) -> bool {
        let finalized = {
            let Some(record) = self.sessions.get_mut(session_id) else {
                return false;
            };
            if record.summary.generation.get() != generation
                || matches!(
                    record.summary.state,
                    LocalSessionState::Exited
                        | LocalSessionState::Failed
                        | LocalSessionState::Closed
                )
            {
                return false;
            }
            record.reader_drained = true;
            let Some(exit) = record.pending_exit.take() else {
                return false;
            };
            finalize_exit(record, exit);
            true
        };
        if finalized {
            self.remove_live(session_id);
            self.bump_snapshot();
        }
        finalized
    }

    pub(super) fn process_failed(
        &mut self,
        session_id: &str,
        generation: u64,
        reason: LocalSessionFailureReason,
    ) -> bool {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return false;
        };
        if record.summary.generation.get() != generation {
            return false;
        }
        if record.summary.state == LocalSessionState::Failed && record.input_write_poisoned {
            let cleanup_pending = reason.code == LocalSessionFailureCode::ProcessCleanupFailed;
            if cleanup_pending {
                transition_record(record, LocalSessionState::Failed, None, Some(reason));
            } else {
                record.process = None;
                record.orphan_process = None;
                record.input_write_poisoned = false;
                self.remove_live(session_id);
            }
            self.bump_snapshot();
            return true;
        }
        if matches!(
            record.summary.state,
            LocalSessionState::Exited | LocalSessionState::Failed | LocalSessionState::Closed
        ) {
            return false;
        }
        let cleanup_pending = reason.code == LocalSessionFailureCode::ProcessCleanupFailed;
        if !cleanup_pending {
            record.process = None;
            record.orphan_process = None;
        }
        record.input_write_poisoned = false;
        record.pending_exit = None;
        record.input_lease = None;
        transition_record(record, LocalSessionState::Failed, None, Some(reason));
        if !cleanup_pending {
            self.remove_live(session_id);
        }
        self.bump_snapshot();
        true
    }

    fn process_owner_spawn_failed(
        &mut self,
        session_id: &str,
        generation: u64,
        error: LocalProcessOwnerSpawnError,
    ) {
        let (source, code, message_key, orphan_process) = match error {
            LocalProcessOwnerSpawnError::SpawnFailed(source) => (
                source,
                LocalSessionFailureCode::PtySpawnFailed,
                "errors.localTerminal.ptySpawnFailed",
                None,
            ),
            LocalProcessOwnerSpawnError::CleanupFailed { source, process } => (
                source,
                LocalSessionFailureCode::ProcessCleanupFailed,
                "errors.localTerminal.processCleanupFailed",
                Some(process),
            ),
        };
        if let Some(record) = self.sessions.get_mut(session_id) {
            // The receiver never started, so retaining this sender would falsely
            // advertise a retry path that can never consume another command.
            record.process = None;
            record.orphan_process = orphan_process;
            record.reader_drained = true;
        }
        self.process_failed(
            session_id,
            generation,
            failure(code, message_key, Some(source)),
        );
    }

    fn remove_live(&self, session_id: &str) {
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }

    fn bump_snapshot(&mut self) {
        self.snapshot_revision = self.snapshot_revision.saturating_add(1);
    }
}

fn spawn_reader(
    actor_tx: mpsc::Sender<Message>,
    process_commands: std_mpsc::SyncSender<LocalProcessCommand>,
    session_id: String,
    generation: u64,
    mut reader: std::fs::File,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name(format!("local-pty-reader-{session_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; LOCAL_TERMINAL_OUTPUT_FRAME_MAX_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if actor_tx
                            .blocking_send(Message::LocalProcessOutput {
                                session_id: session_id.clone(),
                                generation,
                                bytes: buffer[..count].to_vec(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if pty_eof_error(&error) => break,
                    Err(error) => {
                        let _ =
                            process_commands.send(LocalProcessCommand::FailAndTerminate(failure(
                                LocalSessionFailureCode::PtyReadFailed,
                                "errors.localTerminal.ptyReadFailed",
                                Some(error),
                            )));
                        break;
                    }
                }
            }
            let _ = actor_tx.blocking_send(Message::LocalProcessOutputDrained {
                session_id,
                generation,
            });
        })
        .map(|_| ())
}

fn spawn_process_owner(
    actor_tx: mpsc::Sender<Message>,
    session_id: String,
    generation: u64,
    process: LocalPtyProcess,
    commands: std_mpsc::Receiver<LocalProcessCommand>,
) -> Result<(), LocalProcessOwnerSpawnError> {
    let writer = match process.try_clone_writer() {
        Ok(writer) => writer,
        Err(error) => return Err(local_owner_setup_failed(process, error)),
    };
    let (writer_commands, writer_rx) =
        std_mpsc::sync_channel::<LocalWriterCommand>(LOCAL_PROCESS_MAILBOX_CAPACITY);
    if let Err(error) = spawn_process_writer(session_id.clone(), writer, writer_rx) {
        return Err(local_owner_setup_failed(process, error));
    }
    let process = Arc::new(Mutex::new(Some(process)));
    let process_for_thread = process.clone();
    match std::thread::Builder::new()
        .name(format!("local-pty-owner-{session_id}"))
        .spawn(move || {
            let mut process = process_for_thread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("local PTY owner starts with one process");
            let mut poll_process = true;
            loop {
                match commands.recv_timeout(LOCAL_PROCESS_POLL_INTERVAL) {
                    Ok(LocalProcessCommand::Input { bytes, completion }) => {
                        let command = LocalWriterCommand::Input { bytes, completion };
                        if let Err(error) = writer_commands.try_send(command) {
                            let LocalWriterCommand::Input { completion, .. } = match error {
                                std_mpsc::TrySendError::Full(command)
                                | std_mpsc::TrySendError::Disconnected(command) => command,
                            };
                            let _ = completion.send(false);
                            if terminate_after_failure(
                                &actor_tx,
                                &session_id,
                                generation,
                                &mut process,
                                failure(
                                    LocalSessionFailureCode::PtyWriteFailed,
                                    "errors.localTerminal.ptyWriteFailed",
                                    Some(io::Error::other(
                                        "local PTY writer command was not accepted",
                                    )),
                                ),
                            ) {
                                break;
                            }
                            poll_process = false;
                        }
                    }
                    Ok(LocalProcessCommand::Resize {
                        rows,
                        cols,
                        completion,
                    }) => {
                        let result = process.resize(rows, cols);
                        let succeeded = result.is_ok();
                        let _ = completion.send(succeeded);
                        if let Err(error) = result {
                            if terminate_after_failure(
                                &actor_tx,
                                &session_id,
                                generation,
                                &mut process,
                                failure(
                                    LocalSessionFailureCode::PtyResizeFailed,
                                    "errors.localTerminal.ptyResizeFailed",
                                    Some(error),
                                ),
                            ) {
                                break;
                            }
                            poll_process = false;
                        }
                    }
                    Ok(LocalProcessCommand::Terminate) => match process.terminate() {
                        Ok(exit) => {
                            let _ = actor_tx.blocking_send(Message::LocalProcessExited {
                                session_id: session_id.clone(),
                                generation,
                                exit,
                            });
                            break;
                        }
                        Err(error) => {
                            let _ = actor_tx.blocking_send(Message::LocalProcessFailed {
                                session_id: session_id.clone(),
                                generation,
                                failure: failure(
                                    LocalSessionFailureCode::ProcessCleanupFailed,
                                    "errors.localTerminal.processCleanupFailed",
                                    Some(error),
                                ),
                            });
                            poll_process = false;
                        }
                    },
                    Ok(LocalProcessCommand::FailAndTerminate(reason)) => {
                        if terminate_after_failure(
                            &actor_tx,
                            &session_id,
                            generation,
                            &mut process,
                            reason,
                        ) {
                            break;
                        }
                        poll_process = false;
                    }
                    Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                        let result = process.terminate();
                        match result {
                            Ok(exit) => {
                                let _ = actor_tx.blocking_send(Message::LocalProcessExited {
                                    session_id: session_id.clone(),
                                    generation,
                                    exit,
                                });
                            }
                            Err(error) => {
                                let _ = actor_tx.blocking_send(Message::LocalProcessFailed {
                                    session_id: session_id.clone(),
                                    generation,
                                    failure: failure(
                                        LocalSessionFailureCode::ProcessCleanupFailed,
                                        "errors.localTerminal.processCleanupFailed",
                                        Some(error),
                                    ),
                                });
                            }
                        }
                        break;
                    }
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                }
                if !poll_process {
                    continue;
                }
                match process.poll_exit() {
                    Ok(Some(exit)) => {
                        let _ = actor_tx.blocking_send(Message::LocalProcessExited {
                            session_id: session_id.clone(),
                            generation,
                            exit,
                        });
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = actor_tx.blocking_send(Message::LocalProcessFailed {
                            session_id: session_id.clone(),
                            generation,
                            failure: failure(
                                LocalSessionFailureCode::ProcessCleanupFailed,
                                "errors.localTerminal.processCleanupFailed",
                                Some(error),
                            ),
                        });
                        poll_process = false;
                    }
                }
            }
        }) {
        Ok(_) => Ok(()),
        Err(spawn_error) => {
            let mut process = process
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("failed owner spawn retains the local PTY process");
            match process.terminate() {
                Ok(_) => Err(LocalProcessOwnerSpawnError::SpawnFailed(spawn_error)),
                Err(cleanup_error) => Err(LocalProcessOwnerSpawnError::CleanupFailed {
                    source: io::Error::other(format!(
                        "local PTY owner thread failed ({spawn_error}); cleanup failed ({cleanup_error})"
                    )),
                    process,
                }),
            }
        }
    }
}

fn spawn_process_writer(
    session_id: String,
    mut writer: LocalPtyWriter,
    commands: std_mpsc::Receiver<LocalWriterCommand>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name(format!("local-pty-writer-{session_id}"))
        .spawn(move || {
            while let Ok(LocalWriterCommand::Input { bytes, completion }) = commands.recv() {
                let _ = completion.send(writer.write_input(&bytes).is_ok());
            }
        })
        .map(|_| ())
}

fn local_owner_setup_failed(
    mut process: LocalPtyProcess,
    source: io::Error,
) -> LocalProcessOwnerSpawnError {
    match process.terminate() {
        Ok(_) => LocalProcessOwnerSpawnError::SpawnFailed(source),
        Err(cleanup_error) => LocalProcessOwnerSpawnError::CleanupFailed {
            source: io::Error::other(format!(
                "local PTY owner setup failed ({source}); cleanup failed ({cleanup_error})"
            )),
            process,
        },
    }
}

fn terminate_after_failure(
    actor_tx: &mpsc::Sender<Message>,
    session_id: &str,
    generation: u64,
    process: &mut LocalPtyProcess,
    mut reason: LocalSessionFailureReason,
) -> bool {
    let mut cleaned = true;
    if let Err(error) = process.terminate() {
        cleaned = false;
        reason = failure(
            LocalSessionFailureCode::ProcessCleanupFailed,
            "errors.localTerminal.processCleanupFailed",
            Some(error),
        );
    }
    let _ = actor_tx.blocking_send(Message::LocalProcessFailed {
        session_id: session_id.to_owned(),
        generation,
        failure: reason,
    });
    cleaned
}

fn transition_record(
    record: &mut LocalSessionRecord,
    state: LocalSessionState,
    exit: Option<LocalSessionExit>,
    failure_reason: Option<LocalSessionFailureReason>,
) {
    if record.summary.state == state
        && record.summary.exit == exit
        && record.summary.failure_reason == failure_reason
    {
        return;
    }
    let previous_state = record.summary.state;
    record.summary.state = state;
    record.summary.exit = exit.clone();
    record.summary.failure_reason = failure_reason.clone();
    record.summary.state_revision =
        WireSequence::new(record.summary.state_revision.get().saturating_add(1));
    record.summary.updated_at_unix_ms = unix_time_ms();
    emit_payload(
        record,
        LocalSessionEventPayload::StateChanged {
            previous_state,
            state,
            exit,
            failure_reason,
        },
    );
    let _ = record
        .plugin_metadata_events
        .send(PluginSessionMetadataEvent::Local {
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            state_revision: record.summary.state_revision,
            state: record.summary.state,
        });
}

fn finalize_exit(record: &mut LocalSessionRecord, exit: LocalSessionExit) {
    transition_record(record, LocalSessionState::Exited, Some(exit), None);
    if let Some(attachment_id) = record.detach_after_exit.take() {
        let _ = remove_attachment(record, &attachment_id);
        transition_record(
            record,
            LocalSessionState::Closed,
            record.summary.exit.clone(),
            None,
        );
    }
}

fn terminate_orphan_process(
    orphan_process: &mut Option<LocalPtyProcess>,
) -> io::Result<LocalPtyExit> {
    let exit = orphan_process
        .as_mut()
        .expect("orphan process ownership was checked")
        .terminate()?;
    orphan_process.take();
    Ok(exit)
}

fn emit_payload(record: &mut LocalSessionRecord, payload: LocalSessionEventPayload) {
    let event_seq = record.summary.event_seq.get().saturating_add(1);
    record.summary.event_seq = WireSequence::new(event_seq);
    let event = LocalSessionEvent {
        schema_version: LOCAL_TERMINAL_EVENT_SCHEMA_VERSION,
        session_id: record.summary.session_id.clone(),
        generation: record.summary.generation,
        state_revision: record.summary.state_revision,
        event_seq: WireSequence::new(event_seq),
        occurred_at_unix_ms: unix_time_ms(),
        payload,
    };
    for attachment in record.attachments.values() {
        let _ = attachment.events.send(event.clone());
    }
}

fn details_from(record: &LocalSessionRecord) -> LocalSessionDetails {
    LocalSessionDetails {
        session: record.summary.clone(),
        attachments: record
            .attachments
            .values()
            .map(|attachment| attachment.summary.clone())
            .collect(),
        input_lease: record.input_lease.clone(),
    }
}

fn validate_attachment(
    record: &LocalSessionRecord,
    generation: WireSequence,
    attachment_id: &LocalAttachmentId,
    view_id: &norishell_core_api::LocalViewId,
    request_id: &RequestId,
) -> ActorResult<()> {
    if record.summary.generation != generation {
        return Err(local_conflict(request_id.clone()));
    }
    let attachment = record
        .attachments
        .get(attachment_id.as_str())
        .ok_or_else(|| local_not_found(request_id.clone()))?;
    if attachment.summary.generation != generation || attachment.summary.view_id != *view_id {
        return Err(local_conflict(request_id.clone()));
    }
    Ok(())
}

fn validate_lease(
    record: &LocalSessionRecord,
    lease_id: &LocalInputLeaseId,
    input_epoch: WireSequence,
    request_id: &RequestId,
    require_unexpired: bool,
) -> ActorResult<()> {
    let lease = record
        .input_lease
        .as_ref()
        .ok_or_else(|| local_conflict(request_id.clone()))?;
    if lease.lease_id != *lease_id
        || lease.input_epoch != input_epoch
        || (require_unexpired && lease.expires_at_unix_ms <= unix_time_ms())
    {
        return Err(local_conflict(request_id.clone()));
    }
    Ok(())
}

fn remove_attachment(
    record: &mut LocalSessionRecord,
    attachment_id: &LocalAttachmentId,
) -> Option<LocalSessionAttachment> {
    let removed = record.attachments.remove(attachment_id.as_str())?;
    if record
        .input_lease
        .as_ref()
        .is_some_and(|lease| lease.attachment_id == *attachment_id)
    {
        record.input_lease = None;
        emit_payload(
            record,
            LocalSessionEventPayload::InputLeaseChanged {
                change: LocalSessionInputLeaseChange::Released,
                lease: None,
            },
        );
    }
    let next_revision = record.summary.attachment_revision.get().saturating_add(1);
    record.summary.attachment_revision = WireSequence::new(next_revision);
    record.summary.attachment_count = record.attachments.len() as u32;
    emit_payload(
        record,
        LocalSessionEventPayload::AttachmentChanged {
            change: LocalSessionAttachmentChange::Detached,
            attachment_revision: WireSequence::new(next_revision),
            attachment: removed.summary.clone(),
        },
    );
    Some(removed.summary)
}

fn replay_after(
    record: &LocalSessionRecord,
    after: Option<WireSequence>,
) -> Vec<LocalSessionOutputItem> {
    let after = after.map_or(0, WireSequence::get);
    record
        .output_ring
        .iter()
        .filter(|item| match item {
            LocalSessionOutputItem::Frame(frame) => frame.output_seq.get() > after,
            LocalSessionOutputItem::Gap(gap) => gap.resumes_at_output_seq.get() > after,
        })
        .cloned()
        .collect()
}

fn trim_output_ring(record: &mut LocalSessionRecord) {
    let mut first_dropped = None;
    let mut last_dropped = None;
    while record.output_ring_bytes > OUTPUT_RING_MAX_BYTES {
        let Some(item) = record.output_ring.pop_front() else {
            break;
        };
        if let LocalSessionOutputItem::Frame(frame) = item {
            record.output_ring_bytes = record.output_ring_bytes.saturating_sub(frame.bytes.len());
            first_dropped.get_or_insert(frame.output_seq);
            last_dropped = Some(frame.output_seq);
        }
    }
    if let (Some(dropped_from_output_seq), Some(last)) = (first_dropped, last_dropped) {
        while matches!(
            record.output_ring.front(),
            Some(LocalSessionOutputItem::Gap(_))
        ) {
            record.output_ring.pop_front();
        }
        let resumes_at_output_seq = record
            .output_ring
            .front()
            .and_then(|item| match item {
                LocalSessionOutputItem::Frame(frame) => Some(frame.output_seq),
                LocalSessionOutputItem::Gap(_) => None,
            })
            .unwrap_or(WireSequence::new(last.get().saturating_add(1)));
        let Some(pty_id) = record.summary.pty_id.clone() else {
            return;
        };
        let gap = LocalSessionOutputGap {
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            pty_id,
            dropped_from_output_seq,
            resumes_at_output_seq,
            reason: LocalSessionOutputGapReason::RingBufferOverflow,
        };
        record
            .output_ring
            .push_front(LocalSessionOutputItem::Gap(gap.clone()));
        emit_payload(record, LocalSessionEventPayload::OutputGap { gap });
    }
}

fn failure(
    code: LocalSessionFailureCode,
    message_key: &str,
    error: Option<io::Error>,
) -> LocalSessionFailureReason {
    LocalSessionFailureReason {
        code,
        message_key: message_key.to_owned(),
        diagnostic_id: error.map(|_| uuid::Uuid::new_v4().to_string()),
    }
}

fn spawn_failure(error: LocalPtySpawnError) -> LocalSessionFailureReason {
    let (code, message_key) = match error.failure {
        LocalPtySpawnFailure::AccountLookupFailed => (
            LocalSessionFailureCode::AccountLookupFailed,
            "errors.localTerminal.accountLookupFailed",
        ),
        LocalPtySpawnFailure::InvalidDefaultShell => (
            LocalSessionFailureCode::InvalidDefaultShell,
            "errors.localTerminal.invalidDefaultShell",
        ),
        LocalPtySpawnFailure::InvalidHomeDirectory => (
            LocalSessionFailureCode::InvalidHomeDirectory,
            "errors.localTerminal.invalidHomeDirectory",
        ),
        LocalPtySpawnFailure::PtySpawnFailed => (
            LocalSessionFailureCode::PtySpawnFailed,
            "errors.localTerminal.ptySpawnFailed",
        ),
    };
    failure(code, message_key, Some(error.source))
}

fn exit_to_wire(exit: LocalPtyExit) -> Result<LocalSessionExit, LocalSessionFailureReason> {
    if let Some(signal) = exit.signal {
        return Ok(LocalSessionExit::ExitSignal {
            signal_name: signal_name(signal),
        });
    }
    let exit_status = exit.code.unwrap_or_default();
    Ok(LocalSessionExit::ExitStatus { exit_status })
}

fn signal_name(signal: i32) -> String {
    #[cfg(unix)]
    {
        match signal {
            libc::SIGHUP => "SIGHUP",
            libc::SIGINT => "SIGINT",
            libc::SIGQUIT => "SIGQUIT",
            libc::SIGKILL => "SIGKILL",
            libc::SIGTERM => "SIGTERM",
            _ => return format!("SIG{signal}"),
        }
        .to_owned()
    }
    #[cfg(not(unix))]
    format!("SIGNAL_{signal}")
}

fn pty_eof_error(error: &io::Error) -> bool {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EIO) {
        return true;
    }
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
    )
}

fn local_error(
    request_id: RequestId,
    code: &str,
    category: ErrorCategory,
    retry_strategy: RetryStrategy,
    message_key: &str,
) -> Box<CoreApiError> {
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

fn local_validation(request_id: RequestId) -> Box<CoreApiError> {
    local_error(
        request_id,
        "local_terminal.invalid_request",
        ErrorCategory::Validation,
        RetryStrategy::Never,
        "errors.localTerminal.invalidRequest",
    )
}

fn local_conflict(request_id: RequestId) -> Box<CoreApiError> {
    local_error(
        request_id,
        "local_terminal.stale_fence",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.localTerminal.staleFence",
    )
}

pub(super) fn local_stale_focus(request_id: RequestId) -> Box<CoreApiError> {
    local_error(
        request_id,
        "terminal_input.stale_focus",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.terminalInput.staleFocus",
    )
}

fn local_unavailable(request_id: RequestId) -> Box<CoreApiError> {
    local_error(
        request_id,
        "local_terminal.unavailable",
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.localTerminal.unavailable",
    )
}

fn local_not_found(request_id: RequestId) -> Box<CoreApiError> {
    local_error(
        request_id,
        "local_terminal.not_found",
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.localTerminal.notFound",
    )
}

fn replay_operation<Outcome: Clone>(
    ledger: &SshOperationLedger<LocalOperationFingerprint, StoredSshOperationResult<Outcome>>,
    operation_id: &str,
    idempotency_key: &str,
    fingerprint: &LocalOperationFingerprint,
    request_id: RequestId,
) -> Option<ActorResult<Outcome>> {
    match ledger.lookup(operation_id, idempotency_key, fingerprint) {
        SshOperationLookup::Missing => None,
        SshOperationLookup::Replay(stored) => Some(stored.replay(request_id)),
        SshOperationLookup::Conflict => Some(Err(local_conflict(request_id))),
    }
}

fn record_operation<Outcome: Clone>(
    ledger: &mut SshOperationLedger<LocalOperationFingerprint, StoredSshOperationResult<Outcome>>,
    operation_id: String,
    idempotency_key: String,
    fingerprint: LocalOperationFingerprint,
    request_id: RequestId,
    result: ActorResult<Outcome>,
) -> ActorResult<Outcome> {
    let stored = StoredSshOperationResult::from_actor_result(result);
    match ledger.record(operation_id, idempotency_key, fingerprint, stored) {
        SshOperationLookup::Replay(stored) => stored.replay(request_id),
        SshOperationLookup::Conflict => Err(local_conflict(request_id)),
        SshOperationLookup::Missing => unreachable!("record always returns a terminal lookup"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_fixture(
        state: LocalSessionState,
        process: Option<std_mpsc::SyncSender<LocalProcessCommand>>,
        reader_drained: bool,
    ) -> (
        norishell_core_api::LocalSessionId,
        Arc<Mutex<BTreeSet<String>>>,
        LocalSessions,
    ) {
        let session_id = norishell_core_api::LocalSessionId::new();
        let live_sessions = Arc::new(Mutex::new(BTreeSet::from([session_id.as_str().to_owned()])));
        let native_terminal =
            NativeTerminalService::detached(crate::vault_service::VaultService::start(
                std::env::temp_dir()
                    .join(format!("norishell-local-fixture-{}", uuid::Uuid::now_v7())),
            ));
        let (plugin_metadata_events, _) = broadcast::channel(64);
        let mut sessions = LocalSessions::new(
            Arc::clone(&live_sessions),
            native_terminal,
            plugin_metadata_events,
        );
        let record_metadata_events = sessions.plugin_metadata_events.clone();
        sessions.sessions.insert(
            session_id.as_str().to_owned(),
            LocalSessionRecord {
                summary: LocalSessionSummary {
                    session_id: session_id.clone(),
                    open_attempt_id: LocalOpenAttemptId::new(),
                    shell_name: "test-shell".to_owned(),
                    generation: WireSequence::new(1),
                    state_revision: WireSequence::new(2),
                    attachment_revision: WireSequence::new(1),
                    event_seq: WireSequence::new(0),
                    pty_id: Some(LocalPtyId::new()),
                    state,
                    exit: None,
                    failure_reason: None,
                    attachment_count: 0,
                    created_at_unix_ms: 1,
                    updated_at_unix_ms: 1,
                },
                attachments: BTreeMap::new(),
                input_lease: None,
                next_input_epoch: 0,
                last_client_seq: 0,
                last_resize_seq: 0,
                next_output_seq: 1,
                output_ring: VecDeque::new(),
                output_ring_bytes: 0,
                process,
                orphan_process: None,
                input_write_poisoned: false,
                pending_exit: None,
                reader_drained,
                stop_on_ready: false,
                detach_after_exit: None,
                plugin_metadata_events: record_metadata_events,
            },
        );
        (session_id, live_sessions, sessions)
    }

    #[tokio::test]
    async fn local_transitions_publish_secret_free_plugin_metadata() {
        let (session_id, _live_sessions, mut sessions) =
            session_fixture(LocalSessionState::Starting, None, true);
        let mut metadata = sessions.plugin_metadata_events.subscribe();
        let record = sessions
            .sessions
            .get_mut(session_id.as_str())
            .expect("local session");

        transition_record(record, LocalSessionState::Running, None, None);

        assert_eq!(
            metadata.recv().await.expect("metadata event"),
            PluginSessionMetadataEvent::Local {
                session_id,
                generation: WireSequence::new(1),
                state_revision: WireSequence::new(3),
                state: LocalSessionState::Running,
            }
        );
    }

    #[test]
    fn process_exit_waits_for_tail_output_drain_before_publishing_exited() {
        let (session_id, live_sessions, mut sessions) =
            session_fixture(LocalSessionState::Running, None, false);

        assert!(sessions.process_exited(
            session_id.as_str(),
            1,
            LocalPtyExit {
                code: Some(7),
                signal: None,
            },
        ));
        let pending = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(pending.summary.state, LocalSessionState::Running);
        assert_eq!(pending.summary.exit, None);
        assert_eq!(pending.summary.event_seq, WireSequence::new(0));
        assert!(live_sessions.lock().unwrap().contains(session_id.as_str()));

        sessions.process_output(session_id.as_str(), 1, b"final PTY bytes".to_vec());
        let with_tail = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(with_tail.summary.state, LocalSessionState::Running);
        assert_eq!(with_tail.summary.event_seq, WireSequence::new(1));
        assert_eq!(
            replay_after(with_tail, None),
            vec![LocalSessionOutputItem::Frame(LocalSessionOutputFrame {
                session_id: session_id.clone(),
                generation: WireSequence::new(1),
                pty_id: with_tail.summary.pty_id.clone().unwrap(),
                output_seq: WireSequence::new(1),
                bytes: b"final PTY bytes".to_vec(),
            })]
        );

        assert!(!sessions.process_output_drained(session_id.as_str(), 2));
        assert!(sessions.process_output_drained(session_id.as_str(), 1));
        let exited = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(exited.summary.state, LocalSessionState::Exited);
        assert_eq!(
            exited.summary.exit,
            Some(LocalSessionExit::ExitStatus { exit_status: 7 })
        );
        assert_eq!(exited.summary.event_seq, WireSequence::new(2));
        assert!(!live_sessions.lock().unwrap().contains(session_id.as_str()));
    }

    #[test]
    fn cleanup_failure_keeps_owner_retryable_until_a_proven_exit() {
        let (commands, command_rx) =
            std_mpsc::sync_channel::<LocalProcessCommand>(LOCAL_PROCESS_MAILBOX_CAPACITY);
        let (session_id, live_sessions, mut sessions) =
            session_fixture(LocalSessionState::Starting, Some(commands), true);
        assert!(sessions.process_failed(
            session_id.as_str(),
            1,
            failure(
                LocalSessionFailureCode::ProcessCleanupFailed,
                "errors.localTerminal.processCleanupFailed",
                Some(io::Error::other("first cleanup attempt failed")),
            ),
        ));
        let failed = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(failed.summary.state, LocalSessionState::Failed);
        assert!(failed.process.is_some());
        assert!(live_sessions.lock().unwrap().contains(session_id.as_str()));

        let request = LocalSessionTerminateRequest {
            meta: norishell_core_api::RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: norishell_core_api::OperationId::new(),
            idempotency_key: "retry-cleanup".to_owned(),
            session_id: session_id.clone(),
            expected_generation: failed.summary.generation,
            expected_state_revision: failed.summary.state_revision,
        };
        let stopping = sessions.terminate(request).expect("retry cleanup");
        assert_eq!(stopping.state, LocalSessionState::Stopping);
        assert!(matches!(
            command_rx.try_recv(),
            Ok(LocalProcessCommand::Terminate)
        ));
        assert!(live_sessions.lock().unwrap().contains(session_id.as_str()));

        assert!(sessions.process_exited(
            session_id.as_str(),
            1,
            LocalPtyExit {
                code: Some(0),
                signal: None,
            },
        ));
        assert_eq!(
            sessions
                .sessions
                .get(session_id.as_str())
                .unwrap()
                .summary
                .state,
            LocalSessionState::Exited
        );
        assert!(!live_sessions.lock().unwrap().contains(session_id.as_str()));
    }

    #[test]
    fn uncertain_input_poison_keeps_process_blocker_until_owner_confirms_cleanup() {
        let (commands, command_rx) =
            std_mpsc::sync_channel::<LocalProcessCommand>(LOCAL_PROCESS_MAILBOX_CAPACITY);
        let (session_id, live_sessions, mut sessions) =
            session_fixture(LocalSessionState::Running, Some(commands), true);

        assert!(sessions.poison_input(session_id.as_str(), 1));
        let failed = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(failed.summary.state, LocalSessionState::Failed);
        assert_eq!(
            failed
                .summary
                .failure_reason
                .as_ref()
                .map(|reason| reason.code),
            Some(LocalSessionFailureCode::PtyWriteFailed)
        );
        assert!(failed.process.is_some());
        assert!(live_sessions.lock().unwrap().contains(session_id.as_str()));
        assert!(matches!(
            command_rx.try_recv(),
            Ok(LocalProcessCommand::FailAndTerminate(reason))
                if reason.code == LocalSessionFailureCode::PtyWriteFailed
        ));

        assert!(sessions.process_failed(
            session_id.as_str(),
            1,
            failure(
                LocalSessionFailureCode::ProcessCleanupFailed,
                "errors.localTerminal.processCleanupFailed",
                None,
            ),
        ));
        let cleanup_pending = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(
            cleanup_pending
                .summary
                .failure_reason
                .as_ref()
                .map(|reason| reason.code),
            Some(LocalSessionFailureCode::ProcessCleanupFailed)
        );
        assert!(cleanup_pending.process.is_some());
        assert!(live_sessions.lock().unwrap().contains(session_id.as_str()));

        assert!(sessions.process_failed(
            session_id.as_str(),
            1,
            failure(
                LocalSessionFailureCode::PtyWriteFailed,
                "errors.localTerminal.ptyWriteFailed",
                None,
            ),
        ));
        let cleaned = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(cleaned.summary.state, LocalSessionState::Failed);
        assert!(cleaned.process.is_none());
        assert!(!live_sessions.lock().unwrap().contains(session_id.as_str()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn backpressured_writer_does_not_block_owner_or_exit_blocker_cleanup() {
        let (session_id, live_sessions, mut sessions) =
            session_fixture(LocalSessionState::Running, None, true);
        let (_metadata, process) = LocalPtyProcess::spawn(24, 80).expect("spawn backpressure PTY");
        let (actor_tx, mut actor_rx) = mpsc::channel(8);
        let (commands, command_rx) =
            std_mpsc::sync_channel::<LocalProcessCommand>(LOCAL_PROCESS_MAILBOX_CAPACITY);
        assert!(
            spawn_process_owner(
                actor_tx,
                session_id.as_str().to_owned(),
                1,
                process,
                command_rx,
            )
            .is_ok(),
            "spawn responsive PTY owner"
        );
        sessions
            .sessions
            .get_mut(session_id.as_str())
            .expect("backpressure session")
            .process = Some(commands.clone());

        let (sleep_completion, sleep_completed) = oneshot::channel();
        commands
            .send(LocalProcessCommand::Input {
                bytes: b"sleep 30\r".to_vec(),
                completion: sleep_completion,
            })
            .expect("start non-reading foreground command");
        assert!(
            tokio::time::timeout(Duration::from_secs(1), sleep_completed)
                .await
                .expect("initial PTY write ack timeout")
                .expect("initial PTY writer")
        );
        tokio::time::sleep(Duration::from_millis(200)).await;

        let (blocked_completion, mut blocked_completed) = oneshot::channel();
        commands
            .send(LocalProcessCommand::Input {
                bytes: vec![b'x'; 16 * 1024 * 1024],
                completion: blocked_completion,
            })
            .expect("queue backpressured PTY write");
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut blocked_completed)
                .await
                .is_err(),
            "large PTY input unexpectedly completed before the pipe was backpressured"
        );

        assert!(sessions.poison_input(session_id.as_str(), 1));
        assert!(live_sessions.lock().unwrap().contains(session_id.as_str()));
        let failure = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match actor_rx.recv().await.expect("PTY owner cleanup fact") {
                    Message::LocalProcessFailed {
                        session_id: failed_session,
                        generation,
                        failure,
                    } if failed_session == session_id.as_str() => {
                        break (generation, failure);
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("backpressured owner cleanup remained blocked");
        assert!(sessions.process_failed(session_id.as_str(), failure.0, failure.1));
        assert!(!live_sessions.lock().unwrap().contains(session_id.as_str()));
        assert!(
            tokio::time::timeout(Duration::from_secs(2), &mut blocked_completed)
                .await
                .is_ok(),
            "PTY writer did not unblock after the owner terminated the process tree"
        );
    }

    #[test]
    fn owner_spawn_cleanup_failure_retries_the_same_orphan_until_proven_exit() {
        let (_metadata, process) = LocalPtyProcess::spawn(24, 80).expect("spawn orphan test PTY");
        let (session_id, live_sessions, mut sessions) =
            session_fixture(LocalSessionState::Starting, None, false);
        sessions.process_owner_spawn_failed(
            session_id.as_str(),
            1,
            LocalProcessOwnerSpawnError::CleanupFailed {
                source: io::Error::other("owner spawn and synchronous cleanup failed"),
                process,
            },
        );

        let failed = sessions.sessions.get(session_id.as_str()).unwrap();
        assert_eq!(failed.summary.state, LocalSessionState::Failed);
        assert_eq!(
            failed
                .summary
                .failure_reason
                .as_ref()
                .map(|reason| reason.code),
            Some(LocalSessionFailureCode::ProcessCleanupFailed)
        );
        assert!(failed.process.is_none());
        assert!(failed.orphan_process.is_some());
        assert!(live_sessions.lock().unwrap().contains(session_id.as_str()));

        let request = LocalSessionTerminateRequest {
            meta: norishell_core_api::RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: norishell_core_api::OperationId::new(),
            idempotency_key: "retry-orphan-cleanup".to_owned(),
            session_id: session_id.clone(),
            expected_generation: failed.summary.generation,
            expected_state_revision: failed.summary.state_revision,
        };
        let exited = sessions
            .terminate(request)
            .expect("terminate retained orphan");
        assert_eq!(exited.state, LocalSessionState::Exited);
        assert!(
            sessions
                .sessions
                .get(session_id.as_str())
                .unwrap()
                .orphan_process
                .is_none()
        );
        assert!(!live_sessions.lock().unwrap().contains(session_id.as_str()));
    }
}
