//! Core-owned, recoverable delivery records for plugin protocol terminal launches.
use crate::{PluginId, RequestMeta, WireSequence};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolOpen {
    pub provider_id: String,
    #[ts(type = "Record<string, boolean | number | string>")]
    pub configuration: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolLaunchSummary {
    pub launch_id: String,
    pub plugin_id: PluginId,
    pub provider_id: String,
    pub label: String,
    pub tab_id: String,
    pub pane_id: String,
    pub revision: WireSequence,
    pub claimed: bool,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProtocolLaunchRequest {
    pub meta: RequestMeta,
    pub launch_id: String,
    pub expected_revision: WireSequence,
}
