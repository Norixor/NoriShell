//! Category-scoped application data available to an authorized plugin.
//!
//! This catalog is descriptive. A listed category does not itself grant read,
//! export, or restore authority.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::PluginCapability;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginDataCategory {
    Hosts,
    Credentials,
    DesktopProfiles,
    AppPreferences,
    TerminalHistory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataCategoryDescriptor {
    pub category: PluginDataCategory,
    pub required_capability: PluginCapability,
    pub can_read: bool,
    pub can_export: bool,
    pub can_restore: bool,
    /// Preference groups are listed individually because only `desktop` is
    /// currently persisted and readable by Core.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable_groups: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataCatalog {
    pub categories: Vec<PluginDataCategoryDescriptor>,
}

pub const MAX_PLUGIN_HISTORY_READ_LIMIT: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginDataObjectKind {
    Host,
    DesktopProfile,
    Identity,
    Credential,
    Route,
    AuthenticationPlan,
    AlgorithmPolicy,
    HeartbeatPolicy,
    MonitoringPolicy,
    LoginAutomation,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginDataObjectDisplay {
    Host {
        label: String,
        address: String,
        port: u16,
    },
    Credential {
        label: String,
        material_kind: String,
    },
    DesktopProfile {
        label: String,
        protocol: String,
        address: String,
        port: u16,
    },
}

/// Equality and identity tags are Core-keyed. Neither plaintext nor a raw
/// password-verification digest crosses the plugin boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataObjectDescriptor {
    pub category: PluginDataCategory,
    pub kind: PluginDataObjectKind,
    pub stable_id: String,
    pub object_handle: String,
    pub equality_tag: String,
    #[ts(type = "number | null")]
    pub update_time_unix_ms: Option<i64>,
    pub tombstone: bool,
    pub dependency: bool,
    pub display: Option<PluginDataObjectDisplay>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataLocalCounts {
    pub host_count: u32,
    pub credential_count: u32,
    pub desktop_profile_count: u32,
    pub tombstone_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginDataObjectSource {
    Local,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataObjectDecision {
    pub object_handle: String,
    pub source: PluginDataObjectSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataSnapshotRequest {
    pub profile_id: String,
    pub categories: Vec<PluginDataCategory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataInspectRequest {
    pub profile_id: String,
    pub categories: Vec<PluginDataCategory>,
    pub receipt_handle: String,
    pub body_blob_handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataComposeRequest {
    pub profile_id: String,
    pub categories: Vec<PluginDataCategory>,
    pub local_snapshot_handle: String,
    pub remote_inspection_handle: String,
    pub decisions: Vec<PluginDataObjectDecision>,
}

/// Both composed candidates and their provenance are verified by Core before
/// it presents a single protected choice. Every handle is owner scoped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataReviewRequest {
    pub profile_id: String,
    pub categories: Vec<PluginDataCategory>,
    pub local_snapshot_handle: String,
    pub remote_inspection_handle: String,
    pub base_receipt_handle: String,
    pub local_composed_handle: String,
    pub remote_composed_handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataApplyRequest {
    pub profile_id: String,
    pub categories: Vec<PluginDataCategory>,
    pub expected_local_snapshot_handle: String,
    pub composed_handle: String,
    pub export_handle: Option<String>,
    pub authoritative_receipt_handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataExportRequest {
    pub profile_id: String,
    pub categories: Vec<PluginDataCategory>,
    pub source_handle: String,
    pub base_receipt_handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataCheckpointRequest {
    pub profile_id: String,
    pub categories: Vec<PluginDataCategory>,
    pub source_handle: String,
    pub authoritative_receipt_handle: String,
    pub base_receipt_handle: Option<String>,
    pub export_handle: Option<String>,
    pub apply_receipt_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expected_local_snapshot_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub remote_inspection_handle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataReleaseRequest {
    pub profile_id: String,
    #[serde(default)]
    pub state_handles: Vec<String>,
    #[serde(default)]
    pub blob_handles: Vec<String>,
    #[serde(default)]
    pub receipt_handles: Vec<String>,
}

fn validate_exchange_scope(
    profile_id: &str,
    categories: &[PluginDataCategory],
) -> Result<(), crate::PluginApiErrorCode> {
    if profile_id.is_empty()
        || profile_id.len() > 80
        || !profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || categories.is_empty()
        || categories.len() > 3
        || categories.windows(2).any(|pair| pair[0] >= pair[1])
        || categories.iter().any(|category| {
            !matches!(
                category,
                PluginDataCategory::Hosts
                    | PluginDataCategory::Credentials
                    | PluginDataCategory::DesktopProfiles
            )
        })
    {
        return Err(crate::PluginApiErrorCode::InvalidRequest);
    }
    Ok(())
}

fn valid_handle(handle: &str) -> bool {
    uuid::Uuid::parse_str(handle).is_ok()
}

impl PluginDataSnapshotRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        validate_exchange_scope(&self.profile_id, &self.categories)
    }
}

impl PluginDataInspectRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        validate_exchange_scope(&self.profile_id, &self.categories)?;
        if !valid_handle(&self.receipt_handle) || !valid_handle(&self.body_blob_handle) {
            return Err(crate::PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl PluginDataComposeRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        validate_exchange_scope(&self.profile_id, &self.categories)?;
        if !valid_handle(&self.local_snapshot_handle)
            || !valid_handle(&self.remote_inspection_handle)
            || self.decisions.len() > 3_000
            || self.decisions.iter().any(|decision| {
                decision.object_handle.len() != 64
                    || !decision
                        .object_handle
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(crate::PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl PluginDataReviewRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        validate_exchange_scope(&self.profile_id, &self.categories)?;
        if [
            &self.local_snapshot_handle,
            &self.remote_inspection_handle,
            &self.base_receipt_handle,
            &self.local_composed_handle,
            &self.remote_composed_handle,
        ]
        .iter()
        .any(|handle| !valid_handle(handle))
            || self.local_composed_handle == self.remote_composed_handle
        {
            return Err(crate::PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl PluginDataApplyRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        validate_exchange_scope(&self.profile_id, &self.categories)?;
        if !valid_handle(&self.expected_local_snapshot_handle)
            || !valid_handle(&self.composed_handle)
            || self
                .export_handle
                .as_ref()
                .is_some_and(|handle| !valid_handle(handle))
            || !valid_handle(&self.authoritative_receipt_handle)
        {
            return Err(crate::PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl PluginDataExportRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        validate_exchange_scope(&self.profile_id, &self.categories)?;
        if !valid_handle(&self.source_handle) || !valid_handle(&self.base_receipt_handle) {
            return Err(crate::PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl PluginDataCheckpointRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        validate_exchange_scope(&self.profile_id, &self.categories)?;
        if !valid_handle(&self.source_handle)
            || !valid_handle(&self.authoritative_receipt_handle)
            || self
                .base_receipt_handle
                .as_ref()
                .is_some_and(|handle| !valid_handle(handle))
            || self
                .export_handle
                .as_ref()
                .is_some_and(|handle| !valid_handle(handle))
            || self
                .apply_receipt_handle
                .as_ref()
                .is_some_and(|handle| !valid_handle(handle))
            || self
                .expected_local_snapshot_handle
                .as_ref()
                .is_some_and(|handle| !valid_handle(handle))
            || self
                .remote_inspection_handle
                .as_ref()
                .is_some_and(|handle| !valid_handle(handle))
            || self.base_receipt_handle.is_some() != self.export_handle.is_some()
            || self.expected_local_snapshot_handle.is_some()
                != self.remote_inspection_handle.is_some()
            || (self.expected_local_snapshot_handle.is_some()
                && (self.base_receipt_handle.is_some()
                    || self.export_handle.is_some()
                    || self.apply_receipt_handle.is_some()))
        {
            return Err(crate::PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

impl PluginDataReleaseRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        if self.profile_id.is_empty()
            || self.profile_id.len() > 80
            || !self
                .profile_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || !valid_release_handles(&self.state_handles, 24)
            || !valid_release_handles(&self.blob_handles, 12)
            || !valid_release_handles(&self.receipt_handles, 24)
        {
            return Err(crate::PluginApiErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

fn valid_release_handles(handles: &[String], max: usize) -> bool {
    handles.len() <= max
        && handles.iter().all(|handle| valid_handle(handle))
        && handles
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == handles.len()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDataReadRequest {
    pub category: PluginDataCategory,
    pub offset: u16,
    pub limit: u16,
}

impl PluginDataReadRequest {
    pub fn validate(&self) -> Result<(), crate::PluginApiErrorCode> {
        match self.category {
            PluginDataCategory::AppPreferences if self.offset == 0 && self.limit == 1 => Ok(()),
            PluginDataCategory::TerminalHistory
                if self.limit > 0 && self.limit <= MAX_PLUGIN_HISTORY_READ_LIMIT =>
            {
                Ok(())
            }
            PluginDataCategory::Hosts
            | PluginDataCategory::Credentials
            | PluginDataCategory::DesktopProfiles
            | PluginDataCategory::AppPreferences
            | PluginDataCategory::TerminalHistory => Err(crate::PluginApiErrorCode::InvalidRequest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_requires_distinct_valid_candidates_and_one_scope() {
        let mut request = PluginDataReviewRequest {
            profile_id: "primary".into(),
            categories: vec![PluginDataCategory::Hosts],
            local_snapshot_handle: uuid::Uuid::new_v4().to_string(),
            remote_inspection_handle: uuid::Uuid::new_v4().to_string(),
            base_receipt_handle: uuid::Uuid::new_v4().to_string(),
            local_composed_handle: uuid::Uuid::new_v4().to_string(),
            remote_composed_handle: uuid::Uuid::new_v4().to_string(),
        };
        assert_eq!(request.validate(), Ok(()));
        request.remote_composed_handle = request.local_composed_handle.clone();
        assert_eq!(
            request.validate(),
            Err(crate::PluginApiErrorCode::InvalidRequest)
        );
        request.remote_composed_handle = uuid::Uuid::new_v4().to_string();
        request.base_receipt_handle = "guest-supplied-path".into();
        assert_eq!(
            request.validate(),
            Err(crate::PluginApiErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn release_requires_distinct_bounded_handles() {
        let handle = uuid::Uuid::new_v4().to_string();
        let mut request = PluginDataReleaseRequest {
            profile_id: "primary".to_owned(),
            state_handles: vec![handle.clone()],
            blob_handles: Vec::new(),
            receipt_handles: Vec::new(),
        };
        assert_eq!(request.validate(), Ok(()));
        request.state_handles.push(handle);
        assert_eq!(
            request.validate(),
            Err(crate::PluginApiErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn equal_checkpoint_rejects_mixed_upload_or_apply_proof() {
        let handle = uuid::Uuid::new_v4().to_string();
        let mut request = PluginDataCheckpointRequest {
            profile_id: "primary".to_owned(),
            categories: vec![PluginDataCategory::Hosts],
            source_handle: handle.clone(),
            authoritative_receipt_handle: handle.clone(),
            base_receipt_handle: None,
            export_handle: None,
            apply_receipt_handle: None,
            expected_local_snapshot_handle: Some(handle.clone()),
            remote_inspection_handle: Some(handle.clone()),
        };
        assert_eq!(request.validate(), Ok(()));
        request.apply_receipt_handle = Some(handle);
        assert_eq!(
            request.validate(),
            Err(crate::PluginApiErrorCode::InvalidRequest)
        );
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalHistoryEntry {
    pub entry_id: String,
    pub scope: String,
    pub command: String,
    #[ts(type = "number")]
    pub completed_at_unix_ms: i64,
    #[ts(type = "number")]
    pub elapsed_millis: u64,
    pub exit_code: Option<i32>,
}

impl std::fmt::Debug for PluginTerminalHistoryEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginTerminalHistoryEntry")
            .field("entry_id", &self.entry_id)
            .field("scope", &self.scope)
            .field("command", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginDataReadResult {
    AppPreferences {
        groups: Vec<PluginPreferenceGroupSnapshot>,
        migration_required: Vec<String>,
    },
    TerminalHistory {
        entries: Vec<PluginTerminalHistoryEntry>,
        next_offset: Option<u16>,
        total: u16,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPreferenceGroupSnapshot {
    pub group: String,
    pub revision: crate::WireSequence,
    #[ts(type = "unknown")]
    pub value: serde_json::Value,
}

impl std::fmt::Debug for PluginDataReadResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppPreferences { .. } => formatter.write_str("AppPreferences { .. }"),
            Self::TerminalHistory { entries, .. } => formatter
                .debug_struct("TerminalHistory")
                .field("entry_count", &entries.len())
                .finish_non_exhaustive(),
        }
    }
}
