//! User-selected persistence for an exact plugin operation, never a blanket execution grant.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PluginId, RequestMeta, WireSequence};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginApprovalPolicy {
    #[default]
    Once,
    Always,
}

/// The bounded lifetime chosen for an exact remembered operation. The secure
/// renderer submits this intent; Core calculates and persists the absolute
/// expiry point so a restart can never extend it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginApprovalExpiry {
    FifteenMinutes,
    OneHour,
    TwentyFourHours,
    #[default]
    Unlimited,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginRememberPolicy {
    ExactOperation,
    #[default]
    Unavailable,
    UnstableTarget,
    StorageUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginApprovalOperation {
    RemoteExecute,
    ForwardStart,
    ForwardStop,
    NetworkRequest,
    FileAccess,
    SftpRead,
    SftpWrite,
    SerialAccess,
    LocalExecute,
    TerminalInput,
    HostMutation,
    HostSession,
}

/// Only non-secret labels are listed. Exact payloads are not saved in this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPermission {
    pub permission_id: String,
    pub plugin_id: PluginId,
    pub operation: PluginApprovalOperation,
    pub action_label: String,
    pub target_label: String,
    pub created_at_unix_ms: i64,
    /// `None` is an explicit unlimited duration. Finite points are calculated
    /// by Core when the user approves, never by a renderer or plugin.
    pub expires_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPermissionListRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPermissionList {
    pub permissions: Vec<PluginOperationPermission>,
    pub policy_revision: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPermissionRevokeRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    pub permission_id: String,
    pub expected_policy_revision: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPermissionsClearRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    pub expected_policy_revision: Option<WireSequence>,
}
