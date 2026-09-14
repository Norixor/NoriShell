//! Main-window IPC for plugin-owned terminal sessions. Plugins never receive these leases.
use crate::{PluginId, RequestMeta, WireSequence};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalProfile {
    pub plugin_id: PluginId,

    pub provider_id: String,

    pub schema_hash: String,

    #[ts(type = "Record<string, boolean | number | string>")]
    pub configuration: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalSummary {
    pub plugin_id: PluginId,

    pub provider_id: String,

    pub schema_hash: String,

    #[ts(type = "Record<string, boolean | number | string>")]
    pub configuration: BTreeMap<String, serde_json::Value>,

    pub session_id: String,

    pub tab_id: String,

    pub pane_id: String,

    pub label: String,

    pub generation: WireSequence,

    pub state_revision: WireSequence,

    pub attachment_revision: WireSequence,

    pub event_seq: WireSequence,

    pub stream_id: String,

    pub state: PluginTerminalSessionState,

    pub attachment_count: u32,

    pub cleanup_blocked: bool,

    pub failure_reason: Option<PluginTerminalFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginTerminalSessionState {
    Connecting,
    Running,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalFailure {
    pub code: String,

    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalSnapshot {
    pub snapshot_revision: WireSequence,

    pub sessions: Vec<PluginTerminalSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalOpenRequest {
    pub meta: RequestMeta,

    pub operation_id: String,

    pub idempotency_key: String,

    pub plugin_id: PluginId,

    pub provider_id: String,

    pub schema_hash: String,

    #[ts(type = "Record<string, boolean | number | string>")]
    pub configuration: BTreeMap<String, serde_json::Value>,

    pub tab_id: String,

    pub pane_id: String,

    pub label: String,

    pub rows: u16,

    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalOpenResponse {
    pub session: PluginTerminalSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalAttachment {
    pub attachment_id: String,

    pub session_id: String,

    pub generation: WireSequence,

    pub stream_id: String,

    pub view_id: String,

    pub state_revision: WireSequence,

    pub attachment_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalAttachRequest {
    pub meta: RequestMeta,

    pub operation_id: String,

    pub idempotency_key: String,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_state_revision: WireSequence,

    pub stream_id: String,

    pub attach_attempt_id: String,

    pub view_id: String,

    pub after_output_seq: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalAttachResponse {
    pub session: PluginTerminalSummary,

    pub attachment: PluginTerminalAttachment,

    pub replay: Vec<PluginTerminalOutputItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalAttachmentHeartbeatRequest {
    pub meta: RequestMeta,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_attachment_revision: WireSequence,

    pub attachment_id: String,

    pub view_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginTerminalDetachIntent {
    UserClose,
    RendererUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalDetachRequest {
    pub meta: RequestMeta,

    pub operation_id: String,

    pub idempotency_key: String,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_state_revision: WireSequence,

    pub expected_attachment_revision: WireSequence,

    pub attachment_id: String,

    pub view_id: String,

    pub intent: PluginTerminalDetachIntent,

    pub disconnect_if_last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalDetachResponse {
    pub session: PluginTerminalSummary,

    pub remaining_attachment_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalLeaseRenewRequest {
    pub meta: RequestMeta,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_state_revision: WireSequence,

    pub stream_id: String,

    pub attachment_id: String,

    pub view_id: String,

    pub lease_id: String,

    pub focus_epoch: WireSequence,

    pub input_epoch: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalInputRequest {
    pub meta: RequestMeta,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_state_revision: WireSequence,

    pub stream_id: String,

    pub attachment_id: String,

    pub view_id: String,

    pub lease_id: String,

    pub focus_epoch: WireSequence,

    pub input_epoch: WireSequence,

    pub client_seq: WireSequence,

    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalResizeRequest {
    pub meta: RequestMeta,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_state_revision: WireSequence,

    pub stream_id: String,

    pub attachment_id: String,

    pub view_id: String,

    pub lease_id: String,

    pub focus_epoch: WireSequence,

    pub input_epoch: WireSequence,

    pub resize_seq: WireSequence,

    pub rows: u16,

    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalDisconnectRequest {
    pub meta: RequestMeta,

    pub operation_id: String,

    pub idempotency_key: String,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_state_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalReconnectRequest {
    pub meta: RequestMeta,

    pub operation_id: String,

    pub idempotency_key: String,

    pub session_id: String,

    pub expected_generation: WireSequence,

    pub expected_state_revision: WireSequence,

    pub rows: u16,

    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalFrame {
    pub session_id: String,

    pub generation: WireSequence,

    pub stream_id: String,

    pub output_seq: WireSequence,

    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalGap {
    pub session_id: String,

    pub generation: WireSequence,

    pub stream_id: String,

    pub dropped_from_output_seq: WireSequence,

    pub resumes_at_output_seq: WireSequence,

    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum PluginTerminalOutputItem {
    Frame(PluginTerminalFrame),
    Gap(PluginTerminalGap),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginTerminalEventPayload {
    StateChanged {
        session: PluginTerminalSummary,
    },
    AttachmentAttached {
        attachment: PluginTerminalAttachment,
    },
    AttachmentDetached {
        attachment_id: String,
    },
    InputLeaseRevoked {
        focus_epoch: WireSequence,
    },
    Output {
        frame: PluginTerminalFrame,
    },
    OutputGap {
        gap: PluginTerminalGap,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalEvent {
    pub session: PluginTerminalSummary,

    pub session_id: String,

    pub generation: WireSequence,

    pub state_revision: WireSequence,

    pub event_seq: WireSequence,

    pub payload: PluginTerminalEventPayload,
}
