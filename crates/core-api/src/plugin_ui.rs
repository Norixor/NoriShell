//! Bounded, host-rendered plugin UI protocol.
//!
//! This module deliberately contains data only. A plugin supplies a flat UI
//! document and opaque target handles; it never supplies Vue components,
//! executable callbacks, raw HTML, CSS or host resource identifiers.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ts_rs::TS;

use crate::{
    CredentialRefId, HostId, PluginApprovalId, PluginCapability, PluginHostDomOperationBatch,
    PluginHostDomSnapshot, PluginId, PluginTerminalInputSuggestion, RequestMeta, WireSequence,
};

const MAX_PLUGIN_UI_ID_BYTES: usize = 160;

macro_rules! bounded_plugin_ui_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
        #[ts(type = "string")]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_PLUGIN_UI_ID_BYTES
                    || !value.is_ascii()
                    || value.starts_with(['.', ':', '-'])
                    || value.ends_with(['.', ':', '-'])
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-')
                    })
                {
                    return Err("plugin UI identifier must be bounded ASCII");
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = &'static str;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
            }
        }
    };
}

bounded_plugin_ui_id!(
    PluginExtensionTargetId,
    "Stable host-defined extension target identifier."
);
bounded_plugin_ui_id!(
    PluginTargetContextHandle,
    "Opaque, short-lived handle for one concrete extension target instance."
);
bounded_plugin_ui_id!(PluginUiNodeId, "Plugin-local UI node identifier.");
bounded_plugin_ui_id!(PluginUiActionId, "Plugin-local UI action identifier.");
bounded_plugin_ui_id!(PluginUiFieldId, "Plugin-local form field identifier.");
bounded_plugin_ui_id!(PluginPageId, "Plugin-local page identifier.");
bounded_plugin_ui_id!(PluginNavigationId, "Plugin-local navigation identifier.");

