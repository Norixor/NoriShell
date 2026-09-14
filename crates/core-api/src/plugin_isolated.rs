//! Plugin-owned UI hosted inside a separate, sandboxed WebView surface.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{PluginApiCall, PluginApiReply, RequestMeta};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginIsolatedSurfaceOpenRequest {
    pub surface_id: String,
    pub title: String,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginIsolatedSurfaceContentRequest {
    pub meta: RequestMeta,
    pub surface_id: String,
    pub channel_nonce: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginIsolatedSurfaceContent {
    pub surface_id: String,
    pub document_token: String,
    pub next_sequence: u32,
}

/// Sent only by the trusted outer window. The sandbox never receives the nonce.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginIsolatedBridgeRequest {
    pub meta: RequestMeta,
    pub surface_id: String,
    pub channel_nonce: String,
    pub sequence: u32,
    pub action: PluginIsolatedBridgeAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginIsolatedBridgeAction {
    Call {
        call: PluginApiCall,
    },
    /// Consumes an immutable Core-held request; no replacement payload is accepted.
    Approve {
        pending_id: String,
    },
    Cancel {
        pending_id: String,
    },
    Deactivate {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginIsolatedPendingAction {
    pub pending_id: String,
    pub call_id: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginIsolatedBridgeResponse {
    pub next_sequence: u32,
    pub reply: Option<PluginApiReply>,
    pub pending: Option<PluginIsolatedPendingAction>,
}

impl fmt::Debug for PluginIsolatedSurfaceContentRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginIsolatedSurfaceContentRequest")
            .field("meta", &self.meta)
            .field("surface_id", &self.surface_id)
            .field("channel_nonce", &"[redacted]")
            .finish()
    }
}

impl fmt::Debug for PluginIsolatedBridgeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginIsolatedBridgeRequest")
            .field("meta", &self.meta)
            .field("surface_id", &self.surface_id)
            .field("channel_nonce", &"[redacted]")
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}
