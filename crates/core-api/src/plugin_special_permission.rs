//! Protected grant workflow for capabilities that can affect host UI or Hosts.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    HostId, InstalledPluginSummary, PluginApprovalDecision, PluginApprovalId, PluginCapability,
    PluginCapabilityGrant, PluginHostScopeSelection, PluginId, PluginLocalPackagePreview,
    RequestMeta, WireSequence,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSpecialPermissionTarget {
    Installed {
        plugin_id: PluginId,
        expected_plugin_state_version: WireSequence,
    },
    PreparedPackage {
        preparation_id: String,
        expected_package_sha256: String,
        expected_state_version: Option<WireSequence>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSpecialPermissionOutcome {
    Installed { plugin: InstalledPluginSummary },
    PreparedPackage { preview: PluginLocalPackagePreview },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSpecialPermissionHost {
    pub host_id: HostId,
    pub label: String,
    pub endpoint: String,
    pub granted_capabilities: Vec<PluginCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSpecialPermissionSnapshot {
    pub target: PluginSpecialPermissionTarget,
    pub requested_capability: Option<PluginCapability>,
    pub publisher_verified: bool,
    pub approval_id: PluginApprovalId,
    pub approval_state_version: WireSequence,
    pub expires_at_unix_ms: i64,
    pub plugin_id: PluginId,
    pub plugin_name: String,
    pub publisher: String,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub version: String,
    pub package_sha256: String,
    pub special_grants: Vec<PluginCapabilityGrant>,
    pub hosts: Vec<PluginSpecialPermissionHost>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSpecialPermissionOpenRequest {
    pub meta: RequestMeta,
    pub target: PluginSpecialPermissionTarget,
    pub requested_capability: Option<PluginCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSpecialPermissionGetRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginApprovalId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSpecialPermissionDecisionRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginApprovalId,
    pub decision: PluginApprovalDecision,
    pub expected_approval_state_version: WireSequence,
    pub special_grants: Vec<PluginCapabilityGrant>,
    pub host_selections: Vec<PluginHostScopeSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSpecialPermissionDecisionResponse {
    pub approval_id: PluginApprovalId,
    pub decision: PluginApprovalDecision,
    pub target: PluginSpecialPermissionOutcome,
}

#[cfg(test)]
mod tests {
    use super::{PluginSpecialPermissionOutcome, PluginSpecialPermissionTarget};
    use crate::{PluginId, PluginLocalPackagePreview};

    #[test]
    fn prepared_target_and_outcome_keep_the_tagged_wire_contract() {
        let target = PluginSpecialPermissionTarget::PreparedPackage {
            preparation_id: "prepared-1".to_owned(),
            expected_package_sha256: "a".repeat(64),
            expected_state_version: None,
        };
        let target_json = serde_json::to_value(&target).expect("serialize target");
        assert_eq!(target_json["kind"], "preparedPackage");
        assert_eq!(target_json["preparationId"], "prepared-1");
        assert!(target_json["expectedStateVersion"].is_null());

        let outcome = PluginSpecialPermissionOutcome::PreparedPackage {
            preview: PluginLocalPackagePreview {
                preparation_id: "prepared-1".to_owned(),
                plugin_id: PluginId::parse("com.norishell.permission-fixture").expect("plugin id"),
                name: "Permission fixture".to_owned(),
                author: "NoriShell tests".to_owned(),
                version: "1.0.0".to_owned(),
                package_size: 8,
                package_sha256: "a".repeat(64),
                capabilities: Vec::new(),
                current_version: None,
                current_state_version: None,
                retained_capability_grants: Vec::new(),
                approved_special_grants: Vec::new(),
                special_permission_expires_at_unix_ms: None,
                publisher_verified: false,
            },
        };
        let outcome_json = serde_json::to_value(&outcome).expect("serialize outcome");
        assert_eq!(outcome_json["kind"], "preparedPackage");
        assert_eq!(outcome_json["preview"]["preparationId"], "prepared-1");
        assert_eq!(outcome_json["preview"]["publisherVerified"], false);
    }
}