pub const PLUGIN_UI_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginUiDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginUiAlign {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginUiTextStyle {
    Body,
    Secondary,
    Caption,
    Heading,
    Monospace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginUiTone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginUiButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginUiFieldKind {
    Text,
    Search,
    Number,
    Url,
    Multiline,
    /// A plugin-owned secret entered on its own Page. Core never sources this
    /// value from Vault, Host credentials or another application surface.
    Password,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiSelectOption {
    pub value: String,
    pub label: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiTableColumn {
    pub column_id: String,
    pub label: String,
    pub width: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiTableRow {
    pub row_id: String,
    pub cells: Vec<String>,
    pub action_id: Option<PluginUiActionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiTab {
    pub id: String,
    pub label: String,
    pub children: Vec<PluginUiNodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiTreeItem {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub children: Option<Vec<PluginUiTreeItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub action_id: Option<PluginUiActionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginUiChartKind {
    Line,
    Bar,
}

#[derive(Debug, Clone, Copy, TS)]
#[repr(transparent)]
#[ts(type = "number")]
pub struct PluginUiFiniteNumber(pub f64);
impl PartialEq for PluginUiFiniteNumber {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for PluginUiFiniteNumber {}
impl Serialize for PluginUiFiniteNumber {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}
impl<'de> Deserialize<'de> for PluginUiFiniteNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(de::Error::custom("chart value must be finite"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiChartSeries {
    pub label: String,
    pub values: Vec<PluginUiFiniteNumber>,
}

/// Flat nodes keep validation iterative and make cycle/depth/size limits a
/// Core-owned concern instead of relying on recursive deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginUiNode {
    Stack {
        node_id: PluginUiNodeId,
        direction: PluginUiDirection,
        align: PluginUiAlign,
        gap: u8,
        children: Vec<PluginUiNodeId>,
    },
    Grid {
        node_id: PluginUiNodeId,
        columns: u8,
        /// Optional relative column weights. When present, it must contain one
        /// positive bounded value for every column.
        column_weights: Option<Vec<u16>>,
        gap: u8,
        children: Vec<PluginUiNodeId>,
    },
    Section {
        node_id: PluginUiNodeId,
        title: Option<String>,
        children: Vec<PluginUiNodeId>,
    },
    Divider {
        node_id: PluginUiNodeId,
    },
    Text {
        node_id: PluginUiNodeId,
        text: String,
        style: PluginUiTextStyle,
        tone: PluginUiTone,
    },
    Code {
        node_id: PluginUiNodeId,
        text: String,
        language: Option<String>,
        wrap: bool,
    },
    Icon {
        node_id: PluginUiNodeId,
        icon: String,
        accessible_label: String,
        tone: PluginUiTone,
    },
    Status {
        node_id: PluginUiNodeId,
        label: String,
        tone: PluginUiTone,
    },
    Progress {
        node_id: PluginUiNodeId,
        label: Option<String>,
        value_percent: Option<u8>,
        tone: PluginUiTone,
    },
    Button {
        node_id: PluginUiNodeId,
        action_id: PluginUiActionId,
        label: String,
        icon: Option<String>,
        variant: PluginUiButtonVariant,
        disabled: bool,
    },
    CopyButton {
        node_id: PluginUiNodeId,
        action_id: PluginUiActionId,
        label: String,
        disabled: bool,
    },
    TextField {
        node_id: PluginUiNodeId,
        field_id: PluginUiFieldId,
        label: String,
        value: String,
        placeholder: Option<String>,
        field_kind: PluginUiFieldKind,
        required: bool,
        disabled: bool,
    },
    Select {
        node_id: PluginUiNodeId,
        field_id: PluginUiFieldId,
        label: String,
        value: Option<String>,
        options: Vec<PluginUiSelectOption>,
        disabled: bool,
    },
    Checkbox {
        node_id: PluginUiNodeId,
        field_id: PluginUiFieldId,
        label: String,
        checked: bool,
        disabled: bool,
    },
    Switch {
        node_id: PluginUiNodeId,
        field_id: PluginUiFieldId,
        label: String,
        checked: bool,
        disabled: bool,
    },
    Table {
        node_id: PluginUiNodeId,
        label: String,
        columns: Vec<PluginUiTableColumn>,
        rows: Vec<PluginUiTableRow>,
        empty_text: Option<String>,
    },
    Menu {
        node_id: PluginUiNodeId,
        label: String,
        children: Vec<PluginUiNodeId>,
    },
    /// A host-owned, user-triggered modal available only on plugin Pages.
    /// Open/close state, focus trapping and dismissal remain renderer-owned.
    Dialog {
        node_id: PluginUiNodeId,
        title: String,
        description: Option<String>,
        trigger_label: String,
        close_label: String,
        children: Vec<PluginUiNodeId>,
    },
    Disclosure {
        node_id: PluginUiNodeId,
        label: String,
        open: bool,
        children: Vec<PluginUiNodeId>,
    },
    /// Host-owned remote-data browser available only on plugin Pages. The
    /// plugin owns the surrounding summary children but never receives the
    /// renderer's search, selection or pagination state.
    SshSyncBrowser {
        node_id: PluginUiNodeId,
        profile_id: String,
        children: Vec<PluginUiNodeId>,
    },
    Tabs {
        node_id: PluginUiNodeId,
        label: String,
        tabs: Vec<PluginUiTab>,
    },
    Tree {
        node_id: PluginUiNodeId,
        label: String,
        items: Vec<PluginUiTreeItem>,
    },
    Chart {
        node_id: PluginUiNodeId,
        label: String,
        chart_kind: PluginUiChartKind,
        series: Vec<PluginUiChartSeries>,
        labels: Vec<String>,
    },
    Editor {
        node_id: PluginUiNodeId,
        field_id: PluginUiFieldId,
        label: String,
        value: String,
        language: String,
        read_only: bool,
    },
}

impl PluginUiNode {
    #[must_use]
    pub fn node_id(&self) -> &PluginUiNodeId {
        match self {
            Self::Stack { node_id, .. }
            | Self::Grid { node_id, .. }
            | Self::Section { node_id, .. }
            | Self::Divider { node_id }
            | Self::Text { node_id, .. }
            | Self::Code { node_id, .. }
            | Self::Icon { node_id, .. }
            | Self::Status { node_id, .. }
            | Self::Progress { node_id, .. }
            | Self::Button { node_id, .. }
            | Self::CopyButton { node_id, .. }
            | Self::TextField { node_id, .. }
            | Self::Select { node_id, .. }
            | Self::Checkbox { node_id, .. }
            | Self::Switch { node_id, .. }
            | Self::Table { node_id, .. }
            | Self::Menu { node_id, .. }
            | Self::Dialog { node_id, .. }
            | Self::Disclosure { node_id, .. }
            | Self::SshSyncBrowser { node_id, .. }
            | Self::Tabs { node_id, .. }
            | Self::Tree { node_id, .. }
            | Self::Chart { node_id, .. }
            | Self::Editor { node_id, .. } => node_id,
        }
    }

    #[must_use]
    pub fn referenced_child_ids(&self) -> Vec<&PluginUiNodeId> {
        match self {
            Self::Tabs { tabs, .. } => tabs.iter().flat_map(|tab| tab.children.iter()).collect(),
            _ => self.child_ids().iter().collect(),
        }
    }

    #[must_use]
    pub fn child_ids(&self) -> &[PluginUiNodeId] {
        match self {
            Self::Stack { children, .. }
            | Self::Grid { children, .. }
            | Self::Section { children, .. }
            | Self::Menu { children, .. }
            | Self::Dialog { children, .. }
            | Self::Disclosure { children, .. }
            | Self::SshSyncBrowser { children, .. } => children,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiDocument {
    pub schema_version: u16,
    pub root_node_id: PluginUiNodeId,
    pub nodes: Vec<PluginUiNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginExtensionSurfaceKind {
    Inline,
    Toolbar,
    Menu,
    Sidebar,
    Card,
    Page,
    Navigation,
    Overlay,
}

/// Public target metadata. Resource identity stays behind an opaque handle and
/// the handle is always fenced by owner, generation and target revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginExtensionTargetContext {
    pub target_id: PluginExtensionTargetId,
    pub surface_kind: PluginExtensionSurfaceKind,
    pub context_handle: PluginTargetContextHandle,
    pub target_revision: WireSequence,
    pub display_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiContribution {
    pub plugin_id: PluginId,
    pub plugin_name: String,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub package_sha256: String,
    pub instance_generation: WireSequence,
    pub state_version: WireSequence,
    pub contribution_revision: WireSequence,
    pub target: PluginExtensionTargetContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_open_action_id: Option<PluginUiActionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_refresh: Option<PluginUiAutoRefresh>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub document: PluginUiDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiTemplate {
    pub target_id: PluginExtensionTargetId,
    /// Optional no-field lifecycle hook invoked after the target becomes visible.
    /// It is distinct from user-driven document action IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_open_action_id: Option<PluginUiActionId>,
    /// Optional host-scheduled, no-field refresh hook for terminal.footer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_refresh: Option<PluginUiAutoRefresh>,
    /// Exact ordinary application paths; absent means every permitted mount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub document: PluginUiDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiAutoRefresh {
    pub action_id: PluginUiActionId,
    pub interval_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginExtensionTargetDefinition {
    pub target_id: PluginExtensionTargetId,
    pub surface_kind: PluginExtensionSurfaceKind,
    pub required_capability: PluginCapability,
    pub contextual: bool,
    pub accepts_forms: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginExtensionTargetListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTargetContextOpenRequest {
    pub meta: RequestMeta,
    pub target_id: PluginExtensionTargetId,
    /// Trusted renderer lifecycle key. It is hashed in Core and never sent to
    /// the plugin or returned through the public projection.
    pub target_instance_key: String,
    pub display_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTargetContextCloseRequest {
    pub meta: RequestMeta,
    pub context_handle: PluginTargetContextHandle,
    pub expected_target_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiContributionListRequest {
    pub meta: RequestMeta,
    pub target: PluginExtensionTargetContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiActionRequest {
    /// Background invocations may never open protected interaction windows.
    #[serde(default)]
    #[ts(optional)]
    pub background: Option<bool>,
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
    pub action_id: PluginUiActionId,
    pub fields: Vec<PluginUiFieldValue>,
    pub host_dom_snapshot: Option<PluginHostDomSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncOAuthProfile {
    pub authorization_url: String,
    pub token_url: String,
    pub revoke_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    /// Canonical HTTPS origins that may receive this profile's bearer token.
    /// Paths are intentionally excluded so the Core can compare the final
    /// parsed request URL without trusting plugin-controlled string prefixes.
    pub resource_origins: Vec<String>,
}

/// Plugin-owned credential authentication endpoints. The plugin renders the
/// host-managed login/register fields, while Core resolves only the field
/// references from the current explicit action, performs the bounded HTTPS
/// exchange and stores refresh tokens in the local Vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncCredentialProfile {
    pub login_url: String,
    pub registration_url: String,
    pub email_verification_url: String,
    pub mfa_url: String,
    pub token_url: String,
    pub revoke_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub resource_origins: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncHttpMethod {
    Post,
    Put,
}

/// Plugin-chosen HTTPS upload target. Core sends only the opaque encrypted
/// exchange body and attaches a plugin/profile-scoped OAuth token when asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncUploadTarget {
    pub url: String,
    pub method: PluginSshSyncHttpMethod,
    pub use_oauth: bool,
    pub if_match: Option<String>,
}

/// Plugin-chosen HTTPS download target. Response bytes remain in Core until
/// they have been authenticated, decrypted and approved in a secure surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncDownloadSource {
    pub url: String,
    pub use_oauth: bool,
}

/// Plugin-chosen HTTPS deletion target. Core obtains and attaches the current
/// strong ETag only after the user confirms the destructive operation in a
/// protected window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncDeleteTarget {
    pub url: String,
    pub use_oauth: bool,
}

/// A sync plugin owns provider login, HTTPS endpoints and its declarative UI.
/// Protocol minor 7 keeps revision, strong ETag/CAS, encryption, durable
/// baselines and conflict detection in Core after the high-risk capability and
/// action lease have been revalidated. Older variants remain for protocol 4-6
/// compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSshSyncRequest {
    Status {
        profile_id: String,
        /// Provider configuration allows a fresh Core process to restore the
        /// Vault-backed session without a password. OAuth profiles omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<PluginSshSyncCredentialProfile>,
    },
    Authorize {
        profile_id: String,
        oauth: PluginSshSyncOAuthProfile,
    },
    Login {
        profile_id: String,
        auth: PluginSshSyncCredentialProfile,
        username_field_id: PluginUiFieldId,
        password_field_id: PluginUiFieldId,
    },
    Register {
        profile_id: String,
        auth: PluginSshSyncCredentialProfile,
        username_field_id: PluginUiFieldId,
        password_field_id: PluginUiFieldId,
        password_confirmation_field_id: PluginUiFieldId,
        display_name_field_id: Option<PluginUiFieldId>,
    },
    VerifyEmail {
        profile_id: String,
        code_field_id: PluginUiFieldId,
    },
    CompleteMfa {
        profile_id: String,
        code_field_id: PluginUiFieldId,
    },
    Logout {
        profile_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<PluginSshSyncCredentialProfile>,
    },
    /// Refreshes the provider exchange and reports only
    /// aggregate local/remote facts. It never applies remote data.
    Refresh {
        profile_id: String,
        auth: PluginSshSyncCredentialProfile,
        source: PluginSshSyncDownloadSource,
    },
    /// Performs the Core-owned pull/merge/CAS workflow. The
    /// plugin supplies endpoints, never revisions, ETags or sync keys.
    Sync {
        profile_id: String,
        auth: PluginSshSyncCredentialProfile,
        source: PluginSshSyncDownloadSource,
        destination: PluginSshSyncUploadTarget,
    },
    /// Opens the protected scope editor. All eligible portable objects remain
    /// the default when no custom scope has been saved.
    ConfigureScope { profile_id: String },
    /// Permanently removes the current profile's opaque
    /// remote exchange after a Core-owned warning and strong-ETag CAS check.
    /// Local Hosts, credentials, Vault material and the profile sync key are
    /// preserved.
    ResetRemote {
        profile_id: String,
        auth: PluginSshSyncCredentialProfile,
        target: PluginSshSyncDeleteTarget,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncAccountState {
    Disconnected,
    Authorizing,
    NeedsMfa,
    NeedsEmailVerification,
    Connected,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncOperationState {
    Idle,
    Running,
    NeedsReview,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncDifferenceState {
    Unavailable,
    Equal,
    LocalOnly,
    RemoteOnly,
    Different,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncScopeMode {
    AllEligible,
    Custom,
}

/// Stable, non-secret failure classifications suitable for plugin UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSshSyncStableErrorCode {
    VaultMissing,
    VaultLocked,
    InteractionRequired,
    AuthorizationDenied,
    AuthorizationExpired,
    AccessDenied,
    QuotaExceeded,
    NetworkUnavailable,
    ServiceUnavailable,
    StateConflict,
    RemoteDataInvalid,
    RemoteFormatUnsupported,
    OperationRejected,
    Internal,
}

/// Non-secret aggregate facts returned by the host-owned SSH sync broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSshSyncStatus {
    pub profile_id: String,
    pub account_state: PluginSshSyncAccountState,
    pub operation_state: PluginSshSyncOperationState,
    pub last_sync_at_unix_ms: Option<i64>,
    pub host_count: u32,
    pub credential_count: u32,
    pub conflict_count: u32,
    pub stable_error_code: Option<PluginSshSyncStableErrorCode>,
    pub http_status: Option<u16>,
    pub remote_revision: Option<u64>,
    pub etag: Option<String>,
    pub preview_id: Option<String>,
    pub exchange_sha256: Option<String>,
    #[serde(default)]
    pub local_host_count: u32,
    #[serde(default)]
    pub local_credential_count: u32,
    #[serde(default)]
    pub remote_host_count: Option<u32>,
    #[serde(default)]
    pub remote_credential_count: Option<u32>,
    #[serde(default)]
    pub difference_state: Option<PluginSshSyncDifferenceState>,
    #[serde(default)]
    pub scope_mode: Option<PluginSshSyncScopeMode>,
    #[serde(default)]
    pub desktop_profile_count: u32,
    #[serde(default)]
    pub local_desktop_profile_count: u32,
    #[serde(default)]
    pub remote_desktop_profile_count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSyncSecurePromptKind {
    AuthorizeProvider,
    SelectBackup,
    /// Legacy protocol 4-6 flow retained for already installed plugins.
    CreateRecoveryPassword,
    /// Legacy protocol 4-6 flow retained for already installed plugins.
    RecoverExistingKey,
    CreateLocalVault,
    UnlockSynchronizedVault,
    RecoverSynchronizedKey,
    SelectRestore,
    ApproveRestore,
    ResolveConflicts,
    ChooseSyncDirection,
    ResetRemote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSyncSecureDifferenceKind {
    DesktopProfile,
    Host,
    Identity,
    Credential,
    ConnectionRoute,
    Authentication,
    Algorithms,
    Heartbeat,
    Monitoring,
    LoginAutomation,
    EncryptedSecret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSyncSecureDifferenceChange {
    LocalOnly,
    RemoteOnly,
    Changed,
}

/// A bounded, non-secret comparison fact rendered only by the Core-owned
/// protected window. Secret values and OAuth material never enter this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecureDifference {
    pub kind: SshSyncSecureDifferenceKind,
    pub change: SshSyncSecureDifferenceChange,
    pub label: String,
    pub local_summary: Option<String>,
    pub remote_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecureOAuthSummary {
    pub authorization_url: String,
    pub token_url: String,
    pub revoke_url: String,
    pub resource_origins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecureCredential {
    pub credential_ref_id: CredentialRefId,
    pub label: String,
    pub method_label: String,
    pub machine_bound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecureDesktopProfile {
    pub profile_id: String,
    pub label: String,
    pub protocol: crate::DesktopProtocol,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub credentials: Vec<SshSyncSecureCredential>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecureHost {
    pub host_id: HostId,
    pub label: String,
    pub endpoint: String,
    pub credentials: Vec<SshSyncSecureCredential>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecurePromptGetRequest {
    pub meta: RequestMeta,
    pub prompt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecurePrompt {
    pub desktop_profiles: Vec<SshSyncSecureDesktopProfile>,
    pub desktop_profile_count: u32,
    pub remote_desktop_profile_count: u32,
    pub prompt_id: String,
    pub plugin_id: PluginId,
    pub profile_id: String,
    pub remote_origin: Option<String>,
    pub kind: SshSyncSecurePromptKind,
    pub oauth: Option<SshSyncSecureOAuthSummary>,
    pub hosts: Vec<SshSyncSecureHost>,
    pub host_count: u32,
    pub credential_count: u32,
    pub conflict_count: u32,
    #[serde(default)]
    pub update_count: u32,
    #[serde(default)]
    pub delete_count: u32,
    #[serde(default)]
    pub remote_host_count: u32,
    #[serde(default)]
    pub remote_credential_count: u32,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub local_compared_at_unix_ms: Option<i64>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub remote_updated_at_unix_ms: Option<i64>,
    #[serde(default)]
    pub differences: Vec<SshSyncSecureDifference>,
    #[serde(default)]
    pub difference_total_count: u32,
    #[serde(default)]
    pub difference_omitted_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSyncSecureDecision {
    Approve,
    Cancel,
    KeepLocal,
    UseRemote,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecureDecisionRequest {
    pub selected_desktop_profile_ids: Vec<String>,
    pub meta: RequestMeta,
    pub prompt_id: String,
    pub decision: SshSyncSecureDecision,
    pub selected_host_ids: Vec<HostId>,
    pub selected_credential_ref_ids: Vec<CredentialRefId>,
    pub vault_password: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub vault_password_confirmation: Option<String>,
}

impl fmt::Debug for SshSyncSecureDecisionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshSyncSecureDecisionRequest")
            .field("meta", &self.meta)
            .field("prompt_id", &self.prompt_id)
            .field("decision", &self.decision)
            .field("selected_host_ids", &self.selected_host_ids)
            .field(
                "selected_credential_ref_ids",
                &self.selected_credential_ref_ids,
            )
            .field(
                "vault_password_confirmation",
                &self
                    .vault_password_confirmation
                    .as_ref()
                    .map(|_| "[REDACTED]"),
            )
            .field(
                "vault_password",
                &self
                    .vault_password
                    .as_ref()
                    .map(|value| format!("[REDACTED; {} bytes]", value.len())),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSyncSecureDecisionResponse {
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiActionResponse {
    pub contribution: PluginUiContribution,
    /// Present only for a direct CopyButton action after Core revalidates the
    /// separate clipboard.write grant and all action/target fences.
    pub clipboard_text: Option<String>,
    pub host_approval_id: Option<PluginApprovalId>,
    pub host_dom_operations: Option<PluginHostDomOperationBatch>,
    pub terminal_input_suggestion: Option<PluginTerminalInputSuggestion>,
    pub ssh_sync_status: Option<PluginSshSyncStatus>,
    pub isolated_surface_opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiFieldValue {
    pub field_id: PluginUiFieldId,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNavigationContribution {
    pub navigation_id: PluginNavigationId,
    pub label: String,
    pub icon: String,
    pub page_id: PluginPageId,
    pub order: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNavigationItem {
    pub plugin_id: PluginId,
    pub plugin_name: String,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub package_sha256: String,
    pub instance_generation: WireSequence,
    pub state_version: WireSequence,
    pub contribution_revision: WireSequence,
    pub navigation: PluginNavigationContribution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNavigationListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPageContribution {
    pub page_id: PluginPageId,
    pub title: String,
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_open_action_id: Option<PluginUiActionId>,
    pub document: PluginUiDocument,
}

#[cfg(test)]
mod tests {
    use super::{
        PluginExtensionTargetId, PluginSshSyncAccountState, PluginSshSyncDifferenceState,
        PluginSshSyncOperationState, PluginSshSyncRequest, PluginSshSyncScopeMode,
        PluginSshSyncStableErrorCode, PluginSshSyncStatus, PluginUiNodeId,
    };

    #[test]
    fn bounded_ids_reject_markup_whitespace_and_paths() {
        assert!(PluginExtensionTargetId::parse("terminal.toolbar").is_ok());
        assert!(PluginUiNodeId::parse("root:actions_1").is_ok());
        for invalid in ["", " node", "node/child", "<script>", ".node", "node:"] {
            assert!(
                PluginUiNodeId::parse(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn ssh_sync_request_is_provider_scoped_and_rejects_secret_payloads() {
        let status = serde_json::json!({ "action": "status", "profileId": "primary" });
        assert!(serde_json::from_value::<PluginSshSyncRequest>(status.clone()).is_ok());
        for forbidden in ["header", "token", "ciphertext", "hostId", "path", "payload"] {
            let mut value = status.clone();
            value[forbidden] = serde_json::json!("not-allowed");
            assert!(
                serde_json::from_value::<PluginSshSyncRequest>(value).is_err(),
                "accepted forbidden field {forbidden}"
            );
        }
        assert_eq!(
            serde_json::to_value(PluginSshSyncRequest::Status {
                profile_id: "primary".to_owned(),
                auth: None,
            })
            .expect("serialize request"),
            status
        );
        assert!(
            serde_json::from_value::<PluginSshSyncRequest>(serde_json::json!({
                "action": "export",
                "profileId": "primary",
                "revision": 1,
                "baseRevision": null,
                "baseEtag": null,
                "destination": {
                    "url": "https://sync.example.test/exchange",
                    "method": "put",
                    "useOauth": true,
                    "ifMatch": null
                }
            }))
            .is_err()
        );
        let authorize = serde_json::json!({
            "action": "authorize",
            "profileId": "self-hosted",
            "oauth": {
                "authorizationUrl": "https://login.example.test/authorize",
                "tokenUrl": "https://login.example.test/token",
                "revokeUrl": "https://login.example.test/revoke",
                "clientId": "norishell-native",
                "scopes": ["ssh-sync:read", "ssh-sync:write"],
                "resourceOrigins": ["https://sync.example.test"]
            }
        });
        assert!(serde_json::from_value::<PluginSshSyncRequest>(authorize.clone()).is_ok());
        let mut missing_origins = authorize;
        missing_origins["oauth"]
            .as_object_mut()
            .expect("oauth object")
            .remove("resourceOrigins");
        assert!(serde_json::from_value::<PluginSshSyncRequest>(missing_origins).is_err());

        let login = serde_json::json!({
            "action": "login",
            "profileId": "primary",
            "auth": {
                "loginUrl": "https://api.example.test/auth/native/login",
                "registrationUrl": "https://api.example.test/auth/native/register",
                "emailVerificationUrl": "https://api.example.test/auth/native/verify-email",
                "mfaUrl": "https://api.example.test/auth/native/mfa",
                "tokenUrl": "https://api.example.test/auth/native/token",
                "revokeUrl": "https://api.example.test/auth/native/revoke",
                "clientId": "norishell-native",
                "scopes": ["ssh-sync:read", "ssh-sync:write"],
                "resourceOrigins": ["https://api.example.test"]
            },
            "usernameFieldId": "account-email",
            "passwordFieldId": "account-password"
        });
        assert!(serde_json::from_value::<PluginSshSyncRequest>(login.clone()).is_ok());
        for forbidden in ["username", "password", "challengeToken", "accessToken"] {
            let mut value = login.clone();
            value[forbidden] = serde_json::json!("must-not-cross-the-plugin-wire");
            assert!(
                serde_json::from_value::<PluginSshSyncRequest>(value).is_err(),
                "accepted embedded credential field {forbidden}"
            );
        }

        for request in [
            serde_json::json!({
                "action": "logout",
                "profileId": "primary",
                "auth": login["auth"].clone()
            }),
            serde_json::json!({
                "action": "resetRemote",
                "profileId": "primary",
                "auth": login["auth"].clone(),
                "target": {
                    "url": "https://api.example.test/exchange",
                    "useOauth": true
                }
            }),
        ] {
            assert!(serde_json::from_value::<PluginSshSyncRequest>(request).is_ok());
        }

        let register = serde_json::json!({
            "action": "register",
            "profileId": "primary",
            "auth": login["auth"].clone(),
            "usernameFieldId": "register-email",
            "passwordFieldId": "register-password",
            "passwordConfirmationFieldId": "register-password-confirmation",
            "displayNameFieldId": "register-display-name"
        });
        assert!(serde_json::from_value::<PluginSshSyncRequest>(register).is_ok());

        for (action, state) in [
            ("completeMfa", PluginSshSyncAccountState::NeedsMfa),
            (
                "verifyEmail",
                PluginSshSyncAccountState::NeedsEmailVerification,
            ),
        ] {
            assert!(
                serde_json::from_value::<PluginSshSyncRequest>(serde_json::json!({
                    "action": action,
                    "profileId": "primary",
                    "codeFieldId": "verification-code"
                }))
                .is_ok()
            );
            assert_eq!(
                serde_json::to_value(state).expect("serialize challenge state"),
                serde_json::json!(if action == "completeMfa" {
                    "needsMfa"
                } else {
                    "needsEmailVerification"
                })
            );
        }
    }

    #[test]
    fn ssh_sync_status_serializes_only_non_secret_aggregate_facts() {
        let status = PluginSshSyncStatus {
            desktop_profile_count: 0,
            local_desktop_profile_count: 0,
            remote_desktop_profile_count: None,
            profile_id: "primary".to_owned(),
            account_state: PluginSshSyncAccountState::Connected,
            operation_state: PluginSshSyncOperationState::NeedsReview,
            last_sync_at_unix_ms: Some(42),
            host_count: 3,
            credential_count: 4,
            conflict_count: 1,
            stable_error_code: Some(PluginSshSyncStableErrorCode::StateConflict),
            http_status: Some(412),
            remote_revision: Some(4),
            etag: Some("\"revision-4\"".to_owned()),
            preview_id: None,
            exchange_sha256: None,
            local_host_count: 3,
            local_credential_count: 4,
            remote_host_count: Some(2),
            remote_credential_count: Some(5),
            difference_state: Some(PluginSshSyncDifferenceState::Conflict),
            scope_mode: Some(PluginSshSyncScopeMode::AllEligible),
        };
        let encoded = serde_json::to_value(status).expect("serialize status");
        assert_eq!(
            encoded,
            serde_json::json!({
                "profileId": "primary",
                "accountState": "connected",
                "operationState": "needsReview",
                "lastSyncAtUnixMs": 42,
                "hostCount": 3,
                "desktopProfileCount": 0,
                "localDesktopProfileCount": 0,
                "remoteDesktopProfileCount": null,
                "credentialCount": 4,
                "conflictCount": 1,
                "stableErrorCode": "stateConflict",
                "httpStatus": 412,
                "remoteRevision": 4,
                "etag": "\"revision-4\"",
                "previewId": null,
                "exchangeSha256": null,
                "localHostCount": 3,
                "localCredentialCount": 4,
                "remoteHostCount": 2,
                "remoteCredentialCount": 5,
                "differenceState": "conflict",
                "scopeMode": "allEligible",
            })
        );
    }

    #[test]
    fn secure_sync_decision_debug_redacts_vault_password() {
        let request = super::SshSyncSecureDecisionRequest {
            selected_desktop_profile_ids: Vec::new(),
            meta: crate::RequestMeta {
                request_id: crate::RequestId::new(),
            },
            prompt_id: "prompt-1".to_owned(),
            decision: super::SshSyncSecureDecision::Approve,
            selected_host_ids: Vec::new(),
            selected_credential_ref_ids: Vec::new(),
            vault_password: Some("never-log-this-vault-password".to_owned()),
            vault_password_confirmation: Some("never-log-this-confirmation".to_owned()),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("never-log-this-vault-password"));
        assert!(!debug.contains("never-log-this-confirmation"));
        assert!(debug.contains("REDACTED"));
    }
}
