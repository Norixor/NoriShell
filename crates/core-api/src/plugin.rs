use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ts_rs::TS;

use crate::{
    PluginInputApprovalId, PluginObserverId, PluginOperationId, RequestMeta, SshAttachmentId,
    SshChannelId, SshSessionId, SshTerminalInputFocusTarget, SshViewId, WireSequence,
};

const MAX_PLUGIN_ID_BYTES: usize = 160;

/// Application locale supplied by the trusted host UI. Plugins may use it to
/// choose bundled translations, but cannot override the application's locale.
#[derive(Debug, Clone, PartialEq, Eq, TS)]
#[ts(type = "\"en\" | \"zh-CN\"")]
pub struct PluginLocale(String);

impl PluginLocale {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        matches!(value.as_str(), "en" | "zh-CN")
            .then_some(Self(value))
            .ok_or("plugin locale must be en or zh-CN")
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for PluginLocale {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PluginLocale {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// Stable ASCII identifier supplied by a package manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
#[ts(type = "string")]
pub struct PluginId(String);

impl PluginId {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PLUGIN_ID_BYTES
            || !value.is_ascii()
            || value.starts_with(['.', '-'])
            || value.ends_with(['.', '-'])
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            })
            || value.split('.').any(|part| part.is_empty())
        {
            return Err("plugin_id must be bounded lowercase ASCII segments");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for PluginId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for PluginId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginCapability {
    UiPanel,
    UiNavigation,
    UiPage,
    UiWebviewIsolated,
    UiHostDomObserve,
    UiHostDomMutate,
    UiHostCss,
    ClipboardWrite,
    TerminalProvider,
    DeviceSerial,
    TerminalMetadata,
    TerminalObserve,
    TerminalAnnotation,
    TerminalProposeInput,
    TerminalRequestInput,
    HostMetadataRead,
    HostMutationPropose,
    HostSessionRequest,
    RemoteInspect,
    RemoteExecRequest,
    NetworkDomain,
    LocalFiles,
    LocalProcess,
    StoragePlugin,
    CredentialsPlugin,
    SftpRead,
    SftpWrite,
    MetricsRead,
    SshSync,
    AppPreferencesRead,
    TerminalHistoryRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginCompatibility {
    Compatible,
    ProtocolIncompatible,
    AppVersionIncompatible,
    PlatformIncompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginInstallState {
    Enabled,
    Disabled,
    #[ts(skip)]
    UpdateAvailable,
    Crashed,
    Quarantined,
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
pub enum PluginPackageKind {
    #[default]
    Wasm,
    Theme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginOperationKind {
    /// Legacy operation retained only so existing local audit rows remain decodable.
    #[ts(skip)]
    CatalogRefresh,
    Install,
    Update,
    Disable,
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginOperationState {
    Pending,
    Running,
    AwaitingCapabilities,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginErrorCode {
    PackageTooLarge,
    PackageHashMismatch,
    PackageArchiveInvalid,
    PackagePathRejected,
    PackageLimitsExceeded,
    ManifestMismatch,
    CapabilityRejected,
    ProtocolIncompatible,
    AppVersionIncompatible,
    CoreApiIncompatible,
    InstallConflict,
    RuntimeRejected,
    RuntimeQuotaExceeded,
    RuntimeTimedOut,
    OperationNotFound,
    InvalidRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginReadiness {
    pub ready: bool,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub safe_mode_active: bool,
    pub safe_mode_next_start: bool,
}

/// Persisted metadata from older marketplace-enabled installations. It is no
/// longer exposed by the application, but remains decodable so the local
/// plugin database does not need a destructive migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCatalogReleaseDetails {
    pub description: String,
    pub release_notes: Vec<String>,
    pub extension_targets: Vec<String>,
    #[ts(type = "number | null")]
    pub release_published_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilityGrant {
    pub capability: PluginCapability,
    pub granted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginSummary {
    pub plugin_id: PluginId,
    pub name: String,
    pub publisher: String,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub active_version: String,
    pub package_sha256: String,
    #[serde(default)]
    pub package_kind: PluginPackageKind,
    pub capabilities: Vec<PluginCapability>,
    pub grants: Vec<PluginCapabilityGrant>,
    #[serde(default)]
    pub has_settings: bool,
    pub state: PluginInstallState,
    pub state_version: WireSequence,
    pub installed_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginOperationSummary {
    pub operation_id: PluginOperationId,
    pub plugin_id: Option<PluginId>,
    pub kind: PluginOperationKind,
    pub state: PluginOperationState,
    pub progress_percent: u8,
    pub error_code: Option<PluginErrorCode>,
    pub started_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginReadinessGetRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAuditListRequest {
    pub meta: RequestMeta,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAuditEntry {
    pub audit_id: u64,
    pub plugin_id: Option<String>,
    pub operation_id: Option<String>,
    pub action: String,
    pub outcome: String,
    pub detail_code: Option<String>,
    #[ts(type = "bigint")]
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginSafeModeNextStartRequest {
    pub meta: RequestMeta,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstalledListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginLocalInstallRequest {
    pub meta: RequestMeta,
    pub operation_id: PluginOperationId,
    pub idempotency_key: String,
    pub preparation_id: String,
    pub expected_package_sha256: String,
    pub expected_state_version: Option<WireSequence>,
    pub capability_grants: Vec<PluginCapabilityGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginLocalPackagePrepareRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginLocalPackageCancelRequest {
    pub meta: RequestMeta,
    pub preparation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginLocalPackagePreview {
    pub preparation_id: String,
    pub plugin_id: PluginId,
    pub name: String,
    pub author: String,
    pub version: String,
    pub package_size: u64,
    pub package_sha256: String,
    pub capabilities: Vec<PluginCapability>,
    pub current_version: Option<String>,
    pub current_state_version: Option<WireSequence>,
    /// The prior artifact for this version can remain after uninstall.
    pub prior_package_sha256: Option<String>,
    /// Only verified publisher continuity can retain installed decisions.
    #[serde(default)]
    pub retained_capability_grants: Vec<PluginCapabilityGrant>,
    pub approved_special_grants: Vec<PluginCapabilityGrant>,
    pub special_permission_expires_at_unix_ms: Option<i64>,
    pub publisher_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilityGrantsReplaceRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    pub expected_state_version: WireSequence,
    pub grants: Vec<PluginCapabilityGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginStateChangeRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginLocaleSetRequest {
    pub meta: RequestMeta,
    pub locale: PluginLocale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginUninstallRequest {
    pub meta: RequestMeta,
    pub operation_id: PluginOperationId,
    pub idempotency_key: String,
    pub plugin_id: PluginId,
    pub expected_state_version: WireSequence,
    pub delete_data: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginOperationRequest {
    pub meta: RequestMeta,
    pub operation_id: PluginOperationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginHostRequest {
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub request_id: String,
    pub kind: PluginHostMessageKind,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeOutput {
    pub request_id: String,
    pub kind: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginContributionTone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

/// Host-owned extension locations. A package never receives a Vue component,
/// DOM handle or route; it can only target one of these bounded locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginContributionSlot {
    PluginsPage,
    TerminalSidebar,
    TerminalToolbar,
    SftpContextMenu,
    HostDetailTools,
    OverviewCardActions,
    CommandPalette,
}

/// Host-owned declarative UI nodes. Plugins can provide text and status facts,
/// but never HTML, CSS, URLs, event handlers or host component identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginContributionNode {
    Text {
        text: String,
    },
    Status {
        label: String,
        tone: PluginContributionTone,
    },
    /// A host-rendered action. Plugins provide only an opaque identifier and
    /// label; the WebView never receives executable markup or callbacks.
    Action {
        action_id: String,
        label: String,
    },
    /// Clipboard writes are rendered as a host button and only occur after a
    /// direct user click. Plugins cannot write the clipboard during execution.
    Copy {
        copy_id: String,
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginContributionPanel {
    pub plugin_id: PluginId,
    pub plugin_name: String,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub package_sha256: String,
    pub instance_generation: WireSequence,
    pub state_version: WireSequence,
    pub contribution_revision: WireSequence,
    pub slot: PluginContributionSlot,
    pub nodes: Vec<PluginContributionNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginContributionListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginContributionInvokeRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub expected_package_sha256: String,
    pub instance_generation: WireSequence,
    pub expected_state_version: WireSequence,
    pub expected_contribution_revision: WireSequence,
    pub slot: PluginContributionSlot,
    pub action_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginContributionCopyRequest {
    pub meta: RequestMeta,
    pub plugin_id: PluginId,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub expected_package_sha256: String,
    pub instance_generation: WireSequence,
    pub expected_state_version: WireSequence,
    pub expected_contribution_revision: WireSequence,
    pub slot: PluginContributionSlot,
    pub copy_id: String,
}

/// The only moment a plugin-supplied value can reach the WebView clipboard:
/// Core emits it after revalidating the direct user action and grant fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginContributionCopyResponse {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginHostMessageKind {
    Initialize,
    Invoke,
    UiAction,
    SshSyncResult,
    BrokerResult,
    TerminalObservation,
    ProtocolEvent,
    WorkflowEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginTerminalInputDecision {
    Approve,
    Reject,
}

/// Public, non-secret review projection. The one-time execution token remains
/// exclusively in Core memory and is never serialized through this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalInputProposal {
    pub approval_id: PluginInputApprovalId,
    pub plugin_id: PluginId,
    pub plugin_name: String,
    pub publisher: String,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub package_sha256: String,
    pub instance_generation: WireSequence,
    /// Saved Host label when the target originated from a Host record.
    pub host_label: Option<String>,
    /// Canonical non-secret endpoint rendered for the approval decision.
    pub endpoint: String,
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    pub payload: String,
    pub append_enter: bool,
    pub payload_sha256: String,
    pub expires_at_unix_ms: i64,
    pub state_version: WireSequence,
    #[serde(default)]
    pub remember_policy: crate::PluginRememberPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalInputPendingListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalInputOpenRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginInputApprovalId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTerminalInputGetRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginInputApprovalId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalInputDecisionRequest {
    pub meta: RequestMeta,
    pub approval_id: PluginInputApprovalId,
    pub decision: PluginTerminalInputDecision,
    pub expected_state_version: WireSequence,
    #[serde(default)]
    pub policy: crate::PluginApprovalPolicy,
    #[serde(default)]
    pub expiry: crate::PluginApprovalExpiry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalInputDecisionResponse {
    pub approval_id: PluginInputApprovalId,
    pub decision: PluginTerminalInputDecision,
    pub state_version: WireSequence,
    pub consumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalObserveAttachRequest {
    pub meta: RequestMeta,
    pub operation_id: PluginOperationId,
    pub idempotency_key: String,
    pub plugin_id: PluginId,
    #[serde(rename = "artifactFingerprintSha256")]
    pub signer_fingerprint_sha256: String,
    pub instance_generation: WireSequence,
    pub target: SshTerminalInputFocusTarget,
    pub expected_focus_epoch: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalObserverBinding {
    pub observer_id: PluginObserverId,
    pub plugin_id: PluginId,
    pub instance_generation: WireSequence,
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub focus_epoch: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalObserveAttachResponse {
    pub binding: PluginTerminalObserverBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginTerminalObserveDetachRequest {
    pub meta: RequestMeta,
    pub operation_id: PluginOperationId,
    pub idempotency_key: String,
    pub plugin_id: PluginId,
    pub instance_generation: WireSequence,
    pub observer_id: PluginObserverId,
}

#[cfg(test)]
mod tests {
    use super::{PluginCapability, PluginId, PluginLocale};

    #[test]
    fn plugin_id_rejects_ambiguous_or_unbounded_values() {
        assert!(PluginId::parse("com.norishell.example").is_ok());
        for invalid in ["", ".plugin", "plugin.", "Plugin", "plugin/path", "a..b"] {
            assert!(PluginId::parse(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn ssh_sync_capability_uses_stable_camel_case_wire_name() {
        let encoded = serde_json::to_string(&PluginCapability::SshSync).expect("serialize");
        assert_eq!(encoded, "\"sshSync\"");
        assert_eq!(
            serde_json::from_str::<PluginCapability>(&encoded).expect("deserialize"),
            PluginCapability::SshSync
        );
    }

    #[test]
    fn independent_local_data_capabilities_use_stable_camel_case_wire_names() {
        for (capability, name) in [
            (PluginCapability::AppPreferencesRead, "appPreferencesRead"),
            (PluginCapability::TerminalHistoryRead, "terminalHistoryRead"),
        ] {
            let encoded = serde_json::to_string(&capability).expect("serialize");
            assert_eq!(encoded, format!("\"{name}\""));
            assert_eq!(
                serde_json::from_str::<PluginCapability>(&encoded).expect("deserialize"),
                capability
            );
        }
    }

    #[test]
    fn plugin_locale_accepts_only_host_supported_application_locales() {
        for locale in ["en", "zh-CN"] {
            let parsed = PluginLocale::parse(locale).expect("supported locale");
            assert_eq!(parsed.as_str(), locale);
            assert_eq!(
                serde_json::to_string(&parsed).expect("serialize locale"),
                format!("\"{locale}\"")
            );
        }
        for locale in ["", "zh", "en-US", "ZH-cn"] {
            assert!(PluginLocale::parse(locale).is_err(), "accepted {locale}");
        }
    }
}
