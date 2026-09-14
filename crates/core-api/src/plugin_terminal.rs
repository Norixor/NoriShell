//! Secret-free terminal projections and host-rendered plugin suggestions.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::WireSequence;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginTerminalKind {
    Ssh,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginTerminalState {
    Starting,
    Running,
    AwaitingUser,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalMetadataProjection {
    /// Opaque hash bound to plugin, target context, session and generation.
    pub terminal_handle: String,
    /// Stable preset grouping only. Never grants access to, or opens, a connection.
    pub connection_key: Option<String>,
    pub kind: PluginTerminalKind,
    pub label: String,
    pub state: PluginTerminalState,
    pub generation: WireSequence,
    pub attachment_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalInputSuggestion {
    pub text: String,
    pub description: Option<String>,
}
