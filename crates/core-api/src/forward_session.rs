use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ForwardRuleId, ForwardSessionId, HostId, OperationId, RequestMeta, WireSequence};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PortForwardRule {
    Local {
        host_id: HostId,
        local_bind_address: String,
        local_listen_port: u16,
        remote_target_host: String,
        remote_target_port: u16,
    },
    Remote {
        host_id: HostId,
        remote_bind_address: String,
        remote_listen_port: u16,
        local_target_host: String,
        local_target_port: u16,
    },
    Dynamic {
        host_id: HostId,
        local_bind_address: String,
        local_listen_port: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRuleSummary {
    pub rule_id: ForwardRuleId,
    pub label: String,
    pub host_id: HostId,
    pub rule: PortForwardRule,
    pub state_version: WireSequence,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRuleListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRuleListResponse {
    pub rules: Vec<ForwardRuleSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRuleCreateRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub rule_id: ForwardRuleId,
    pub label: String,
    pub rule: PortForwardRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRuleUpdateRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub rule_id: ForwardRuleId,
    pub expected_state_version: WireSequence,
    pub label: String,
    pub rule: PortForwardRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRuleDeleteRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub rule_id: ForwardRuleId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRulePreflightRequest {
    pub meta: RequestMeta,
    pub rule: PortForwardRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRulePreflightResponse {
    pub local_bind_available_at_check: Option<bool>,
    pub checked_address: Option<String>,
    pub checked_port: Option<u16>,
    pub advisory_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ForwardSessionState {
    Starting,
    Running,
    Failed,
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ForwardFailureCode {
    InvalidRule,
    HostUnavailable,
    HostKeyReviewRequired,
    HostKeyRejected,
    HostKeyMismatch,
    VaultLocked,
    CredentialUnavailable,
    AuthenticationRejected,
    TransportConnect,
    Bind,
    RemoteRegistrationRejected,
    RemoteRegistrationUncertain,
    TransportLost,
    Protocol,
    ResourceLimit,
    SocksTruncated,
    SocksAuthenticationRejected,
    SocksCommandRejected,
    SocksAddressRejected,
    ConnectTimeout,
    TargetConnect,
    Relay,
    CleanupUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardFailure {
    pub code: ForwardFailureCode,
    pub stage: String,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ActualForwardBind {
    Local { address: String, port: u16 },
    Remote { address: String, port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CleanupStepState {
    NotRequired,
    Complete,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardCleanupFacts {
    pub listener_closed_or_remote_cancelled: CleanupStepState,
    pub children_cleared: CleanupStepState,
    pub transport_disconnected: CleanupStepState,
    pub abandoned_child_count: u32,
    pub uncertain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSessionSummary {
    pub session_id: ForwardSessionId,
    pub host_id: HostId,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub state: ForwardSessionState,
    pub rule_id: Option<ForwardRuleId>,
    pub rule_revision: Option<WireSequence>,
    pub rule_snapshot: PortForwardRule,
    pub started_at_unix_ms: i64,
    pub actual_bind: Option<ActualForwardBind>,
    pub child_count: u32,
    pub listener_to_target_bytes: WireSequence,
    pub target_to_listener_bytes: WireSequence,
    pub failure: Option<ForwardFailure>,
    pub last_child_failure: Option<ForwardFailure>,
    pub cleanup: ForwardCleanupFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSessionStartRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: ForwardSessionId,
    pub rule_id: Option<ForwardRuleId>,
    pub rule_revision: Option<WireSequence>,
    pub rule: PortForwardRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSessionSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSessionSnapshot {
    pub snapshot_revision: WireSequence,
    pub sessions: Vec<ForwardSessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSessionStopRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: ForwardSessionId,
    pub expected_generation: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardCleanupRetainRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: ForwardSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub retain_uncertain_cleanup_for_exit_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSessionEvent {
    pub schema_version: u16,
    pub event_seq: WireSequence,
    pub session: ForwardSessionSummary,
}

#[cfg(test)]
mod tests {
    use super::{ForwardSessionStartRequest, PortForwardRule};
    use crate::{ForwardSessionId, HostId, OperationId, RequestId, RequestMeta};

    #[test]
    fn rule_is_a_tagged_union_and_keeps_listener_side_explicit() {
        let request = ForwardSessionStartRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "forward-start-1".to_owned(),
            session_id: ForwardSessionId::new(),
            rule_id: None,
            rule_revision: None,
            rule: PortForwardRule::Dynamic {
                host_id: HostId::new(),
                local_bind_address: "127.0.0.1".to_owned(),
                local_listen_port: 1080,
            },
        };

        let encoded = serde_json::to_string(&request).expect("serialize forward request");
        assert!(encoded.contains("\"kind\":\"dynamic\""));
        assert!(encoded.contains("\"localBindAddress\":\"127.0.0.1\""));
        assert!(!encoded.contains("remoteTargetHost"));
    }
}
