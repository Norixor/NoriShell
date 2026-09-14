use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    OperationId, RequestMeta, TelnetAttachAttemptId, TelnetAttachmentId, TelnetInputLeaseId,
    TelnetOpenAttemptId, TelnetSessionId, TelnetSocketId, TelnetViewId, WireSequence,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TelnetSessionState {
    Connecting,
    Running,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TelnetSessionCloseReason {
    UserRequested,
    RemoteClosed,
    ConnectionFailed,
    CoreShutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TelnetSessionFailureCode {
    ResolveFailed,
    ConnectTimeout,
    ConnectFailed,
    ProtocolViolation,
    IoTimeout,
    IoFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionFailureReason {
    pub code: TelnetSessionFailureCode,
    pub message_key: String,
    pub diagnostic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetEndpoint {
    pub address: String,
    pub port: u16,
}

/// Per-operation acknowledgement for Telnet's three independent risks. The
/// Core validates the endpoint against the open/reconnect request before DNS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetRiskConfirmation {
    pub endpoint: TelnetEndpoint,
    pub accepts_cleartext_transport: bool,
    pub accepts_missing_server_identity: bool,
    pub accepts_observation_and_tampering: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionSummary {
    pub session_id: TelnetSessionId,
    pub open_attempt_id: TelnetOpenAttemptId,
    pub endpoint: TelnetEndpoint,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    pub event_seq: WireSequence,
    pub socket_id: Option<TelnetSocketId>,
    pub state: TelnetSessionState,
    pub attachment_count: u32,
    pub close_reason: Option<TelnetSessionCloseReason>,
    pub failure_reason: Option<TelnetSessionFailureReason>,
    #[ts(type = "number")]
    pub created_at_unix_ms: i64,
    #[ts(type = "number")]
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionAttachment {
    pub attachment_id: TelnetAttachmentId,
    pub attach_attempt_id: TelnetAttachAttemptId,
    pub session_id: TelnetSessionId,
    pub generation: WireSequence,
    pub socket_id: Option<TelnetSocketId>,
    pub view_id: TelnetViewId,
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    #[ts(type = "number")]
    pub attached_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionOpenRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub open_attempt_id: TelnetOpenAttemptId,
    pub attach_attempt_id: TelnetAttachAttemptId,
    pub view_id: TelnetViewId,
    pub endpoint: TelnetEndpoint,
    pub risk_confirmation: TelnetRiskConfirmation,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionOpenResponse {
    pub session: TelnetSessionSummary,
    pub attachment: TelnetSessionAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionSnapshot {
    pub snapshot_revision: WireSequence,
    pub sessions: Vec<TelnetSessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionAttachRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub attach_attempt_id: TelnetAttachAttemptId,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub view_id: TelnetViewId,
    pub after_output_seq: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionAttachResponse {
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    pub attachment: TelnetSessionAttachment,
    pub replay: Vec<TelnetSessionOutputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionAttachmentHeartbeatRequest {
    pub meta: RequestMeta,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_attachment_revision: WireSequence,
    pub attachment_id: TelnetAttachmentId,
    pub view_id: TelnetViewId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
pub enum TelnetSessionDetachIntent {
    #[default]
    UserClose,
    RendererUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionDetachRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub expected_attachment_revision: WireSequence,
    pub attachment_id: TelnetAttachmentId,
    pub view_id: TelnetViewId,
    pub intent: TelnetSessionDetachIntent,
    pub disconnect_if_last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionDetachResponse {
    pub session: TelnetSessionSummary,
    pub remaining_attachment_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionInputLeaseRenewRequest {
    pub meta: RequestMeta,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub socket_id: TelnetSocketId,
    pub attachment_id: TelnetAttachmentId,
    pub view_id: TelnetViewId,
    pub lease_id: TelnetInputLeaseId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionInputRequest {
    pub meta: RequestMeta,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub socket_id: TelnetSocketId,
    pub attachment_id: TelnetAttachmentId,
    pub view_id: TelnetViewId,
    pub lease_id: TelnetInputLeaseId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    pub client_seq: WireSequence,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionResizeRequest {
    pub meta: RequestMeta,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub socket_id: TelnetSocketId,
    pub attachment_id: TelnetAttachmentId,
    pub view_id: TelnetViewId,
    pub lease_id: TelnetInputLeaseId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    pub resize_seq: WireSequence,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionDisconnectRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionReconnectRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub risk_confirmation: TelnetRiskConfirmation,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TelnetSessionOutputGapReason {
    RingBufferOverflow,
    AttachmentBufferOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionOutputFrame {
    pub session_id: TelnetSessionId,
    pub generation: WireSequence,
    pub socket_id: TelnetSocketId,
    pub output_seq: WireSequence,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionOutputGap {
    pub session_id: TelnetSessionId,
    pub generation: WireSequence,
    pub socket_id: TelnetSocketId,
    pub dropped_from_output_seq: WireSequence,
    pub resumes_at_output_seq: WireSequence,
    pub reason: TelnetSessionOutputGapReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
pub enum TelnetSessionOutputItem {
    Frame(TelnetSessionOutputFrame),
    Gap(TelnetSessionOutputGap),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TelnetSessionEventPayload {
    StateChanged { session: TelnetSessionSummary },
    AttachmentAttached { attachment: TelnetSessionAttachment },
    AttachmentDetached { attachment_id: TelnetAttachmentId },
    InputLeaseRevoked { focus_epoch: WireSequence },
    Output { frame: TelnetSessionOutputFrame },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetSessionEvent {
    pub schema_version: u16,
    pub session_id: TelnetSessionId,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub event_seq: WireSequence,
    #[ts(type = "number")]
    pub occurred_at_unix_ms: i64,
    pub payload: TelnetSessionEventPayload,
}

pub const TELNET_TERMINAL_EVENT_SCHEMA_VERSION: u16 = 1;

#[cfg(test)]
mod tests {
    use super::{TelnetEndpoint, TelnetRiskConfirmation};

    #[test]
    fn risk_confirmation_serializes_all_three_telnet_risks() {
        let confirmation = TelnetRiskConfirmation {
            endpoint: TelnetEndpoint {
                address: "example.test".to_owned(),
                port: 23,
            },
            accepts_cleartext_transport: true,
            accepts_missing_server_identity: true,
            accepts_observation_and_tampering: true,
        };
        let encoded = serde_json::to_value(confirmation).expect("serialize confirmation");
        assert_eq!(encoded["endpoint"]["address"], "example.test");
        assert_eq!(encoded["acceptsCleartextTransport"], true);
        assert_eq!(encoded["acceptsMissingServerIdentity"], true);
        assert_eq!(encoded["acceptsObservationAndTampering"], true);
    }
}
