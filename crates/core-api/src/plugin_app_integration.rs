//! Bounded application contributions. These reference verified document actions, never host code.
use crate::{PluginExtensionTargetId, PluginId, PluginUiActionId, RequestMeta, WireSequence};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAppCommand {
    pub id: String,
    pub label: String,
    pub target_id: PluginExtensionTargetId,
    pub action_id: PluginUiActionId,
    #[serde(default)]
    #[ts(optional)]
    pub page_id: Option<String>,
    /// Only Alt+Shift+letter is accepted; host binding remains opt-in.
    pub shortcut: Option<String>,
    /// A file command invokes the declared action, whose FilePick call still requires native
    /// selection and the existing exact-scope approval before a fresh handle is issued.
    pub file_extensions: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAppStatus {
    pub id: String,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAppRegistration {
    pub commands: Vec<PluginAppCommand>,
    pub statuses: Vec<PluginAppStatus>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAppNotification {
    pub id: String,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAppIntegrationSnapshot {
    pub plugin_id: PluginId,
    pub plugin_name: String,
    pub package_sha256: String,
    pub instance_generation: WireSequence,
    pub registration: PluginAppRegistration,
    pub notifications: Vec<PluginAppNotification>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAppIntegrationListRequest {
    pub meta: RequestMeta,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PluginAppNavigation {
    App { path: String },
    PluginPage { page_id: String },
}
