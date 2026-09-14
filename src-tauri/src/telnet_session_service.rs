//! Telnet terminal-session actor.
//!
//! The actor owns only direct, ephemeral Telnet sessions. It deliberately has
//! no Host, Known Hosts, Vault, CredentialRef, RoutePlan, algorithm policy,
//! login-automation, heartbeat, or persistence dependency. The application
//! level focus broker remains authoritative: it grants a fenced lease through
//! `grant_input` and revokes it on every global focus change.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use norishell_core_api as wire;
use norishell_telnet_runtime::{
    DEFAULT_TELNET_PORT, TelnetConnectConfig, TelnetConnection, TelnetEvent, TelnetRuntimeError,
};
use serde::{Deserialize, Serialize};
use tauri::{State, ipc::Channel};
use thiserror::Error;
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::AbortHandle,
};
use uuid::Uuid;

use crate::time::unix_time_ms;

type CoreResult<T> = Result<T, Box<wire::CoreApiError>>;

const ACTOR_MAILBOX_CAPACITY: usize = 256;
const OWNER_MAILBOX_CAPACITY: usize = 128;
const EVENT_CHANNEL_CAPACITY: usize = 256;
#[cfg(not(test))]
const OUTPUT_RING_MAX_BYTES: usize = 4 * 1024 * 1024;
#[cfg(test)]
const OUTPUT_RING_MAX_BYTES: usize = 64;
const ATTACHMENT_TIMEOUT_MILLIS: i64 = 45_000;
const ATTACHMENT_REAPER_INTERVAL: Duration = Duration::from_secs(5);
const INPUT_LEASE_MILLIS: i64 = 15_000;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(6);
const SHUTDOWN_ABORT_GRACE: Duration = Duration::from_millis(500);
const OPERATION_LEDGER_CAPACITY: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TelnetSessionState {
    Connecting,
    Running,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TelnetCloseReason {
    UserRequested,
    RemoteClosed,
    ConnectionFailed,
    CoreShutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TelnetFailureCode {
    ResolveFailed,
    ConnectTimeout,
    ConnectFailed,
    ProtocolViolation,
    IoTimeout,
    IoFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetFailureReason {
    pub code: TelnetFailureCode,
    /// Stable locale key; OS/socket details and user-facing prose are not exposed.
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionSummary {
    pub session_id: String,
    pub open_attempt_id: String,
    pub address: String,
    pub port: u16,
    pub generation: u64,
    pub state_revision: u64,
    pub attachment_revision: u64,
    pub event_sequence: u64,
    pub state: TelnetSessionState,
    pub socket_id: Option<String>,
    pub attachment_count: u32,
    pub close_reason: Option<TelnetCloseReason>,
    pub failure_reason: Option<TelnetFailureReason>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetAttachment {
    pub attachment_id: String,
    pub attach_attempt_id: String,
    pub session_id: String,
    pub generation: u64,
    pub socket_id: Option<String>,
    pub view_id: String,
    pub state_revision: u64,
    pub attachment_revision: u64,
    pub attached_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetOutputFrame {
    pub generation: u64,
    pub sequence: u64,
    pub data: Vec<u8>,
    event_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TelnetReplayItem {
    Gap {
        generation: u64,
        dropped_through_sequence: u64,
    },
    Frame(TelnetOutputFrame),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TelnetSessionEvent {
    StateChanged {
        session: TelnetSessionSummary,
    },
    AttachmentChanged {
        session: TelnetSessionSummary,
        attachment: TelnetAttachment,
    },
    AttachmentDetached {
        session: TelnetSessionSummary,
        attachment_id: String,
    },
    InputLeaseRevoked {
        session: TelnetSessionSummary,
        focus_epoch: u64,
    },
    Output {
        session: TelnetSessionSummary,
        frame: TelnetOutputFrame,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetOpenRequest {
    pub operation_id: String,
    pub idempotency_key: String,
    pub open_attempt_id: String,
    pub address: String,
    pub port: Option<u16>,
    pub rows: u16,
    pub cols: u16,
    pub cleartext_risk_accepted: bool,
    pub attach_attempt_id: String,
    pub view_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetOpenResponse {
    pub session: TelnetSessionSummary,
    pub attachment: TelnetAttachment,
}

struct TelnetOpenBinding {
    response: TelnetOpenResponse,
    events: broadcast::Receiver<TelnetSessionEvent>,
    resync_immediately: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetAttachRequest {
    pub operation_id: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub expected_generation: u64,
    pub expected_state_revision: u64,
    pub attach_attempt_id: String,
    pub view_id: String,
    pub after_output_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetDetachRequest {
    pub operation_id: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub expected_generation: u64,
    pub expected_state_revision: u64,
    pub expected_attachment_revision: u64,
    pub attachment_id: String,
    pub view_id: String,
    pub intent: TelnetDetachIntent,
    pub disconnect_if_last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetDisconnectRequest {
    pub operation_id: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub expected_generation: u64,
    pub expected_state_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetAttachResponse {
    pub attachment: TelnetAttachment,
    pub replay: Vec<TelnetReplayItem>,
}

struct TelnetAttachBinding {
    response: TelnetAttachResponse,
    events: broadcast::Receiver<TelnetSessionEvent>,
}

#[derive(Clone)]
struct TelnetChannelResync {
    session: TelnetSessionSummary,
    attachment: Option<TelnetAttachment>,
    attachment_event_sequence: Option<u64>,
    detachment_event_sequence: Option<u64>,
    input_lease: Option<TelnetInputLease>,
    last_revoked_focus_epoch: u64,
    last_revoked_event_sequence: u64,
    last_state_event_sequence: u64,
    frames: Vec<TelnetOutputFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TelnetDetachIntent {
    UserClose,
    RendererUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetInputLease {
    pub lease_id: String,
    pub session_id: String,
    pub generation: u64,
    pub socket_id: String,
    pub attachment_id: String,
    pub view_id: String,
    pub state_revision: u64,
    pub focus_epoch: u64,
    pub input_epoch: u64,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetInputGrantRequest {
    pub session_id: String,
    pub expected_generation: u64,
    pub expected_socket_id: String,
    pub attachment_id: String,
    pub view_id: String,
    pub expected_state_revision: u64,
    /// Epoch already committed by the single application-level focus broker.
    pub focus_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetInputRequest {
    pub session_id: String,
    pub expected_generation: u64,
    pub socket_id: String,
    pub attachment_id: String,
    pub view_id: String,
    pub lease_id: String,
    pub focus_epoch: u64,
    pub input_epoch: u64,
    pub client_sequence: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetResizeRequest {
    pub session_id: String,
    pub expected_generation: u64,
    pub socket_id: String,
    pub attachment_id: String,
    pub view_id: String,
    pub lease_id: String,
    pub focus_epoch: u64,
    pub input_epoch: u64,
    pub resize_sequence: u64,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetReconnectRequest {
    pub operation_id: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub expected_generation: u64,
    pub expected_state_revision: u64,
    pub rows: u16,
    pub cols: u16,
    pub cleartext_risk_accepted: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TelnetServiceError {
    #[error("invalid Telnet session request")]
    Validation,
    #[error("cleartext Telnet risk must be accepted for every connection")]
    CleartextRiskNotAccepted,
    #[error("Telnet session was not found")]
    NotFound,
    #[error("stale or conflicting Telnet session fence")]
    Conflict,
    #[error("the final active attachment requires an explicit disconnect")]
    LastAttachmentRequiresDisconnect,
    #[error("Telnet input is not authorized by the current focus lease")]
    InputNotAuthorized,
    #[error("Telnet session service is unavailable")]
    Unavailable,
    #[error("Telnet session cleanup did not converge")]
    CleanupIncomplete,
}

struct AttachmentRecord {
    summary: TelnetAttachment,
    last_seen_at_unix_ms: i64,
    event_sequence: u64,
}

struct SessionRecord {
    summary: TelnetSessionSummary,
    attachments: BTreeMap<String, AttachmentRecord>,
    events: broadcast::Sender<TelnetSessionEvent>,
    input_lease: Option<TelnetInputLease>,
    last_revoked_focus_epoch: u64,
    last_revoked_event_sequence: u64,
    last_state_event_sequence: u64,
    detached_attachment_events: BTreeMap<String, u64>,
    detached_attachment_order: VecDeque<String>,
    next_input_epoch: u64,
    last_client_sequence: u64,
    last_resize_sequence: u64,
    next_output_sequence: u64,
    output_ring: VecDeque<TelnetOutputFrame>,
    output_ring_bytes: usize,
    dropped_through_sequence: u64,
    owner: Option<OwnerHandle>,
    connect_abort: Option<AbortHandle>,
    detach_after_close: Option<String>,
    focus_invalidations: mpsc::UnboundedSender<TelnetFocusInvalidation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelnetFocusInvalidation {
    pub(crate) session_id: String,
    pub(crate) generation: u64,
    pub(crate) state_revision: u64,
    pub(crate) socket_id: String,
    pub(crate) attachment_id: String,
    pub(crate) view_id: String,
    pub(crate) focus_epoch: u64,
}

impl From<&TelnetInputLease> for TelnetFocusInvalidation {
    fn from(lease: &TelnetInputLease) -> Self {
        Self {
            session_id: lease.session_id.clone(),
            generation: lease.generation,
            state_revision: lease.state_revision,
            socket_id: lease.socket_id.clone(),
            attachment_id: lease.attachment_id.clone(),
            view_id: lease.view_id.clone(),
            focus_epoch: lease.focus_epoch,
        }
    }
}

struct OwnerHandle {
    commands: mpsc::Sender<OwnerCommand>,
    abort: AbortHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenFingerprint {
    open_attempt_id: String,
    address: String,
    port: u16,
    rows: u16,
    cols: u16,
    cleartext_risk_accepted: bool,
    attach_attempt_id: String,
    view_id: String,
}

#[derive(Clone)]
struct OpenLedgerEntry {
    idempotency_key: String,
    fingerprint: OpenFingerprint,
    response: TelnetOpenResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconnectFingerprint {
    session_id: String,
    expected_generation: u64,
    expected_state_revision: u64,
    rows: u16,
    cols: u16,
    cleartext_risk_accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachFingerprint {
    session_id: String,
    expected_generation: u64,
    expected_state_revision: u64,
    attach_attempt_id: String,
    view_id: String,
    after_output_sequence: Option<u64>,
}

#[derive(Clone)]
struct AttachLedgerEntry {
    idempotency_key: String,
    fingerprint: AttachFingerprint,
    result: Result<TelnetAttachResponse, TelnetServiceError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetachFingerprint {
    session_id: String,
    expected_generation: u64,
    expected_state_revision: u64,
    expected_attachment_revision: u64,
    attachment_id: String,
    view_id: String,
    intent: TelnetDetachIntent,
    disconnect_if_last: bool,
}

#[derive(Clone)]
struct DetachLedgerEntry {
    idempotency_key: String,
    fingerprint: DetachFingerprint,
    result: Result<TelnetSessionSummary, TelnetServiceError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisconnectFingerprint {
    session_id: String,
    expected_generation: u64,
    expected_state_revision: u64,
}

#[derive(Clone)]
struct DisconnectLedgerEntry {
    idempotency_key: String,
    fingerprint: DisconnectFingerprint,
    result: Result<TelnetSessionSummary, TelnetServiceError>,
}

#[derive(Clone)]
struct ReconnectLedgerEntry {
    idempotency_key: String,
    fingerprint: ReconnectFingerprint,
    result: Result<TelnetSessionSummary, TelnetServiceError>,
}

struct Actor {
    tx: mpsc::Sender<Message>,
    sessions: BTreeMap<String, SessionRecord>,
    live_sessions: Arc<Mutex<BTreeSet<String>>>,
    open_ledger: BTreeMap<String, OpenLedgerEntry>,
    open_order: VecDeque<String>,
    reconnect_ledger: BTreeMap<String, ReconnectLedgerEntry>,
    reconnect_order: VecDeque<String>,
    attach_ledger: BTreeMap<String, AttachLedgerEntry>,
    attach_order: VecDeque<String>,
    detach_ledger: BTreeMap<String, DetachLedgerEntry>,
    detach_order: VecDeque<String>,
    disconnect_ledger: BTreeMap<String, DisconnectLedgerEntry>,
    disconnect_order: VecDeque<String>,
    focus_invalidations: mpsc::UnboundedSender<TelnetFocusInvalidation>,
}

enum Message {
    Open {
        request: TelnetOpenRequest,
        reply: oneshot::Sender<Result<TelnetOpenBinding, TelnetServiceError>>,
    },
    Subscribe {
        session_id: String,
        reply: oneshot::Sender<Result<broadcast::Receiver<TelnetSessionEvent>, TelnetServiceError>>,
    },
    ChannelResync {
        session_id: String,
        attachment_id: String,
        reply: oneshot::Sender<Result<TelnetChannelResync, TelnetServiceError>>,
    },
    Snapshot {
        reply: oneshot::Sender<Vec<TelnetSessionSummary>>,
    },
    Attach {
        request: TelnetAttachRequest,
        reply: oneshot::Sender<Result<TelnetAttachBinding, TelnetServiceError>>,
    },
    AttachmentHeartbeat {
        session_id: String,
        expected_generation: u64,
        expected_attachment_revision: u64,
        attachment_id: String,
        view_id: String,
        reply: oneshot::Sender<Result<TelnetAttachment, TelnetServiceError>>,
    },
    Detach {
        request: TelnetDetachRequest,
        reply: oneshot::Sender<Result<TelnetSessionSummary, TelnetServiceError>>,
    },
    GrantInput {
        request: TelnetInputGrantRequest,
        reply: oneshot::Sender<Result<TelnetInputLease, TelnetServiceError>>,
    },
    RenewInput {
        lease: TelnetInputLease,
        reply: oneshot::Sender<Result<TelnetInputLease, TelnetServiceError>>,
    },
    RevokeInput {
        session_id: String,
        expected_focus_epoch: u64,
        reply: oneshot::Sender<Result<(), TelnetServiceError>>,
    },
    Input {
        request: TelnetInputRequest,
        reply: oneshot::Sender<Result<(), TelnetServiceError>>,
    },
    Resize {
        request: TelnetResizeRequest,
        reply: oneshot::Sender<Result<(), TelnetServiceError>>,
    },
    Disconnect {
        request: TelnetDisconnectRequest,
        reply: oneshot::Sender<Result<TelnetSessionSummary, TelnetServiceError>>,
    },
    Reconnect {
        request: TelnetReconnectRequest,
        reply: oneshot::Sender<Result<TelnetSessionSummary, TelnetServiceError>>,
    },
    Connected {
        session_id: String,
        generation: u64,
        connection: Box<TelnetConnection>,
    },
    ConnectFailed {
        session_id: String,
        generation: u64,
        failure: TelnetFailureReason,
    },
    OwnerOutput {
        session_id: String,
        generation: u64,
        data: Vec<u8>,
    },
    OwnerClosed {
        session_id: String,
        generation: u64,
        reason: TelnetCloseReason,
        failure: Option<TelnetFailureReason>,
    },
    ReapStaleAttachments,
    ShutdownAll {
        reply: oneshot::Sender<Result<(), TelnetServiceError>>,
    },
    Teardown {
        reply: Option<oneshot::Sender<()>>,
    },
    #[cfg(test)]
    Barrier {
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
    #[cfg(test)]
    ReplaceOwner {
        session_id: String,
        commands: mpsc::Sender<OwnerCommand>,
        abort: AbortHandle,
        reply: oneshot::Sender<Result<(), TelnetServiceError>>,
    },
}

enum OwnerCommand {
    Input {
        data: Vec<u8>,
        reply: oneshot::Sender<Result<(), TelnetServiceError>>,
    },
    Resize {
        rows: u16,
        cols: u16,
        reply: oneshot::Sender<Result<(), TelnetServiceError>>,
    },
    Disconnect {
        reason: TelnetCloseReason,
        notify_actor: bool,
        reply: Option<oneshot::Sender<()>>,
    },
}

struct ServiceInner {
    tx: mpsc::Sender<Message>,
    live_sessions: Arc<Mutex<BTreeSet<String>>>,
    focus_invalidations: Mutex<Option<mpsc::UnboundedReceiver<TelnetFocusInvalidation>>>,
}

impl Drop for ServiceInner {
    fn drop(&mut self) {
        let _ = self.tx.try_send(Message::Teardown { reply: None });
    }
}

#[derive(Clone)]
pub struct TelnetSessionService {
    inner: Arc<ServiceInner>,
}

impl TelnetSessionService {
    #[must_use]
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel(ACTOR_MAILBOX_CAPACITY);
        let (focus_invalidation_tx, focus_invalidation_rx) = mpsc::unbounded_channel();
        let live_sessions = Arc::new(Mutex::new(BTreeSet::new()));
        let actor = Actor {
            tx: tx.clone(),
            sessions: BTreeMap::new(),
            live_sessions: live_sessions.clone(),
            open_ledger: BTreeMap::new(),
            open_order: VecDeque::new(),
            reconnect_ledger: BTreeMap::new(),
            reconnect_order: VecDeque::new(),
            attach_ledger: BTreeMap::new(),
            attach_order: VecDeque::new(),
            detach_ledger: BTreeMap::new(),
            detach_order: VecDeque::new(),
            disconnect_ledger: BTreeMap::new(),
            disconnect_order: VecDeque::new(),
            focus_invalidations: focus_invalidation_tx,
        };
        tauri::async_runtime::spawn(run_actor(actor, rx));
        let reaper_tx = tx.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(ATTACHMENT_REAPER_INTERVAL).await;
                if reaper_tx.send(Message::ReapStaleAttachments).await.is_err() {
                    break;
                }
            }
        });
        Self {
            inner: Arc::new(ServiceInner {
                tx,
                live_sessions,
                focus_invalidations: Mutex::new(Some(focus_invalidation_rx)),
            }),
        }
    }

    /// Starts the one-way coordinator that projects Telnet lifecycle facts
    /// into the process-wide focus actor. The Telnet actor never awaits or
    /// retains the global broker, so resource ownership stays acyclic.
    pub(crate) fn ensure_focus_coordinator(
        &self,
        sessions: crate::ssh_session_service::SshSessionService,
    ) {
        let receiver = self
            .inner
            .focus_invalidations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(mut receiver) = receiver else {
            return;
        };
        tauri::async_runtime::spawn(async move {
            while let Some(invalidation) = receiver.recv().await {
                let broker = sessions.focus_broker();
                broker
                    .linearize(async {
                        let _ = sessions
                            .clear_telnet_focus_exact_unserialized(invalidation)
                            .await;
                    })
                    .await;
            }
        });
    }

    #[must_use]
    pub fn exit_blockers(&self) -> Vec<String> {
        self.inner
            .live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    pub async fn open(
        &self,
        request: TelnetOpenRequest,
    ) -> Result<TelnetOpenResponse, TelnetServiceError> {
        self.open_subscribed(request)
            .await
            .map(|binding| binding.response)
    }

    async fn open_subscribed(
        &self,
        request: TelnetOpenRequest,
    ) -> Result<TelnetOpenBinding, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Open { request, reply }).await
    }

    pub async fn subscribe(
        &self,
        session_id: String,
    ) -> Result<broadcast::Receiver<TelnetSessionEvent>, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Subscribe {
            session_id,
            reply,
        })
        .await
    }

    pub async fn snapshot(&self) -> Result<Vec<TelnetSessionSummary>, TelnetServiceError> {
        let (reply, response) = oneshot::channel();
        self.inner
            .tx
            .send(Message::Snapshot { reply })
            .await
            .map_err(|_| TelnetServiceError::Unavailable)?;
        response.await.map_err(|_| TelnetServiceError::Unavailable)
    }

    pub async fn attach(
        &self,
        request: TelnetAttachRequest,
    ) -> Result<TelnetAttachResponse, TelnetServiceError> {
        self.attach_subscribed(request)
            .await
            .map(|binding| binding.response)
    }

    async fn attach_subscribed(
        &self,
        request: TelnetAttachRequest,
    ) -> Result<TelnetAttachBinding, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Attach { request, reply }).await
    }

    pub async fn attachment_heartbeat(
        &self,
        session_id: String,
        expected_generation: u64,
        expected_attachment_revision: u64,
        attachment_id: String,
        view_id: String,
    ) -> Result<TelnetAttachment, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::AttachmentHeartbeat {
            session_id,
            expected_generation,
            expected_attachment_revision,
            attachment_id,
            view_id,
            reply,
        })
        .await
    }

    pub async fn detach(
        &self,
        request: TelnetDetachRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Detach { request, reply }).await
    }

    /// Called only after the process-level focus broker commits this Telnet
    /// target. This service validates the resulting lease but does not create a
    /// second focus domain.
    pub async fn grant_input(
        &self,
        request: TelnetInputGrantRequest,
    ) -> Result<TelnetInputLease, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::GrantInput {
            request,
            reply,
        })
        .await
    }

    pub async fn renew_input(
        &self,
        lease: TelnetInputLease,
    ) -> Result<TelnetInputLease, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::RenewInput { lease, reply }).await
    }

    pub async fn revoke_input(
        &self,
        session_id: String,
        expected_focus_epoch: u64,
    ) -> Result<(), TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::RevokeInput {
            session_id,
            expected_focus_epoch,
            reply,
        })
        .await
    }

    pub async fn input(&self, request: TelnetInputRequest) -> Result<(), TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Input { request, reply }).await
    }

    pub async fn resize(&self, request: TelnetResizeRequest) -> Result<(), TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Resize { request, reply }).await
    }

    pub async fn disconnect(
        &self,
        request: TelnetDisconnectRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Disconnect {
            request,
            reply,
        })
        .await
    }

    pub async fn reconnect(
        &self,
        request: TelnetReconnectRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        request_reply(&self.inner.tx, |reply| Message::Reconnect {
            request,
            reply,
        })
        .await
    }

    pub async fn shutdown_all(&self) -> Result<(), TelnetServiceError> {
        let (reply, response) = oneshot::channel();
        self.inner
            .tx
            .send(Message::ShutdownAll { reply })
            .await
            .map_err(|_| TelnetServiceError::Unavailable)?;
        response
            .await
            .map_err(|_| TelnetServiceError::Unavailable)?
    }

    #[cfg(test)]
    async fn shutdown(&self) -> Result<(), TelnetServiceError> {
        let (reply, response) = oneshot::channel();
        self.inner
            .tx
            .send(Message::Teardown { reply: Some(reply) })
            .await
            .map_err(|_| TelnetServiceError::Unavailable)?;
        response.await.map_err(|_| TelnetServiceError::Unavailable)
    }

    #[cfg(test)]
    pub(crate) async fn barrier(
        &self,
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) -> Result<(), TelnetServiceError> {
        self.inner
            .tx
            .send(Message::Barrier { entered, release })
            .await
            .map_err(|_| TelnetServiceError::Unavailable)
    }
}

#[tauri::command]
pub async fn telnet_terminal_open(
    request: wire::TelnetSessionOpenRequest,
    on_event: Channel<wire::TelnetSessionEvent>,
    service: State<'_, TelnetSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<wire::TelnetSessionOpenResponse> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    validate_wire_risk_confirmation(&request)
        .map_err(|error| wire_error(request_id.clone(), error))?;
    let internal = TelnetOpenRequest {
        operation_id: request.operation_id.as_str().to_owned(),
        idempotency_key: request.idempotency_key,
        open_attempt_id: request.open_attempt_id.as_str().to_owned(),
        address: request.endpoint.address,
        port: Some(request.endpoint.port),
        rows: request.rows,
        cols: request.cols,
        cleartext_risk_accepted: true,
        attach_attempt_id: request.attach_attempt_id.as_str().to_owned(),
        view_id: request.view_id.as_str().to_owned(),
    };
    let opened = service
        .open_subscribed(internal)
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?;
    let TelnetOpenBinding {
        response: opened,
        events,
        resync_immediately,
    } = opened;
    spawn_telnet_event_forwarder(
        service.inner.tx.clone(),
        opened.session.session_id.clone(),
        opened.attachment.attachment_id.clone(),
        events,
        on_event,
        resync_immediately,
    );
    Ok(wire::TelnetSessionOpenResponse {
        session: wire_summary(opened.session)
            .map_err(|error| wire_error(request_id.clone(), error))?,
        attachment: wire_attachment(opened.attachment)
            .map_err(|error| wire_error(request_id, error))?,
    })
}

#[tauri::command]
pub async fn telnet_terminal_snapshot(
    request: wire::TelnetSessionSnapshotRequest,
    service: State<'_, TelnetSessionService>,
) -> CoreResult<wire::TelnetSessionSnapshot> {
    let request_id = request.meta.request_id;
    let sessions = service
        .snapshot()
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?;
    let snapshot_revision = sessions
        .iter()
        .map(|session| session.event_sequence)
        .max()
        .unwrap_or(0);
    Ok(wire::TelnetSessionSnapshot {
        snapshot_revision: wire::WireSequence::new(snapshot_revision),
        sessions: sessions
            .into_iter()
            .map(wire_summary)
            .collect::<Result<_, _>>()
            .map_err(|error| wire_error(request_id, error))?,
    })
}

#[tauri::command]
pub async fn telnet_terminal_attach(
    request: wire::TelnetSessionAttachRequest,
    on_event: Channel<wire::TelnetSessionEvent>,
    service: State<'_, TelnetSessionService>,
) -> CoreResult<wire::TelnetSessionAttachResponse> {
    let request_id = request.meta.request_id.clone();
    let attached = service
        .attach_subscribed(TelnetAttachRequest {
            operation_id: request.operation_id.as_str().to_owned(),
            idempotency_key: request.idempotency_key,
            session_id: request.session_id.as_str().to_owned(),
            expected_generation: request.expected_generation.get(),
            expected_state_revision: request.expected_state_revision.get(),
            attach_attempt_id: request.attach_attempt_id.as_str().to_owned(),
            view_id: request.view_id.as_str().to_owned(),
            after_output_sequence: request.after_output_seq.map(wire::WireSequence::get),
        })
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?;
    let TelnetAttachBinding {
        response: attached,
        events,
    } = attached;
    spawn_telnet_event_forwarder(
        service.inner.tx.clone(),
        attached.attachment.session_id.clone(),
        attached.attachment.attachment_id.clone(),
        events,
        on_event,
        false,
    );
    let attachment = wire_attachment(attached.attachment)
        .map_err(|error| wire_error(request_id.clone(), error))?;
    let replay = attached
        .replay
        .into_iter()
        .map(|item| wire_replay_item(&attachment, item))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| wire_error(request_id, error))?;
    Ok(wire::TelnetSessionAttachResponse {
        state_revision: attachment.state_revision,
        attachment_revision: attachment.attachment_revision,
        attachment,
        replay,
    })
}

fn spawn_telnet_event_forwarder(
    service_tx: mpsc::Sender<Message>,
    session_id: String,
    attachment_id: String,
    mut events: broadcast::Receiver<TelnetSessionEvent>,
    on_event: Channel<wire::TelnetSessionEvent>,
    resync_immediately: bool,
) {
    tokio::spawn(async move {
        if resync_immediately
            && send_channel_resync(&service_tx, &session_id, &attachment_id, &on_event)
                .await
                .is_err()
        {
            return;
        }
        loop {
            match events.recv().await {
                Ok(event) => {
                    let Some(event) = wire_event(event) else {
                        break;
                    };
                    if on_event.send(event).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if send_channel_resync(&service_tx, &session_id, &attachment_id, &on_event)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn send_channel_resync(
    service_tx: &mpsc::Sender<Message>,
    session_id: &str,
    attachment_id: &str,
    on_event: &Channel<wire::TelnetSessionEvent>,
) -> Result<(), ()> {
    let resync = request_reply(service_tx, |reply| Message::ChannelResync {
        session_id: session_id.to_owned(),
        attachment_id: attachment_id.to_owned(),
        reply,
    })
    .await
    .map_err(|_| ())?;
    for event in channel_resync_events(resync, attachment_id)? {
        on_event.send(event).map_err(|_| ())?;
    }
    Ok(())
}

fn channel_resync_events(
    resync: TelnetChannelResync,
    attachment_id: &str,
) -> Result<Vec<wire::TelnetSessionEvent>, ()> {
    let session = resync.session.clone();
    let mut sequenced = Vec::new();
    if resync.last_state_event_sequence > 0 {
        sequenced.push((
            resync.last_state_event_sequence,
            TelnetSessionEvent::StateChanged {
                session: session.clone(),
            },
        ));
    }
    if let (Some(attachment), Some(event_sequence)) =
        (resync.attachment.clone(), resync.attachment_event_sequence)
    {
        sequenced.push((
            event_sequence,
            TelnetSessionEvent::AttachmentChanged {
                session: session.clone(),
                attachment,
            },
        ));
    } else if resync.attachment.is_none()
        && let Some(event_sequence) = resync.detachment_event_sequence
    {
        sequenced.push((
            event_sequence,
            TelnetSessionEvent::AttachmentDetached {
                session: session.clone(),
                attachment_id: attachment_id.to_owned(),
            },
        ));
    }
    let owns_current_lease = resync
        .input_lease
        .as_ref()
        .is_some_and(|lease| lease.attachment_id == attachment_id);
    if !owns_current_lease
        && resync.last_revoked_focus_epoch > 0
        && resync.last_revoked_event_sequence > 0
    {
        sequenced.push((
            resync.last_revoked_event_sequence,
            TelnetSessionEvent::InputLeaseRevoked {
                session: session.clone(),
                focus_epoch: resync.last_revoked_focus_epoch,
            },
        ));
    }
    for frame in resync.frames {
        sequenced.push((
            frame.event_sequence,
            TelnetSessionEvent::Output {
                session: session.clone(),
                frame,
            },
        ));
    }
    sequenced.sort_by_key(|(event_sequence, _)| *event_sequence);
    sequenced
        .into_iter()
        .map(|(event_sequence, event)| {
            let mut event = wire_event(event).ok_or(())?;
            event.event_seq = wire::WireSequence::new(event_sequence);
            Ok(event)
        })
        .collect()
}

#[tauri::command]
pub async fn telnet_terminal_attachment_heartbeat(
    request: wire::TelnetSessionAttachmentHeartbeatRequest,
    service: State<'_, TelnetSessionService>,
) -> CoreResult<wire::TelnetSessionAttachment> {
    let request_id = request.meta.request_id;
    let attachment = service
        .attachment_heartbeat(
            request.session_id.as_str().to_owned(),
            request.expected_generation.get(),
            request.expected_attachment_revision.get(),
            request.attachment_id.as_str().to_owned(),
            request.view_id.as_str().to_owned(),
        )
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?;
    wire_attachment(attachment).map_err(|error| wire_error(request_id, error))
}

#[tauri::command]
pub async fn telnet_terminal_detach(
    request: wire::TelnetSessionDetachRequest,
    service: State<'_, TelnetSessionService>,
) -> CoreResult<wire::TelnetSessionDetachResponse> {
    let request_id = request.meta.request_id;
    let intent = match request.intent {
        wire::TelnetSessionDetachIntent::UserClose => TelnetDetachIntent::UserClose,
        wire::TelnetSessionDetachIntent::RendererUnavailable => {
            TelnetDetachIntent::RendererUnavailable
        }
    };
    let summary = service
        .detach(TelnetDetachRequest {
            operation_id: request.operation_id.as_str().to_owned(),
            idempotency_key: request.idempotency_key,
            session_id: request.session_id.as_str().to_owned(),
            expected_generation: request.expected_generation.get(),
            expected_state_revision: request.expected_state_revision.get(),
            expected_attachment_revision: request.expected_attachment_revision.get(),
            attachment_id: request.attachment_id.as_str().to_owned(),
            view_id: request.view_id.as_str().to_owned(),
            intent,
            disconnect_if_last: request.disconnect_if_last,
        })
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?;
    let remaining_attachment_count = summary.attachment_count;
    Ok(wire::TelnetSessionDetachResponse {
        session: wire_summary(summary).map_err(|error| wire_error(request_id, error))?,
        remaining_attachment_count,
    })
}

#[tauri::command]
pub async fn telnet_terminal_input_lease_renew(
    request: wire::TelnetSessionInputLeaseRenewRequest,
    service: State<'_, TelnetSessionService>,
    broker: State<'_, crate::ssh_session_service::SshSessionService>,
) -> CoreResult<wire::TelnetInputLease> {
    let focus_broker = broker.focus_broker();
    focus_broker
        .linearize(async {
            let request_id = request.meta.request_id;
            require_wire_focus_unserialized(
                &broker,
                request_id.clone(),
                &request.session_id,
                request.expected_generation,
                request.expected_state_revision,
                &request.socket_id,
                &request.attachment_id,
                &request.view_id,
                request.focus_epoch,
            )
            .await?;
            let lease = service
                .renew_input(TelnetInputLease {
                    lease_id: request.lease_id.as_str().to_owned(),
                    session_id: request.session_id.as_str().to_owned(),
                    generation: request.expected_generation.get(),
                    socket_id: request.socket_id.as_str().to_owned(),
                    attachment_id: request.attachment_id.as_str().to_owned(),
                    view_id: request.view_id.as_str().to_owned(),
                    state_revision: request.expected_state_revision.get(),
                    focus_epoch: request.focus_epoch.get(),
                    input_epoch: request.input_epoch.get(),
                    expires_at_unix_ms: 0,
                })
                .await
                .map_err(|error| wire_error(request_id.clone(), error))?;
            wire_lease(lease, request.socket_id).map_err(|error| wire_error(request_id, error))
        })
        .await
}

#[tauri::command]
pub async fn telnet_terminal_input(
    request: wire::TelnetSessionInputRequest,
    service: State<'_, TelnetSessionService>,
    broker: State<'_, crate::ssh_session_service::SshSessionService>,
) -> CoreResult<()> {
    let focus_broker = broker.focus_broker();
    focus_broker
        .linearize(async {
            let request_id = request.meta.request_id;
            require_wire_focus_unserialized(
                &broker,
                request_id.clone(),
                &request.session_id,
                request.expected_generation,
                request.expected_state_revision,
                &request.socket_id,
                &request.attachment_id,
                &request.view_id,
                request.focus_epoch,
            )
            .await?;
            service
                .input(TelnetInputRequest {
                    session_id: request.session_id.as_str().to_owned(),
                    expected_generation: request.expected_generation.get(),
                    socket_id: request.socket_id.as_str().to_owned(),
                    attachment_id: request.attachment_id.as_str().to_owned(),
                    view_id: request.view_id.as_str().to_owned(),
                    lease_id: request.lease_id.as_str().to_owned(),
                    focus_epoch: request.focus_epoch.get(),
                    input_epoch: request.input_epoch.get(),
                    client_sequence: request.client_seq.get(),
                    data: request.bytes,
                })
                .await
                .map_err(|error| wire_error(request_id, error))
        })
        .await
}

#[tauri::command]
pub async fn telnet_terminal_resize(
    request: wire::TelnetSessionResizeRequest,
    service: State<'_, TelnetSessionService>,
    broker: State<'_, crate::ssh_session_service::SshSessionService>,
) -> CoreResult<()> {
    let focus_broker = broker.focus_broker();
    focus_broker
        .linearize(async {
            let request_id = request.meta.request_id;
            require_wire_focus_unserialized(
                &broker,
                request_id.clone(),
                &request.session_id,
                request.expected_generation,
                request.expected_state_revision,
                &request.socket_id,
                &request.attachment_id,
                &request.view_id,
                request.focus_epoch,
            )
            .await?;
            service
                .resize(TelnetResizeRequest {
                    session_id: request.session_id.as_str().to_owned(),
                    expected_generation: request.expected_generation.get(),
                    socket_id: request.socket_id.as_str().to_owned(),
                    attachment_id: request.attachment_id.as_str().to_owned(),
                    view_id: request.view_id.as_str().to_owned(),
                    lease_id: request.lease_id.as_str().to_owned(),
                    focus_epoch: request.focus_epoch.get(),
                    input_epoch: request.input_epoch.get(),
                    resize_sequence: request.resize_seq.get(),
                    rows: request.rows,
                    cols: request.cols,
                })
                .await
                .map_err(|error| wire_error(request_id, error))
        })
        .await
}

#[tauri::command]
pub async fn telnet_terminal_disconnect(
    request: wire::TelnetSessionDisconnectRequest,
    service: State<'_, TelnetSessionService>,
) -> CoreResult<wire::TelnetSessionSummary> {
    let request_id = request.meta.request_id;
    let summary = service
        .disconnect(TelnetDisconnectRequest {
            operation_id: request.operation_id.as_str().to_owned(),
            idempotency_key: request.idempotency_key,
            session_id: request.session_id.as_str().to_owned(),
            expected_generation: request.expected_generation.get(),
            expected_state_revision: request.expected_state_revision.get(),
        })
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?;
    wire_summary(summary).map_err(|error| wire_error(request_id, error))
}

#[tauri::command]
pub async fn telnet_terminal_reconnect(
    request: wire::TelnetSessionReconnectRequest,
    service: State<'_, TelnetSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<wire::TelnetSessionSummary> {
    let request_id = request.meta.request_id;
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    let current = service
        .snapshot()
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?
        .into_iter()
        .find(|summary| summary.session_id == request.session_id.as_str())
        .ok_or_else(|| wire_error(request_id.clone(), TelnetServiceError::NotFound))?;
    let confirmation = &request.risk_confirmation;
    if confirmation.endpoint.address != current.address
        || confirmation.endpoint.port != current.port
        || !confirmation.accepts_cleartext_transport
        || !confirmation.accepts_missing_server_identity
        || !confirmation.accepts_observation_and_tampering
    {
        return Err(wire_error(
            request_id,
            TelnetServiceError::CleartextRiskNotAccepted,
        ));
    }
    let summary = service
        .reconnect(TelnetReconnectRequest {
            operation_id: request.operation_id.as_str().to_owned(),
            idempotency_key: request.idempotency_key,
            session_id: request.session_id.as_str().to_owned(),
            expected_generation: request.expected_generation.get(),
            expected_state_revision: request.expected_state_revision.get(),
            rows: request.rows,
            cols: request.cols,
            cleartext_risk_accepted: true,
        })
        .await
        .map_err(|error| wire_error(request_id.clone(), error))?;
    wire_summary(summary).map_err(|error| wire_error(request_id, error))
}

#[allow(clippy::too_many_arguments)]
async fn require_wire_focus_unserialized(
    broker: &crate::ssh_session_service::SshSessionService,
    request_id: wire::RequestId,
    session_id: &wire::TelnetSessionId,
    generation: wire::WireSequence,
    state_revision: wire::WireSequence,
    socket_id: &wire::TelnetSocketId,
    attachment_id: &wire::TelnetAttachmentId,
    view_id: &wire::TelnetViewId,
    focus_epoch: wire::WireSequence,
) -> CoreResult<()> {
    let snapshot = broker
        .terminal_focus_snapshot_unserialized(request_id.clone())
        .await?;
    let valid = matches!(
        snapshot.target,
        Some(wire::TerminalInputFocusTarget::Telnet(target))
            if snapshot.focus_epoch == focus_epoch
                && target.session_id == *session_id
                && target.expected_generation == generation
                && target.expected_state_revision == state_revision
                && target.socket_id == *socket_id
                && target.attachment_id == *attachment_id
                && target.view_id == *view_id
    );
    if valid {
        Ok(())
    } else {
        Err(wire_error(
            request_id,
            TelnetServiceError::InputNotAuthorized,
        ))
    }
}

fn wire_lease(
    lease: TelnetInputLease,
    socket_id: wire::TelnetSocketId,
) -> Result<wire::TelnetInputLease, TelnetServiceError> {
    Ok(wire::TelnetInputLease {
        lease_id: wire::TelnetInputLeaseId::parse(lease.lease_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        session_id: wire::TelnetSessionId::parse(lease.session_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        generation: wire::WireSequence::new(lease.generation),
        socket_id,
        attachment_id: wire::TelnetAttachmentId::parse(lease.attachment_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        view_id: wire::TelnetViewId::parse(lease.view_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        focus_epoch: wire::WireSequence::new(lease.focus_epoch),
        input_epoch: wire::WireSequence::new(lease.input_epoch),
        expires_at_unix_ms: lease.expires_at_unix_ms,
    })
}

fn wire_replay_item(
    attachment: &wire::TelnetSessionAttachment,
    item: TelnetReplayItem,
) -> Result<wire::TelnetSessionOutputItem, TelnetServiceError> {
    let socket_id = attachment
        .socket_id
        .clone()
        .ok_or(TelnetServiceError::Conflict)?;
    Ok(match item {
        TelnetReplayItem::Frame(frame) => {
            wire::TelnetSessionOutputItem::Frame(wire::TelnetSessionOutputFrame {
                session_id: attachment.session_id.clone(),
                generation: wire::WireSequence::new(frame.generation),
                socket_id,
                output_seq: wire::WireSequence::new(frame.sequence),
                bytes: frame.data,
            })
        }
        TelnetReplayItem::Gap {
            generation,
            dropped_through_sequence,
        } => wire::TelnetSessionOutputItem::Gap(wire::TelnetSessionOutputGap {
            session_id: attachment.session_id.clone(),
            generation: wire::WireSequence::new(generation),
            socket_id,
            dropped_from_output_seq: wire::WireSequence::new(1),
            resumes_at_output_seq: wire::WireSequence::new(
                dropped_through_sequence.saturating_add(1),
            ),
            reason: wire::TelnetSessionOutputGapReason::RingBufferOverflow,
        }),
    })
}

fn validate_wire_risk_confirmation(
    request: &wire::TelnetSessionOpenRequest,
) -> Result<(), TelnetServiceError> {
    let confirmation = &request.risk_confirmation;
    if confirmation.endpoint != request.endpoint
        || !confirmation.accepts_cleartext_transport
        || !confirmation.accepts_missing_server_identity
        || !confirmation.accepts_observation_and_tampering
    {
        return Err(TelnetServiceError::CleartextRiskNotAccepted);
    }
    Ok(())
}

fn wire_summary(
    summary: TelnetSessionSummary,
) -> Result<wire::TelnetSessionSummary, TelnetServiceError> {
    Ok(wire::TelnetSessionSummary {
        session_id: wire::TelnetSessionId::parse(summary.session_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        open_attempt_id: wire::TelnetOpenAttemptId::parse(summary.open_attempt_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        endpoint: wire::TelnetEndpoint {
            address: summary.address,
            port: summary.port,
        },
        generation: wire::WireSequence::new(summary.generation),
        state_revision: wire::WireSequence::new(summary.state_revision),
        attachment_revision: wire::WireSequence::new(summary.attachment_revision),
        event_seq: wire::WireSequence::new(summary.event_sequence),
        socket_id: summary
            .socket_id
            .map(wire::TelnetSocketId::parse)
            .transpose()
            .map_err(|_| TelnetServiceError::Conflict)?,
        state: wire_state(summary.state),
        attachment_count: summary.attachment_count,
        close_reason: summary.close_reason.map(wire_close_reason),
        failure_reason: summary.failure_reason.map(wire_failure_reason),
        created_at_unix_ms: summary.created_at_unix_ms,
        updated_at_unix_ms: summary.updated_at_unix_ms,
    })
}

fn wire_attachment(
    attachment: TelnetAttachment,
) -> Result<wire::TelnetSessionAttachment, TelnetServiceError> {
    Ok(wire::TelnetSessionAttachment {
        attachment_id: wire::TelnetAttachmentId::parse(attachment.attachment_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        attach_attempt_id: wire::TelnetAttachAttemptId::parse(attachment.attach_attempt_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        session_id: wire::TelnetSessionId::parse(attachment.session_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        generation: wire::WireSequence::new(attachment.generation),
        socket_id: attachment
            .socket_id
            .map(wire::TelnetSocketId::parse)
            .transpose()
            .map_err(|_| TelnetServiceError::Conflict)?,
        view_id: wire::TelnetViewId::parse(attachment.view_id)
            .map_err(|_| TelnetServiceError::Conflict)?,
        state_revision: wire::WireSequence::new(attachment.state_revision),
        attachment_revision: wire::WireSequence::new(attachment.attachment_revision),
        attached_at_unix_ms: attachment.attached_at_unix_ms,
    })
}

fn wire_state(state: TelnetSessionState) -> wire::TelnetSessionState {
    match state {
        TelnetSessionState::Connecting => wire::TelnetSessionState::Connecting,
        TelnetSessionState::Running => wire::TelnetSessionState::Running,
        TelnetSessionState::Disconnecting => wire::TelnetSessionState::Disconnecting,
        TelnetSessionState::Closed => wire::TelnetSessionState::Closed,
        TelnetSessionState::Failed => wire::TelnetSessionState::Failed,
    }
}

fn wire_close_reason(reason: TelnetCloseReason) -> wire::TelnetSessionCloseReason {
    match reason {
        TelnetCloseReason::UserRequested => wire::TelnetSessionCloseReason::UserRequested,
        TelnetCloseReason::RemoteClosed => wire::TelnetSessionCloseReason::RemoteClosed,
        TelnetCloseReason::ConnectionFailed => wire::TelnetSessionCloseReason::ConnectionFailed,
        TelnetCloseReason::CoreShutdown => wire::TelnetSessionCloseReason::CoreShutdown,
    }
}

fn wire_failure_reason(reason: TelnetFailureReason) -> wire::TelnetSessionFailureReason {
    let code = match reason.code {
        TelnetFailureCode::ResolveFailed => wire::TelnetSessionFailureCode::ResolveFailed,
        TelnetFailureCode::ConnectTimeout => wire::TelnetSessionFailureCode::ConnectTimeout,
        TelnetFailureCode::ConnectFailed => wire::TelnetSessionFailureCode::ConnectFailed,
        TelnetFailureCode::ProtocolViolation => wire::TelnetSessionFailureCode::ProtocolViolation,
        TelnetFailureCode::IoTimeout => wire::TelnetSessionFailureCode::IoTimeout,
        TelnetFailureCode::IoFailed => wire::TelnetSessionFailureCode::IoFailed,
    };
    wire::TelnetSessionFailureReason {
        code,
        message_key: reason.message_key,
        diagnostic_id: None,
    }
}

fn wire_event(event: TelnetSessionEvent) -> Option<wire::TelnetSessionEvent> {
    let (summary, payload) = match event {
        TelnetSessionEvent::StateChanged { session } => {
            let wire_session = wire_summary(session.clone()).ok()?;
            (
                session,
                wire::TelnetSessionEventPayload::StateChanged {
                    session: wire_session,
                },
            )
        }
        TelnetSessionEvent::AttachmentChanged {
            session,
            attachment,
        } => (
            session,
            wire::TelnetSessionEventPayload::AttachmentAttached {
                attachment: wire_attachment(attachment).ok()?,
            },
        ),
        TelnetSessionEvent::AttachmentDetached {
            session,
            attachment_id,
        } => (
            session,
            wire::TelnetSessionEventPayload::AttachmentDetached {
                attachment_id: wire::TelnetAttachmentId::parse(attachment_id).ok()?,
            },
        ),
        TelnetSessionEvent::InputLeaseRevoked {
            session,
            focus_epoch,
        } => (
            session,
            wire::TelnetSessionEventPayload::InputLeaseRevoked {
                focus_epoch: wire::WireSequence::new(focus_epoch),
            },
        ),
        TelnetSessionEvent::Output { session, frame } => {
            let socket_id = wire::TelnetSocketId::parse(session.socket_id.clone()?).ok()?;
            let session_id = wire::TelnetSessionId::parse(session.session_id.clone()).ok()?;
            (
                session,
                wire::TelnetSessionEventPayload::Output {
                    frame: wire::TelnetSessionOutputFrame {
                        session_id,
                        generation: wire::WireSequence::new(frame.generation),
                        socket_id,
                        output_seq: wire::WireSequence::new(frame.sequence),
                        bytes: frame.data,
                    },
                },
            )
        }
    };
    Some(wire::TelnetSessionEvent {
        schema_version: wire::TELNET_TERMINAL_EVENT_SCHEMA_VERSION,
        session_id: wire::TelnetSessionId::parse(summary.session_id).ok()?,
        generation: wire::WireSequence::new(summary.generation),
        state_revision: wire::WireSequence::new(summary.state_revision),
        event_seq: wire::WireSequence::new(summary.event_sequence),
        occurred_at_unix_ms: summary.updated_at_unix_ms,
        payload,
    })
}

fn wire_error(request_id: wire::RequestId, error: TelnetServiceError) -> Box<wire::CoreApiError> {
    let (code, category, retry_strategy, message_key) = match error {
        TelnetServiceError::Validation | TelnetServiceError::CleartextRiskNotAccepted => (
            "telnet.validation",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
            "errors.telnet.validation",
        ),
        TelnetServiceError::NotFound => (
            "telnet.not_found",
            wire::ErrorCategory::Conflict,
            wire::RetryStrategy::RefreshSnapshot,
            "errors.telnet.notFound",
        ),
        TelnetServiceError::Conflict
        | TelnetServiceError::LastAttachmentRequiresDisconnect
        | TelnetServiceError::InputNotAuthorized => (
            "telnet.conflict",
            wire::ErrorCategory::Conflict,
            wire::RetryStrategy::RefreshSnapshot,
            "errors.telnet.conflict",
        ),
        TelnetServiceError::Unavailable | TelnetServiceError::CleanupIncomplete => (
            "telnet.unavailable",
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::AfterMilliseconds(500),
            "errors.telnet.unavailable",
        ),
    };
    Box::new(wire::CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: message_key.to_owned(),
        params: Default::default(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}

async fn request_reply<T>(
    tx: &mpsc::Sender<Message>,
    build: impl FnOnce(oneshot::Sender<Result<T, TelnetServiceError>>) -> Message,
) -> Result<T, TelnetServiceError> {
    let (reply, response) = oneshot::channel();
    tx.send(build(reply))
        .await
        .map_err(|_| TelnetServiceError::Unavailable)?;
    response
        .await
        .map_err(|_| TelnetServiceError::Unavailable)?
}

fn trim_ledger<T>(ledger: &mut BTreeMap<String, T>, order: &mut VecDeque<String>) {
    while order.len() > OPERATION_LEDGER_CAPACITY {
        if let Some(expired) = order.pop_front() {
            ledger.remove(&expired);
        }
    }
}

async fn run_actor(mut actor: Actor, mut rx: mpsc::Receiver<Message>) {
    while let Some(message) = rx.recv().await {
        match message {
            Message::Open { request, reply } => {
                let _ = reply.send(actor.open(request));
            }
            Message::Subscribe { session_id, reply } => {
                let result = actor
                    .sessions
                    .get(&session_id)
                    .map(|record| record.events.subscribe())
                    .ok_or(TelnetServiceError::NotFound);
                let _ = reply.send(result);
            }
            Message::ChannelResync {
                session_id,
                attachment_id,
                reply,
            } => {
                let result = actor
                    .sessions
                    .get(&session_id)
                    .map(|record| TelnetChannelResync {
                        session: record.summary.clone(),
                        attachment: record
                            .attachments
                            .get(&attachment_id)
                            .map(|item| item.summary.clone()),
                        attachment_event_sequence: record
                            .attachments
                            .get(&attachment_id)
                            .map(|item| item.event_sequence)
                            .filter(|sequence| *sequence > 0),
                        detachment_event_sequence: record
                            .detached_attachment_events
                            .get(&attachment_id)
                            .copied(),
                        input_lease: record.input_lease.clone(),
                        last_revoked_focus_epoch: record.last_revoked_focus_epoch,
                        last_revoked_event_sequence: record.last_revoked_event_sequence,
                        last_state_event_sequence: record.last_state_event_sequence,
                        frames: record.output_ring.iter().cloned().collect(),
                    })
                    .ok_or(TelnetServiceError::NotFound);
                let _ = reply.send(result);
            }
            Message::Snapshot { reply } => {
                let _ = reply.send(
                    actor
                        .sessions
                        .values()
                        .map(|record| record.summary.clone())
                        .collect(),
                );
            }
            Message::Attach { request, reply } => {
                let _ = reply.send(actor.attach(request));
            }
            Message::AttachmentHeartbeat {
                session_id,
                expected_generation,
                expected_attachment_revision,
                attachment_id,
                view_id,
                reply,
            } => {
                let _ = reply.send(actor.attachment_heartbeat(
                    &session_id,
                    expected_generation,
                    expected_attachment_revision,
                    &attachment_id,
                    &view_id,
                ));
            }
            Message::Detach { request, reply } => {
                let _ = reply.send(actor.detach(request));
            }
            Message::GrantInput { request, reply } => {
                let _ = reply.send(actor.grant_input(request));
            }
            Message::RenewInput { lease, reply } => {
                let _ = reply.send(actor.renew_input(lease));
            }
            Message::RevokeInput {
                session_id,
                expected_focus_epoch,
                reply,
            } => {
                let _ = reply.send(actor.revoke_input(&session_id, expected_focus_epoch));
            }
            Message::Input { request, reply } => {
                let _ = reply.send(actor.input(request).await);
            }
            Message::Resize { request, reply } => {
                let _ = reply.send(actor.resize(request).await);
            }
            Message::Disconnect { request, reply } => {
                let _ = reply.send(actor.disconnect(request));
            }
            Message::Reconnect { request, reply } => {
                let _ = reply.send(actor.reconnect(request));
            }
            Message::Connected {
                session_id,
                generation,
                connection,
            } => actor.connected(&session_id, generation, connection),
            Message::ConnectFailed {
                session_id,
                generation,
                failure,
            } => actor.connect_failed(&session_id, generation, failure),
            Message::OwnerOutput {
                session_id,
                generation,
                data,
            } => actor.owner_output(&session_id, generation, data),
            Message::OwnerClosed {
                session_id,
                generation,
                reason,
                failure,
            } => actor.owner_closed(&session_id, generation, reason, failure),
            Message::ReapStaleAttachments => actor.reap_stale_attachments(),
            Message::ShutdownAll { reply } => {
                let _ = reply.send(actor.shutdown_all().await);
            }
            Message::Teardown { reply } => {
                actor.teardown().await;
                if let Some(reply) = reply {
                    let _ = reply.send(());
                }
                break;
            }
            #[cfg(test)]
            Message::Barrier { entered, release } => {
                let _ = entered.send(());
                let _ = release.await;
            }
            #[cfg(test)]
            Message::ReplaceOwner {
                session_id,
                commands,
                abort,
                reply,
            } => {
                let result = actor
                    .sessions
                    .get_mut(&session_id)
                    .ok_or(TelnetServiceError::NotFound)
                    .map(|record| {
                        if let Some(owner) = record.owner.take() {
                            owner.abort.abort();
                        }
                        record.owner = Some(OwnerHandle { commands, abort });
                    });
                let _ = reply.send(result);
            }
        }
    }
}

impl Actor {
    fn open(
        &mut self,
        request: TelnetOpenRequest,
    ) -> Result<TelnetOpenBinding, TelnetServiceError> {
        let port = request.port.unwrap_or(DEFAULT_TELNET_PORT);
        validate_open(&request, port)?;
        let fingerprint = OpenFingerprint {
            open_attempt_id: request.open_attempt_id.clone(),
            address: request.address.trim().to_owned(),
            port,
            rows: request.rows,
            cols: request.cols,
            cleartext_risk_accepted: request.cleartext_risk_accepted,
            attach_attempt_id: request.attach_attempt_id.clone(),
            view_id: request.view_id.clone(),
        };
        if let Some(existing) = self.open_ledger.get(&request.operation_id) {
            if existing.idempotency_key == request.idempotency_key
                && existing.fingerprint == fingerprint
            {
                let original = existing.response.clone();
                let record = self
                    .sessions
                    .get(&original.session.session_id)
                    .ok_or(TelnetServiceError::NotFound)?;
                let attachment = record
                    .attachments
                    .get(&original.attachment.attachment_id)
                    .map(|item| item.summary.clone())
                    .ok_or(TelnetServiceError::Conflict)?;
                return Ok(TelnetOpenBinding {
                    response: TelnetOpenResponse {
                        session: record.summary.clone(),
                        attachment,
                    },
                    events: record.events.subscribe(),
                    resync_immediately: true,
                });
            }
            return Err(TelnetServiceError::Conflict);
        }

        let now = unix_time_ms();
        let session_id = new_id();
        let attachment = TelnetAttachment {
            attachment_id: new_id(),
            attach_attempt_id: request.attach_attempt_id,
            session_id: session_id.clone(),
            generation: 1,
            socket_id: None,
            view_id: request.view_id,
            state_revision: 1,
            attachment_revision: 1,
            attached_at_unix_ms: now,
        };
        let summary = TelnetSessionSummary {
            session_id: session_id.clone(),
            open_attempt_id: request.open_attempt_id,
            address: fingerprint.address.clone(),
            port,
            generation: 1,
            state_revision: 1,
            attachment_revision: 1,
            event_sequence: 0,
            state: TelnetSessionState::Connecting,
            socket_id: None,
            attachment_count: 1,
            close_reason: None,
            failure_reason: None,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let event_receiver = events.subscribe();
        let tx = self.tx.clone();
        let connect_session_id = session_id.clone();
        let connect_config = TelnetConnectConfig {
            address: fingerprint.address.clone(),
            port,
            rows: request.rows,
            cols: request.cols,
            cleartext_risk_accepted: request.cleartext_risk_accepted,
            connect_timeout: Duration::from_secs(15),
            io_timeout: Duration::from_secs(10),
        };
        let connect_task = tokio::spawn(async move {
            match TelnetConnection::connect(connect_config).await {
                Ok(connection) => {
                    let _ = tx
                        .send(Message::Connected {
                            session_id: connect_session_id,
                            generation: 1,
                            connection: Box::new(connection),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = tx
                        .send(Message::ConnectFailed {
                            session_id: connect_session_id,
                            generation: 1,
                            failure: map_runtime_failure(&error),
                        })
                        .await;
                }
            }
        });
        self.sessions.insert(
            session_id.clone(),
            SessionRecord {
                summary: summary.clone(),
                attachments: BTreeMap::from([(
                    attachment.attachment_id.clone(),
                    AttachmentRecord {
                        summary: attachment.clone(),
                        last_seen_at_unix_ms: now,
                        event_sequence: 0,
                    },
                )]),
                events,
                input_lease: None,
                last_revoked_focus_epoch: 0,
                last_revoked_event_sequence: 0,
                last_state_event_sequence: 0,
                detached_attachment_events: BTreeMap::new(),
                detached_attachment_order: VecDeque::new(),
                next_input_epoch: 0,
                last_client_sequence: 0,
                last_resize_sequence: 0,
                next_output_sequence: 1,
                output_ring: VecDeque::new(),
                output_ring_bytes: 0,
                dropped_through_sequence: 0,
                owner: None,
                connect_abort: Some(connect_task.abort_handle()),
                detach_after_close: None,
                focus_invalidations: self.focus_invalidations.clone(),
            },
        );
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id);
        let response = TelnetOpenResponse {
            session: summary,
            attachment,
        };
        self.record_open(
            request.operation_id,
            OpenLedgerEntry {
                idempotency_key: request.idempotency_key,
                fingerprint,
                response: response.clone(),
            },
        );
        Ok(TelnetOpenBinding {
            response,
            events: event_receiver,
            resync_immediately: false,
        })
    }

    fn record_open(&mut self, operation_id: String, entry: OpenLedgerEntry) {
        self.open_ledger.insert(operation_id.clone(), entry);
        self.open_order.push_back(operation_id);
        while self.open_order.len() > OPERATION_LEDGER_CAPACITY {
            if let Some(expired) = self.open_order.pop_front() {
                self.open_ledger.remove(&expired);
            }
        }
    }

    fn connected(&mut self, session_id: &str, generation: u64, connection: Box<TelnetConnection>) {
        let connection = *connection;
        let Some(record) = self.sessions.get_mut(session_id) else {
            tokio::spawn(async move {
                let _ = connection.disconnect().await;
            });
            return;
        };
        if record.summary.generation != generation
            || record.summary.state != TelnetSessionState::Connecting
        {
            tokio::spawn(async move {
                let _ = connection.disconnect().await;
            });
            return;
        }
        record.connect_abort = None;
        let socket_id = new_id();
        record.summary.socket_id = Some(socket_id.clone());
        for attachment in record.attachments.values_mut() {
            attachment.summary.socket_id = Some(socket_id.clone());
        }
        let (commands_tx, commands_rx) = mpsc::channel(OWNER_MAILBOX_CAPACITY);
        transition(record, TelnetSessionState::Running, None, None);
        emit_state(record);
        emit_current_attachments(record);
        let owner_task = tokio::spawn(run_owner(
            session_id.to_owned(),
            generation,
            connection,
            commands_rx,
            self.tx.clone(),
        ));
        record.owner = Some(OwnerHandle {
            commands: commands_tx,
            abort: owner_task.abort_handle(),
        });
    }

    fn connect_failed(&mut self, session_id: &str, generation: u64, failure: TelnetFailureReason) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation != generation
            || record.summary.state != TelnetSessionState::Connecting
        {
            return;
        }
        record.connect_abort = None;
        transition(
            record,
            TelnetSessionState::Failed,
            Some(TelnetCloseReason::ConnectionFailed),
            Some(failure),
        );
        emit_state(record);
        self.remove_live(session_id);
    }

    fn attach(
        &mut self,
        request: TelnetAttachRequest,
    ) -> Result<TelnetAttachBinding, TelnetServiceError> {
        if request.operation_id.trim().is_empty() || request.idempotency_key.trim().is_empty() {
            return Err(TelnetServiceError::Validation);
        }
        let fingerprint = AttachFingerprint {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            attach_attempt_id: request.attach_attempt_id.clone(),
            view_id: request.view_id.clone(),
            after_output_sequence: request.after_output_sequence,
        };
        if let Some(existing) = self.attach_ledger.get(&request.operation_id) {
            if existing.idempotency_key != request.idempotency_key
                || existing.fingerprint != fingerprint
            {
                return Err(TelnetServiceError::Conflict);
            }
            let response = existing.result.clone()?;
            let events = self
                .sessions
                .get(&response.attachment.session_id)
                .map(|record| record.events.subscribe())
                .ok_or(TelnetServiceError::NotFound)?;
            return Ok(TelnetAttachBinding { response, events });
        }
        let result = self.attach_once(request.clone());
        self.attach_ledger.insert(
            request.operation_id.clone(),
            AttachLedgerEntry {
                idempotency_key: request.idempotency_key,
                fingerprint,
                result: result.clone(),
            },
        );
        self.attach_order.push_back(request.operation_id);
        trim_ledger(&mut self.attach_ledger, &mut self.attach_order);
        let response = result?;
        let events = self
            .sessions
            .get(&response.attachment.session_id)
            .map(|record| record.events.subscribe())
            .ok_or(TelnetServiceError::NotFound)?;
        Ok(TelnetAttachBinding { response, events })
    }

    fn attach_once(
        &mut self,
        request: TelnetAttachRequest,
    ) -> Result<TelnetAttachResponse, TelnetServiceError> {
        if request.attach_attempt_id.trim().is_empty() || request.view_id.trim().is_empty() {
            return Err(TelnetServiceError::Validation);
        }
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        validate_generation(record, request.expected_generation)?;
        if record.summary.state_revision != request.expected_state_revision {
            return Err(TelnetServiceError::Conflict);
        }
        if record
            .attachments
            .values()
            .any(|item| item.summary.attach_attempt_id == request.attach_attempt_id)
        {
            return Err(TelnetServiceError::Conflict);
        }
        let replaced = record
            .attachments
            .iter()
            .find(|(_, item)| item.summary.view_id == request.view_id)
            .map(|(key, _)| key.clone())
            .and_then(|key| record.attachments.remove(&key));
        if let Some(replaced) = replaced {
            revoke_lease_for_attachment(record, &replaced.summary.attachment_id);
            record.summary.attachment_revision += 1;
            record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
            remember_detachment(
                record,
                replaced.summary.attachment_id.clone(),
                record.summary.event_sequence,
            );
            let _ = record.events.send(TelnetSessionEvent::AttachmentDetached {
                session: record.summary.clone(),
                attachment_id: replaced.summary.attachment_id,
            });
        }
        record.summary.attachment_revision += 1;
        let attachment = TelnetAttachment {
            attachment_id: new_id(),
            attach_attempt_id: request.attach_attempt_id,
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            socket_id: record.summary.socket_id.clone(),
            view_id: request.view_id,
            state_revision: record.summary.state_revision,
            attachment_revision: record.summary.attachment_revision,
            attached_at_unix_ms: unix_time_ms(),
        };
        record.attachments.insert(
            attachment.attachment_id.clone(),
            AttachmentRecord {
                summary: attachment.clone(),
                last_seen_at_unix_ms: unix_time_ms(),
                event_sequence: 0,
            },
        );
        record.summary.attachment_count = record.attachments.len() as u32;
        record.summary.updated_at_unix_ms = unix_time_ms();
        record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
        if let Some(inserted) = record.attachments.get_mut(&attachment.attachment_id) {
            inserted.event_sequence = record.summary.event_sequence;
        }
        let replay = replay_after(record, request.after_output_sequence);
        let _ = record.events.send(TelnetSessionEvent::AttachmentChanged {
            session: record.summary.clone(),
            attachment: attachment.clone(),
        });
        Ok(TelnetAttachResponse { attachment, replay })
    }

    fn attachment_heartbeat(
        &mut self,
        session_id: &str,
        generation: u64,
        expected_attachment_revision: u64,
        attachment_id: &str,
        view_id: &str,
    ) -> Result<TelnetAttachment, TelnetServiceError> {
        let record = self
            .sessions
            .get_mut(session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        validate_generation(record, generation)?;
        if record.summary.attachment_revision != expected_attachment_revision {
            return Err(TelnetServiceError::Conflict);
        }
        let attachment = validate_attachment_mut(record, attachment_id, view_id)?;
        attachment.last_seen_at_unix_ms = unix_time_ms();
        Ok(attachment.summary.clone())
    }

    fn detach(
        &mut self,
        request: TelnetDetachRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        if request.operation_id.trim().is_empty() || request.idempotency_key.trim().is_empty() {
            return Err(TelnetServiceError::Validation);
        }
        let fingerprint = DetachFingerprint {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            expected_attachment_revision: request.expected_attachment_revision,
            attachment_id: request.attachment_id.clone(),
            view_id: request.view_id.clone(),
            intent: request.intent,
            disconnect_if_last: request.disconnect_if_last,
        };
        if let Some(existing) = self.detach_ledger.get(&request.operation_id) {
            if existing.idempotency_key == request.idempotency_key
                && existing.fingerprint == fingerprint
            {
                return existing.result.clone();
            }
            return Err(TelnetServiceError::Conflict);
        }
        let result = self.detach_once(&request);
        self.detach_ledger.insert(
            request.operation_id.clone(),
            DetachLedgerEntry {
                idempotency_key: request.idempotency_key,
                fingerprint,
                result: result.clone(),
            },
        );
        self.detach_order.push_back(request.operation_id);
        trim_ledger(&mut self.detach_ledger, &mut self.detach_order);
        result
    }

    fn detach_once(
        &mut self,
        request: &TelnetDetachRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        let session_id = request.session_id.as_str();
        let attachment_id = request.attachment_id.as_str();
        let record = self
            .sessions
            .get_mut(session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        validate_generation(record, request.expected_generation)?;
        if record.summary.state_revision != request.expected_state_revision
            || record.summary.attachment_revision != request.expected_attachment_revision
        {
            return Err(TelnetServiceError::Conflict);
        }
        validate_attachment(record, attachment_id, &request.view_id)?;
        if request.intent == TelnetDetachIntent::RendererUnavailable {
            remove_attachment(record, attachment_id);
            return Ok(record.summary.clone());
        }
        let active = matches!(
            record.summary.state,
            TelnetSessionState::Connecting
                | TelnetSessionState::Running
                | TelnetSessionState::Disconnecting
        );
        if record.attachments.len() == 1 && active {
            if !request.disconnect_if_last {
                return Err(TelnetServiceError::LastAttachmentRequiresDisconnect);
            }
            record.detach_after_close = Some(attachment_id.to_owned());
            let result = disconnect_record(record, TelnetCloseReason::UserRequested);
            if record.summary.state == TelnetSessionState::Closed {
                record.detach_after_close = None;
                remove_attachment(record, attachment_id);
                self.live_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(session_id);
                return result.map(|_| record.summary.clone());
            }
            return result;
        }
        remove_attachment(record, attachment_id);
        Ok(record.summary.clone())
    }

    fn grant_input(
        &mut self,
        request: TelnetInputGrantRequest,
    ) -> Result<TelnetInputLease, TelnetServiceError> {
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        validate_generation(record, request.expected_generation)?;
        if record.summary.state != TelnetSessionState::Running {
            return Err(TelnetServiceError::Conflict);
        }
        if record.summary.state_revision != request.expected_state_revision {
            return Err(TelnetServiceError::Conflict);
        }
        validate_attachment(record, &request.attachment_id, &request.view_id)?;
        if record.summary.socket_id.as_deref() != Some(request.expected_socket_id.as_str()) {
            return Err(TelnetServiceError::Conflict);
        }
        if let Some(current) = &record.input_lease
            && current.generation == request.expected_generation
            && current.socket_id == request.expected_socket_id
            && current.attachment_id == request.attachment_id
            && current.view_id == request.view_id
            && current.state_revision == request.expected_state_revision
            && current.focus_epoch == request.focus_epoch
            && current.expires_at_unix_ms > unix_time_ms()
        {
            return Ok(current.clone());
        }
        if let Some(old) = take_input_lease(record, true) {
            record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
            record.last_revoked_event_sequence = record.summary.event_sequence;
            let _ = record.events.send(TelnetSessionEvent::InputLeaseRevoked {
                session: record.summary.clone(),
                focus_epoch: old.focus_epoch,
            });
        }
        record.next_input_epoch = record.next_input_epoch.saturating_add(1);
        let lease = TelnetInputLease {
            lease_id: new_id(),
            session_id: request.session_id,
            generation: request.expected_generation,
            socket_id: request.expected_socket_id,
            attachment_id: request.attachment_id,
            view_id: request.view_id,
            state_revision: request.expected_state_revision,
            focus_epoch: request.focus_epoch,
            input_epoch: record.next_input_epoch,
            expires_at_unix_ms: unix_time_ms().saturating_add(INPUT_LEASE_MILLIS),
        };
        record.input_lease = Some(lease.clone());
        Ok(lease)
    }

    fn renew_input(
        &mut self,
        lease: TelnetInputLease,
    ) -> Result<TelnetInputLease, TelnetServiceError> {
        let record = self
            .sessions
            .get_mut(&lease.session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        validate_lease(record, &lease)?;
        let current = record
            .input_lease
            .as_mut()
            .ok_or(TelnetServiceError::InputNotAuthorized)?;
        current.expires_at_unix_ms = unix_time_ms().saturating_add(INPUT_LEASE_MILLIS);
        Ok(current.clone())
    }

    fn revoke_input(
        &mut self,
        session_id: &str,
        expected_focus_epoch: u64,
    ) -> Result<(), TelnetServiceError> {
        let record = self
            .sessions
            .get_mut(session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        if let Some(lease) = &record.input_lease
            && lease.focus_epoch != expected_focus_epoch
        {
            return Err(TelnetServiceError::Conflict);
        }
        if let Some(lease) = take_input_lease(record, false) {
            record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
            record.last_revoked_event_sequence = record.summary.event_sequence;
            let _ = record.events.send(TelnetSessionEvent::InputLeaseRevoked {
                session: record.summary.clone(),
                focus_epoch: lease.focus_epoch,
            });
        }
        Ok(())
    }

    async fn input(&mut self, request: TelnetInputRequest) -> Result<(), TelnetServiceError> {
        let validation = (|| {
            let record = self
                .sessions
                .get_mut(&request.session_id)
                .ok_or(TelnetServiceError::NotFound)?;
            validate_input_request(record, &request)?;
            if request.client_sequence <= record.last_client_sequence {
                return Err(TelnetServiceError::Conflict);
            }
            record
                .owner
                .as_ref()
                .map(|owner| owner.commands.clone())
                .ok_or(TelnetServiceError::Unavailable)
        })();
        let owner = validation?;
        let (owner_reply, owner_response) = oneshot::channel();
        owner
            .try_send(OwnerCommand::Input {
                data: request.data,
                reply: owner_reply,
            })
            .map_err(|_| TelnetServiceError::Unavailable)?;
        owner_response
            .await
            .map_err(|_| TelnetServiceError::Unavailable)??;
        self.sessions
            .get_mut(&request.session_id)
            .ok_or(TelnetServiceError::NotFound)?
            .last_client_sequence = request.client_sequence;
        Ok(())
    }

    async fn resize(&mut self, request: TelnetResizeRequest) -> Result<(), TelnetServiceError> {
        let validation = self.validate_resize(&request);
        let owner = validation?;
        let (owner_reply, owner_response) = oneshot::channel();
        owner
            .try_send(OwnerCommand::Resize {
                rows: request.rows,
                cols: request.cols,
                reply: owner_reply,
            })
            .map_err(|_| TelnetServiceError::Unavailable)?;
        owner_response
            .await
            .map_err(|_| TelnetServiceError::Unavailable)??;
        self.sessions
            .get_mut(&request.session_id)
            .ok_or(TelnetServiceError::NotFound)?
            .last_resize_sequence = request.resize_sequence;
        Ok(())
    }

    fn validate_resize(
        &self,
        request: &TelnetResizeRequest,
    ) -> Result<mpsc::Sender<OwnerCommand>, TelnetServiceError> {
        if request.rows == 0 || request.cols == 0 {
            return Err(TelnetServiceError::Validation);
        }
        let record = self
            .sessions
            .get(&request.session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        validate_lease_fields(
            record,
            TelnetLeaseFence {
                generation: request.expected_generation,
                socket_id: &request.socket_id,
                attachment_id: &request.attachment_id,
                view_id: &request.view_id,
                lease_id: &request.lease_id,
                focus_epoch: request.focus_epoch,
                input_epoch: request.input_epoch,
            },
        )?;
        if request.resize_sequence <= record.last_resize_sequence {
            return Err(TelnetServiceError::Conflict);
        }
        record
            .owner
            .as_ref()
            .map(|owner| owner.commands.clone())
            .ok_or(TelnetServiceError::Unavailable)
    }

    fn disconnect(
        &mut self,
        request: TelnetDisconnectRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        if request.operation_id.trim().is_empty() || request.idempotency_key.trim().is_empty() {
            return Err(TelnetServiceError::Validation);
        }
        let fingerprint = DisconnectFingerprint {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
        };
        if let Some(existing) = self.disconnect_ledger.get(&request.operation_id) {
            if existing.idempotency_key == request.idempotency_key
                && existing.fingerprint == fingerprint
            {
                return existing.result.clone();
            }
            return Err(TelnetServiceError::Conflict);
        }
        let session_id = request.session_id.as_str();
        let (result, closed) = match self.sessions.get_mut(session_id) {
            Some(record) => {
                let result =
                    validate_generation(record, request.expected_generation).and_then(|()| {
                        if record.summary.state_revision != request.expected_state_revision {
                            return Err(TelnetServiceError::Conflict);
                        }
                        disconnect_record(record, TelnetCloseReason::UserRequested)
                    });
                (result, record.summary.state == TelnetSessionState::Closed)
            }
            None => (Err(TelnetServiceError::NotFound), false),
        };
        if closed {
            self.remove_live(session_id);
        }
        self.disconnect_ledger.insert(
            request.operation_id.clone(),
            DisconnectLedgerEntry {
                idempotency_key: request.idempotency_key,
                fingerprint,
                result: result.clone(),
            },
        );
        self.disconnect_order.push_back(request.operation_id);
        trim_ledger(&mut self.disconnect_ledger, &mut self.disconnect_order);
        result
    }

    fn reconnect(
        &mut self,
        request: TelnetReconnectRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        if request.operation_id.trim().is_empty()
            || request.idempotency_key.trim().is_empty()
            || !request.cleartext_risk_accepted
            || request.rows == 0
            || request.cols == 0
        {
            return Err(if request.cleartext_risk_accepted {
                TelnetServiceError::Validation
            } else {
                TelnetServiceError::CleartextRiskNotAccepted
            });
        }
        let fingerprint = ReconnectFingerprint {
            session_id: request.session_id.clone(),
            expected_generation: request.expected_generation,
            expected_state_revision: request.expected_state_revision,
            rows: request.rows,
            cols: request.cols,
            cleartext_risk_accepted: request.cleartext_risk_accepted,
        };
        if let Some(existing) = self.reconnect_ledger.get(&request.operation_id) {
            if existing.idempotency_key == request.idempotency_key
                && existing.fingerprint == fingerprint
            {
                return existing.result.clone();
            }
            return Err(TelnetServiceError::Conflict);
        }

        let result = self.reconnect_once(&request);
        self.reconnect_ledger.insert(
            request.operation_id.clone(),
            ReconnectLedgerEntry {
                idempotency_key: request.idempotency_key,
                fingerprint,
                result: result.clone(),
            },
        );
        self.reconnect_order.push_back(request.operation_id);
        trim_ledger(&mut self.reconnect_ledger, &mut self.reconnect_order);
        result
    }

    fn reconnect_once(
        &mut self,
        request: &TelnetReconnectRequest,
    ) -> Result<TelnetSessionSummary, TelnetServiceError> {
        let tx = self.tx.clone();
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .ok_or(TelnetServiceError::NotFound)?;
        validate_generation(record, request.expected_generation)?;
        if record.summary.state_revision != request.expected_state_revision {
            return Err(TelnetServiceError::Conflict);
        }
        if !matches!(
            record.summary.state,
            TelnetSessionState::Closed | TelnetSessionState::Failed
        ) || record.owner.is_some()
            || record.connect_abort.is_some()
        {
            return Err(TelnetServiceError::Conflict);
        }
        record.summary.generation = record.summary.generation.saturating_add(1);
        let generation = record.summary.generation;
        record.summary.socket_id = None;
        record.summary.close_reason = None;
        record.summary.failure_reason = None;
        let _ = take_input_lease(record, true);
        record.last_client_sequence = 0;
        record.last_resize_sequence = 0;
        record.next_output_sequence = 1;
        record.output_ring.clear();
        record.output_ring_bytes = 0;
        record.dropped_through_sequence = 0;
        for attachment in record.attachments.values_mut() {
            attachment.summary.generation = generation;
            attachment.summary.socket_id = None;
        }
        transition(record, TelnetSessionState::Connecting, None, None);
        emit_state(record);
        let session_id = record.summary.session_id.clone();
        let connect_config = TelnetConnectConfig {
            address: record.summary.address.clone(),
            port: record.summary.port,
            rows: request.rows,
            cols: request.cols,
            cleartext_risk_accepted: true,
            connect_timeout: Duration::from_secs(15),
            io_timeout: Duration::from_secs(10),
        };
        let connect_session_id = session_id.clone();
        let connect_task = tokio::spawn(async move {
            match TelnetConnection::connect(connect_config).await {
                Ok(connection) => {
                    let _ = tx
                        .send(Message::Connected {
                            session_id: connect_session_id,
                            generation,
                            connection: Box::new(connection),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = tx
                        .send(Message::ConnectFailed {
                            session_id: connect_session_id,
                            generation,
                            failure: map_runtime_failure(&error),
                        })
                        .await;
                }
            }
        });
        record.connect_abort = Some(connect_task.abort_handle());
        let response = record.summary.clone();
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id);
        Ok(response)
    }

    fn owner_output(&mut self, session_id: &str, generation: u64, data: Vec<u8>) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation != generation
            || record.summary.state != TelnetSessionState::Running
            || data.is_empty()
        {
            return;
        }
        let frame = TelnetOutputFrame {
            generation,
            sequence: record.next_output_sequence,
            data,
            event_sequence: record.summary.event_sequence.saturating_add(1),
        };
        record.next_output_sequence = record.next_output_sequence.saturating_add(1);
        record.output_ring_bytes = record.output_ring_bytes.saturating_add(frame.data.len());
        record.output_ring.push_back(frame.clone());
        while record.output_ring_bytes > OUTPUT_RING_MAX_BYTES {
            let Some(dropped) = record.output_ring.pop_front() else {
                break;
            };
            record.output_ring_bytes = record.output_ring_bytes.saturating_sub(dropped.data.len());
            record.dropped_through_sequence = dropped.sequence;
        }
        record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
        let _ = record.events.send(TelnetSessionEvent::Output {
            session: record.summary.clone(),
            frame,
        });
    }

    fn owner_closed(
        &mut self,
        session_id: &str,
        generation: u64,
        reason: TelnetCloseReason,
        failure: Option<TelnetFailureReason>,
    ) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation != generation
            || matches!(
                record.summary.state,
                TelnetSessionState::Closed | TelnetSessionState::Failed
            )
        {
            return;
        }
        record.owner = None;
        let _ = take_input_lease(record, true);
        let state = if failure.is_some() {
            TelnetSessionState::Failed
        } else {
            TelnetSessionState::Closed
        };
        transition(record, state, Some(reason), failure);
        if let Some(attachment_id) = record.detach_after_close.take() {
            remove_attachment(record, &attachment_id);
        }
        emit_state(record);
        self.remove_live(session_id);
    }

    fn reap_stale_attachments(&mut self) {
        let now = unix_time_ms();
        for record in self.sessions.values_mut() {
            let stale: Vec<_> = record
                .attachments
                .values()
                .filter(|item| {
                    now.saturating_sub(item.last_seen_at_unix_ms) > ATTACHMENT_TIMEOUT_MILLIS
                })
                .map(|item| item.summary.attachment_id.clone())
                .collect();
            for attachment_id in stale {
                remove_attachment(record, &attachment_id);
            }
        }
    }

    async fn shutdown_all(&mut self) -> Result<(), TelnetServiceError> {
        let mut tasks = Vec::new();
        for record in self.sessions.values_mut() {
            if let Some(task) = record.connect_abort.take() {
                task.abort();
                tasks.push(task);
            }
            let _ = take_input_lease(record, true);
            if let Some(owner) = &record.owner {
                if owner
                    .commands
                    .try_send(OwnerCommand::Disconnect {
                        reason: TelnetCloseReason::CoreShutdown,
                        notify_actor: false,
                        reply: None,
                    })
                    .is_err()
                {
                    // Exit cleanup cannot wait behind a saturated writer
                    // mailbox. Aborting the owner drops both TCP halves.
                    owner.abort.abort();
                }
                tasks.push(owner.abort.clone());
            }
            if !matches!(
                record.summary.state,
                TelnetSessionState::Closed | TelnetSessionState::Failed
            ) {
                transition(record, TelnetSessionState::Disconnecting, None, None);
                emit_state(record);
            }
        }

        if !wait_for_tasks(&tasks, SHUTDOWN_TIMEOUT).await {
            for task in &tasks {
                if !task.is_finished() {
                    task.abort();
                }
            }
            if !wait_for_tasks(&tasks, SHUTDOWN_ABORT_GRACE).await {
                return Err(TelnetServiceError::CleanupIncomplete);
            }
        }

        for record in self.sessions.values_mut() {
            record.connect_abort = None;
            record.owner = None;
            if !matches!(
                record.summary.state,
                TelnetSessionState::Closed | TelnetSessionState::Failed
            ) {
                transition(
                    record,
                    TelnetSessionState::Closed,
                    Some(TelnetCloseReason::CoreShutdown),
                    None,
                );
                emit_state(record);
            }
        }
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Ok(())
    }

    async fn teardown(&mut self) {
        let _ = self.shutdown_all().await;
        for record in self.sessions.values_mut() {
            if let Some(task) = record.connect_abort.take() {
                task.abort();
            }
            if let Some(owner) = record.owner.take() {
                owner.abort.abort();
            }
        }
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn remove_live(&self, session_id: &str) {
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }
}

async fn wait_for_tasks(tasks: &[AbortHandle], timeout: Duration) -> bool {
    if tasks.iter().all(AbortHandle::is_finished) {
        return true;
    }
    tokio::time::timeout(timeout, async {
        loop {
            if tasks.iter().all(AbortHandle::is_finished) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

async fn run_owner(
    session_id: String,
    generation: u64,
    mut connection: TelnetConnection,
    mut commands: mpsc::Receiver<OwnerCommand>,
    actor: mpsc::Sender<Message>,
) {
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                match command {
                    Some(OwnerCommand::Input { data, reply }) => {
                        let result = connection.send_input(&data).await;
                        let failure = result.as_ref().err().map(map_runtime_failure);
                        let _ = reply.send(result.map_err(|_| TelnetServiceError::Unavailable));
                        if let Some(failure) = failure {
                            let _ = actor.send(Message::OwnerClosed {
                                session_id,
                                generation,
                                reason: TelnetCloseReason::ConnectionFailed,
                                failure: Some(failure),
                            }).await;
                            break;
                        }
                    }
                    Some(OwnerCommand::Resize { rows, cols, reply }) => {
                        let result = connection.resize(rows, cols).await;
                        let failure = result.as_ref().err().map(map_runtime_failure);
                        let _ = reply.send(result.map_err(|_| TelnetServiceError::Unavailable));
                        if let Some(failure) = failure {
                            let _ = actor.send(Message::OwnerClosed {
                                session_id,
                                generation,
                                reason: TelnetCloseReason::ConnectionFailed,
                                failure: Some(failure),
                            }).await;
                            break;
                        }
                    }
                    Some(OwnerCommand::Disconnect { reason, notify_actor, reply }) => {
                        let _ = connection.disconnect().await;
                        if let Some(reply) = reply {
                            let _ = reply.send(());
                        }
                        if notify_actor {
                            let _ = actor.send(Message::OwnerClosed {
                                session_id,
                                generation,
                                reason,
                                failure: None,
                            }).await;
                        }
                        break;
                    }
                    None => {
                        let _ = connection.disconnect().await;
                        break;
                    }
                }
            }
            event = connection.next_event() => {
                match event {
                    Ok(TelnetEvent::Data(data)) => {
                        if actor.send(Message::OwnerOutput {
                            session_id: session_id.clone(),
                            generation,
                            data,
                        }).await.is_err() {
                            break;
                        }
                    }
                    Ok(TelnetEvent::Closed) => {
                        let _ = actor.send(Message::OwnerClosed {
                            session_id,
                            generation,
                            reason: TelnetCloseReason::RemoteClosed,
                            failure: None,
                        }).await;
                        break;
                    }
                    Err(error) => {
                        let _ = actor.send(Message::OwnerClosed {
                            session_id,
                            generation,
                            reason: TelnetCloseReason::ConnectionFailed,
                            failure: Some(map_runtime_failure(&error)),
                        }).await;
                        break;
                    }
                }
            }
        }
    }
}

fn validate_open(request: &TelnetOpenRequest, port: u16) -> Result<(), TelnetServiceError> {
    if !request.cleartext_risk_accepted {
        return Err(TelnetServiceError::CleartextRiskNotAccepted);
    }
    if request.operation_id.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || request.address.trim().is_empty()
        || request.attach_attempt_id.trim().is_empty()
        || request.view_id.trim().is_empty()
        || port == 0
        || request.rows == 0
        || request.cols == 0
    {
        return Err(TelnetServiceError::Validation);
    }
    Ok(())
}

fn validate_generation(record: &SessionRecord, generation: u64) -> Result<(), TelnetServiceError> {
    if record.summary.generation == generation {
        Ok(())
    } else {
        Err(TelnetServiceError::Conflict)
    }
}

fn validate_attachment<'a>(
    record: &'a SessionRecord,
    attachment_id: &str,
    view_id: &str,
) -> Result<&'a AttachmentRecord, TelnetServiceError> {
    record
        .attachments
        .get(attachment_id)
        .filter(|item| item.summary.view_id == view_id)
        .ok_or(TelnetServiceError::Conflict)
}

fn validate_attachment_mut<'a>(
    record: &'a mut SessionRecord,
    attachment_id: &str,
    view_id: &str,
) -> Result<&'a mut AttachmentRecord, TelnetServiceError> {
    record
        .attachments
        .get_mut(attachment_id)
        .filter(|item| item.summary.view_id == view_id)
        .ok_or(TelnetServiceError::Conflict)
}

fn validate_lease(
    record: &SessionRecord,
    lease: &TelnetInputLease,
) -> Result<(), TelnetServiceError> {
    validate_lease_fields(
        record,
        TelnetLeaseFence {
            generation: lease.generation,
            socket_id: &lease.socket_id,
            attachment_id: &lease.attachment_id,
            view_id: &lease.view_id,
            lease_id: &lease.lease_id,
            focus_epoch: lease.focus_epoch,
            input_epoch: lease.input_epoch,
        },
    )
}

fn validate_input_request(
    record: &SessionRecord,
    request: &TelnetInputRequest,
) -> Result<(), TelnetServiceError> {
    validate_lease_fields(
        record,
        TelnetLeaseFence {
            generation: request.expected_generation,
            socket_id: &request.socket_id,
            attachment_id: &request.attachment_id,
            view_id: &request.view_id,
            lease_id: &request.lease_id,
            focus_epoch: request.focus_epoch,
            input_epoch: request.input_epoch,
        },
    )
}

struct TelnetLeaseFence<'a> {
    generation: u64,
    socket_id: &'a str,
    attachment_id: &'a str,
    view_id: &'a str,
    lease_id: &'a str,
    focus_epoch: u64,
    input_epoch: u64,
}

fn validate_lease_fields(
    record: &SessionRecord,
    fence: TelnetLeaseFence<'_>,
) -> Result<(), TelnetServiceError> {
    validate_generation(record, fence.generation)?;
    validate_attachment(record, fence.attachment_id, fence.view_id)?;
    if record.summary.state != TelnetSessionState::Running
        || record.summary.socket_id.as_deref() != Some(fence.socket_id)
    {
        return Err(TelnetServiceError::InputNotAuthorized);
    }
    let lease = record
        .input_lease
        .as_ref()
        .ok_or(TelnetServiceError::InputNotAuthorized)?;
    if lease.lease_id != fence.lease_id
        || lease.generation != fence.generation
        || lease.socket_id != fence.socket_id
        || lease.attachment_id != fence.attachment_id
        || lease.view_id != fence.view_id
        || lease.focus_epoch != fence.focus_epoch
        || lease.input_epoch != fence.input_epoch
        || lease.expires_at_unix_ms < unix_time_ms()
    {
        return Err(TelnetServiceError::InputNotAuthorized);
    }
    Ok(())
}

fn disconnect_record(
    record: &mut SessionRecord,
    reason: TelnetCloseReason,
) -> Result<TelnetSessionSummary, TelnetServiceError> {
    if matches!(
        record.summary.state,
        TelnetSessionState::Closed | TelnetSessionState::Failed
    ) {
        return Ok(record.summary.clone());
    }
    let _ = take_input_lease(record, true);
    if let Some(connect) = record.connect_abort.take() {
        connect.abort();
        transition(record, TelnetSessionState::Closed, Some(reason), None);
        emit_state(record);
        return Ok(record.summary.clone());
    }
    let owner = record
        .owner
        .as_ref()
        .ok_or(TelnetServiceError::Unavailable)?;
    owner
        .commands
        .try_send(OwnerCommand::Disconnect {
            reason,
            notify_actor: true,
            reply: None,
        })
        .map_err(|_| TelnetServiceError::Unavailable)?;
    transition(record, TelnetSessionState::Disconnecting, None, None);
    emit_state(record);
    Ok(record.summary.clone())
}

fn remove_attachment(record: &mut SessionRecord, attachment_id: &str) {
    let Some(removed) = record.attachments.remove(attachment_id) else {
        return;
    };
    revoke_lease_for_attachment(record, attachment_id);
    record.summary.attachment_revision = record.summary.attachment_revision.saturating_add(1);
    record.summary.attachment_count = record.attachments.len() as u32;
    record.summary.updated_at_unix_ms = unix_time_ms();
    record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
    remember_detachment(
        record,
        removed.summary.attachment_id.clone(),
        record.summary.event_sequence,
    );
    let _ = record.events.send(TelnetSessionEvent::AttachmentDetached {
        session: record.summary.clone(),
        attachment_id: removed.summary.attachment_id,
    });
}

fn remember_detachment(record: &mut SessionRecord, attachment_id: String, event_sequence: u64) {
    if record
        .detached_attachment_events
        .insert(attachment_id.clone(), event_sequence)
        .is_none()
    {
        record.detached_attachment_order.push_back(attachment_id);
    }
    while record.detached_attachment_order.len() > OPERATION_LEDGER_CAPACITY {
        if let Some(expired) = record.detached_attachment_order.pop_front() {
            record.detached_attachment_events.remove(&expired);
        }
    }
}

fn revoke_lease_for_attachment(record: &mut SessionRecord, attachment_id: &str) {
    if record
        .input_lease
        .as_ref()
        .is_some_and(|lease| lease.attachment_id == attachment_id)
        && let Some(lease) = take_input_lease(record, true)
    {
        record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
        record.last_revoked_event_sequence = record.summary.event_sequence;
        let _ = record.events.send(TelnetSessionEvent::InputLeaseRevoked {
            session: record.summary.clone(),
            focus_epoch: lease.focus_epoch,
        });
    }
}

fn take_input_lease(
    record: &mut SessionRecord,
    invalidate_global_focus: bool,
) -> Option<TelnetInputLease> {
    let lease = record.input_lease.take()?;
    record.last_revoked_focus_epoch = lease.focus_epoch;
    if invalidate_global_focus {
        let _ = record
            .focus_invalidations
            .send(TelnetFocusInvalidation::from(&lease));
    }
    Some(lease)
}

fn transition(
    record: &mut SessionRecord,
    state: TelnetSessionState,
    close_reason: Option<TelnetCloseReason>,
    failure_reason: Option<TelnetFailureReason>,
) {
    record.summary.state = state;
    record.summary.state_revision = record.summary.state_revision.saturating_add(1);
    for attachment in record.attachments.values_mut() {
        attachment.summary.state_revision = record.summary.state_revision;
    }
    record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
    record.last_state_event_sequence = record.summary.event_sequence;
    record.summary.close_reason = close_reason;
    record.summary.failure_reason = failure_reason;
    record.summary.updated_at_unix_ms = unix_time_ms();
}

fn emit_state(record: &SessionRecord) {
    let _ = record.events.send(TelnetSessionEvent::StateChanged {
        session: record.summary.clone(),
    });
}

fn emit_current_attachments(record: &mut SessionRecord) {
    let attachment_ids = record.attachments.keys().cloned().collect::<Vec<_>>();
    for attachment_id in attachment_ids {
        record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
        let Some(attachment) = record.attachments.get_mut(&attachment_id) else {
            continue;
        };
        attachment.event_sequence = record.summary.event_sequence;
        let _ = record.events.send(TelnetSessionEvent::AttachmentChanged {
            session: record.summary.clone(),
            attachment: attachment.summary.clone(),
        });
    }
}

fn replay_after(record: &SessionRecord, after: Option<u64>) -> Vec<TelnetReplayItem> {
    let after = after.unwrap_or(0);
    let mut replay = Vec::new();
    if after < record.dropped_through_sequence {
        replay.push(TelnetReplayItem::Gap {
            generation: record.summary.generation,
            dropped_through_sequence: record.dropped_through_sequence,
        });
    }
    replay.extend(
        record
            .output_ring
            .iter()
            .filter(|frame| frame.sequence > after)
            .cloned()
            .map(TelnetReplayItem::Frame),
    );
    replay
}

fn map_runtime_failure(error: &TelnetRuntimeError) -> TelnetFailureReason {
    let (code, message_key) = match error {
        TelnetRuntimeError::ResolveFailed => (
            TelnetFailureCode::ResolveFailed,
            "telnetSession.failures.resolveFailed",
        ),
        TelnetRuntimeError::ConnectTimeout => (
            TelnetFailureCode::ConnectTimeout,
            "telnetSession.failures.connectTimeout",
        ),
        TelnetRuntimeError::ConnectFailed(_) => (
            TelnetFailureCode::ConnectFailed,
            "telnetSession.failures.connectFailed",
        ),
        TelnetRuntimeError::Protocol(_) => (
            TelnetFailureCode::ProtocolViolation,
            "telnetSession.failures.protocolViolation",
        ),
        TelnetRuntimeError::IoTimeout => (
            TelnetFailureCode::IoTimeout,
            "telnetSession.failures.ioTimeout",
        ),
        _ => (
            TelnetFailureCode::IoFailed,
            "telnetSession.failures.ioFailed",
        ),
    };
    TelnetFailureReason {
        code,
        message_key: message_key.to_owned(),
    }
}

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    #[test]
    fn service_starts_without_an_entered_tokio_runtime() {
        let service = TelnetSessionService::start();
        assert!(service.exit_blockers().is_empty());
    }

    fn open_request(port: u16, risk: bool) -> TelnetOpenRequest {
        TelnetOpenRequest {
            operation_id: new_id(),
            idempotency_key: "open-once".to_owned(),
            open_attempt_id: new_id(),
            address: "127.0.0.1".to_owned(),
            port: Some(port),
            rows: 24,
            cols: 80,
            cleartext_risk_accepted: risk,
            attach_attempt_id: new_id(),
            view_id: new_id(),
        }
    }

    async fn wait_for_state(
        service: &TelnetSessionService,
        session_id: &str,
        state: TelnetSessionState,
    ) -> TelnetSessionSummary {
        for _ in 0..100 {
            if let Some(summary) = service
                .snapshot()
                .await
                .expect("snapshot")
                .into_iter()
                .find(|summary| summary.session_id == session_id)
                && summary.state == state
            {
                return summary;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("state did not converge to {state:?}");
    }

    #[tokio::test]
    async fn requires_fresh_cleartext_risk_acknowledgement() {
        let service = TelnetSessionService::start();
        assert_eq!(
            service.open(open_request(23, false)).await,
            Err(TelnetServiceError::CleartextRiskNotAccepted)
        );
        assert!(service.exit_blockers().is_empty());
        service.shutdown().await.expect("shutdown");
    }

    #[test]
    fn runtime_failures_project_stable_locale_keys() {
        let cases = [
            (
                TelnetRuntimeError::ResolveFailed,
                TelnetFailureCode::ResolveFailed,
                "telnetSession.failures.resolveFailed",
            ),
            (
                TelnetRuntimeError::ConnectTimeout,
                TelnetFailureCode::ConnectTimeout,
                "telnetSession.failures.connectTimeout",
            ),
            (
                TelnetRuntimeError::IoTimeout,
                TelnetFailureCode::IoTimeout,
                "telnetSession.failures.ioTimeout",
            ),
            (
                TelnetRuntimeError::ConnectionClosed,
                TelnetFailureCode::IoFailed,
                "telnetSession.failures.ioFailed",
            ),
        ];

        for (error, code, message_key) in cases {
            let failure = map_runtime_failure(&error);
            assert_eq!(failure.code, code);
            assert_eq!(failure.message_key, message_key);
            assert_eq!(wire_failure_reason(failure).message_key, message_key);
        }
    }

    #[tokio::test]
    async fn open_subscribes_before_an_immediate_server_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            socket.write_all(b"ready\r\n").await.expect("write output");
        });
        let service = TelnetSessionService::start();
        let TelnetOpenBinding {
            response,
            mut events,
            ..
        } = service
            .open_subscribed(open_request(port, true))
            .await
            .expect("open subscribed");

        let frame = loop {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("event timeout")
                .expect("event");
            if let TelnetSessionEvent::Output { frame, .. } = event {
                break frame;
            }
        };
        assert_eq!(frame.data, b"ready\r\n");
        server.await.expect("server task");
        wait_for_state(
            &service,
            &response.session.session_id,
            TelnetSessionState::Closed,
        )
        .await;
        service.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn actor_fences_input_and_cleans_up_remote_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            tokio::time::sleep(Duration::from_millis(50)).await;
            socket
                .write_all(&[b'r', b'e', b'a', b'd', b'y', b'\r', b'\n', 255, 255])
                .await
                .expect("write output");
            let mut input = [0_u8; 4];
            socket.read_exact(&mut input).await.expect("read input");
            assert_eq!(input, *b"ping");
        });

        let service = TelnetSessionService::start();
        let opened = service
            .open(open_request(port, true))
            .await
            .expect("open accepted");
        let mut events = service
            .subscribe(opened.session.session_id.clone())
            .await
            .expect("subscribe");
        wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;
        let output = loop {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("output timeout")
                .expect("output event");
            if let TelnetSessionEvent::Output { frame, .. } = event {
                break frame;
            }
        };
        assert_eq!(
            output.data,
            [b'r', b'e', b'a', b'd', b'y', b'\r', b'\n', 255]
        );
        let attached = service
            .attach(TelnetAttachRequest {
                operation_id: new_id(),
                idempotency_key: "attach-second".to_owned(),
                session_id: opened.session.session_id.clone(),
                expected_generation: 1,
                expected_state_revision: wait_for_state(
                    &service,
                    &opened.session.session_id,
                    TelnetSessionState::Running,
                )
                .await
                .state_revision,
                attach_attempt_id: new_id(),
                view_id: new_id(),
                after_output_sequence: None,
            })
            .await
            .expect("second attachment");
        assert!(matches!(
            attached.replay.as_slice(),
            [TelnetReplayItem::Frame(frame)] if frame.data == output.data
        ));
        let after_renderer_detach = service
            .detach(TelnetDetachRequest {
                operation_id: new_id(),
                idempotency_key: "detach-renderer".to_owned(),
                session_id: opened.session.session_id.clone(),
                expected_generation: 1,
                expected_state_revision: attached.attachment.state_revision,
                expected_attachment_revision: attached.attachment.attachment_revision,
                attachment_id: attached.attachment.attachment_id,
                view_id: attached.attachment.view_id,
                intent: TelnetDetachIntent::RendererUnavailable,
                disconnect_if_last: false,
            })
            .await
            .expect("renderer detach");
        assert_eq!(after_renderer_detach.state, TelnetSessionState::Running);
        assert_eq!(after_renderer_detach.attachment_count, 1);
        let lease = service
            .grant_input(TelnetInputGrantRequest {
                session_id: opened.session.session_id.clone(),
                expected_generation: 1,
                expected_socket_id: wait_for_state(
                    &service,
                    &opened.session.session_id,
                    TelnetSessionState::Running,
                )
                .await
                .socket_id
                .expect("running socket"),
                attachment_id: opened.attachment.attachment_id.clone(),
                view_id: opened.attachment.view_id.clone(),
                expected_state_revision: after_renderer_detach.state_revision,
                focus_epoch: 7,
            })
            .await
            .expect("input grant");
        let mut stale = lease.clone();
        stale.focus_epoch = 6;
        let input = |lease: &TelnetInputLease, sequence| TelnetInputRequest {
            session_id: lease.session_id.clone(),
            expected_generation: lease.generation,
            socket_id: lease.socket_id.clone(),
            attachment_id: lease.attachment_id.clone(),
            view_id: lease.view_id.clone(),
            lease_id: lease.lease_id.clone(),
            focus_epoch: lease.focus_epoch,
            input_epoch: lease.input_epoch,
            client_sequence: sequence,
            data: b"ping".to_vec(),
        };
        assert_eq!(
            service.input(input(&stale, 1)).await,
            Err(TelnetServiceError::InputNotAuthorized)
        );
        service.input(input(&lease, 1)).await.expect("input");
        server.await.expect("server task");
        wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Closed,
        )
        .await;
        assert!(service.exit_blockers().is_empty());
        service.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn input_and_resize_commit_only_after_owner_ack_and_stale_work_stays_fenced() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut bytes = Vec::new();
            let _ = socket.read_to_end(&mut bytes).await;
        });
        let service = TelnetSessionService::start();
        let opened = service.open(open_request(port, true)).await.expect("open");
        let running = wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;

        let (owner_commands, mut commands) = mpsc::channel(8);
        let owner_task = tokio::spawn(std::future::pending::<()>());
        let owner_abort = owner_task.abort_handle();
        let (replace_reply, replace_response) = oneshot::channel();
        service
            .inner
            .tx
            .send(Message::ReplaceOwner {
                session_id: running.session_id.clone(),
                commands: owner_commands,
                abort: owner_abort.clone(),
                reply: replace_reply,
            })
            .await
            .expect("replace owner request");
        replace_response
            .await
            .expect("replace owner response")
            .expect("replace owner");
        server.await.expect("original owner closed socket");

        let lease = service
            .grant_input(TelnetInputGrantRequest {
                session_id: running.session_id.clone(),
                expected_generation: running.generation,
                expected_socket_id: running.socket_id.clone().expect("socket"),
                attachment_id: opened.attachment.attachment_id.clone(),
                view_id: opened.attachment.view_id.clone(),
                expected_state_revision: running.state_revision,
                focus_epoch: 7,
            })
            .await
            .expect("grant input");
        let input_request = |sequence, data: &[u8]| TelnetInputRequest {
            session_id: lease.session_id.clone(),
            expected_generation: lease.generation,
            socket_id: lease.socket_id.clone(),
            attachment_id: lease.attachment_id.clone(),
            view_id: lease.view_id.clone(),
            lease_id: lease.lease_id.clone(),
            focus_epoch: lease.focus_epoch,
            input_epoch: lease.input_epoch,
            client_sequence: sequence,
            data: data.to_vec(),
        };
        let resize_request = |sequence| TelnetResizeRequest {
            session_id: lease.session_id.clone(),
            expected_generation: lease.generation,
            socket_id: lease.socket_id.clone(),
            attachment_id: lease.attachment_id.clone(),
            view_id: lease.view_id.clone(),
            lease_id: lease.lease_id.clone(),
            focus_epoch: lease.focus_epoch,
            input_epoch: lease.input_epoch,
            resize_sequence: sequence,
            rows: 32,
            cols: 120,
        };

        let (input_reply, mut input_response) = oneshot::channel();
        service
            .inner
            .tx
            .send(Message::Input {
                request: input_request(1, b"first"),
                reply: input_reply,
            })
            .await
            .expect("input request");
        let OwnerCommand::Input { data, reply } = commands.recv().await.expect("owner input")
        else {
            panic!("expected owner input");
        };
        assert_eq!(data, b"first");
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut input_response)
                .await
                .is_err(),
            "IPC input completed before the owner acknowledged the write"
        );
        reply.send(Ok(())).expect("ack input");
        input_response
            .await
            .expect("input response")
            .expect("input succeeds after ack");

        let (resize_reply, mut resize_response) = oneshot::channel();
        service
            .inner
            .tx
            .send(Message::Resize {
                request: resize_request(1),
                reply: resize_reply,
            })
            .await
            .expect("resize request");
        let OwnerCommand::Resize { rows, cols, reply } =
            commands.recv().await.expect("owner resize")
        else {
            panic!("expected owner resize");
        };
        assert_eq!((rows, cols), (32, 120));
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut resize_response)
                .await
                .is_err(),
            "IPC resize completed before the owner acknowledged the resize"
        );
        reply.send(Ok(())).expect("ack resize");
        resize_response
            .await
            .expect("resize response")
            .expect("resize succeeds after ack");

        let (pending_input_reply, mut pending_input_response) = oneshot::channel();
        service
            .inner
            .tx
            .send(Message::Input {
                request: input_request(2, b"second"),
                reply: pending_input_reply,
            })
            .await
            .expect("second input request");
        let OwnerCommand::Input { data, reply } =
            commands.recv().await.expect("second owner input")
        else {
            panic!("expected second owner input");
        };
        assert_eq!(data, b"second");

        let (detach_reply, mut detach_response) = oneshot::channel();
        service
            .inner
            .tx
            .send(Message::Detach {
                request: TelnetDetachRequest {
                    operation_id: new_id(),
                    idempotency_key: "detach-behind-owner-ack".to_owned(),
                    session_id: running.session_id.clone(),
                    expected_generation: running.generation,
                    expected_state_revision: running.state_revision,
                    expected_attachment_revision: opened.attachment.attachment_revision,
                    attachment_id: opened.attachment.attachment_id.clone(),
                    view_id: opened.attachment.view_id.clone(),
                    intent: TelnetDetachIntent::RendererUnavailable,
                    disconnect_if_last: false,
                },
                reply: detach_reply,
            })
            .await
            .expect("queued detach");
        let (stale_input_reply, stale_input_response) = oneshot::channel();
        service
            .inner
            .tx
            .send(Message::Input {
                request: input_request(3, b"stale"),
                reply: stale_input_reply,
            })
            .await
            .expect("queued stale input");
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut detach_response)
                .await
                .is_err(),
            "detach overtook an owner write awaiting acknowledgement"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut pending_input_response)
                .await
                .is_err(),
            "second input completed before the owner acknowledgement"
        );

        reply.send(Ok(())).expect("ack second input");
        pending_input_response
            .await
            .expect("second input response")
            .expect("second input succeeds after ack");
        let detached = detach_response
            .await
            .expect("detach response")
            .expect("detach after owner ack");
        assert_eq!(detached.attachment_count, 0);
        assert_eq!(
            stale_input_response.await.expect("stale input response"),
            Err(TelnetServiceError::Conflict)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(25), commands.recv())
                .await
                .is_err(),
            "stale queued input reached the owner after its lease was revoked"
        );

        owner_abort.abort();
        service.shutdown_all().await.expect("shutdown all");
        service.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn same_operation_replays_without_second_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let (accepted, first_connection) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.expect("accept");
            let _ = accepted.send(());
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err()
        });
        let service = TelnetSessionService::start();
        let request = open_request(port, true);
        let first = service.open(request.clone()).await.expect("first open");
        tokio::time::timeout(Duration::from_secs(2), first_connection)
            .await
            .expect("first connection timeout")
            .expect("first connection signal");
        wait_for_state(
            &service,
            &first.session.session_id,
            TelnetSessionState::Running,
        )
        .await;
        let replay = service.open(request.clone()).await.expect("replay");
        assert_eq!(first.session.session_id, replay.session.session_id);
        assert_eq!(
            first.attachment.attachment_id,
            replay.attachment.attachment_id
        );
        assert_eq!(replay.session.state, TelnetSessionState::Running);
        assert!(replay.session.socket_id.is_some());
        let mut changed_attempt = request;
        changed_attempt.open_attempt_id = new_id();
        assert_eq!(
            service.open(changed_attempt).await,
            Err(TelnetServiceError::Conflict),
            "the open attempt identity is part of the operation fingerprint"
        );
        service.shutdown().await.expect("shutdown");
        assert!(
            server.await.expect("server task"),
            "second connection opened"
        );
    }

    #[tokio::test]
    async fn output_flood_resyncs_running_control_state_and_open_replay_ring() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let _ = release_rx.await;
            let _ = socket.shutdown().await;
        });
        let service = TelnetSessionService::start();
        let request = open_request(port, true);
        let opened = service.open(request.clone()).await.expect("open");
        let running = wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;
        let mut events = service
            .subscribe(opened.session.session_id.clone())
            .await
            .expect("subscribe before flood");

        for index in 0..300_u16 {
            service
                .inner
                .tx
                .send(Message::OwnerOutput {
                    session_id: opened.session.session_id.clone(),
                    generation: running.generation,
                    data: vec![(index % 251) as u8],
                })
                .await
                .expect("enqueue output");
        }
        for _ in 0..100 {
            let current = service
                .snapshot()
                .await
                .expect("snapshot")
                .into_iter()
                .find(|item| item.session_id == opened.session.session_id)
                .expect("session");
            if current.event_sequence >= running.event_sequence.saturating_add(300) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(matches!(
            events.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));

        let resync = request_reply(&service.inner.tx, |reply| Message::ChannelResync {
            session_id: opened.session.session_id.clone(),
            attachment_id: opened.attachment.attachment_id.clone(),
            reply,
        })
        .await
        .expect("resync");
        assert_eq!(resync.session.state, TelnetSessionState::Running);
        assert_eq!(
            resync
                .attachment
                .as_ref()
                .map(|item| item.attachment_id.as_str()),
            Some(opened.attachment.attachment_id.as_str())
        );
        assert_eq!(resync.frames.len(), OUTPUT_RING_MAX_BYTES);
        assert_eq!(resync.frames.first().map(|frame| frame.sequence), Some(237));
        assert_eq!(resync.frames.last().map(|frame| frame.sequence), Some(300));
        let resync_events =
            channel_resync_events(resync.clone(), opened.attachment.attachment_id.as_str())
                .expect("wire resync events");
        assert!(
            resync_events
                .windows(2)
                .all(|events| { events[0].event_seq.get() < events[1].event_seq.get() })
        );
        assert!(resync_events.iter().any(|event| matches!(
            &event.payload,
            wire::TelnetSessionEventPayload::StateChanged { session }
                if session.state == wire::TelnetSessionState::Running
        )));
        assert!(resync_events.iter().any(|event| matches!(
            &event.payload,
            wire::TelnetSessionEventPayload::AttachmentAttached { attachment }
                if attachment.attachment_id.as_str()
                    == opened.attachment.attachment_id.as_str()
                    && attachment.socket_id.is_some()
        )));
        let replayed_sequences = resync_events
            .iter()
            .filter_map(|event| match &event.payload {
                wire::TelnetSessionEventPayload::Output { frame } => Some(frame.output_seq.get()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(replayed_sequences.first(), Some(&237));
        assert_eq!(replayed_sequences.last(), Some(&300));

        let replay = service
            .open_subscribed(request)
            .await
            .expect("exact open replay");
        assert!(replay.resync_immediately);
        assert_eq!(replay.response.session.state, TelnetSessionState::Running);
        assert_eq!(
            replay.response.attachment.attachment_id,
            opened.attachment.attachment_id
        );

        let _ = release_tx.send(());
        server.await.expect("server");
        wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Closed,
        )
        .await;
        service.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn attach_detach_disconnect_and_reconnect_fences_replay_exactly() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut bytes = Vec::new();
                let _ = socket.read_to_end(&mut bytes).await;
            }
        });
        let service = TelnetSessionService::start();
        let missing_disconnect = TelnetDisconnectRequest {
            operation_id: new_id(),
            idempotency_key: "missing-disconnect".to_owned(),
            session_id: new_id(),
            expected_generation: 1,
            expected_state_revision: 1,
        };
        assert_eq!(
            service.disconnect(missing_disconnect.clone()).await,
            Err(TelnetServiceError::NotFound)
        );
        assert_eq!(
            service.disconnect(missing_disconnect.clone()).await,
            Err(TelnetServiceError::NotFound)
        );
        assert_eq!(
            service
                .disconnect(TelnetDisconnectRequest {
                    expected_state_revision: 2,
                    ..missing_disconnect
                })
                .await,
            Err(TelnetServiceError::Conflict)
        );
        let opened = service.open(open_request(port, true)).await.expect("open");
        let running = wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;

        let attach = TelnetAttachRequest {
            operation_id: new_id(),
            idempotency_key: "attach-ledger".to_owned(),
            session_id: running.session_id.clone(),
            expected_generation: running.generation,
            expected_state_revision: running.state_revision,
            attach_attempt_id: new_id(),
            view_id: new_id(),
            after_output_sequence: None,
        };
        let attached = service.attach(attach.clone()).await.expect("attach");
        assert_eq!(
            service.attach(attach.clone()).await.expect("attach replay"),
            attached
        );
        assert_eq!(
            service
                .attach(TelnetAttachRequest {
                    view_id: new_id(),
                    ..attach
                })
                .await,
            Err(TelnetServiceError::Conflict)
        );
        assert_eq!(
            service
                .attachment_heartbeat(
                    running.session_id.clone(),
                    running.generation,
                    attached.attachment.attachment_revision.saturating_sub(1),
                    attached.attachment.attachment_id.clone(),
                    attached.attachment.view_id.clone(),
                )
                .await,
            Err(TelnetServiceError::Conflict)
        );

        let detach = TelnetDetachRequest {
            operation_id: new_id(),
            idempotency_key: "detach-ledger".to_owned(),
            session_id: running.session_id.clone(),
            expected_generation: running.generation,
            expected_state_revision: attached.attachment.state_revision,
            expected_attachment_revision: attached.attachment.attachment_revision,
            attachment_id: attached.attachment.attachment_id.clone(),
            view_id: attached.attachment.view_id.clone(),
            intent: TelnetDetachIntent::RendererUnavailable,
            disconnect_if_last: false,
        };
        let detached = service.detach(detach.clone()).await.expect("detach");
        assert_eq!(
            service.detach(detach.clone()).await.expect("detach replay"),
            detached
        );
        assert_eq!(
            service
                .detach(TelnetDetachRequest {
                    expected_attachment_revision: detach
                        .expected_attachment_revision
                        .saturating_add(1),
                    ..detach
                })
                .await,
            Err(TelnetServiceError::Conflict)
        );

        let current = service
            .snapshot()
            .await
            .expect("snapshot")
            .into_iter()
            .find(|item| item.session_id == running.session_id)
            .expect("session");
        let disconnect = TelnetDisconnectRequest {
            operation_id: new_id(),
            idempotency_key: "disconnect-ledger".to_owned(),
            session_id: current.session_id.clone(),
            expected_generation: current.generation,
            expected_state_revision: current.state_revision,
        };
        let disconnecting = service
            .disconnect(disconnect.clone())
            .await
            .expect("disconnect");
        assert_eq!(
            service
                .disconnect(disconnect.clone())
                .await
                .expect("disconnect replay"),
            disconnecting
        );
        assert_eq!(
            service
                .disconnect(TelnetDisconnectRequest {
                    expected_state_revision: disconnect.expected_state_revision.saturating_add(1),
                    ..disconnect
                })
                .await,
            Err(TelnetServiceError::Conflict)
        );
        let closed =
            wait_for_state(&service, &running.session_id, TelnetSessionState::Closed).await;
        let stale_reconnect = TelnetReconnectRequest {
            operation_id: new_id(),
            idempotency_key: "stale-reconnect".to_owned(),
            session_id: closed.session_id.clone(),
            expected_generation: closed.generation,
            expected_state_revision: closed.state_revision.saturating_sub(1),
            rows: 24,
            cols: 80,
            cleartext_risk_accepted: true,
        };
        assert_eq!(
            service.reconnect(stale_reconnect.clone()).await,
            Err(TelnetServiceError::Conflict)
        );
        assert_eq!(
            service.reconnect(stale_reconnect.clone()).await,
            Err(TelnetServiceError::Conflict)
        );
        assert_eq!(
            service
                .reconnect(TelnetReconnectRequest {
                    rows: 25,
                    ..stale_reconnect
                })
                .await,
            Err(TelnetServiceError::Conflict)
        );
        let reconnect = TelnetReconnectRequest {
            operation_id: new_id(),
            idempotency_key: "fresh-reconnect".to_owned(),
            session_id: closed.session_id.clone(),
            expected_generation: closed.generation,
            expected_state_revision: closed.state_revision,
            rows: 24,
            cols: 80,
            cleartext_risk_accepted: true,
        };
        let connecting = service
            .reconnect(reconnect.clone())
            .await
            .expect("reconnect");
        assert_eq!(
            service
                .reconnect(reconnect)
                .await
                .expect("reconnect replay"),
            connecting
        );
        wait_for_state(&service, &closed.session_id, TelnetSessionState::Running).await;
        service.shutdown_all().await.expect("shutdown all");
        server.await.expect("server");
        service.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn lifecycle_changes_emit_exact_focus_invalidations() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let _ = release_rx.await;
            let _ = socket.shutdown().await;
        });
        let service = TelnetSessionService::start();
        let mut invalidations = service
            .inner
            .focus_invalidations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("invalidation receiver");
        let opened = service.open(open_request(port, true)).await.expect("open");
        let running = wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;
        let original = service
            .attachment_heartbeat(
                running.session_id.clone(),
                running.generation,
                running.attachment_revision,
                opened.attachment.attachment_id.clone(),
                opened.attachment.view_id.clone(),
            )
            .await
            .expect("current original attachment");
        let grant = |attachment: &TelnetAttachment, focus_epoch| TelnetInputGrantRequest {
            session_id: running.session_id.clone(),
            expected_generation: running.generation,
            expected_socket_id: running.socket_id.clone().expect("socket"),
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            expected_state_revision: running.state_revision,
            focus_epoch,
        };
        service
            .grant_input(grant(&original, 7))
            .await
            .expect("first grant");
        let replacement = service
            .attach(TelnetAttachRequest {
                operation_id: new_id(),
                idempotency_key: "replace-view".to_owned(),
                session_id: running.session_id.clone(),
                expected_generation: running.generation,
                expected_state_revision: running.state_revision,
                attach_attempt_id: new_id(),
                view_id: original.view_id.clone(),
                after_output_sequence: None,
            })
            .await
            .expect("same-view replacement")
            .attachment;
        let replaced = invalidations
            .recv()
            .await
            .expect("replacement invalidation");
        assert_eq!(replaced.focus_epoch, 7);
        assert_eq!(replaced.attachment_id, original.attachment_id);

        service
            .grant_input(grant(&replacement, 8))
            .await
            .expect("replacement grant");
        service
            .detach(TelnetDetachRequest {
                operation_id: new_id(),
                idempotency_key: "detach-focused".to_owned(),
                session_id: running.session_id.clone(),
                expected_generation: running.generation,
                expected_state_revision: running.state_revision,
                expected_attachment_revision: replacement.attachment_revision,
                attachment_id: replacement.attachment_id.clone(),
                view_id: replacement.view_id.clone(),
                intent: TelnetDetachIntent::RendererUnavailable,
                disconnect_if_last: false,
            })
            .await
            .expect("focused detach");
        let detached = invalidations.recv().await.expect("detach invalidation");
        assert_eq!(detached.focus_epoch, 8);
        assert_eq!(detached.attachment_id, replacement.attachment_id);

        let fresh = service
            .attach(TelnetAttachRequest {
                operation_id: new_id(),
                idempotency_key: "attach-for-close".to_owned(),
                session_id: running.session_id.clone(),
                expected_generation: running.generation,
                expected_state_revision: running.state_revision,
                attach_attempt_id: new_id(),
                view_id: new_id(),
                after_output_sequence: None,
            })
            .await
            .expect("fresh attachment")
            .attachment;
        service
            .grant_input(grant(&fresh, 9))
            .await
            .expect("fresh grant");
        let _ = release_tx.send(());
        server.await.expect("server");
        wait_for_state(&service, &running.session_id, TelnetSessionState::Closed).await;
        let closed = invalidations
            .recv()
            .await
            .expect("remote close invalidation");
        assert_eq!(closed.focus_epoch, 9);
        assert_eq!(closed.attachment_id, fresh.attachment_id);
        service.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn shutdown_all_is_repeatable_and_keeps_the_actor_available() {
        let service = TelnetSessionService::start();

        for _ in 0..2 {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
            let port = listener.local_addr().expect("address").port();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut byte = [0_u8; 1];
                tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
                    .await
                    .expect("shutdown timeout")
                    .expect("server read")
            });
            let opened = service
                .open(open_request(port, true))
                .await
                .expect("open accepted");
            wait_for_state(
                &service,
                &opened.session.session_id,
                TelnetSessionState::Running,
            )
            .await;

            service.shutdown_all().await.expect("repeatable cleanup");
            assert_eq!(server.await.expect("server task"), 0);
            assert!(service.exit_blockers().is_empty());
            service.snapshot().await.expect("actor remains available");
        }

        service.shutdown().await.expect("final teardown");
    }

    #[tokio::test]
    async fn shutdown_closes_idle_tcp_and_clears_exit_blocker() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut byte = [0_u8; 1];
            tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
                .await
                .expect("shutdown timeout")
                .expect("server read")
        });
        let service = TelnetSessionService::start();
        let opened = service
            .open(open_request(port, true))
            .await
            .expect("open accepted");
        wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;

        service.shutdown().await.expect("shutdown");
        assert_eq!(server.await.expect("server task"), 0);
        assert!(service.exit_blockers().is_empty());
    }

    #[tokio::test]
    async fn reconnect_requires_fresh_risk_and_creates_a_new_generation() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut bytes = Vec::new();
                let _ = socket.read_to_end(&mut bytes).await;
            }
        });
        let service = TelnetSessionService::start();
        let opened = service
            .open(open_request(port, true))
            .await
            .expect("first open");
        let first = wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;
        assert_eq!(first.generation, 1);
        service
            .disconnect(TelnetDisconnectRequest {
                operation_id: new_id(),
                idempotency_key: "disconnect-first".to_owned(),
                session_id: opened.session.session_id.clone(),
                expected_generation: 1,
                expected_state_revision: first.state_revision,
            })
            .await
            .expect("disconnect first generation");
        let closed = wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Closed,
        )
        .await;

        let operation_id = new_id();
        let reconnect = TelnetReconnectRequest {
            operation_id: operation_id.clone(),
            idempotency_key: "reconnect-once".to_owned(),
            session_id: opened.session.session_id.clone(),
            expected_generation: 1,
            expected_state_revision: closed.state_revision,
            rows: 24,
            cols: 80,
            cleartext_risk_accepted: false,
        };
        assert_eq!(
            service.reconnect(reconnect.clone()).await,
            Err(TelnetServiceError::CleartextRiskNotAccepted)
        );
        let reconnect = TelnetReconnectRequest {
            cleartext_risk_accepted: true,
            ..reconnect
        };
        let connecting = service
            .reconnect(reconnect.clone())
            .await
            .expect("reconnect accepted");
        assert_eq!(connecting.generation, 2);
        assert_eq!(connecting.state, TelnetSessionState::Connecting);
        assert_eq!(
            service
                .reconnect(reconnect)
                .await
                .expect("idempotent replay"),
            connecting
        );
        let second = wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Running,
        )
        .await;
        assert_eq!(second.generation, 2);
        assert_ne!(first.socket_id, second.socket_id);
        service
            .disconnect(TelnetDisconnectRequest {
                operation_id: new_id(),
                idempotency_key: "disconnect-second".to_owned(),
                session_id: opened.session.session_id.clone(),
                expected_generation: 2,
                expected_state_revision: second.state_revision,
            })
            .await
            .expect("disconnect second generation");
        wait_for_state(
            &service,
            &opened.session.session_id,
            TelnetSessionState::Closed,
        )
        .await;
        server.await.expect("server");
        service.shutdown().await.expect("shutdown");
    }
}
