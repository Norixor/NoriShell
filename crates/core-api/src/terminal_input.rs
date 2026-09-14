use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    LocalAttachmentId, LocalInputLeaseId, LocalPtyId, LocalSessionId, LocalViewId, OperationId,
    RequestMeta, SshSessionInputLease, SshTerminalInputFocusTarget, TelnetAttachmentId,
    TelnetInputLeaseId, TelnetSessionId, TelnetSocketId, TelnetViewId, WireSequence,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalTerminalInputFocusTarget {
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub pty_id: LocalPtyId,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetTerminalInputFocusTarget {
    pub session_id: TelnetSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub socket_id: TelnetSocketId,
    pub attachment_id: TelnetAttachmentId,
    pub view_id: TelnetViewId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalInputFocusTarget {
    pub session_id: String,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub stream_id: String,
    pub attachment_id: String,
    pub view_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalInputLease {
    pub lease_id: String,
    pub session_id: String,
    pub generation: WireSequence,
    pub stream_id: String,
    pub attachment_id: String,
    pub view_id: String,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

/// One exact writable Pane target for the process-wide terminal focus broker.
/// Both variants are serialized through the same actor mailbox; callers must
/// never infer a Local target from SSH identifiers or vice versa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "target", rename_all = "camelCase")]
pub enum TerminalInputFocusTarget {
    Ssh(SshTerminalInputFocusTarget),
    Local(LocalTerminalInputFocusTarget),
    Telnet(TelnetTerminalInputFocusTarget),
    Plugin(PluginTerminalInputFocusTarget),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionInputLease {
    pub lease_id: LocalInputLeaseId,
    pub session_id: LocalSessionId,
    pub generation: WireSequence,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TelnetInputLease {
    pub lease_id: TelnetInputLeaseId,
    pub session_id: TelnetSessionId,
    pub generation: WireSequence,
    pub socket_id: TelnetSocketId,
    pub attachment_id: TelnetAttachmentId,
    pub view_id: TelnetViewId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "lease", rename_all = "camelCase")]
pub enum TerminalInputLease {
    Ssh(SshSessionInputLease),
    Local(LocalSessionInputLease),
    Telnet(TelnetInputLease),
    Plugin(PluginTerminalInputLease),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputFocusSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputFocusSnapshot {
    pub focus_epoch: WireSequence,
    pub target: Option<TerminalInputFocusTarget>,
    pub lease: Option<TerminalInputLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputFocusChangeRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub expected_focus_epoch: WireSequence,
    pub target: Option<TerminalInputFocusTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputFocusChangeResponse {
    pub focus_epoch: WireSequence,
    pub target: Option<TerminalInputFocusTarget>,
    pub lease: Option<TerminalInputLease>,
}

#[cfg(test)]
mod tests {
    use super::{LocalTerminalInputFocusTarget, TerminalInputFocusTarget};
    use crate::{LocalAttachmentId, LocalPtyId, LocalSessionId, LocalViewId, WireSequence};

    #[test]
    fn unified_focus_target_keeps_local_identity_and_fences_explicit() {
        let target = TerminalInputFocusTarget::Local(LocalTerminalInputFocusTarget {
            session_id: LocalSessionId::new(),
            expected_generation: WireSequence::new(7),
            expected_state_revision: WireSequence::new(11),
            pty_id: LocalPtyId::new(),
            attachment_id: LocalAttachmentId::new(),
            view_id: LocalViewId::new(),
        });

        let encoded = serde_json::to_value(&target).expect("serialize local focus target");
        assert_eq!(encoded["kind"], "local");
        assert_eq!(encoded["target"]["expectedGeneration"], "7");
        assert_eq!(encoded["target"]["expectedStateRevision"], "11");
        let decoded: TerminalInputFocusTarget =
            serde_json::from_value(encoded).expect("deserialize local focus target");
        assert_eq!(decoded, target);
    }
}
