use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    LocalAttachAttemptId, LocalAttachmentId, LocalInputLeaseId, LocalOpenAttemptId, LocalPtyId,
    LocalSessionId, LocalViewId, OperationId, RequestMeta, WireSequence,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LocalSessionState {
    Starting,
    Running,
    Stopping,
    Exited,
    Failed,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LocalSessionExit {
    ExitStatus {
        #[ts(type = "number")]
        exit_status: i64,
    },
    ExitSignal {
        signal_name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LocalSessionFailureCode {
    AccountLookupFailed,
    InvalidDefaultShell,
    InvalidHomeDirectory,
    PtySpawnFailed,
    PtyReadFailed,
    PtyWriteFailed,
    PtyResizeFailed,
    ProcessCleanupFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionFailureReason {
    pub code: LocalSessionFailureCode,
    pub message_key: String,
    pub diagnostic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionSummary {
    pub session_id: LocalSessionId,
    pub open_attempt_id: LocalOpenAttemptId,
    pub shell_name: String,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    pub event_seq: WireSequence,
    pub pty_id: Option<LocalPtyId>,
    pub state: LocalSessionState,
    pub exit: Option<LocalSessionExit>,
    pub failure_reason: Option<LocalSessionFailureReason>,
    pub attachment_count: u32,
    #[ts(type = "number")]
    pub created_at_unix_ms: i64,
    #[ts(type = "number")]
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionAttachment {
    pub attachment_id: LocalAttachmentId,
    pub attach_attempt_id: LocalAttachAttemptId,
    pub session_id: LocalSessionId,
    pub generation: WireSequence,
    pub pty_id: Option<LocalPtyId>,
    pub view_id: LocalViewId,
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    #[ts(type = "number")]
    pub attached_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionOpenRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub open_attempt_id: LocalOpenAttemptId,
    pub attach_attempt_id: LocalAttachAttemptId,
    pub view_id: LocalViewId,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionOpenResponse {
    pub session: LocalSessionSummary,
    pub attachment: LocalSessionAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionSnapshot {
    pub snapshot_revision: WireSequence,
    pub sessions: Vec<LocalSessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionGetRequest {
    pub meta: RequestMeta,
    pub session_id: LocalSessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionDetails {
    pub session: LocalSessionSummary,
    pub attachments: Vec<LocalSessionAttachment>,
    pub input_lease: Option<crate::LocalSessionInputLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionAttachRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub attach_attempt_id: LocalAttachAttemptId,
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub view_id: LocalViewId,
    pub after_output_seq: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionAttachResponse {
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    pub attachment: LocalSessionAttachment,
    pub replay: Vec<LocalSessionOutputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionAttachmentHeartbeatRequest {
    pub meta: RequestMeta,
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_attachment_revision: WireSequence,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
pub enum LocalSessionDetachIntent {
    #[default]
    UserClose,
    RendererUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LocalSessionLastDetachAction {
    TerminateAndDetach,
    KeepAttached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionLastDetachConfirmation {
    pub action: LocalSessionLastDetachAction,
    pub expected_state_revision: WireSequence,
    pub expected_attachment_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionDetachRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
    pub intent: LocalSessionDetachIntent,
    pub confirmation: Option<LocalSessionLastDetachConfirmation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LocalSessionDetachResult {
    Detached {
        session: LocalSessionSummary,
        remaining_attachment_count: u32,
    },
    ConfirmationRequired {
        session: LocalSessionSummary,
        expected_state_revision: WireSequence,
        expected_attachment_revision: WireSequence,
    },
    KeptAttached {
        session: LocalSessionSummary,
    },
    Stopping {
        session: LocalSessionSummary,
    },
    TerminatedAndDetached {
        session: LocalSessionSummary,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionInputLeaseRenewRequest {
    pub meta: RequestMeta,
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub pty_id: LocalPtyId,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
    pub lease_id: LocalInputLeaseId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionInputRequest {
    pub meta: RequestMeta,
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub pty_id: LocalPtyId,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
    pub lease_id: LocalInputLeaseId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    pub client_seq: WireSequence,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionResizeRequest {
    pub meta: RequestMeta,
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub pty_id: LocalPtyId,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
    pub resize_seq: WireSequence,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionTerminateRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LocalSessionOutputGapReason {
    RingBufferOverflow,
    AttachmentBufferOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionOutputFrame {
    pub session_id: LocalSessionId,
    pub generation: WireSequence,
    pub pty_id: LocalPtyId,
    pub output_seq: WireSequence,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionOutputGap {
    pub session_id: LocalSessionId,
    pub generation: WireSequence,
    pub pty_id: LocalPtyId,
    pub dropped_from_output_seq: WireSequence,
    pub resumes_at_output_seq: WireSequence,
    pub reason: LocalSessionOutputGapReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
pub enum LocalSessionOutputItem {
    Frame(LocalSessionOutputFrame),
    Gap(LocalSessionOutputGap),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LocalSessionAttachmentChange {
    Attached,
    Detached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LocalSessionInputLeaseChange {
    Acquired,
    Renewed,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LocalSessionEventPayload {
    StateChanged {
        previous_state: LocalSessionState,
        state: LocalSessionState,
        exit: Option<LocalSessionExit>,
        failure_reason: Option<LocalSessionFailureReason>,
    },
    AttachmentChanged {
        change: LocalSessionAttachmentChange,
        attachment_revision: WireSequence,
        attachment: LocalSessionAttachment,
    },
    InputLeaseChanged {
        change: LocalSessionInputLeaseChange,
        lease: Option<crate::LocalSessionInputLease>,
    },
    OutputFrame {
        frame: LocalSessionOutputFrame,
    },
    OutputGap {
        gap: LocalSessionOutputGap,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionEvent {
    pub schema_version: u16,
    pub session_id: LocalSessionId,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub event_seq: WireSequence,
    #[ts(type = "number")]
    pub occurred_at_unix_ms: i64,
    pub payload: LocalSessionEventPayload,
}

pub const LOCAL_TERMINAL_EVENT_SCHEMA_VERSION: u16 = 1;
pub const LOCAL_TERMINAL_OUTPUT_FRAME_MAX_BYTES: usize = 64 * 1024;
