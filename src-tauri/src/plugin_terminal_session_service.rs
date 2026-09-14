//! Core-owned session actor for terminal surfaces launched by a plugin.
//!
//! This module intentionally knows nothing about SSH, Hosts, Vault, plugin
//! guest messages, or Tauri IPC.  The caller consumes the frontend launch
//! authorization before calling [`PluginTerminalSessionService::open_after_frontend_launch`]
//! and supplies a Core-only driver factory.  A driver owns the actual provider
//! resource; this actor owns the bounded session projection, attachment
//! lifecycle, replay ring, and input fences around that resource.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use thiserror::Error;
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::AbortHandle,
    time::timeout,
};
use uuid::Uuid;

use crate::time::unix_time_ms;

const ACTOR_MAILBOX_CAPACITY: usize = 128;
const EVENT_BROADCAST_CAPACITY: usize = 128;
const DEFAULT_MAX_SESSIONS: usize = 32;
const DEFAULT_MAX_ATTACHMENTS_PER_SESSION: usize = 16;
const DEFAULT_OUTPUT_RING_MAX_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_DRIVER_OUTPUT_BYTES: usize = 64 * 1024;
const ATTACHMENT_TIMEOUT_MILLIS: i64 = 45_000;
const ATTACHMENT_REAPER_INTERVAL: Duration = Duration::from_secs(5);
const INPUT_LEASE_MILLIS: i64 = 15_000;
const DEFAULT_DRIVER_ACK_TIMEOUT: Duration = Duration::from_secs(10);
const OPERATION_LEDGER_CAPACITY: usize = 1_024;

/// The root integration creates this only after it has consumed the matching
/// frontend launch record.  It is deliberately an opaque Core value rather
/// than a plugin-visible authorization token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PluginTerminalFrontendLaunchClaim {
    id: Uuid,
}

impl PluginTerminalFrontendLaunchClaim {
    #[must_use]
    pub(crate) fn new(id: Uuid) -> Option<Self> {
        (!id.is_nil()).then_some(Self { id })
    }

    #[must_use]
    pub(crate) fn id(self) -> Uuid {
        self.id
    }
}

/// Non-secret metadata which has already been validated by the capability
/// broker.  It is included in the durable-in-memory operation fingerprint so
/// a retried operation cannot silently point at a different provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalProviderMetadata {
    pub(crate) plugin_id: String,
    pub(crate) provider_id: String,
    pub(crate) display_label: String,
}

/// A Core-only private identifier for the real provider resource.  It is sent
/// only from the actor to the driver factory and never appears in a summary,
/// event, attachment, lease, or plugin guest payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PluginTerminalConnectionHandle(Uuid);

impl PluginTerminalConnectionHandle {
    #[must_use]
    pub(crate) fn new(id: Uuid) -> Option<Self> {
        (!id.is_nil()).then_some(Self(id))
    }

