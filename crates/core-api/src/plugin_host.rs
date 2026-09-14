//! Secret-free Host capability projections for plugins.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ts_rs::TS;

use crate::{HostId, PluginApprovalId, PluginCapability, PluginId, RequestMeta, WireSequence};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
#[ts(type = "string")]
pub struct PluginHostHandle(String);

impl PluginHostHandle {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 160
            || !value.is_ascii()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err("plugin Host handle must be bounded ASCII");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginHostHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for PluginHostHandle {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for PluginHostHandle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PluginHostHandle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostScopeSelection {
    pub host_id: HostId,
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostScopeSummary {
    pub host_id: HostId,
    pub host_label: String,
    pub endpoint: String,
    pub capabilities: Vec<PluginCapability>,
    pub scope_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostScopeSnapshot {
    pub scope_state_version: Option<WireSequence>,
    pub hosts: Vec<PluginHostScopeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostScopeListRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostScopeReplaceRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    pub expected_plugin_state_version: WireSequence,
    pub expected_scope_state_version: Option<WireSequence>,
    pub selections: Vec<PluginHostScopeSelection>,
}

/// Intentionally excludes HostId, IdentityId, CredentialRef, RoutePlan,
/// AuthenticationPlan and all secret-bearing or security-decision fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostMetadataProjection {
    pub host_handle: PluginHostHandle,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub favorite: bool,
    pub host_state_version: WireSequence,
    pub scope_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostMutationPatch {
    pub label: Option<String>,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub clear_username: bool,
    pub favorite: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginHostSessionKind {
    Terminal,
    Sftp,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginDockerContainerShell {
    Sh,
    Bash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostMutationProposalRequest {
    pub host_handle: PluginHostHandle,
    pub expected_host_state_version: WireSequence,
    pub patch: PluginHostMutationPatch,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostSessionRequest {
    pub host_handle: PluginHostHandle,
    pub kind: PluginHostSessionKind,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginHostApprovalKind {
    Mutation,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginApprovalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostApprovalSummary {
    pub approval_id: PluginApprovalId,
    pub plugin_id: PluginId,
    pub plugin_name: String,
    pub kind: PluginHostApprovalKind,
    pub host_label: String,
    pub endpoint: String,
    pub reason: String,
    pub mutation_patch: Option<PluginHostMutationPatch>,
    pub session_kind: Option<PluginHostSessionKind>,
    pub expires_at_unix_ms: i64,
    pub state_version: WireSequence,
    #[serde(default)]
    pub remember_policy: crate::PluginRememberPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostApprovalGetRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginApprovalId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostApprovalDecisionRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginApprovalId,
    pub decision: PluginApprovalDecision,
    pub expected_state_version: WireSequence,
    #[serde(default)]
    pub policy: crate::PluginApprovalPolicy,
    #[serde(default)]
    pub expiry: crate::PluginApprovalExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApprovedHostSessionLaunch {
    pub operation_id: PluginApprovalId,
    pub authorization_token: PluginApprovalId,
    pub host_id: HostId,
    pub kind: PluginHostSessionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostApprovalDecisionResponse {
    pub approval_id: PluginApprovalId,
    pub decision: PluginApprovalDecision,
    pub host_updated: bool,
    pub session_launch: Option<PluginApprovedHostSessionLaunch>,
}

#[cfg(test)]
mod tests {
    use super::PluginHostMetadataProjection;

    #[test]
    fn metadata_projection_has_no_secret_or_credential_fields() {
        let fields = serde_json::to_value(PluginHostMetadataProjection {
            host_handle: super::PluginHostHandle::parse("019d0000-0000-4000-8000-000000000001")
                .expect("handle"),
            label: "Host".to_owned(),
            address: "host.test".to_owned(),
            port: 22,
            username: Some("user".to_owned()),
            favorite: false,
            host_state_version: crate::WireSequence::new(1),
            scope_state_version: crate::WireSequence::new(1),
        })
        .expect("serialize");
        let object = fields.as_object().expect("object");
        for forbidden in [
            "hostId",
            "identityId",
            "credentialRefId",
            "secretRefId",
            "authenticationPlan",
            "routePlan",
        ] {
            assert!(!object.contains_key(forbidden));
        }
    }
}
