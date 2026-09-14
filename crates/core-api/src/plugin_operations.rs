//! Versioned operations on the current user-owned SSH session.
//! Protected one-time approval never authorizes creating a connection.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    PluginApprovalDecision, PluginApprovalId, PluginId, PluginLocale, RequestMeta, WireSequence,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoteOperationRequest {
    pub terminal_handle: String,
    /// A JSON-encoded, versioned RemoteOperation from the public operation catalog.
    pub operation_json: String,
    pub reason: String,
}

impl std::fmt::Debug for PluginRemoteOperationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginRemoteOperationRequest")
            .field("terminal_handle", &self.terminal_handle)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginRemoteOperationState {
    Succeeded,
    Failed,
    Rejected,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginRemoteOperationData {
    CrontabSnapshot {
        sha256: String,
        content: String,
    },
    ProcessSnapshot {
        pid: u32,
        start_time_ticks: String,
        details: String,
    },
    CpuUsage {
        basis_points: u16,
        sample_duration_ms: u32,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoteOperationResult {
    pub operation_id: PluginApprovalId,
    pub state: PluginRemoteOperationState,
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<u32>,
    pub duration_ms: u64,
    pub truncated: bool,
    pub stable_error: Option<String>,
    pub data: Option<PluginRemoteOperationData>,
}

impl std::fmt::Debug for PluginRemoteOperationResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginRemoteOperationResult")
            .field("operation_id", &self.operation_id)
            .field("state", &self.state)
            .field("exit_status", &self.exit_status)
            .field("stable_error", &self.stable_error)
            .finish_non_exhaustive()
    }
}

/// This content is only serialized to an independent protected surface.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginRemoteApprovalContent {
    ForwardStart {
        rule: crate::PluginForwardRule,
        reason: String,
    },
    ForwardStop {
        rule: crate::PluginForwardRule,
        actual_bind: Option<crate::PluginActualForwardBind>,
        forward_handle: String,
        reason: String,
    },
    Execute {
        command: String,
        stdin: Option<String>,
        reason: String,
    },
    VaultAccess {
        create: bool,
    },
    Credential {
        label: String,
        target: crate::PluginCredentialTarget,
    },
    Access {
        operation: crate::PluginApprovalOperation,
        details: String,
        reason: String,
    },
}

impl std::fmt::Debug for PluginRemoteApprovalContent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PluginRemoteApprovalContent([PROTECTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoteApprovalPrompt {
    pub approval_id: PluginApprovalId,
    pub plugin_id: PluginId,
    pub plugin_name: String,
    pub locale: PluginLocale,
    pub host_label: String,
    pub endpoint: String,
    pub content: PluginRemoteApprovalContent,
    pub state_version: WireSequence,
    pub expires_at_unix_ms: i64,
    #[serde(default)]
    pub remember_policy: crate::PluginRememberPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoteApprovalGetRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginApprovalId,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoteApprovalDecisionRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginApprovalId,
    pub expected_state_version: WireSequence,
    pub decision: PluginApprovalDecision,
    #[serde(default)]
    pub policy: crate::PluginApprovalPolicy,
    #[serde(default)]
    pub expiry: crate::PluginApprovalExpiry,
}

impl std::fmt::Debug for PluginRemoteApprovalDecisionRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginRemoteApprovalDecisionRequest")
            .field("approval_id", &self.approval_id)
            .field("decision", &self.decision)
            .finish_non_exhaustive()
    }
}

/// Submitted only by a protected credential input window; no plugin guest entry point exists.
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCredentialInputDecisionRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginApprovalId,
    pub expected_state_version: WireSequence,
    pub secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirmation: Option<String>,
}

impl std::fmt::Debug for PluginCredentialInputDecisionRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PluginCredentialInputDecisionRequest([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApprovedTerminalChannelLaunch {
    pub operation_id: crate::OperationId,
    pub authorization_token: PluginApprovalId,
    pub target: crate::SshSessionTarget,
    pub label: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_operation_rejects_host_selection_and_plugin_claimed_safety() {
        let value = serde_json::json!({"terminalHandle":"a".repeat(64),"operationJson":"{}","reason":"read","readOnly":true});
        assert!(serde_json::from_value::<PluginRemoteOperationRequest>(value).is_err());
        let value =
            serde_json::json!({"hostHandle":"a".repeat(64),"operationJson":"{}","reason":"read"});
        assert!(serde_json::from_value::<PluginRemoteOperationRequest>(value).is_err());
    }

    #[test]
    fn cpu_usage_data_is_a_bounded_typed_projection() {
        assert_eq!(
            serde_json::to_value(PluginRemoteOperationData::CpuUsage {
                basis_points: 10_000,
                sample_duration_ms: 200,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "cpuUsage",
                "basisPoints": 10_000,
                "sampleDurationMs": 200,
            })
        );
    }

    #[test]
    fn access_approval_keeps_network_details_in_the_protected_content() {
        let content = PluginRemoteApprovalContent::Access {
            operation: crate::PluginApprovalOperation::NetworkRequest,
            details: "UDP 198.51.100.24:443".to_owned(),
            reason: "Query resolver".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(&content).unwrap(),
            serde_json::json!({
                "kind": "access",
                "operation": "networkRequest",
                "details": "UDP 198.51.100.24:443",
                "reason": "Query resolver",
            })
        );
        assert!(!format!("{content:?}").contains("198.51.100.24"));
    }
}