    #[must_use]
    pub(crate) fn as_uuid(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginTerminalSessionState {
    Connecting,
    Running,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginTerminalCloseReason {
    UserRequested,
    DriverExited,
    DriverFailed,
    CoreShutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginTerminalFailureCode {
    DriverStartFailed,
    DriverEventStreamClosed,
    DriverFailed,
    DriverAckUnknown,
    OutputLimitExceeded,
    CleanupIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalFailureReason {
    pub(crate) code: PluginTerminalFailureCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalSessionSummary {
    pub(crate) session_id: Uuid,
    pub(crate) provider: PluginTerminalProviderMetadata,
    pub(crate) generation: u64,
    pub(crate) stream_id: Uuid,
    pub(crate) state_revision: u64,
    pub(crate) attachment_revision: u64,
    pub(crate) event_sequence: u64,
    pub(crate) state: PluginTerminalSessionState,
    pub(crate) attachment_count: u32,
    /// True means the actor still owns a driver which has not confirmed its
    /// cleanup.  It is an application-exit blocker until retry succeeds.
    pub(crate) cleanup_blocked: bool,
    pub(crate) close_reason: Option<PluginTerminalCloseReason>,
    pub(crate) failure_reason: Option<PluginTerminalFailureReason>,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalAttachment {
    pub(crate) attachment_id: Uuid,
    pub(crate) attach_attempt_id: Uuid,
    pub(crate) session_id: Uuid,
    pub(crate) generation: u64,
    pub(crate) stream_id: Uuid,
    pub(crate) view_id: String,
    pub(crate) state_revision: u64,
    pub(crate) attachment_revision: u64,
    pub(crate) attached_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalOutputFrame {
    pub(crate) generation: u64,
    pub(crate) stream_id: Uuid,
    pub(crate) sequence: u64,
    pub(crate) data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginTerminalReplayItem {
    Gap {
        generation: u64,
        stream_id: Uuid,
        dropped_through_sequence: u64,
    },
    Frame(PluginTerminalOutputFrame),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginTerminalSessionEvent {
    StateChanged {
        session: PluginTerminalSessionSummary,
    },
    AttachmentChanged {
        session: PluginTerminalSessionSummary,
        attachment: PluginTerminalAttachment,
    },
    AttachmentDetached {
        session: PluginTerminalSessionSummary,
        attachment_id: Uuid,
    },
    InputLeaseRevoked {
        session: PluginTerminalSessionSummary,
        focus_epoch: u64,
    },
    Output {
        session: PluginTerminalSessionSummary,
        frame: PluginTerminalOutputFrame,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalOpenRequest {
    pub(crate) operation_id: Uuid,
    pub(crate) idempotency_key: String,
    /// The Core-only resource binding already associated with the claimed
    /// protocol launch.  Plugin guest code never receives this value.
    pub(crate) connection_handle: PluginTerminalConnectionHandle,
    pub(crate) provider: PluginTerminalProviderMetadata,
    pub(crate) attach_attempt_id: Uuid,
    pub(crate) view_id: String,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalOpenResponse {
    pub(crate) session: PluginTerminalSessionSummary,
    pub(crate) attachment: PluginTerminalAttachment,
}

pub(crate) struct PluginTerminalOpenBinding {
    pub(crate) response: PluginTerminalOpenResponse,
    pub(crate) events: broadcast::Receiver<PluginTerminalSessionEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalAttachRequest {
    pub(crate) operation_id: Uuid,
    pub(crate) idempotency_key: String,
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) expected_state_revision: u64,
    pub(crate) attach_attempt_id: Uuid,
    pub(crate) view_id: String,
    pub(crate) after_output_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalAttachResponse {
    pub(crate) session: PluginTerminalSessionSummary,
    pub(crate) attachment: PluginTerminalAttachment,
    pub(crate) replay: Vec<PluginTerminalReplayItem>,
}

pub(crate) struct PluginTerminalAttachBinding {
    pub(crate) response: PluginTerminalAttachResponse,
    pub(crate) events: broadcast::Receiver<PluginTerminalSessionEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginTerminalDetachIntent {
    UserClose,
    RendererUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalDetachRequest {
    pub(crate) operation_id: Uuid,
    pub(crate) idempotency_key: String,
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) expected_state_revision: u64,
    pub(crate) expected_attachment_revision: u64,
    pub(crate) attachment_id: Uuid,
    pub(crate) view_id: String,
    pub(crate) intent: PluginTerminalDetachIntent,
    pub(crate) disconnect_if_last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalInputLease {
    pub(crate) lease_id: Uuid,
    pub(crate) session_id: Uuid,
    pub(crate) generation: u64,
    pub(crate) stream_id: Uuid,
    pub(crate) attachment_id: Uuid,
    pub(crate) view_id: String,
    pub(crate) state_revision: u64,
    pub(crate) focus_epoch: u64,
    pub(crate) input_epoch: u64,
    pub(crate) expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalInputGrantRequest {
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) attachment_id: Uuid,
    pub(crate) view_id: String,
    pub(crate) expected_state_revision: u64,
    /// Already committed by the process-wide terminal focus broker.  This
    /// actor validates it but never creates its own focus domain.
    pub(crate) focus_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalInputFence {
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) expected_state_revision: u64,
    pub(crate) attachment_id: Uuid,
    pub(crate) view_id: String,
    pub(crate) lease_id: Uuid,
    pub(crate) focus_epoch: u64,
    pub(crate) input_epoch: u64,
}

impl From<&PluginTerminalInputLease> for PluginTerminalInputFence {
    fn from(lease: &PluginTerminalInputLease) -> Self {
        Self {
            session_id: lease.session_id,
            expected_generation: lease.generation,
            expected_stream_id: lease.stream_id,
            expected_state_revision: lease.state_revision,
            attachment_id: lease.attachment_id,
            view_id: lease.view_id.clone(),
            lease_id: lease.lease_id,
            focus_epoch: lease.focus_epoch,
            input_epoch: lease.input_epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalInputRequest {
    pub(crate) fence: PluginTerminalInputFence,
    pub(crate) client_sequence: u64,
    pub(crate) data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalResizeRequest {
    pub(crate) fence: PluginTerminalInputFence,
    pub(crate) resize_sequence: u64,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalAttachmentRenewRequest {
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) expected_attachment_revision: u64,
    pub(crate) attachment_id: Uuid,
    pub(crate) view_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalDisconnectRequest {
    pub(crate) operation_id: Uuid,
    pub(crate) idempotency_key: String,
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) expected_state_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalCleanupRetryRequest {
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) expected_state_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginTerminalReconnectRequest {
    pub(crate) operation_id: Uuid,
    pub(crate) idempotency_key: String,
    pub(crate) session_id: Uuid,
    pub(crate) expected_generation: u64,
    pub(crate) expected_stream_id: Uuid,
    pub(crate) expected_state_revision: u64,
    pub(crate) connection_handle: PluginTerminalConnectionHandle,
    pub(crate) provider: PluginTerminalProviderMetadata,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginTerminalDriverError {
    Rejected,
    Failed,
    Unknown,
}

/// Commands sent only from the Core actor to the provider driver.  A driver
/// must acknowledge a command only after the real provider owner completed the
/// requested operation.  In particular, sending a command into a driver
/// mailbox is not an input or resize success.
pub(crate) enum PluginTerminalDriverCommand {
    Input {
        bytes: Vec<u8>,
        ack: oneshot::Sender<Result<(), PluginTerminalDriverError>>,
    },
    Resize {
        rows: u16,
        cols: u16,
        ack: oneshot::Sender<Result<(), PluginTerminalDriverError>>,
    },
    Close {
        ack: oneshot::Sender<Result<(), PluginTerminalDriverError>>,
    },
}

/// Facts emitted by the provider driver.  `Exit` means the driver observed
/// that its owned provider resource has ended.  `Failed` is not cleanup proof:
/// the actor retains an exit blocker and sends `Close` until that close is
/// acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginTerminalDriverEvent {
    Ready,
    Output(Vec<u8>),
    Exit,
    Failed(PluginTerminalDriverError),
}

/// Driver runtime cancellation is a best-effort interruption request.  It
/// must release the actual driver task/resource when possible, but it never
/// clears the actor's cleanup blocker; only `Close` acknowledgement or `Exit`
/// can prove cleanup.
pub(crate) trait PluginTerminalDriverRuntime: Send + Sync {
    fn cancel(&self);
}

pub(crate) struct PluginTerminalDriverBinding {
    pub(crate) commands: mpsc::Sender<PluginTerminalDriverCommand>,
    pub(crate) events: mpsc::Receiver<PluginTerminalDriverEvent>,
    pub(crate) runtime: Arc<dyn PluginTerminalDriverRuntime>,
}

#[derive(Debug, Clone)]
pub(crate) struct PluginTerminalDriverStart {
    connection_handle: PluginTerminalConnectionHandle,
    pub(crate) provider: PluginTerminalProviderMetadata,
    pub(crate) generation: u64,
    pub(crate) stream_id: Uuid,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

impl PluginTerminalDriverStart {
    #[must_use]
    pub(crate) fn connection_handle(&self) -> PluginTerminalConnectionHandle {
        self.connection_handle
    }
}

/// The bridge implemented by the plugin/resource runtime.  It may create a
/// worker, Wasm bridge, or provider-specific channel, but it receives only
/// validated non-secret metadata and Core-private identity.  Returning `Err`
/// promises no resource requiring later cleanup was handed to this actor.
pub(crate) trait PluginTerminalDriverFactory: Send + Sync {
    fn start(
        &self,
        start: PluginTerminalDriverStart,
    ) -> Result<PluginTerminalDriverBinding, PluginTerminalDriverError>;
}

#[derive(Debug, Clone)]
pub(crate) struct PluginTerminalSessionLimits {
    pub(crate) max_sessions: usize,
    pub(crate) max_attachments_per_session: usize,
    pub(crate) output_ring_max_bytes: usize,
    pub(crate) max_driver_output_bytes: usize,
    pub(crate) driver_ack_timeout: Duration,
}

impl Default for PluginTerminalSessionLimits {
    fn default() -> Self {
        Self {
            max_sessions: DEFAULT_MAX_SESSIONS,
            max_attachments_per_session: DEFAULT_MAX_ATTACHMENTS_PER_SESSION,
            output_ring_max_bytes: DEFAULT_OUTPUT_RING_MAX_BYTES,
            max_driver_output_bytes: DEFAULT_MAX_DRIVER_OUTPUT_BYTES,
            driver_ack_timeout: DEFAULT_DRIVER_ACK_TIMEOUT,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum PluginTerminalSessionError {
    #[error("invalid plugin terminal session request")]
    Validation,
    #[error("plugin terminal session was not found")]
    NotFound,
    #[error("stale or conflicting plugin terminal session fence")]
    Conflict,
    #[error("plugin terminal input is not authorized by the current focus lease")]
    InputNotAuthorized,
    #[error("the final active attachment requires an explicit disconnect")]
    LastAttachmentRequiresDisconnect,
    #[error("plugin terminal driver rejected the operation")]
    DriverRejected,
    #[error("plugin terminal operation outcome is unknown")]
    OutcomeUnknown,
    #[error("plugin terminal driver or actor is unavailable")]
    Unavailable,
    #[error("plugin terminal service quota was exceeded")]
    QuotaExceeded,
    #[error("plugin terminal driver cleanup did not converge")]
    CleanupIncomplete,
    #[error("plugin terminal session is busy with an owner operation")]
    Busy,
    #[error("plugin terminal driver could not start")]
    DriverStartFailed,
}

struct AttachmentRecord {
    attachment: PluginTerminalAttachment,
    last_seen_at_unix_ms: i64,
}

struct DriverRecord {
    commands: mpsc::Sender<PluginTerminalDriverCommand>,
    runtime: Arc<dyn PluginTerminalDriverRuntime>,
    event_pump_abort: AbortHandle,
    runtime_cancelled: bool,
}

#[derive(Debug, Clone)]
enum PendingDriverKind {
    Input {
        sequence: u64,
        fence: PluginTerminalInputFence,
    },
    Resize {
        sequence: u64,
        fence: PluginTerminalInputFence,
    },
}

struct PendingDriverCommand {
    command_id: Uuid,
    kind: PendingDriverKind,
    reply: oneshot::Sender<Result<(), PluginTerminalSessionError>>,
}

struct PendingClose {
    command_id: Uuid,
    reason: PluginTerminalCloseReason,
    preserve_failure: bool,
    reply:
        Option<oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>>,
}

struct SessionRecord {
    summary: PluginTerminalSessionSummary,
    attachments: BTreeMap<Uuid, AttachmentRecord>,
    events: broadcast::Sender<PluginTerminalSessionEvent>,
    input_lease: Option<PluginTerminalInputLease>,
    next_input_epoch: u64,
    last_client_sequence: u64,
    last_resize_sequence: u64,
    next_output_sequence: u64,
    output_ring: VecDeque<PluginTerminalOutputFrame>,
    output_ring_bytes: usize,
    dropped_through_sequence: u64,
    driver: Option<DriverRecord>,
    pending_command: Option<PendingDriverCommand>,
    pending_close: Option<PendingClose>,
    detach_after_close: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenFingerprint {
    launch_claim_id: Uuid,
    connection_handle: PluginTerminalConnectionHandle,
    provider: PluginTerminalProviderMetadata,
    attach_attempt_id: Uuid,
    view_id: String,
    rows: u16,
    cols: u16,
}

#[derive(Clone)]
struct OpenLedgerEntry {
    idempotency_key: String,
    fingerprint: OpenFingerprint,
    result: Result<PluginTerminalOpenResponse, PluginTerminalSessionError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachFingerprint {
    session_id: Uuid,
    expected_generation: u64,
    expected_stream_id: Uuid,
    expected_state_revision: u64,
    attach_attempt_id: Uuid,
    view_id: String,
    after_output_sequence: Option<u64>,
}

#[derive(Clone)]
struct AttachLedgerEntry {
    idempotency_key: String,
    fingerprint: AttachFingerprint,
    result: Result<PluginTerminalAttachResponse, PluginTerminalSessionError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconnectFingerprint {
    launch_claim_id: Uuid,
    session_id: Uuid,
    expected_generation: u64,
    expected_stream_id: Uuid,
    expected_state_revision: u64,
    connection_handle: PluginTerminalConnectionHandle,
    provider: PluginTerminalProviderMetadata,
    rows: u16,
    cols: u16,
}

#[derive(Clone)]
struct ReconnectLedgerEntry {
    idempotency_key: String,
    fingerprint: ReconnectFingerprint,
    result: Result<PluginTerminalSessionSummary, PluginTerminalSessionError>,
}

enum DriverAckOutcome {
    Completed(Result<(), PluginTerminalDriverError>),
    Unknown,
}

enum DriverAckKind {
    Command,
    Close,
}

enum Message {
    Open {
        claim: PluginTerminalFrontendLaunchClaim,
        request: PluginTerminalOpenRequest,
        factory: Arc<dyn PluginTerminalDriverFactory>,
        reply: oneshot::Sender<Result<PluginTerminalOpenBinding, PluginTerminalSessionError>>,
    },
    Subscribe {
        session_id: Uuid,
        reply: oneshot::Sender<
            Result<broadcast::Receiver<PluginTerminalSessionEvent>, PluginTerminalSessionError>,
        >,
    },
    Get {
        session_id: Uuid,
        reply: oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    },
    Snapshot {
        reply: oneshot::Sender<Vec<PluginTerminalSessionSummary>>,
    },
    Attach {
        request: PluginTerminalAttachRequest,
        reply: oneshot::Sender<Result<PluginTerminalAttachBinding, PluginTerminalSessionError>>,
    },
    RenewAttachment {
        request: PluginTerminalAttachmentRenewRequest,
        reply: oneshot::Sender<Result<PluginTerminalAttachment, PluginTerminalSessionError>>,
    },
    Detach {
        request: PluginTerminalDetachRequest,
        reply: oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    },
    GrantInput {
        request: PluginTerminalInputGrantRequest,
        reply: oneshot::Sender<Result<PluginTerminalInputLease, PluginTerminalSessionError>>,
    },
    RenewInput {
        lease: PluginTerminalInputLease,
        reply: oneshot::Sender<Result<PluginTerminalInputLease, PluginTerminalSessionError>>,
    },
    RevokeInput {
        session_id: Uuid,
        expected_focus_epoch: u64,
        reply: oneshot::Sender<Result<(), PluginTerminalSessionError>>,
    },
    ValidateFence {
        fence: PluginTerminalInputFence,
        reply: oneshot::Sender<Result<PluginTerminalInputLease, PluginTerminalSessionError>>,
    },
    Input {
        request: PluginTerminalInputRequest,
        reply: oneshot::Sender<Result<(), PluginTerminalSessionError>>,
    },
    Resize {
        request: PluginTerminalResizeRequest,
        reply: oneshot::Sender<Result<(), PluginTerminalSessionError>>,
    },
    Disconnect {
        request: PluginTerminalDisconnectRequest,
        reply: oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    },
    RetryCleanup {
        request: PluginTerminalCleanupRetryRequest,
        reply: oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    },
    Reconnect {
        claim: PluginTerminalFrontendLaunchClaim,
        request: PluginTerminalReconnectRequest,
        factory: Arc<dyn PluginTerminalDriverFactory>,
        reply: oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    },
    DriverEvent {
        session_id: Uuid,
        generation: u64,
        stream_id: Uuid,
        event: PluginTerminalDriverEvent,
    },
    DriverEventsEnded {
        session_id: Uuid,
        generation: u64,
        stream_id: Uuid,
    },
    DriverAck {
        session_id: Uuid,
        generation: u64,
        stream_id: Uuid,
        command_id: Uuid,
        kind: DriverAckKind,
        outcome: DriverAckOutcome,
    },
    ReapAttachments,
    ShutdownAll {
        reply: oneshot::Sender<Result<(), PluginTerminalSessionError>>,
    },
    RuntimeShutdown,
}

struct ServiceInner {
    tx: mpsc::Sender<Message>,
    exit_blockers: Arc<Mutex<BTreeSet<Uuid>>>,
}

impl Drop for ServiceInner {
    fn drop(&mut self) {
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            let _ = tx.send(Message::RuntimeShutdown).await;
        });
    }
}

#[derive(Clone)]
pub(crate) struct PluginTerminalSessionService {
    inner: Arc<ServiceInner>,
}

impl PluginTerminalSessionService {
    #[must_use]
    pub(crate) fn start() -> Self {
        Self::start_with_limits(PluginTerminalSessionLimits::default())
    }

    #[must_use]
    pub(crate) fn start_with_limits(limits: PluginTerminalSessionLimits) -> Self {
        assert!(limits.max_sessions > 0);
        assert!(limits.max_attachments_per_session > 0);
        assert!(limits.output_ring_max_bytes > 0);
        assert!(limits.max_driver_output_bytes > 0);
        assert!(!limits.driver_ack_timeout.is_zero());

        let (tx, rx) = mpsc::channel(ACTOR_MAILBOX_CAPACITY);
        let exit_blockers = Arc::new(Mutex::new(BTreeSet::new()));
        let actor = Actor {
            tx: tx.clone(),
            limits,
            sessions: BTreeMap::new(),
            open_ledger: BTreeMap::new(),
            open_order: VecDeque::new(),
            attach_ledger: BTreeMap::new(),
            attach_order: VecDeque::new(),
            reconnect_ledger: BTreeMap::new(),
            reconnect_order: VecDeque::new(),
            shutdown_waiters: Vec::new(),
            exit_blockers: exit_blockers.clone(),
        };
        tauri::async_runtime::spawn(run_actor(actor, rx));
        let reaper_tx = tx.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(ATTACHMENT_REAPER_INTERVAL).await;
                if reaper_tx.send(Message::ReapAttachments).await.is_err() {
                    break;
                }
            }
        });

        Self {
            inner: Arc::new(ServiceInner { tx, exit_blockers }),
        }
    }

    /// Starts a provider driver only after the root integration has consumed a
    /// frontend launch claim.  The actor never receives a plugin guest handle.
    pub(crate) async fn open_after_frontend_launch(
        &self,
        claim: PluginTerminalFrontendLaunchClaim,
        request: PluginTerminalOpenRequest,
        factory: Arc<dyn PluginTerminalDriverFactory>,
    ) -> Result<PluginTerminalOpenBinding, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Open {
            claim,
            request,
            factory,
            reply,
        })
        .await
    }

    pub(crate) async fn subscribe(
        &self,
        session_id: Uuid,
    ) -> Result<broadcast::Receiver<PluginTerminalSessionEvent>, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Subscribe {
            session_id,
            reply,
        })
        .await
    }

    pub(crate) async fn get(
        &self,
        session_id: Uuid,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Get { session_id, reply }).await
    }

    pub(crate) async fn snapshot(&self) -> Vec<PluginTerminalSessionSummary> {
        let (reply, response) = oneshot::channel();
        if self
            .inner
            .tx
            .send(Message::Snapshot { reply })
            .await
            .is_err()
        {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }

    pub(crate) async fn attach(
        &self,
        request: PluginTerminalAttachRequest,
    ) -> Result<PluginTerminalAttachBinding, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Attach { request, reply }).await
    }

    pub(crate) async fn renew_attachment(
        &self,
        request: PluginTerminalAttachmentRenewRequest,
    ) -> Result<PluginTerminalAttachment, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::RenewAttachment {
            request,
            reply,
        })
        .await
    }

    pub(crate) async fn detach(
        &self,
        request: PluginTerminalDetachRequest,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Detach { request, reply }).await
    }

    /// Called only after the canonical process-wide focus broker has committed
    /// this exact target.  This actor validates the fence and does not acquire
    /// or hold the broker itself.
    pub(crate) async fn grant_input(
        &self,
        request: PluginTerminalInputGrantRequest,
    ) -> Result<PluginTerminalInputLease, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::GrantInput {
            request,
            reply,
        })
        .await
    }

    pub(crate) async fn renew_input(
        &self,
        lease: PluginTerminalInputLease,
    ) -> Result<PluginTerminalInputLease, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::RenewInput { lease, reply }).await
    }

    pub(crate) async fn revoke_input(
        &self,
        session_id: Uuid,
        expected_focus_epoch: u64,
    ) -> Result<(), PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::RevokeInput {
            session_id,
            expected_focus_epoch,
            reply,
        })
        .await
    }

    pub(crate) async fn validate_fence(
        &self,
        fence: PluginTerminalInputFence,
    ) -> Result<PluginTerminalInputLease, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::ValidateFence {
            fence,
            reply,
        })
        .await
    }

    pub(crate) async fn input(
        &self,
        request: PluginTerminalInputRequest,
    ) -> Result<(), PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Input { request, reply }).await
    }

    pub(crate) async fn resize(
        &self,
        request: PluginTerminalResizeRequest,
    ) -> Result<(), PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Resize { request, reply }).await
    }

    /// The result resolves only after `Close` has a real driver acknowledgement
    /// (or a driver `Exit` confirms cleanup), while the actor continues to
    /// process output and driver events in the meantime.
    pub(crate) async fn disconnect(
        &self,
        request: PluginTerminalDisconnectRequest,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Disconnect {
            request,
            reply,
        })
        .await
    }

    pub(crate) async fn retry_cleanup(
        &self,
        request: PluginTerminalCleanupRetryRequest,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::RetryCleanup {
            request,
            reply,
        })
        .await
    }

