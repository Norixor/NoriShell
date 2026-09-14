//! Host-owned, read-only projection for a verified SSH-sync remote bundle.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    DesktopProtocol, PluginExtensionTargetId, PluginId, PluginTargetContextHandle, PluginUiNodeId,
    RequestMeta, WireSequence,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncBrowserState {
    NotLoaded,
    Ready,
    Empty,
    NeedsCreation,
    NeedsUnlock,
    PermissionDenied,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncBrowserCredentialMaterialKind {
    Password,
    PrivateKey,
    Certificate,
    KeyboardInteractive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncBrowserHost {
    /// Opaque identifier scoped to the current in-memory cache revision.
    pub row_id: String,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub tags: Vec<String>,
}

/// Non-secret remote-desktop metadata scoped to the current in-memory cache revision.
/// It intentionally excludes local identifiers, credentials, routes, display preferences, and
/// every other local-only detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncBrowserDesktopProfile {
    /// Opaque identifier scoped to the current in-memory cache revision.
    pub row_id: String,
    pub label: String,
    pub protocol: DesktopProtocol,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncBrowserCredential {
    /// Opaque identifier scoped to the current in-memory cache revision.
    pub row_id: String,
    pub label: String,
    pub material_kind: PluginSshSyncBrowserCredentialMaterialKind,
    /// Opaque row identifiers for returned Hosts that reference this credential.
    pub host_row_ids: Vec<String>,
    /// Opaque row identifiers for returned remote desktops that reference this credential.
    pub desktop_profile_row_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncBrowserSnapshot {
    pub state: PluginSshSyncBrowserState,
    pub profile_id: String,
    pub cache_revision: WireSequence,
    pub host_count: u32,
    pub credential_count: u32,
    pub desktop_profile_count: u32,
    pub host_rows_omitted: u32,
    pub credential_rows_omitted: u32,
    pub desktop_profile_rows_omitted: u32,
    #[ts(type = "number | null")]
    pub remote_updated_at_unix_ms: Option<i64>,
    pub hosts: Vec<PluginSshSyncBrowserHost>,
    pub credentials: Vec<PluginSshSyncBrowserCredential>,
    pub desktop_profiles: Vec<PluginSshSyncBrowserDesktopProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncBrowserReadRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub expected_package_sha256: String,
    pub instance_generation: WireSequence,
    pub expected_state_version: WireSequence,
    pub expected_contribution_revision: WireSequence,
    pub target_id: PluginExtensionTargetId,
    pub context_handle: PluginTargetContextHandle,
    pub expected_target_revision: WireSequence,
    pub node_id: PluginUiNodeId,
}

/// Tells the trusted renderer to discard any local copy of the Core cache.
/// Both fields are absent when a global event, such as Vault lock, applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncBrowserInvalidated {
    pub plugin_id: Option<PluginId>,
    pub profile_id: Option<String>,
}