    /// Reconnect is accepted only after the old driver's cleanup is proven.
    /// The root must supply a new launch claim and a fresh driver binding.
    pub(crate) async fn reconnect_after_frontend_launch(
        &self,
        claim: PluginTerminalFrontendLaunchClaim,
        request: PluginTerminalReconnectRequest,
        factory: Arc<dyn PluginTerminalDriverFactory>,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::Reconnect {
            claim,
            request,
            factory,
            reply,
        })
        .await
    }

    pub(crate) async fn shutdown_all(&self) -> Result<(), PluginTerminalSessionError> {
        request_reply(&self.inner.tx, |reply| Message::ShutdownAll { reply }).await
    }

    /// Current runtime resources and unproven cleanup attempts.  The root
    /// lifecycle integration can project these as application-exit blockers.
    #[must_use]
    pub(crate) fn exit_blockers(&self) -> Vec<Uuid> {
        self.inner
            .exit_blockers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }
}

async fn request_reply<T>(
    tx: &mpsc::Sender<Message>,
    message: impl FnOnce(oneshot::Sender<Result<T, PluginTerminalSessionError>>) -> Message,
) -> Result<T, PluginTerminalSessionError> {
    let (reply, response) = oneshot::channel();
    tx.send(message(reply))
        .await
        .map_err(|_| PluginTerminalSessionError::Unavailable)?;
    response
        .await
        .map_err(|_| PluginTerminalSessionError::Unavailable)?
}

struct Actor {
    tx: mpsc::Sender<Message>,
    limits: PluginTerminalSessionLimits,
    sessions: BTreeMap<Uuid, SessionRecord>,
    open_ledger: BTreeMap<Uuid, OpenLedgerEntry>,
    open_order: VecDeque<Uuid>,
    attach_ledger: BTreeMap<Uuid, AttachLedgerEntry>,
    attach_order: VecDeque<Uuid>,
    reconnect_ledger: BTreeMap<Uuid, ReconnectLedgerEntry>,
    reconnect_order: VecDeque<Uuid>,
    shutdown_waiters: Vec<oneshot::Sender<Result<(), PluginTerminalSessionError>>>,
    exit_blockers: Arc<Mutex<BTreeSet<Uuid>>>,
}

async fn run_actor(mut actor: Actor, mut rx: mpsc::Receiver<Message>) {
    while let Some(message) = rx.recv().await {
        match message {
            Message::Open {
                claim,
                request,
                factory,
                reply,
            } => {
                let _ = reply.send(actor.open(claim, request, factory));
            }
            Message::Subscribe { session_id, reply } => {
                let result = actor
                    .sessions
                    .get(&session_id)
                    .map(|record| record.events.subscribe())
                    .ok_or(PluginTerminalSessionError::NotFound);
                let _ = reply.send(result);
            }
            Message::Get { session_id, reply } => {
                let result = actor
                    .sessions
                    .get(&session_id)
                    .map(|record| record.summary.clone())
                    .ok_or(PluginTerminalSessionError::NotFound);
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
            Message::RenewAttachment { request, reply } => {
                let _ = reply.send(actor.renew_attachment(request));
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
                let _ = reply.send(actor.revoke_input(session_id, expected_focus_epoch));
            }
            Message::ValidateFence { fence, reply } => {
                let result = actor
                    .sessions
                    .get(&fence.session_id)
                    .ok_or(PluginTerminalSessionError::NotFound)
                    .and_then(|record| validate_fence(record, &fence));
                let _ = reply.send(result);
            }
            Message::Input { request, reply } => actor.input(request, reply),
            Message::Resize { request, reply } => actor.resize(request, reply),
            Message::Disconnect { request, reply } => actor.disconnect(request, reply),
            Message::RetryCleanup { request, reply } => actor.retry_cleanup(request, reply),
            Message::Reconnect {
                claim,
                request,
                factory,
                reply,
            } => {
                let _ = reply.send(actor.reconnect(claim, request, factory));
            }
            Message::DriverEvent {
                session_id,
                generation,
                stream_id,
                event,
            } => actor.driver_event(session_id, generation, stream_id, event),
            Message::DriverEventsEnded {
                session_id,
                generation,
                stream_id,
            } => actor.driver_events_ended(session_id, generation, stream_id),
            Message::DriverAck {
                session_id,
                generation,
                stream_id,
                command_id,
                kind,
                outcome,
            } => actor.driver_ack(session_id, generation, stream_id, command_id, kind, outcome),
            Message::ReapAttachments => actor.reap_attachments(),
            Message::ShutdownAll { reply } => {
                actor.shutdown_waiters.push(reply);
                actor.begin_shutdown();
            }
            Message::RuntimeShutdown => actor.begin_shutdown(),
        }
        actor.reclaim_terminal_history();
        actor.refresh_exit_blockers();
        actor.finish_shutdown_if_ready();
    }
}

impl Actor {
    fn open(
        &mut self,
        claim: PluginTerminalFrontendLaunchClaim,
        request: PluginTerminalOpenRequest,
        factory: Arc<dyn PluginTerminalDriverFactory>,
    ) -> Result<PluginTerminalOpenBinding, PluginTerminalSessionError> {
        validate_open_request(&request)?;
        let fingerprint = OpenFingerprint {
            launch_claim_id: claim.id(),
            connection_handle: request.connection_handle,
            provider: request.provider.clone(),
            attach_attempt_id: request.attach_attempt_id,
            view_id: request.view_id.clone(),
            rows: request.rows,
            cols: request.cols,
        };
        if let Some(existing) = self.open_ledger.get(&request.operation_id) {
            if existing.idempotency_key != request.idempotency_key
                || existing.fingerprint != fingerprint
            {
                return Err(PluginTerminalSessionError::Conflict);
            }
            let response = existing.result.clone()?;
            let events = self
                .sessions
                .get(&response.session.session_id)
                .map(|record| record.events.subscribe())
                .ok_or(PluginTerminalSessionError::NotFound)?;
            return Ok(PluginTerminalOpenBinding { response, events });
        }
        if self.sessions.len() >= self.limits.max_sessions {
            self.reclaim_terminal_history();
        }
        if self.sessions.len() >= self.limits.max_sessions {
            return Err(PluginTerminalSessionError::QuotaExceeded);
        }

        let session_id = Uuid::now_v7();
        let connection_handle = request.connection_handle;
        let stream_id = Uuid::now_v7();
        let driver_start = PluginTerminalDriverStart {
            connection_handle,
            provider: request.provider.clone(),
            generation: 1,
            stream_id,
            rows: request.rows,
            cols: request.cols,
        };
        let binding = match factory.start(driver_start) {
            Ok(binding) => binding,
            Err(_) => {
                let error = PluginTerminalSessionError::DriverStartFailed;
                self.open_ledger.insert(
                    request.operation_id,
                    OpenLedgerEntry {
                        idempotency_key: request.idempotency_key,
                        fingerprint,
                        result: Err(error.clone()),
                    },
                );
                self.open_order.push_back(request.operation_id);
                trim_ledger(&mut self.open_ledger, &mut self.open_order);
                return Err(error);
            }
        };

        let now = unix_time_ms();
        let (events, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        let attachment_id = Uuid::now_v7();
        let attachment = PluginTerminalAttachment {
            attachment_id,
            attach_attempt_id: request.attach_attempt_id,
            session_id,
            generation: 1,
            stream_id,
            view_id: request.view_id,
            state_revision: 1,
            attachment_revision: 1,
            attached_at_unix_ms: now,
        };
        let summary = PluginTerminalSessionSummary {
            session_id,
            provider: request.provider,
            generation: 1,
            stream_id,
            state_revision: 1,
            attachment_revision: 1,
            event_sequence: 0,
            state: PluginTerminalSessionState::Connecting,
            attachment_count: 1,
            cleanup_blocked: false,
            close_reason: None,
            failure_reason: None,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let event_pump_abort =
            spawn_driver_event_pump(self.tx.clone(), session_id, 1, stream_id, binding.events);
        let record = SessionRecord {
            summary: summary.clone(),
            attachments: BTreeMap::from([(
                attachment_id,
                AttachmentRecord {
                    attachment: attachment.clone(),
                    last_seen_at_unix_ms: now,
                },
            )]),
            events: events.clone(),
            input_lease: None,
            next_input_epoch: 0,
            last_client_sequence: 0,
            last_resize_sequence: 0,
            next_output_sequence: 1,
            output_ring: VecDeque::new(),
            output_ring_bytes: 0,
            dropped_through_sequence: 0,
            driver: Some(DriverRecord {
                commands: binding.commands,
                runtime: binding.runtime,
                event_pump_abort,
                runtime_cancelled: false,
            }),
            pending_command: None,
            pending_close: None,
            detach_after_close: None,
        };
        let response = PluginTerminalOpenResponse {
            session: summary,
            attachment,
        };
        self.sessions.insert(session_id, record);
        self.open_ledger.insert(
            request.operation_id,
            OpenLedgerEntry {
                idempotency_key: request.idempotency_key,
                fingerprint,
                result: Ok(response.clone()),
            },
        );
        self.open_order.push_back(request.operation_id);
        trim_ledger(&mut self.open_ledger, &mut self.open_order);
        Ok(PluginTerminalOpenBinding {
            response,
            events: events.subscribe(),
        })
    }

    fn attach(
        &mut self,
        request: PluginTerminalAttachRequest,
    ) -> Result<PluginTerminalAttachBinding, PluginTerminalSessionError> {
        validate_attach_request(&request)?;
        let fingerprint = AttachFingerprint {
            session_id: request.session_id,
            expected_generation: request.expected_generation,
            expected_stream_id: request.expected_stream_id,
            expected_state_revision: request.expected_state_revision,
            attach_attempt_id: request.attach_attempt_id,
            view_id: request.view_id.clone(),
            after_output_sequence: request.after_output_sequence,
        };
        if let Some(existing) = self.attach_ledger.get(&request.operation_id) {
            if existing.idempotency_key != request.idempotency_key
                || existing.fingerprint != fingerprint
            {
                return Err(PluginTerminalSessionError::Conflict);
            }
            let response = existing.result.clone()?;
            let events = self
                .sessions
                .get(&request.session_id)
                .map(|record| record.events.subscribe())
                .ok_or(PluginTerminalSessionError::NotFound)?;
            return Ok(PluginTerminalAttachBinding { response, events });
        }
        let result = self.attach_once(&request);
        self.attach_ledger.insert(
            request.operation_id,
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
            .get(&request.session_id)
            .map(|record| record.events.subscribe())
            .ok_or(PluginTerminalSessionError::NotFound)?;
        Ok(PluginTerminalAttachBinding { response, events })
    }

    fn attach_once(
        &mut self,
        request: &PluginTerminalAttachRequest,
    ) -> Result<PluginTerminalAttachResponse, PluginTerminalSessionError> {
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .ok_or(PluginTerminalSessionError::NotFound)?;
        validate_generation_and_stream(
            record,
            request.expected_generation,
            request.expected_stream_id,
        )?;
        if record.summary.state_revision != request.expected_state_revision {
            return Err(PluginTerminalSessionError::Conflict);
        }
        let previous = record
            .attachments
            .values()
            .find(|item| item.attachment.view_id == request.view_id)
            .map(|item| item.attachment.attachment_id);
        if previous.is_none() && record.attachments.len() >= self.limits.max_attachments_per_session
        {
            return Err(PluginTerminalSessionError::QuotaExceeded);
        }
        if let Some(previous) = previous {
            remove_attachment(record, previous);
        }
        let attachment = add_attachment(record, request.attach_attempt_id, request.view_id.clone());
        let replay = replay_after(record, request.after_output_sequence);
        Ok(PluginTerminalAttachResponse {
            session: record.summary.clone(),
            attachment,
            replay,
        })
    }

    fn renew_attachment(
        &mut self,
        request: PluginTerminalAttachmentRenewRequest,
    ) -> Result<PluginTerminalAttachment, PluginTerminalSessionError> {
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .ok_or(PluginTerminalSessionError::NotFound)?;
        validate_generation_and_stream(
            record,
            request.expected_generation,
            request.expected_stream_id,
        )?;
        if record.summary.attachment_revision != request.expected_attachment_revision {
            return Err(PluginTerminalSessionError::Conflict);
        }
        let attachment = record
            .attachments
            .get_mut(&request.attachment_id)
            .filter(|item| item.attachment.view_id == request.view_id)
            .ok_or(PluginTerminalSessionError::Conflict)?;
        attachment.last_seen_at_unix_ms = unix_time_ms();
        Ok(attachment.attachment.clone())
    }

    fn detach(
        &mut self,
        request: PluginTerminalDetachRequest,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        validate_detach_request(&request)?;
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .ok_or(PluginTerminalSessionError::NotFound)?;
        validate_generation_and_stream(
            record,
            request.expected_generation,
            request.expected_stream_id,
        )?;
        if record.summary.state_revision != request.expected_state_revision
            || record.summary.attachment_revision != request.expected_attachment_revision
        {
            return Err(PluginTerminalSessionError::Conflict);
        }
        validate_attachment(record, request.attachment_id, &request.view_id)?;
        let active = matches!(
            record.summary.state,
            PluginTerminalSessionState::Connecting
                | PluginTerminalSessionState::Running
                | PluginTerminalSessionState::Closing
        );
        if request.intent == PluginTerminalDetachIntent::UserClose
            && record.attachments.len() == 1
            && active
        {
            if !request.disconnect_if_last {
                return Err(PluginTerminalSessionError::LastAttachmentRequiresDisconnect);
            }
            record.detach_after_close = Some(request.attachment_id);
            begin_close(
                &tx,
                &limits,
                record,
                PluginTerminalCloseReason::UserRequested,
                false,
                None,
            )?;
            return Ok(record.summary.clone());
        }
        remove_attachment(record, request.attachment_id);
        Ok(record.summary.clone())
    }

    fn grant_input(
        &mut self,
        request: PluginTerminalInputGrantRequest,
    ) -> Result<PluginTerminalInputLease, PluginTerminalSessionError> {
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .ok_or(PluginTerminalSessionError::NotFound)?;
        validate_generation_and_stream(
            record,
            request.expected_generation,
            request.expected_stream_id,
        )?;
        if record.summary.state != PluginTerminalSessionState::Running
            || record.summary.state_revision != request.expected_state_revision
        {
            return Err(PluginTerminalSessionError::Conflict);
        }
        validate_attachment(record, request.attachment_id, &request.view_id)?;
        if let Some(current) = &record.input_lease
            && current.generation == request.expected_generation
            && current.stream_id == request.expected_stream_id
            && current.attachment_id == request.attachment_id
            && current.view_id == request.view_id
            && current.state_revision == request.expected_state_revision
            && current.focus_epoch == request.focus_epoch
            && current.expires_at_unix_ms > unix_time_ms()
        {
            return Ok(current.clone());
        }
        revoke_lease(record);
        record.next_input_epoch = record.next_input_epoch.saturating_add(1);
        let lease = PluginTerminalInputLease {
            lease_id: Uuid::now_v7(),
            session_id: request.session_id,
            generation: request.expected_generation,
            stream_id: request.expected_stream_id,
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
        lease: PluginTerminalInputLease,
    ) -> Result<PluginTerminalInputLease, PluginTerminalSessionError> {
        let record = self
            .sessions
            .get_mut(&lease.session_id)
            .ok_or(PluginTerminalSessionError::NotFound)?;
        validate_fence(record, &PluginTerminalInputFence::from(&lease))?;
        let current = record
            .input_lease
            .as_mut()
            .ok_or(PluginTerminalSessionError::InputNotAuthorized)?;
        current.expires_at_unix_ms = unix_time_ms().saturating_add(INPUT_LEASE_MILLIS);
        Ok(current.clone())
    }

    fn revoke_input(
        &mut self,
        session_id: Uuid,
        expected_focus_epoch: u64,
    ) -> Result<(), PluginTerminalSessionError> {
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        let record = self
            .sessions
            .get_mut(&session_id)
            .ok_or(PluginTerminalSessionError::NotFound)?;
        if let Some(lease) = &record.input_lease
            && lease.focus_epoch != expected_focus_epoch
        {
            return Err(PluginTerminalSessionError::Conflict);
        }
        if record.pending_command.is_some() {
            fail_generation(
                &tx,
                &limits,
                record,
                PluginTerminalFailureCode::DriverAckUnknown,
            );
        } else {
            revoke_lease(record);
        }
        Ok(())
    }

    fn input(
        &mut self,
        request: PluginTerminalInputRequest,
        reply: oneshot::Sender<Result<(), PluginTerminalSessionError>>,
    ) {
        if request.data.is_empty() || request.data.len() > self.limits.max_driver_output_bytes {
            let _ = reply.send(Err(PluginTerminalSessionError::Validation));
            return;
        }
        let tx = self.tx.clone();
        let timeout_duration = self.limits.driver_ack_timeout;
        let Some(record) = self.sessions.get_mut(&request.fence.session_id) else {
            let _ = reply.send(Err(PluginTerminalSessionError::NotFound));
            return;
        };
        if let Err(error) = validate_fence(record, &request.fence) {
            let _ = reply.send(Err(error));
            return;
        }
        if request.client_sequence <= record.last_client_sequence
            || record.pending_command.is_some()
        {
            let _ = reply.send(Err(PluginTerminalSessionError::Busy));
            return;
        }
        let Some(driver) = record.driver.as_ref() else {
            let _ = reply.send(Err(PluginTerminalSessionError::Unavailable));
            return;
        };
        let command_id = Uuid::now_v7();
        let (ack, response) = oneshot::channel();
        if driver
            .commands
            .try_send(PluginTerminalDriverCommand::Input {
                bytes: request.data,
                ack,
            })
            .is_err()
        {
            let _ = reply.send(Err(PluginTerminalSessionError::Unavailable));
            return;
        }
        let generation = record.summary.generation;
        let stream_id = record.summary.stream_id;
        record.pending_command = Some(PendingDriverCommand {
            command_id,
            kind: PendingDriverKind::Input {
                sequence: request.client_sequence,
                fence: request.fence,
            },
            reply,
        });
        spawn_driver_ack_wait(
            tx,
            timeout_duration,
            record.summary.session_id,
            generation,
            stream_id,
            command_id,
            DriverAckKind::Command,
            response,
        );
    }

    fn resize(
        &mut self,
        request: PluginTerminalResizeRequest,
        reply: oneshot::Sender<Result<(), PluginTerminalSessionError>>,
    ) {
        if request.rows == 0 || request.cols == 0 {
            let _ = reply.send(Err(PluginTerminalSessionError::Validation));
            return;
        }
        let tx = self.tx.clone();
        let timeout_duration = self.limits.driver_ack_timeout;
        let Some(record) = self.sessions.get_mut(&request.fence.session_id) else {
            let _ = reply.send(Err(PluginTerminalSessionError::NotFound));
            return;
        };
        if let Err(error) = validate_fence(record, &request.fence) {
            let _ = reply.send(Err(error));
            return;
        }
        if request.resize_sequence <= record.last_resize_sequence
            || record.pending_command.is_some()
        {
            let _ = reply.send(Err(PluginTerminalSessionError::Busy));
            return;
        }
        let Some(driver) = record.driver.as_ref() else {
            let _ = reply.send(Err(PluginTerminalSessionError::Unavailable));
            return;
        };
        let command_id = Uuid::now_v7();
        let (ack, response) = oneshot::channel();
        if driver
            .commands
            .try_send(PluginTerminalDriverCommand::Resize {
                rows: request.rows,
                cols: request.cols,
                ack,
            })
            .is_err()
        {
            let _ = reply.send(Err(PluginTerminalSessionError::Unavailable));
            return;
        }
        let generation = record.summary.generation;
        let stream_id = record.summary.stream_id;
        record.pending_command = Some(PendingDriverCommand {
            command_id,
            kind: PendingDriverKind::Resize {
                sequence: request.resize_sequence,
                fence: request.fence,
            },
            reply,
        });
        spawn_driver_ack_wait(
            tx,
            timeout_duration,
            record.summary.session_id,
            generation,
            stream_id,
            command_id,
            DriverAckKind::Command,
            response,
        );
    }

    fn disconnect(
        &mut self,
        request: PluginTerminalDisconnectRequest,
        reply: oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    ) {
        if request.idempotency_key.trim().is_empty() {
            let _ = reply.send(Err(PluginTerminalSessionError::Validation));
            return;
        }
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        let Some(record) = self.sessions.get_mut(&request.session_id) else {
            let _ = reply.send(Err(PluginTerminalSessionError::NotFound));
            return;
        };
        if let Err(error) = validate_generation_and_stream(
            record,
            request.expected_generation,
            request.expected_stream_id,
        )
        .and_then(|()| {
            (record.summary.state_revision == request.expected_state_revision)
                .then_some(())
                .ok_or(PluginTerminalSessionError::Conflict)
        }) {
            let _ = reply.send(Err(error));
            return;
        }
        if record.summary.cleanup_blocked || record.pending_close.is_some() {
            let _ = reply.send(Err(PluginTerminalSessionError::Busy));
            return;
        }
        if record.summary.state == PluginTerminalSessionState::Closed {
            let _ = reply.send(Ok(record.summary.clone()));
            return;
        }
        revoke_lease(record);
        let _ = begin_close(
            &tx,
            &limits,
            record,
            PluginTerminalCloseReason::UserRequested,
            record.summary.state == PluginTerminalSessionState::Failed,
            Some(reply),
        );
    }

    fn retry_cleanup(
        &mut self,
        request: PluginTerminalCleanupRetryRequest,
        reply: oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    ) {
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        let Some(record) = self.sessions.get_mut(&request.session_id) else {
            let _ = reply.send(Err(PluginTerminalSessionError::NotFound));
            return;
        };
        if let Err(error) = validate_generation_and_stream(
            record,
            request.expected_generation,
            request.expected_stream_id,
        )
        .and_then(|()| {
            (record.summary.state_revision == request.expected_state_revision)
                .then_some(())
                .ok_or(PluginTerminalSessionError::Conflict)
        }) {
            let _ = reply.send(Err(error));
            return;
        }
        if !record.summary.cleanup_blocked || record.pending_close.is_some() {
            let _ = reply.send(Err(PluginTerminalSessionError::Conflict));
            return;
        }
        let _ = begin_close(
            &tx,
            &limits,
            record,
            PluginTerminalCloseReason::CoreShutdown,
            true,
            Some(reply),
        );
    }

    fn reconnect(
        &mut self,
        claim: PluginTerminalFrontendLaunchClaim,
        request: PluginTerminalReconnectRequest,
        factory: Arc<dyn PluginTerminalDriverFactory>,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        validate_reconnect_request(&request)?;
        let fingerprint = ReconnectFingerprint {
            launch_claim_id: claim.id(),
            session_id: request.session_id,
            expected_generation: request.expected_generation,
            expected_stream_id: request.expected_stream_id,
            expected_state_revision: request.expected_state_revision,
            connection_handle: request.connection_handle,
            provider: request.provider.clone(),
            rows: request.rows,
            cols: request.cols,
        };
        if let Some(existing) = self.reconnect_ledger.get(&request.operation_id) {
            if existing.idempotency_key != request.idempotency_key
                || existing.fingerprint != fingerprint
            {
                return Err(PluginTerminalSessionError::Conflict);
            }
            return existing.result.clone();
        }
        let result = self.reconnect_once(&request, factory);
        self.reconnect_ledger.insert(
            request.operation_id,
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
        request: &PluginTerminalReconnectRequest,
        factory: Arc<dyn PluginTerminalDriverFactory>,
    ) -> Result<PluginTerminalSessionSummary, PluginTerminalSessionError> {
        let (connection_handle, generation, stream_id) = {
            let record = self
                .sessions
                .get_mut(&request.session_id)
                .ok_or(PluginTerminalSessionError::NotFound)?;
            validate_generation_and_stream(
                record,
                request.expected_generation,
                request.expected_stream_id,
            )?;
            if record.summary.state_revision != request.expected_state_revision
                || record.summary.provider != request.provider
                || record.summary.cleanup_blocked
                || record.driver.is_some()
                || record.pending_command.is_some()
                || record.pending_close.is_some()
                || !matches!(
                    record.summary.state,
                    PluginTerminalSessionState::Closed | PluginTerminalSessionState::Failed
                )
            {
                return Err(PluginTerminalSessionError::Conflict);
            }
            record.summary.generation = record.summary.generation.saturating_add(1);
            record.summary.stream_id = Uuid::now_v7();
            record.summary.close_reason = None;
            record.summary.failure_reason = None;
            record.summary.cleanup_blocked = false;
            record.next_input_epoch = 0;
            record.last_client_sequence = 0;
            record.last_resize_sequence = 0;
            record.next_output_sequence = 1;
            record.output_ring.clear();
            record.output_ring_bytes = 0;
            record.dropped_through_sequence = 0;
            for attachment in record.attachments.values_mut() {
                attachment.attachment.generation = record.summary.generation;
                attachment.attachment.stream_id = record.summary.stream_id;
            }
            transition(record, PluginTerminalSessionState::Connecting, None, None);
            emit_state(record);
            (
                request.connection_handle,
                record.summary.generation,
                record.summary.stream_id,
            )
        };
        let driver_start = PluginTerminalDriverStart {
            connection_handle,
            provider: request.provider.clone(),
            generation,
            stream_id,
            rows: request.rows,
            cols: request.cols,
        };
        let binding = match factory.start(driver_start) {
            Ok(binding) => binding,
            Err(_) => {
                let record = self
                    .sessions
                    .get_mut(&request.session_id)
                    .expect("record exists while reconnecting");
                transition(
                    record,
                    PluginTerminalSessionState::Failed,
                    None,
                    Some(PluginTerminalFailureReason {
                        code: PluginTerminalFailureCode::DriverStartFailed,
                    }),
                );
                emit_state(record);
                return Err(PluginTerminalSessionError::DriverStartFailed);
            }
        };
        let event_pump_abort = spawn_driver_event_pump(
            self.tx.clone(),
            request.session_id,
            generation,
            stream_id,
            binding.events,
        );
        let record = self
            .sessions
            .get_mut(&request.session_id)
            .expect("record exists while reconnecting");
        record.driver = Some(DriverRecord {
            commands: binding.commands,
            runtime: binding.runtime,
            event_pump_abort,
            runtime_cancelled: false,
        });
        Ok(record.summary.clone())
    }

    fn driver_event(
        &mut self,
        session_id: Uuid,
        generation: u64,
        stream_id: Uuid,
        event: PluginTerminalDriverEvent,
    ) {
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        let Some(record) = self.sessions.get_mut(&session_id) else {
            return;
        };
        if record.summary.generation != generation || record.summary.stream_id != stream_id {
            return;
        }
        match event {
            PluginTerminalDriverEvent::Ready => {
                if record.summary.state == PluginTerminalSessionState::Connecting {
                    transition(record, PluginTerminalSessionState::Running, None, None);
                    emit_state(record);
                }
            }
            PluginTerminalDriverEvent::Output(data) => {
                if !matches!(
                    record.summary.state,
                    PluginTerminalSessionState::Running | PluginTerminalSessionState::Closing
                ) {
                    return;
                }
                if data.len() > limits.max_driver_output_bytes {
                    fail_generation(
                        &tx,
                        &limits,
                        record,
                        PluginTerminalFailureCode::OutputLimitExceeded,
                    );
                    return;
                }
                if !data.is_empty() {
                    append_output(record, limits.output_ring_max_bytes, data);
                }
            }
            PluginTerminalDriverEvent::Exit => finish_driver_exit(record),
            PluginTerminalDriverEvent::Failed(_) => fail_generation(
                &tx,
                &limits,
                record,
                PluginTerminalFailureCode::DriverFailed,
            ),
        }
    }

    fn driver_events_ended(&mut self, session_id: Uuid, generation: u64, stream_id: Uuid) {
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        let Some(record) = self.sessions.get_mut(&session_id) else {
            return;
        };
        if record.summary.generation != generation
            || record.summary.stream_id != stream_id
            || record.driver.is_none()
            || record.summary.state == PluginTerminalSessionState::Closed
        {
            return;
        }
        fail_generation(
            &tx,
            &limits,
            record,
            PluginTerminalFailureCode::DriverEventStreamClosed,
        );
    }

    fn driver_ack(
        &mut self,
        session_id: Uuid,
        generation: u64,
        stream_id: Uuid,
        command_id: Uuid,
        kind: DriverAckKind,
        outcome: DriverAckOutcome,
    ) {
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        let Some(record) = self.sessions.get_mut(&session_id) else {
            return;
        };
        if record.summary.generation != generation || record.summary.stream_id != stream_id {
            return;
        }
        match kind {
            DriverAckKind::Command => {
                let matches = record
                    .pending_command
                    .as_ref()
                    .is_some_and(|pending| pending.command_id == command_id);
                if !matches {
                    return;
                }
                let pending = record
                    .pending_command
                    .take()
                    .expect("checked pending command");
                match outcome {
                    DriverAckOutcome::Completed(Ok(())) => {
                        let fence = match &pending.kind {
                            PendingDriverKind::Input { fence, .. }
                            | PendingDriverKind::Resize { fence, .. } => fence,
                        };
                        if validate_fence(record, fence).is_ok() {
                            match pending.kind {
                                PendingDriverKind::Input { sequence, .. } => {
                                    record.last_client_sequence = sequence;
                                }
                                PendingDriverKind::Resize { sequence, .. } => {
                                    record.last_resize_sequence = sequence;
                                }
                            }
                            let _ = pending.reply.send(Ok(()));
                        } else {
                            let _ = pending
                                .reply
                                .send(Err(PluginTerminalSessionError::InputNotAuthorized));
                            fail_generation(
                                &tx,
                                &limits,
                                record,
                                PluginTerminalFailureCode::DriverAckUnknown,
                            );
                        }
                    }
                    DriverAckOutcome::Completed(Err(PluginTerminalDriverError::Unknown))
                    | DriverAckOutcome::Unknown => {
                        let _ = pending
                            .reply
                            .send(Err(PluginTerminalSessionError::OutcomeUnknown));
                        fail_generation(
                            &tx,
                            &limits,
                            record,
                            PluginTerminalFailureCode::DriverAckUnknown,
                        );
                    }
                    DriverAckOutcome::Completed(Err(_)) => {
                        let _ = pending
                            .reply
                            .send(Err(PluginTerminalSessionError::DriverRejected));
                    }
                }
            }
            DriverAckKind::Close => {
                let matches = record
                    .pending_close
                    .as_ref()
                    .is_some_and(|pending| pending.command_id == command_id);
                if !matches {
                    return;
                }
                let pending = record.pending_close.take().expect("checked pending close");
                match outcome {
                    DriverAckOutcome::Completed(Ok(())) => {
                        finish_driver_cleanup(
                            record,
                            pending.reason,
                            pending.preserve_failure,
                            pending.reply,
                        );
                    }
                    DriverAckOutcome::Completed(Err(_)) | DriverAckOutcome::Unknown => {
                        let reply = pending.reply;
                        cleanup_failed(
                            record,
                            PluginTerminalFailureCode::CleanupIncomplete,
                            pending.reason == PluginTerminalCloseReason::CoreShutdown,
                        );
                        if let Some(reply) = reply {
                            let _ = reply.send(Err(PluginTerminalSessionError::CleanupIncomplete));
                        }
                    }
                }
            }
        }
    }

    fn reap_attachments(&mut self) {
        let now = unix_time_ms();
        for record in self.sessions.values_mut() {
            let stale = record
                .attachments
                .iter()
                .filter_map(|(id, item)| {
                    (now.saturating_sub(item.last_seen_at_unix_ms) > ATTACHMENT_TIMEOUT_MILLIS)
                        .then_some(*id)
                })
                .collect::<Vec<_>>();
            for attachment_id in stale {
                remove_attachment(record, attachment_id);
            }
        }
    }

    fn begin_shutdown(&mut self) {
        let tx = self.tx.clone();
        let limits = self.limits.clone();
        for record in self.sessions.values_mut() {
            if record.pending_close.is_some() || !session_requires_exit_blocker(record) {
                continue;
            }
            revoke_lease(record);
            let preserve_failure = record.summary.state == PluginTerminalSessionState::Failed;
            let _ = begin_close(
                &tx,
                &limits,
                record,
                PluginTerminalCloseReason::CoreShutdown,
                preserve_failure,
                None,
            );
        }
    }

    fn refresh_exit_blockers(&self) {
        let next = self
            .sessions
            .iter()
            .filter_map(|(session_id, record)| {
                session_requires_exit_blocker(record).then_some(*session_id)
            })
            .collect::<BTreeSet<_>>();
        *self
            .exit_blockers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }

    /// Session rows are only an in-memory projection.  A terminal that has no
    /// attachment, no driver, no pending owner command and no unproven cleanup
    /// has no remaining Core resource or user-visible recovery target.  Keep
    /// all other terminal states, especially zero-attachment Running sessions
    /// and cleanup failures, as exit blockers.
    fn reclaim_terminal_history(&mut self) {
        self.sessions.retain(|_, record| {
            !record.attachments.is_empty()
                || record.driver.is_some()
                || record.pending_command.is_some()
                || record.pending_close.is_some()
                || record.summary.cleanup_blocked
                || !matches!(
                    record.summary.state,
                    PluginTerminalSessionState::Closed | PluginTerminalSessionState::Failed
                )
        });
    }

    fn finish_shutdown_if_ready(&mut self) {
        if self.shutdown_waiters.is_empty() {
            return;
        }
        let blocked = self
            .sessions
            .values()
            .any(|record| record.summary.cleanup_blocked);
        if blocked {
            let waiters = std::mem::take(&mut self.shutdown_waiters);
            for reply in waiters {
                let _ = reply.send(Err(PluginTerminalSessionError::CleanupIncomplete));
            }
            return;
        }
        if self
            .sessions
            .values()
            .all(|record| !session_requires_exit_blocker(record))
        {
            let waiters = std::mem::take(&mut self.shutdown_waiters);
            for reply in waiters {
                let _ = reply.send(Ok(()));
            }
        }
    }
}

fn validate_open_request(
    request: &PluginTerminalOpenRequest,
) -> Result<(), PluginTerminalSessionError> {
    if request.operation_id.is_nil()
        || request.attach_attempt_id.is_nil()
        || request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > 128
        || request.rows == 0
        || request.cols == 0
        || !valid_metadata(&request.provider)
        || !valid_view_id(&request.view_id)
    {
        return Err(PluginTerminalSessionError::Validation);
    }
    Ok(())
}

fn validate_attach_request(
    request: &PluginTerminalAttachRequest,
) -> Result<(), PluginTerminalSessionError> {
    if request.operation_id.is_nil()
        || request.attach_attempt_id.is_nil()
        || request.session_id.is_nil()
        || request.expected_stream_id.is_nil()
        || request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > 128
        || !valid_view_id(&request.view_id)
    {
        return Err(PluginTerminalSessionError::Validation);
    }
    Ok(())
}

fn validate_detach_request(
    request: &PluginTerminalDetachRequest,
) -> Result<(), PluginTerminalSessionError> {
    if request.operation_id.is_nil()
        || request.session_id.is_nil()
        || request.expected_stream_id.is_nil()
        || request.attachment_id.is_nil()
        || request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > 128
        || !valid_view_id(&request.view_id)
    {
        return Err(PluginTerminalSessionError::Validation);
    }
    Ok(())
}

fn validate_reconnect_request(
    request: &PluginTerminalReconnectRequest,
) -> Result<(), PluginTerminalSessionError> {
    if request.operation_id.is_nil()
        || request.session_id.is_nil()
        || request.expected_stream_id.is_nil()
        || request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > 128
        || request.rows == 0
        || request.cols == 0
        || !valid_metadata(&request.provider)
    {
        return Err(PluginTerminalSessionError::Validation);
    }
    Ok(())
}

fn valid_metadata(metadata: &PluginTerminalProviderMetadata) -> bool {
    [
        metadata.plugin_id.as_str(),
        metadata.provider_id.as_str(),
        metadata.display_label.as_str(),
    ]
    .into_iter()
    .all(|value| {
        !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
    })
}

fn valid_view_id(view_id: &str) -> bool {
    !view_id.trim().is_empty() && view_id.len() <= 128 && !view_id.chars().any(char::is_control)
}

fn validate_generation_and_stream(
    record: &SessionRecord,
    generation: u64,
    stream_id: Uuid,
) -> Result<(), PluginTerminalSessionError> {
    if record.summary.generation == generation && record.summary.stream_id == stream_id {
        Ok(())
    } else {
        Err(PluginTerminalSessionError::Conflict)
    }
}

fn validate_attachment(
    record: &SessionRecord,
    attachment_id: Uuid,
    view_id: &str,
) -> Result<(), PluginTerminalSessionError> {
    record
        .attachments
        .get(&attachment_id)
        .filter(|item| item.attachment.view_id == view_id)
        .map(|_| ())
        .ok_or(PluginTerminalSessionError::Conflict)
}

fn validate_fence(
    record: &SessionRecord,
    fence: &PluginTerminalInputFence,
) -> Result<PluginTerminalInputLease, PluginTerminalSessionError> {
    validate_generation_and_stream(record, fence.expected_generation, fence.expected_stream_id)?;
    if record.summary.state != PluginTerminalSessionState::Running
        || record.summary.state_revision != fence.expected_state_revision
    {
        return Err(PluginTerminalSessionError::InputNotAuthorized);
    }
    validate_attachment(record, fence.attachment_id, &fence.view_id)
        .map_err(|_| PluginTerminalSessionError::InputNotAuthorized)?;
    let lease = record
        .input_lease
        .as_ref()
        .ok_or(PluginTerminalSessionError::InputNotAuthorized)?;
    if lease.lease_id != fence.lease_id
        || lease.session_id != fence.session_id
        || lease.generation != fence.expected_generation
        || lease.stream_id != fence.expected_stream_id
        || lease.attachment_id != fence.attachment_id
        || lease.view_id != fence.view_id
        || lease.state_revision != fence.expected_state_revision
        || lease.focus_epoch != fence.focus_epoch
        || lease.input_epoch != fence.input_epoch
        || lease.expires_at_unix_ms <= unix_time_ms()
    {
        return Err(PluginTerminalSessionError::InputNotAuthorized);
    }
    Ok(lease.clone())
}

fn add_attachment(
    record: &mut SessionRecord,
    attach_attempt_id: Uuid,
    view_id: String,
) -> PluginTerminalAttachment {
    let attachment = PluginTerminalAttachment {
        attachment_id: Uuid::now_v7(),
        attach_attempt_id,
        session_id: record.summary.session_id,
        generation: record.summary.generation,
        stream_id: record.summary.stream_id,
        view_id,
        state_revision: record.summary.state_revision,
        attachment_revision: record.summary.attachment_revision.saturating_add(1),
        attached_at_unix_ms: unix_time_ms(),
    };
    record.summary.attachment_revision = record.summary.attachment_revision.saturating_add(1);
    record.summary.attachment_count = record.attachments.len().saturating_add(1) as u32;
    record.summary.updated_at_unix_ms = unix_time_ms();
    refresh_attachment_revisions(record);
    record.attachments.insert(
        attachment.attachment_id,
        AttachmentRecord {
            attachment: attachment.clone(),
            last_seen_at_unix_ms: unix_time_ms(),
        },
    );
    bump_event_sequence(record);
    let _ = record
        .events
        .send(PluginTerminalSessionEvent::AttachmentChanged {
            session: record.summary.clone(),
            attachment: attachment.clone(),
        });
    attachment
}

fn remove_attachment(record: &mut SessionRecord, attachment_id: Uuid) {
    if record.attachments.remove(&attachment_id).is_none() {
        return;
    }
    revoke_lease_for_attachment(record, attachment_id);
    record.summary.attachment_revision = record.summary.attachment_revision.saturating_add(1);
    record.summary.attachment_count = record.attachments.len() as u32;
    record.summary.updated_at_unix_ms = unix_time_ms();
    refresh_attachment_revisions(record);
    bump_event_sequence(record);
    let _ = record
        .events
        .send(PluginTerminalSessionEvent::AttachmentDetached {
            session: record.summary.clone(),
            attachment_id,
        });
}

fn refresh_attachment_revisions(record: &mut SessionRecord) {
    for attachment in record.attachments.values_mut() {
        attachment.attachment.state_revision = record.summary.state_revision;
        attachment.attachment.attachment_revision = record.summary.attachment_revision;
    }
}

fn replay_after(
    record: &SessionRecord,
    after_output_sequence: Option<u64>,
) -> Vec<PluginTerminalReplayItem> {
    let mut replay = Vec::new();
    let after = after_output_sequence.unwrap_or_default();
    if record.dropped_through_sequence > 0 && after < record.dropped_through_sequence {
        replay.push(PluginTerminalReplayItem::Gap {
            generation: record.summary.generation,
            stream_id: record.summary.stream_id,
            dropped_through_sequence: record.dropped_through_sequence,
        });
    }
    replay.extend(
        record
            .output_ring
            .iter()
            .filter(|frame| frame.sequence > after)
            .cloned()
            .map(PluginTerminalReplayItem::Frame),
    );
    replay
}

fn append_output(record: &mut SessionRecord, max_ring_bytes: usize, data: Vec<u8>) {
    let frame = PluginTerminalOutputFrame {
        generation: record.summary.generation,
        stream_id: record.summary.stream_id,
        sequence: record.next_output_sequence,
        data,
    };
    record.next_output_sequence = record.next_output_sequence.saturating_add(1);
    record.output_ring_bytes = record.output_ring_bytes.saturating_add(frame.data.len());
    record.output_ring.push_back(frame.clone());
    while record.output_ring_bytes > max_ring_bytes {
        let Some(dropped) = record.output_ring.pop_front() else {
            break;
        };
        record.output_ring_bytes = record.output_ring_bytes.saturating_sub(dropped.data.len());
        record.dropped_through_sequence = dropped.sequence;
    }
    bump_event_sequence(record);
    let _ = record.events.send(PluginTerminalSessionEvent::Output {
        session: record.summary.clone(),
        frame,
    });
}

fn transition(
    record: &mut SessionRecord,
    state: PluginTerminalSessionState,
    close_reason: Option<PluginTerminalCloseReason>,
    failure_reason: Option<PluginTerminalFailureReason>,
) {
    record.summary.state = state;
    record.summary.state_revision = record.summary.state_revision.saturating_add(1);
    record.summary.close_reason = close_reason;
    record.summary.failure_reason = failure_reason;
    record.summary.updated_at_unix_ms = unix_time_ms();
    refresh_attachment_revisions(record);
    bump_event_sequence(record);
}

fn emit_state(record: &SessionRecord) {
    let _ = record
        .events
        .send(PluginTerminalSessionEvent::StateChanged {
            session: record.summary.clone(),
        });
}

fn bump_event_sequence(record: &mut SessionRecord) {
    record.summary.event_sequence = record.summary.event_sequence.saturating_add(1);
}

fn revoke_lease_for_attachment(record: &mut SessionRecord, attachment_id: Uuid) {
    if record
        .input_lease
        .as_ref()
        .is_some_and(|lease| lease.attachment_id == attachment_id)
    {
        revoke_lease(record);
    }
}

fn revoke_lease(record: &mut SessionRecord) {
    let Some(lease) = record.input_lease.take() else {
        return;
    };
    bump_event_sequence(record);
    let _ = record
        .events
        .send(PluginTerminalSessionEvent::InputLeaseRevoked {
            session: record.summary.clone(),
            focus_epoch: lease.focus_epoch,
        });
}

fn session_requires_exit_blocker(record: &SessionRecord) -> bool {
    record.summary.cleanup_blocked
        || matches!(
            record.summary.state,
            PluginTerminalSessionState::Connecting
                | PluginTerminalSessionState::Running
                | PluginTerminalSessionState::Closing
        )
}

fn begin_close(
    tx: &mpsc::Sender<Message>,
    limits: &PluginTerminalSessionLimits,
    record: &mut SessionRecord,
    reason: PluginTerminalCloseReason,
    preserve_failure: bool,
    reply: Option<
        oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    >,
) -> Result<(), PluginTerminalSessionError> {
    if record.pending_close.is_some() {
        if let Some(reply) = reply {
            let _ = reply.send(Err(PluginTerminalSessionError::Busy));
        }
        return Err(PluginTerminalSessionError::Busy);
    }
    let Some(driver) = record.driver.as_ref() else {
        cleanup_failed(
            record,
            PluginTerminalFailureCode::CleanupIncomplete,
            reason == PluginTerminalCloseReason::CoreShutdown,
        );
        if let Some(reply) = reply {
            let _ = reply.send(Err(PluginTerminalSessionError::CleanupIncomplete));
        }
        return Err(PluginTerminalSessionError::CleanupIncomplete);
    };
    let command_id = Uuid::now_v7();
    let (ack, response) = oneshot::channel();
    if driver
        .commands
        .try_send(PluginTerminalDriverCommand::Close { ack })
        .is_err()
    {
        cleanup_failed(
            record,
            PluginTerminalFailureCode::CleanupIncomplete,
            reason == PluginTerminalCloseReason::CoreShutdown,
        );
        if let Some(reply) = reply {
            let _ = reply.send(Err(PluginTerminalSessionError::CleanupIncomplete));
        }
        return Err(PluginTerminalSessionError::CleanupIncomplete);
    }
    if !preserve_failure && record.summary.state != PluginTerminalSessionState::Closing {
        transition(record, PluginTerminalSessionState::Closing, None, None);
        emit_state(record);
    }
    let session_id = record.summary.session_id;
    let generation = record.summary.generation;
    let stream_id = record.summary.stream_id;
    record.pending_close = Some(PendingClose {
        command_id,
        reason,
        preserve_failure,
        reply,
    });
    spawn_driver_ack_wait(
        tx.clone(),
        limits.driver_ack_timeout,
        session_id,
        generation,
        stream_id,
        command_id,
        DriverAckKind::Close,
        response,
    );
    Ok(())
}

fn fail_generation(
    tx: &mpsc::Sender<Message>,
    limits: &PluginTerminalSessionLimits,
    record: &mut SessionRecord,
    code: PluginTerminalFailureCode,
) {
    if let Some(pending) = record.pending_command.take() {
        let _ = pending
            .reply
            .send(Err(PluginTerminalSessionError::OutcomeUnknown));
    }
    revoke_lease(record);
    let cleanup_changed = !record.summary.cleanup_blocked;
    record.summary.cleanup_blocked = true;
    if record.summary.state != PluginTerminalSessionState::Failed {
        transition(
            record,
            PluginTerminalSessionState::Failed,
            Some(PluginTerminalCloseReason::DriverFailed),
            Some(PluginTerminalFailureReason { code }),
        );
        emit_state(record);
    } else if cleanup_changed {
        record.summary.updated_at_unix_ms = unix_time_ms();
        bump_event_sequence(record);
        emit_state(record);
    }
    if record.pending_close.is_none() {
        let _ = begin_close(
            tx,
            limits,
            record,
            PluginTerminalCloseReason::DriverFailed,
            true,
            None,
        );
    }
}

fn cleanup_failed(
    record: &mut SessionRecord,
    code: PluginTerminalFailureCode,
    cancel_runtime: bool,
) {
    if let Some(pending) = record.pending_command.take() {
        let _ = pending
            .reply
            .send(Err(PluginTerminalSessionError::OutcomeUnknown));
    }
    revoke_lease(record);
    if cancel_runtime {
        cancel_driver_runtime(record);
    }
    record.summary.cleanup_blocked = true;
    transition(
        record,
        PluginTerminalSessionState::Failed,
        Some(PluginTerminalCloseReason::CoreShutdown),
        Some(PluginTerminalFailureReason { code }),
    );
    emit_state(record);
}

fn finish_driver_cleanup(
    record: &mut SessionRecord,
    close_reason: PluginTerminalCloseReason,
    preserve_failure: bool,
    reply: Option<
        oneshot::Sender<Result<PluginTerminalSessionSummary, PluginTerminalSessionError>>,
    >,
) {
    if let Some(pending) = record.pending_command.take() {
        let _ = pending
            .reply
            .send(Err(PluginTerminalSessionError::OutcomeUnknown));
    }
    revoke_lease(record);
    if let Some(driver) = record.driver.take() {
        driver.event_pump_abort.abort();
        if !driver.runtime_cancelled {
            driver.runtime.cancel();
        }
    }
    let cleanup_changed = record.summary.cleanup_blocked;
    record.summary.cleanup_blocked = false;
    if !preserve_failure && record.summary.state != PluginTerminalSessionState::Closed {
        transition(
            record,
            PluginTerminalSessionState::Closed,
            Some(close_reason),
            None,
        );
        emit_state(record);
    } else if cleanup_changed {
        record.summary.updated_at_unix_ms = unix_time_ms();
        bump_event_sequence(record);
        emit_state(record);
    }
    if let Some(attachment_id) = record.detach_after_close.take() {
        remove_attachment(record, attachment_id);
    }
    if let Some(reply) = reply {
        let _ = reply.send(Ok(record.summary.clone()));
    }
}

fn finish_driver_exit(record: &mut SessionRecord) {
    let pending_close = record.pending_close.take();
    let preserve_failure = pending_close
        .as_ref()
        .is_some_and(|pending| pending.preserve_failure)
        || record.summary.state == PluginTerminalSessionState::Failed;
    let reply = pending_close.and_then(|pending| pending.reply);
    if !preserve_failure && record.summary.state != PluginTerminalSessionState::Closed {
        transition(
            record,
            PluginTerminalSessionState::Closed,
            Some(PluginTerminalCloseReason::DriverExited),
            None,
        );
        emit_state(record);
    }
    finish_driver_cleanup(
        record,
        PluginTerminalCloseReason::DriverExited,
        preserve_failure,
        reply,
    );
}

fn cancel_driver_runtime(record: &mut SessionRecord) {
    let Some(driver) = record.driver.as_mut() else {
        return;
    };
    if !driver.runtime_cancelled {
        driver.runtime.cancel();
        driver.runtime_cancelled = true;
    }
}

fn spawn_driver_event_pump(
    tx: mpsc::Sender<Message>,
    session_id: Uuid,
    generation: u64,
    stream_id: Uuid,
    mut events: mpsc::Receiver<PluginTerminalDriverEvent>,
) -> AbortHandle {
    let task = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if tx
                .send(Message::DriverEvent {
                    session_id,
                    generation,
                    stream_id,
                    event,
                })
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = tx
            .send(Message::DriverEventsEnded {
                session_id,
                generation,
                stream_id,
            })
            .await;
    });
    task.abort_handle()
}

// A pending driver command is addressed by its full session, generation, stream, and command identity.
#[allow(clippy::too_many_arguments)]
fn spawn_driver_ack_wait(
    tx: mpsc::Sender<Message>,
    timeout_duration: Duration,
    session_id: Uuid,
    generation: u64,
    stream_id: Uuid,
    command_id: Uuid,
    kind: DriverAckKind,
    response: oneshot::Receiver<Result<(), PluginTerminalDriverError>>,
) {
    tauri::async_runtime::spawn(async move {
        let outcome = match timeout(timeout_duration, response).await {
            Ok(Ok(result)) => DriverAckOutcome::Completed(result),
            Ok(Err(_)) | Err(_) => DriverAckOutcome::Unknown,
        };
        let _ = tx
            .send(Message::DriverAck {
                session_id,
                generation,
                stream_id,
                command_id,
                kind,
                outcome,
            })
            .await;
    });
}

fn trim_ledger<T>(ledger: &mut BTreeMap<Uuid, T>, order: &mut VecDeque<Uuid>) {
    while order.len() > OPERATION_LEDGER_CAPACITY {
        if let Some(expired) = order.pop_front() {
            ledger.remove(&expired);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use tokio::sync::Mutex as AsyncMutex;

    use super::*;

    #[derive(Default)]
    struct TestRuntime {
        cancelled: AtomicUsize,
    }

    impl PluginTerminalDriverRuntime for TestRuntime {
        fn cancel(&self) {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    struct TestDriver {
        commands: Arc<AsyncMutex<mpsc::Receiver<PluginTerminalDriverCommand>>>,
        events: mpsc::Sender<PluginTerminalDriverEvent>,
        runtime: Arc<TestRuntime>,
    }

    impl TestDriver {
        async fn command(&self) -> PluginTerminalDriverCommand {
            timeout(Duration::from_secs(1), self.commands.lock().await.recv())
                .await
                .expect("driver command deadline")
                .expect("driver command")
        }

        async fn ready(&self) {
            self.events
                .send(PluginTerminalDriverEvent::Ready)
                .await
                .expect("send ready");
        }

        async fn output(&self, bytes: Vec<u8>) {
            self.events
                .send(PluginTerminalDriverEvent::Output(bytes))
                .await
                .expect("send output");
        }
    }

    #[derive(Clone)]
    struct TestFactory {
        drivers: Arc<Mutex<Vec<TestDriver>>>,
        event_capacity: usize,
    }

    impl TestFactory {
        fn new(event_capacity: usize) -> Self {
            Self {
                drivers: Arc::new(Mutex::new(Vec::new())),
                event_capacity,
            }
        }

        fn driver(&self, index: usize) -> TestDriver {
            self.drivers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[index]
                .clone()
        }
    }

    impl PluginTerminalDriverFactory for TestFactory {
        fn start(
            &self,
            _start: PluginTerminalDriverStart,
        ) -> Result<PluginTerminalDriverBinding, PluginTerminalDriverError> {
            let (commands, receiver) = mpsc::channel(16);
            let (events, event_receiver) = mpsc::channel(self.event_capacity);
            let runtime = Arc::new(TestRuntime::default());
            self.drivers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(TestDriver {
                    commands: Arc::new(AsyncMutex::new(receiver)),
                    events,
                    runtime: runtime.clone(),
                });
            Ok(PluginTerminalDriverBinding {
                commands,
                events: event_receiver,
                runtime,
            })
        }
    }

    fn claim() -> PluginTerminalFrontendLaunchClaim {
        PluginTerminalFrontendLaunchClaim::new(Uuid::now_v7()).expect("claim")
    }

    fn connection_handle() -> PluginTerminalConnectionHandle {
        PluginTerminalConnectionHandle::new(Uuid::now_v7()).expect("connection handle")
    }

    fn metadata(label: &str) -> PluginTerminalProviderMetadata {
        PluginTerminalProviderMetadata {
            plugin_id: "plugin.example".to_owned(),
            provider_id: "container-shell".to_owned(),
            display_label: label.to_owned(),
        }
    }

    fn open_request(label: &str) -> PluginTerminalOpenRequest {
        PluginTerminalOpenRequest {
            operation_id: Uuid::now_v7(),
            idempotency_key: Uuid::now_v7().to_string(),
            connection_handle: connection_handle(),
            provider: metadata(label),
            attach_attempt_id: Uuid::now_v7(),
            view_id: format!("view-{label}"),
            rows: 24,
            cols: 80,
        }
    }

    async fn open_running(
        service: &PluginTerminalSessionService,
        factory: &TestFactory,
        label: &str,
    ) -> PluginTerminalOpenResponse {
        let binding = service
            .open_after_frontend_launch(claim(), open_request(label), Arc::new(factory.clone()))
            .await
            .expect("open");
        let index = factory
            .drivers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
            - 1;
        factory.driver(index).ready().await;
        wait_for_state(
            service,
            binding.response.session.session_id,
            PluginTerminalSessionState::Running,
        )
        .await;
        binding.response
    }

    async fn wait_for_state(
        service: &PluginTerminalSessionService,
        session_id: Uuid,
        state: PluginTerminalSessionState,
    ) {
        timeout(Duration::from_secs(1), async {
            loop {
                if service.get(session_id).await.expect("summary").state == state {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("state deadline");
    }

    fn grant_request(
        response: &PluginTerminalOpenResponse,
        focus_epoch: u64,
    ) -> PluginTerminalInputGrantRequest {
        PluginTerminalInputGrantRequest {
            session_id: response.session.session_id,
            expected_generation: response.session.generation,
            expected_stream_id: response.session.stream_id,
            attachment_id: response.attachment.attachment_id,
            view_id: response.attachment.view_id.clone(),
            expected_state_revision: 2,
            focus_epoch,
        }
    }

    #[tokio::test]
    async fn two_sessions_keep_driver_and_output_streams_isolated() {
        let service = PluginTerminalSessionService::start();
        let factory = TestFactory::new(32);
        let a = open_running(&service, &factory, "A").await;
        let b = open_running(&service, &factory, "B").await;
        let mut events_a = service
            .subscribe(a.session.session_id)
            .await
            .expect("subscribe a");
        let mut events_b = service
            .subscribe(b.session.session_id)
            .await
            .expect("subscribe b");
        factory.driver(0).output(vec![0, 0xff, b'A']).await;
        factory.driver(1).output(vec![b'B', 0x1b, b'[']).await;

        let a_frame = loop {
            if let PluginTerminalSessionEvent::Output { frame, .. } =
                events_a.recv().await.expect("event a")
            {
                break frame;
            }
        };
        let b_frame = loop {
            if let PluginTerminalSessionEvent::Output { frame, .. } =
                events_b.recv().await.expect("event b")
            {
                break frame;
            }
        };
        assert_eq!(a_frame.data, vec![0, 0xff, b'A']);
        assert_eq!(b_frame.data, vec![b'B', 0x1b, b'[']);
        assert_eq!(a_frame.stream_id, a.session.stream_id);
        assert_eq!(b_frame.stream_id, b.session.stream_id);
        assert_ne!(a.session.session_id, b.session.session_id);
    }

    #[tokio::test]
    async fn focus_revoke_makes_the_old_a_lease_stale_before_b_can_write() {
        let service = PluginTerminalSessionService::start();
        let factory = TestFactory::new(32);
        let a = open_running(&service, &factory, "A").await;
        let b = open_running(&service, &factory, "B").await;
        let lease_a = service
            .grant_input(grant_request(&a, 7))
            .await
            .expect("lease a");
        service
            .revoke_input(a.session.session_id, 7)
            .await
            .expect("revoke a");
        let lease_b = service
            .grant_input(grant_request(&b, 8))
            .await
            .expect("lease b");
        assert_eq!(
            service
                .validate_fence(PluginTerminalInputFence::from(&lease_a))
                .await,
            Err(PluginTerminalSessionError::InputNotAuthorized)
        );
        assert_eq!(
            service
                .validate_fence(PluginTerminalInputFence::from(&lease_b))
                .await
                .expect("current b")
                .lease_id,
            lease_b.lease_id
        );
    }

    #[tokio::test]
    async fn byte_fidelity_and_ring_overflow_are_replayed_with_a_gap() {
        let limits = PluginTerminalSessionLimits {
            output_ring_max_bytes: 4,
            ..PluginTerminalSessionLimits::default()
        };
        let service = PluginTerminalSessionService::start_with_limits(limits);
        let factory = TestFactory::new(32);
        let opened = open_running(&service, &factory, "A").await;
        let mut events = service
            .subscribe(opened.session.session_id)
            .await
            .expect("subscribe");
        factory.driver(0).output(vec![0, 0xff]).await;
        factory.driver(0).output(vec![1, 2, 3]).await;
        factory.driver(0).output(vec![4, 5, 6]).await;
        let mut seen = Vec::new();
        while seen.len() < 3 {
            if let PluginTerminalSessionEvent::Output { frame, .. } =
                events.recv().await.expect("output")
            {
                seen.push(frame.data);
            }
        }
        assert_eq!(seen, vec![vec![0, 0xff], vec![1, 2, 3], vec![4, 5, 6]]);
        let summary = service
            .get(opened.session.session_id)
            .await
            .expect("summary");
        let attach = service
            .attach(PluginTerminalAttachRequest {
                operation_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7().to_string(),
                session_id: opened.session.session_id,
                expected_generation: summary.generation,
                expected_stream_id: summary.stream_id,
                expected_state_revision: summary.state_revision,
                attach_attempt_id: Uuid::now_v7(),
                view_id: "other-view".to_owned(),
                after_output_sequence: Some(0),
            })
            .await
            .expect("attach");
        assert!(matches!(
            attach.response.replay.first(),
            Some(PluginTerminalReplayItem::Gap {
                dropped_through_sequence: 2,
                ..
            })
        ));
        assert!(matches!(
            attach.response.replay.last(),
            Some(PluginTerminalReplayItem::Frame(PluginTerminalOutputFrame { data, .. })) if data == &vec![4, 5, 6]
        ));
    }

    #[tokio::test]
    async fn pending_input_ack_does_not_stop_output_drain() {
        let service = PluginTerminalSessionService::start();
        let factory = TestFactory::new(8);
        let opened = open_running(&service, &factory, "A").await;
        let lease = service
            .grant_input(grant_request(&opened, 9))
            .await
            .expect("lease");
        let service_for_input = service.clone();
        let input = tokio::spawn(async move {
            service_for_input
                .input(PluginTerminalInputRequest {
                    fence: PluginTerminalInputFence::from(&lease),
                    client_sequence: 1,
                    data: vec![0, 0xff, b'x'],
                })
                .await
        });
        let command = factory.driver(0).command().await;
        let PluginTerminalDriverCommand::Input { ack, bytes } = command else {
            panic!("expected input command");
        };
        assert_eq!(bytes, vec![0, 0xff, b'x']);
        timeout(Duration::from_millis(750), async {
            for index in 0..300u16 {
                factory.driver(0).output(index.to_be_bytes().to_vec()).await;
            }
        })
        .await
        .expect("actor continues draining while ack is pending");
        timeout(Duration::from_secs(1), async {
            loop {
                let summary = service
                    .get(opened.session.session_id)
                    .await
                    .expect("summary");
                if summary.event_sequence >= 300 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all output frames drain before the pending input acknowledgement");
        ack.send(Ok(())).expect("ack input");
        assert_eq!(input.await.expect("input task"), Ok(()));
    }

    #[tokio::test]
    async fn cleanup_failure_is_retained_then_retry_permits_reconnect_and_rejects_old_generation() {
        let service = PluginTerminalSessionService::start();
        let factory = TestFactory::new(32);
        let opened = open_running(&service, &factory, "A").await;
        let old_lease = service
            .grant_input(grant_request(&opened, 3))
            .await
            .expect("lease");
        let old_session_id = opened.session.session_id;
        let old_generation = opened.session.generation;
        let old_stream_id = opened.session.stream_id;
        let service_for_disconnect = service.clone();
        let disconnect = tokio::spawn(async move {
            service_for_disconnect
                .disconnect(PluginTerminalDisconnectRequest {
                    operation_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7().to_string(),
                    session_id: old_session_id,
                    expected_generation: old_generation,
                    expected_stream_id: old_stream_id,
                    expected_state_revision: 2,
                })
                .await
        });
        let PluginTerminalDriverCommand::Close { ack } = factory.driver(0).command().await else {
            panic!("expected close");
        };
        ack.send(Err(PluginTerminalDriverError::Failed))
            .expect("fail cleanup");
        assert_eq!(
            disconnect.await.expect("disconnect task"),
            Err(PluginTerminalSessionError::CleanupIncomplete)
        );
        let failed = service.get(old_session_id).await.expect("failed summary");
        assert_eq!(failed.state, PluginTerminalSessionState::Failed);
        assert!(failed.cleanup_blocked);
        assert_eq!(
            factory.driver(0).runtime.cancelled.load(Ordering::SeqCst),
            0
        );
        assert!(service.exit_blockers().contains(&old_session_id));

        let service_for_retry = service.clone();
        let retry = tokio::spawn(async move {
            service_for_retry
                .retry_cleanup(PluginTerminalCleanupRetryRequest {
                    session_id: failed.session_id,
                    expected_generation: failed.generation,
                    expected_stream_id: failed.stream_id,
                    expected_state_revision: failed.state_revision,
                })
                .await
        });
        let PluginTerminalDriverCommand::Close { ack } = factory.driver(0).command().await else {
            panic!("expected cleanup retry close");
        };
        ack.send(Ok(())).expect("cleanup ack");
        let cleaned = retry.await.expect("retry task").expect("cleanup complete");
        assert!(!cleaned.cleanup_blocked);
        assert_eq!(cleaned.state, PluginTerminalSessionState::Failed);
        assert_eq!(
            factory.driver(0).runtime.cancelled.load(Ordering::SeqCst),
            1
        );

        let reconnected = service
            .reconnect_after_frontend_launch(
                claim(),
                PluginTerminalReconnectRequest {
                    operation_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7().to_string(),
                    session_id: cleaned.session_id,
                    expected_generation: cleaned.generation,
                    expected_stream_id: cleaned.stream_id,
                    expected_state_revision: cleaned.state_revision,
                    connection_handle: connection_handle(),
                    provider: metadata("A"),
                    rows: 24,
                    cols: 80,
                },
                Arc::new(factory.clone()),
            )
            .await
            .expect("reconnect");
        assert_eq!(reconnected.generation, old_generation + 1);
        assert_ne!(reconnected.stream_id, old_stream_id);
        assert_eq!(
            service
                .validate_fence(PluginTerminalInputFence::from(&old_lease))
                .await,
            Err(PluginTerminalSessionError::Conflict)
        );
    }

    #[tokio::test]
    async fn closed_session_without_attachment_is_reclaimed_before_open_quota() {
        let limits = PluginTerminalSessionLimits {
            max_sessions: 1,
            ..PluginTerminalSessionLimits::default()
        };
        let service = PluginTerminalSessionService::start_with_limits(limits);
        let factory = TestFactory::new(32);
        let opened = open_running(&service, &factory, "first").await;
        let running = service
            .get(opened.session.session_id)
            .await
            .expect("running");
        let service_for_disconnect = service.clone();
        let disconnect = tokio::spawn(async move {
            service_for_disconnect
                .disconnect(PluginTerminalDisconnectRequest {
                    operation_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7().to_string(),
                    session_id: running.session_id,
                    expected_generation: running.generation,
                    expected_stream_id: running.stream_id,
                    expected_state_revision: running.state_revision,
                })
                .await
        });
        let PluginTerminalDriverCommand::Close { ack } = factory.driver(0).command().await else {
            panic!("expected close");
        };
        ack.send(Ok(())).expect("close ack");
        let closed = disconnect.await.expect("disconnect task").expect("closed");
        assert_eq!(closed.state, PluginTerminalSessionState::Closed);
        service
            .detach(PluginTerminalDetachRequest {
                operation_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7().to_string(),
                session_id: closed.session_id,
                expected_generation: closed.generation,
                expected_stream_id: closed.stream_id,
                expected_state_revision: closed.state_revision,
                expected_attachment_revision: closed.attachment_revision,
                attachment_id: opened.attachment.attachment_id,
                view_id: opened.attachment.view_id.clone(),
                intent: PluginTerminalDetachIntent::UserClose,
                disconnect_if_last: false,
            })
            .await
            .expect("detach closed attachment");

        let next = service
            .open_after_frontend_launch(claim(), open_request("second"), Arc::new(factory.clone()))
            .await
            .expect("closed history no longer consumes the bounded session quota");
        assert_ne!(next.response.session.session_id, opened.session.session_id);
    }
}
