use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    CredentialRefId, HostId, IdentityId, KnownHostId, OperationId, RequestMeta, WireSequence,
};

macro_rules! host_metadata_id {
    ($name:ident, $error:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
        #[ts(type = "string")]
        pub struct $name(HostId);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
                HostId::parse(value).map(Self).map_err(|_| $error)
            }

            #[must_use]
            #[cfg(not(target_arch = "wasm32"))]
            pub fn new() -> Self {
                Self(HostId::new())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

host_metadata_id!(HostGroupId, "host_group_id must be a UUIDv7");
host_metadata_id!(HostTagId, "host_tag_id must be a UUIDv7");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CredentialKind {
    Password,
    PrivateKey,
}

/// Authentication methods supported by the revisioned connection model. Vault import commands
/// continue to use `CredentialKind`, whose narrower variants preserve the existing M1 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AuthenticationMethodKind {
    Password,
    PrivateKey,
    KeyboardInteractive,
    SshAgent,
    Certificate,
    HardwareKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshAgentScope {
    DefaultEnvironment,
}

/// The exact kind of identity selected from the system SSH Agent. This remains separate from the
/// SQLite credential kind so older databases can migrate without guessing from algorithm names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AgentIdentityKind {
    Ordinary,
    Certificate,
    HardwareKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AgentIdentitySource {
    SystemSshAgent,
}

/// NoriShell only accepts user certificates for client authentication. Host certificates
/// must be rejected by the parser before a credential can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshCertificateType {
    User,
}

/// One public critical-option entry from an OpenSSH certificate. Unknown entries are retained so
/// authentication can fail closed instead of silently dropping a server-enforced constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshCertificateCriticalOption {
    pub name: String,
    pub value: Vec<u8>,
    pub recognized: bool,
}

/// One public OpenSSH certificate extension. Unknown extensions are retained for display but do
/// not block authentication; unlike critical options, their protocol semantics are opt-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshCertificateExtension {
    pub name: String,
    pub value: Vec<u8>,
    pub recognized: bool,
}

/// Public metadata parsed from an Agent-provided OpenSSH user certificate. The certificate and
/// subject public-key blobs are public material; no SecretRef, Agent comment or socket path is
/// part of this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshCertificateMetadata {
    pub source: AgentIdentitySource,
    pub certificate_blob: Vec<u8>,
    pub certificate_algorithm: String,
    pub certificate_fingerprint: String,
    /// Exact unsigned 64-bit OpenSSH serial in canonical decimal form.
    pub serial: String,
    pub subject_public_key_blob: Vec<u8>,
    pub subject_public_key_algorithm: String,
    pub subject_public_key_fingerprint: String,
    pub ca_public_key_fingerprint: String,
    pub key_id: String,
    pub valid_principals: Vec<String>,
    pub certificate_type: SshCertificateType,
    #[ts(type = "number")]
    pub valid_after_unix_seconds: i64,
    /// `None` represents OpenSSH's forever sentinel. Finite values are non-negative Unix seconds.
    #[ts(type = "number | null")]
    pub valid_before_unix_seconds: Option<i64>,
    pub critical_options: Vec<SshCertificateCriticalOption>,
    pub extensions: Vec<SshCertificateExtension>,
}

impl SshCertificateMetadata {
    /// Unknown critical options must make the credential ineligible for authentication. They are
    /// still exposed in the public summary so the UI can explain why the certificate is blocked.
    #[must_use]
    pub fn has_unknown_critical_options(&self) -> bool {
        self.critical_options
            .iter()
            .any(|option| !option.recognized)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEndpoint {
    pub address: String,
    pub normalized_address: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProxyDnsMode {
    Local,
    Proxy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RouteIngress {
    DirectTcp,
    HttpConnectProxy {
        endpoint: ProxyEndpoint,
        proxy_auth_credential_ref_id: Option<CredentialRefId>,
    },
    Socks5Proxy {
        endpoint: ProxyEndpoint,
        dns_mode: ProxyDnsMode,
        proxy_auth_credential_ref_id: Option<CredentialRefId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RoutePlanSummary {
    pub host_id: HostId,
    pub revision: WireSequence,
    pub ingress: RouteIngress,
    pub jump_host_ids: Vec<HostId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AuthenticationPlanMode {
    Identity,
    HostOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationPlanSummary {
    pub host_id: HostId,
    pub revision: WireSequence,
    pub mode: AuthenticationPlanMode,
    /// Ordered, bounded references. Empty means that no saved authentication method is ready.
    pub credential_ref_ids: Vec<CredentialRefId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AlgorithmCategory {
    KeyExchange,
    HostKey,
    Cipher,
    Mac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum AlgorithmRisk {
    Modern,
    Legacy,
    Weak,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmCatalogEntry {
    /// Stable Core-owned identifier. This is never an arbitrary SSH name.
    pub stable_id: String,
    pub category: AlgorithmCategory,
    pub algorithm_name: String,
    pub available: bool,
    pub enabled_by_default: bool,
    pub selectable_exception: bool,
    pub risk: AlgorithmRisk,
    pub risk_message_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmPolicyCatalog {
    pub catalog_version: String,
    pub default_policy_id: String,
    pub entries: Vec<AlgorithmCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmCompatibilityException {
    pub category: AlgorithmCategory,
    /// A catalog-owned stable identifier, never an arbitrary SSH algorithm name.
    pub exception_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmPolicySummary {
    pub host_id: HostId,
    pub revision: WireSequence,
    /// References a policy exposed by the Core algorithm catalog. The catalog itself is not stored.
    pub policy_id: String,
    pub compatibility_exceptions: Vec<AlgorithmCompatibilityException>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmPolicyCatalogGetRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ShellHeartbeatLineEnding {
    None,
    Cr,
    Lf,
    Crlf,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HeartbeatPolicy {
    Disabled,
    TransportKeepalive {
        interval_seconds: u32,
        reply_timeout_seconds: u32,
        failure_threshold: u8,
    },
    ShellHeartbeat {
        payload_text: String,
        line_ending: ShellHeartbeatLineEnding,
        interval_seconds: u32,
        user_idle_seconds: u32,
    },
}

impl fmt::Debug for HeartbeatPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("Disabled"),
            Self::TransportKeepalive {
                interval_seconds,
                reply_timeout_seconds,
                failure_threshold,
            } => formatter
                .debug_struct("TransportKeepalive")
                .field("interval_seconds", interval_seconds)
                .field("reply_timeout_seconds", reply_timeout_seconds)
                .field("failure_threshold", failure_threshold)
                .finish(),
            Self::ShellHeartbeat {
                payload_text,
                line_ending,
                interval_seconds,
                user_idle_seconds,
            } => formatter
                .debug_struct("ShellHeartbeat")
                .field(
                    "payload_text",
                    &format_args!("[REDACTED; {} bytes]", payload_text.len()),
                )
                .field("line_ending", line_ending)
                .field("interval_seconds", interval_seconds)
                .field("user_idle_seconds", user_idle_seconds)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatPolicySummary {
    pub host_id: HostId,
    pub revision: WireSequence,
    pub policy: HeartbeatPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DiskResourceId {
    Root,
}

impl DiskResourceId {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Root => "root",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NetworkResourceId {
    AggregateNonLoopback,
}

impl NetworkResourceId {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AggregateNonLoopback => "aggregateNonLoopback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringPolicy {
    pub enabled: bool,
    pub sample_interval_millis: u32,
    pub sample_timeout_millis: u32,
    /// Stable identifiers selected from a MetricsProvider result, not shell fragments.
    pub disk_mount_ids: Vec<DiskResourceId>,
    /// Stable identifiers selected from a MetricsProvider result, not shell fragments.
    pub network_interface_ids: Vec<NetworkResourceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringPolicySummary {
    pub host_id: HostId,
    pub revision: WireSequence,
    pub policy: MonitoringPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MonitoringPolicyReplaceResponse {
    pub policy: MonitoringPolicySummary,
    /// True only after the Metrics actor accepted the persisted policy and
    /// synchronously completed any required worker shutdown.
    pub runtime_reconciled: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LoginAutomationStepInput {
    Expect {
        literal_text: String,
        timeout_seconds: u8,
    },
    SendText {
        text: String,
        append_enter: bool,
        timeout_seconds: u8,
    },
    SendSecret {
        #[serde(rename = "stagedSecretId")]
        #[ts(rename = "stagedSecretId")]
        secret_ref_id: crate::LoginAutomationSecretStageId,
        secret_label: String,
        append_enter: bool,
        timeout_seconds: u8,
    },
    /// Reuses the opaque SecretRef from one SendSecret step in the exact
    /// expected revision. The public summary never exposes that reference.
    PreserveExistingSecret {
        existing_ordinal: u8,
        append_enter: bool,
        timeout_seconds: u8,
    },
}

impl fmt::Debug for LoginAutomationStepInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expect {
                literal_text,
                timeout_seconds,
            } => formatter
                .debug_struct("Expect")
                .field(
                    "literal_text",
                    &format_args!("[REDACTED; {} bytes]", literal_text.len()),
                )
                .field("timeout_seconds", timeout_seconds)
                .finish(),
            Self::SendText {
                text,
                append_enter,
                timeout_seconds,
            } => formatter
                .debug_struct("SendText")
                .field("text", &format_args!("[REDACTED; {} bytes]", text.len()))
                .field("append_enter", append_enter)
                .field("timeout_seconds", timeout_seconds)
                .finish(),
            Self::SendSecret {
                secret_label,
                append_enter,
                timeout_seconds,
                ..
            } => formatter
                .debug_struct("SendSecret")
                .field("staged_secret_id", &"[REDACTED]")
                .field("secret_label", secret_label)
                .field("append_enter", append_enter)
                .field("timeout_seconds", timeout_seconds)
                .finish(),
            Self::PreserveExistingSecret {
                existing_ordinal,
                append_enter,
                timeout_seconds,
            } => formatter
                .debug_struct("PreserveExistingSecret")
                .field("existing_ordinal", existing_ordinal)
                .field("append_enter", append_enter)
                .field("timeout_seconds", timeout_seconds)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LoginAutomationStepSummary {
    Expect {
        literal_text: String,
        timeout_seconds: u8,
    },
    SendText {
        text: String,
        append_enter: bool,
        timeout_seconds: u8,
    },
    SendSecret {
        secret_label: String,
        append_enter: bool,
        timeout_seconds: u8,
    },
}

impl fmt::Debug for LoginAutomationStepSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expect {
                timeout_seconds, ..
            } => formatter
                .debug_struct("Expect")
                .field("literal_text", &"[REDACTED]")
                .field("timeout_seconds", timeout_seconds)
                .finish(),
            Self::SendText {
                append_enter,
                timeout_seconds,
                ..
            } => formatter
                .debug_struct("SendText")
                .field("text", &"[REDACTED]")
                .field("append_enter", append_enter)
                .field("timeout_seconds", timeout_seconds)
                .finish(),
            Self::SendSecret {
                secret_label,
                append_enter,
                timeout_seconds,
            } => formatter
                .debug_struct("SendSecret")
                .field("secret_label", secret_label)
                .field("append_enter", append_enter)
                .field("timeout_seconds", timeout_seconds)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoginAutomationSummary {
    pub host_id: HostId,
    pub revision: WireSequence,
    pub confirmed_revision: Option<WireSequence>,
    pub enabled: bool,
    pub steps: Vec<LoginAutomationStepSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostConnectionConfigSummary {
    pub route_plan: RoutePlanSummary,
    pub authentication_plan: AuthenticationPlanSummary,
    pub algorithm_policy: AlgorithmPolicySummary,
    pub heartbeat_policy: HeartbeatPolicySummary,
    pub monitoring_policy: MonitoringPolicySummary,
    pub login_automation: LoginAutomationSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostConnectionConfigGetRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
}

macro_rules! host_policy_update_request {
    ($name:ident, $field:ident, $type:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            pub meta: RequestMeta,
            pub host_id: HostId,
            pub expected_revision: WireSequence,
            pub $field: $type,
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RoutePlanReplaceRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_revision: WireSequence,
    pub ingress: RouteIngress,
    pub jump_host_ids: Vec<HostId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationPlanReplaceRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_revision: WireSequence,
    pub mode: AuthenticationPlanMode,
    pub credential_ref_ids: Vec<CredentialRefId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmPolicyReplaceRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_revision: WireSequence,
    pub policy_id: String,
    pub compatibility_exceptions: Vec<AlgorithmCompatibilityException>,
}

host_policy_update_request!(HeartbeatPolicyReplaceRequest, policy, HeartbeatPolicy);
host_policy_update_request!(MonitoringPolicyReplaceRequest, policy, MonitoringPolicy);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoginAutomationReplaceRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_revision: WireSequence,
    pub enabled: bool,
    pub steps: Vec<LoginAutomationStepInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoginAutomationConfirmRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_revision: WireSequence,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoginAutomationSecretCreateRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub host_id: HostId,
    pub expected_automation_revision: WireSequence,
    pub label: String,
    pub value: String,
}

impl fmt::Debug for LoginAutomationSecretCreateRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginAutomationSecretCreateRequest")
            .field("meta", &self.meta)
            .field("operation_id", &self.operation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("host_id", &self.host_id)
            .field(
                "expected_automation_revision",
                &self.expected_automation_revision,
            )
            .field("label", &self.label)
            .field(
                "value",
                &format_args!("[REDACTED; {} bytes]", self.value.len()),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoginAutomationSecretCreateResponse {
    pub staged_secret_id: crate::LoginAutomationSecretStageId,
    pub label: String,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
/// Abandons the original staging operation by its durable request identity, so
/// cleanup remains possible even when the create response was lost.
pub struct LoginAutomationSecretCancelRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LoginAutomationSecretCancelResponse {
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostSummary {
    pub host_id: HostId,
    pub label: String,
    pub address: String,
    pub normalized_address: String,
    pub port: u16,
    pub username: Option<String>,
    pub identity_id: Option<IdentityId>,
    pub favorite: bool,
    #[serde(default)]
    pub has_ready_credential: bool,
    pub state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostGroupSummary {
    pub group_id: HostGroupId,
    pub label: String,
    pub state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostTagSummary {
    pub tag_id: HostTagId,
    pub label: String,
    pub state_version: WireSequence,
}

/// Independently revisioned Host classification. A Host belongs to at most one Group and may
/// reference a bounded set of reusable Tags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostOrganizationSummary {
    pub host_id: HostId,
    pub group_id: Option<HostGroupId>,
    pub tag_ids: Vec<HostTagId>,
    pub host_state_version: WireSequence,
}

/// The latest successful connection fact for one saved Host. Failure details and authentication
/// material never enter this DTO or the application database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecentConnectionSummary {
    pub host_id: HostId,
    #[ts(type = "number")]
    pub connected_at_unix_ms: i64,
    pub recency_sequence: WireSequence,
    pub successful_connection_count: WireSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum HostCatalogSort {
    Label,
    FavoriteThenLabel,
    RecentlyConnected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostCatalogEntry {
    pub host: HostSummary,
    pub group: Option<HostGroupSummary>,
    pub tags: Vec<HostTagSummary>,
    pub recent_connection: Option<RecentConnectionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentitySummary {
    pub identity_id: IdentityId,
    pub label: String,
    pub username: Option<String>,
    pub state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRefSummary {
    pub credential_ref_id: CredentialRefId,
    pub identity_id: IdentityId,
    pub method: AuthenticationMethodKind,
    pub priority: u32,
    pub label: String,
    pub details: CredentialRefDetails,
    pub state_version: WireSequence,
}

/// Public, non-secret metadata for one authentication reference. The tagged details keep
/// method-specific fields explicit and make it impossible to serialize a SecretRef for an Agent
/// credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CredentialRefDetails {
    Password,
    PrivateKey {
        public_key_algorithm: Option<String>,
        public_key_fingerprint: Option<String>,
    },
    KeyboardInteractive {
        max_rounds: u8,
    },
    SshAgent {
        public_key_algorithm: String,
        public_key_fingerprint: String,
        public_key_blob: Vec<u8>,
        scope: SshAgentScope,
    },
    Certificate {
        certificate: Box<SshCertificateMetadata>,
        scope: SshAgentScope,
    },
    HardwareKey {
        public_key_algorithm: String,
        public_key_fingerprint: String,
        public_key_blob: Vec<u8>,
        application: String,
        scope: SshAgentScope,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostSummary {
    pub known_host_id: KnownHostId,
    pub normalized_address: String,
    pub port: u16,
    pub key_algorithm: String,
    pub public_key_base64: String,
    pub fingerprint_sha256: String,
    #[ts(type = "number")]
    pub first_trusted_at_unix_ms: i64,
    #[ts(type = "number")]
    pub last_verified_at_unix_ms: i64,
    pub state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostCatalogListRequest {
    pub meta: RequestMeta,
    pub sort: HostCatalogSort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostCreateRequest {
    pub meta: RequestMeta,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub identity_id: Option<IdentityId>,
    pub favorite: bool,
}

/// Login automation accepted while a Host is first created. Secret-bearing steps deliberately
/// use the separate durable staging lifecycle and cannot be smuggled into this atomic SQLite
/// mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostCreateLoginAutomationStep {
    Expect {
        literal_text: String,
        timeout_seconds: u8,
    },
    SendText {
        text: String,
        append_enter: bool,
        timeout_seconds: u8,
    },
}

/// Creates one saved Host and its complete non-secret configuration as one idempotent database
/// operation. A replay must use the same operation id, idempotency key and normalized payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostConfiguredCreateRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub identity_id: Option<IdentityId>,
    pub favorite: bool,
    pub group_id: Option<HostGroupId>,
    pub tag_ids: Vec<HostTagId>,
    pub ingress: RouteIngress,
    pub jump_host_ids: Vec<HostId>,
    pub authentication_mode: AuthenticationPlanMode,
    pub credential_ref_ids: Vec<CredentialRefId>,
    pub algorithm_policy_id: String,
    pub compatibility_exceptions: Vec<AlgorithmCompatibilityException>,
    pub heartbeat_policy: HeartbeatPolicy,
    pub monitoring_policy: MonitoringPolicy,
    pub login_automation_enabled: bool,
    /// Explicit confirmation of the exact revision-1 automation saved by this operation.
    /// It is invalid when automation is disabled.
    pub login_automation_confirmed: bool,
    pub login_automation_steps: Vec<HostCreateLoginAutomationStep>,
    /// Optional password staged for this exact operation/idempotency pair. When present, Core
    /// creates a dedicated Identity and ready password credential inside the Host transaction.
    pub staged_password_id: Option<crate::HostCreatePasswordStageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostConfiguredCreateResponse {
    pub host: HostSummary,
    pub organization: HostOrganizationSummary,
    pub connection_config: HostConnectionConfigSummary,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostCreatePasswordStageRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub identity_label: String,
    pub credential_label: String,
    pub password: String,
}

impl fmt::Debug for HostCreatePasswordStageRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostCreatePasswordStageRequest")
            .field("meta", &self.meta)
            .field("operation_id", &self.operation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("identity_label", &self.identity_label)
            .field("credential_label", &self.credential_label)
            .field(
                "password",
                &format_args!("[REDACTED; {} bytes]", self.password.len()),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostCreatePasswordStageResponse {
    pub staged_password_id: crate::HostCreatePasswordStageId,
    pub identity_label: String,
    pub credential_label: String,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostCreatePasswordCancelRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostCreatePasswordCancelResponse {
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostUpdateRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_state_version: WireSequence,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub identity_id: Option<IdentityId>,
    pub favorite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostDeleteRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostGroupListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostGroupCreateRequest {
    pub meta: RequestMeta,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostGroupUpdateRequest {
    pub meta: RequestMeta,
    pub group_id: HostGroupId,
    pub expected_state_version: WireSequence,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostGroupDeleteRequest {
    pub meta: RequestMeta,
    pub group_id: HostGroupId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostTagListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostTagCreateRequest {
    pub meta: RequestMeta,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostTagUpdateRequest {
    pub meta: RequestMeta,
    pub tag_id: HostTagId,
    pub expected_state_version: WireSequence,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostTagDeleteRequest {
    pub meta: RequestMeta,
    pub tag_id: HostTagId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostOrganizationGetRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostOrganizationReplaceRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_host_state_version: WireSequence,
    pub group_id: Option<HostGroupId>,
    pub tag_ids: Vec<HostTagId>,
}

/// Explicit favorite mutation so callers do not need to resend the full Host endpoint form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostFavoriteUpdateRequest {
    pub meta: RequestMeta,
    pub host_id: HostId,
    pub expected_state_version: WireSequence,
    pub favorite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecentConnectionListRequest {
    pub meta: RequestMeta,
    /// Bounded by Core; valid values are 1 through 100.
    pub limit: u16,
}

/// Internal Core fact emitted only after a saved Host reaches authenticated connection success.
/// It is exported for typed service boundaries but is not a frontend command request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HostConnectionSucceededRecord {
    pub host_id: HostId,
    #[ts(type = "number")]
    pub connected_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityCreateRequest {
    pub meta: RequestMeta,
    pub label: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityUpdateRequest {
    pub meta: RequestMeta,
    pub identity_id: IdentityId,
    pub expected_state_version: WireSequence,
    pub label: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDeleteImpactRequest {
    pub meta: RequestMeta,
    pub identity_id: IdentityId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDeleteImpact {
    pub identity: IdentitySummary,
    pub referencing_hosts: Vec<HostSummary>,
    pub referencing_host_count: u32,
    pub referencing_credential_refs: Vec<CredentialRefSummary>,
    pub referencing_credential_ref_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDeleteRequest {
    pub meta: RequestMeta,
    pub identity_id: IdentityId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDeleteResponse {
    pub identity_id: IdentityId,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRefListRequest {
    pub meta: RequestMeta,
    pub identity_id: IdentityId,
}

/// Explicitly asks Core to inspect the platform's standard SSH Agent endpoint. Agent discovery is
/// never performed while rendering a Host or resolving an ordinary list view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshAgentKeyListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshAgentKeySummary {
    /// Short-lived, one-use handle. It contains no socket path or public-key bytes.
    pub key_handle: String,
    pub public_key_algorithm: String,
    pub public_key_fingerprint: String,
    pub identity_kind: AgentIdentityKind,
    pub certificate: Option<SshCertificateMetadata>,
    pub hardware_key_application: Option<String>,
    pub comment: Option<String>,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshAgentCredentialCreateRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub identity_id: IdentityId,
    pub key_handle: String,
    pub expected_identity_kind: AgentIdentityKind,
    pub priority: u32,
    pub label: String,
}

/// Creates a non-secret keyboard-interactive credential policy. Individual
/// sensitive answers remain one-time Core values unless a later explicit Vault
/// answer slot is configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardInteractiveCredentialCreateRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub identity_id: IdentityId,
    pub max_rounds: u8,
    pub priority: u32,
    pub label: String,
}

/// Imports authentication material into the encrypted Vault and creates the
/// corresponding non-secret CredentialRef atomically. Public-key metadata is
/// derived by Core from `secret`; callers cannot assert an algorithm or
/// fingerprint.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CredentialImportRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub identity_id: IdentityId,
    pub kind: CredentialKind,
    pub secret: String,
    pub passphrase: Option<String>,
    pub priority: u32,
    pub label: String,
}

impl fmt::Debug for CredentialImportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialImportRequest")
            .field("meta", &self.meta)
            .field("operation_id", &self.operation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("identity_id", &self.identity_id)
            .field("kind", &self.kind)
            .field("secret", &"[REDACTED]")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field("priority", &self.priority)
            .field("label", &self.label)
            .finish()
    }
}

/// Lets Core obtain a user-selected private-key file without exposing the key
/// material or its external path to the WebView. The copied bytes are handled
/// by the same Vault-backed import lifecycle as pasted private keys.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PrivateKeyFileImportRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub identity_id: IdentityId,
    pub passphrase: Option<String>,
    pub priority: u32,
    pub label: String,
}

impl fmt::Debug for PrivateKeyFileImportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivateKeyFileImportRequest")
            .field("meta", &self.meta)
            .field("operation_id", &self.operation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("identity_id", &self.identity_id)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field("priority", &self.priority)
            .field("label", &self.label)
            .finish()
    }
}

/// Prepares authentication material for one direct connection without
/// persisting it to the Vault or the application database. Core returns only
/// an opaque, short-lived reference; the material is consumed after server
/// identity verification and can be used at most once.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TransientCredentialPrepareRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub kind: CredentialKind,
    pub secret: String,
    pub passphrase: Option<String>,
}

impl fmt::Debug for TransientCredentialPrepareRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransientCredentialPrepareRequest")
            .field("meta", &self.meta)
            .field("operation_id", &self.operation_id)
            .field("idempotency_key", &self.idempotency_key)
            .field("kind", &self.kind)
            .field("secret", &"[REDACTED]")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TransientCredentialRef {
    pub credential_ref_id: CredentialRefId,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostListRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostDeleteRequest {
    pub meta: RequestMeta,
    pub known_host_id: KnownHostId,
    pub expected_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostDeleteResponse {
    pub known_host_id: KnownHostId,
    pub deleted: bool,
    pub deleted_known_host: Option<KnownHostSummary>,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentIdentitySource, AuthenticationMethodKind, CredentialImportRequest, CredentialKind,
        CredentialRefDetails, CredentialRefSummary, HeartbeatPolicy, HostCatalogSort, HostGroupId,
        HostOrganizationReplaceRequest, HostTagId, LoginAutomationSecretCancelRequest,
        LoginAutomationSecretCreateRequest, LoginAutomationSecretCreateResponse,
        LoginAutomationStepInput, LoginAutomationStepSummary, LoginAutomationSummary,
        PrivateKeyFileImportRequest, SshAgentScope, SshCertificateCriticalOption,
        SshCertificateExtension, SshCertificateMetadata, SshCertificateType,
        TransientCredentialPrepareRequest,
    };
    use crate::{
        CredentialRefId, HostId, IdentityId, OperationId, RequestId, RequestMeta, WireSequence,
    };

    #[test]
    fn credential_summary_does_not_serialize_a_secret_reference() {
        let summary = CredentialRefSummary {
            credential_ref_id: CredentialRefId::new(),
            identity_id: IdentityId::new(),
            method: AuthenticationMethodKind::PrivateKey,
            priority: 1,
            label: "deploy key".to_owned(),
            details: CredentialRefDetails::PrivateKey {
                public_key_algorithm: Some("ssh-ed25519".to_owned()),
                public_key_fingerprint: Some("SHA256:public".to_owned()),
            },
            state_version: WireSequence::new(2),
        };

        let encoded = serde_json::to_string(&summary).expect("serialize credential summary");
        assert!(!encoded.contains("secretRef"));
    }

    #[test]
    fn certificate_summary_preserves_public_safety_metadata_without_secrets() {
        let metadata = SshCertificateMetadata {
            source: AgentIdentitySource::SystemSshAgent,
            certificate_blob: b"certificate-blob".to_vec(),
            certificate_algorithm: "ssh-ed25519-cert-v01@openssh.com".to_owned(),
            certificate_fingerprint: "SHA256:certificate".to_owned(),
            serial: "18446744073709551615".to_owned(),
            subject_public_key_blob: b"subject-key-blob".to_vec(),
            subject_public_key_algorithm: "ssh-ed25519".to_owned(),
            subject_public_key_fingerprint: "SHA256:subject".to_owned(),
            ca_public_key_fingerprint: "SHA256:ca".to_owned(),
            key_id: "deploy@example".to_owned(),
            valid_principals: vec!["deploy".to_owned()],
            certificate_type: SshCertificateType::User,
            valid_after_unix_seconds: 1_700_000_000,
            valid_before_unix_seconds: Some(1_800_000_000),
            critical_options: vec![SshCertificateCriticalOption {
                name: "future-policy".to_owned(),
                value: vec![1, 2, 3],
                recognized: false,
            }],
            extensions: vec![SshCertificateExtension {
                name: "no-touch-required".to_owned(),
                value: Vec::new(),
                recognized: true,
            }],
        };
        assert!(metadata.has_unknown_critical_options());
        let summary = CredentialRefSummary {
            credential_ref_id: CredentialRefId::new(),
            identity_id: IdentityId::new(),
            method: AuthenticationMethodKind::Certificate,
            priority: 0,
            label: "Agent certificate".to_owned(),
            details: CredentialRefDetails::Certificate {
                certificate: Box::new(metadata),
                scope: SshAgentScope::DefaultEnvironment,
            },
            state_version: WireSequence::new(1),
        };

        let encoded = serde_json::to_string(&summary).expect("serialize certificate summary");
        assert!(encoded.contains("\"method\":\"certificate\""));
        assert!(encoded.contains("\"serial\":\"18446744073709551615\""));
        assert!(encoded.contains("future-policy"));
        assert!(encoded.contains("no-touch-required"));
        assert!(!encoded.contains("secretRef"));
        assert!(!encoded.contains("socket"));
        assert!(!encoded.contains("comment"));
    }

    #[test]
    fn credential_import_debug_redacts_secret_material() {
        let request = CredentialImportRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "credential-import-once".to_owned(),
            identity_id: IdentityId::new(),
            kind: CredentialKind::PrivateKey,
            secret: "private-key-material".to_owned(),
            passphrase: Some("key-passphrase".to_owned()),
            priority: 10,
            label: "deploy key".to_owned(),
        };

        let debug = format!("{request:?}");
        assert!(!debug.contains("private-key-material"));
        assert!(!debug.contains("key-passphrase"));
        assert_eq!(debug.matches("[REDACTED]").count(), 2);

        let encoded = serde_json::to_value(&request).expect("serialize import request");
        assert!(encoded.get("secretRefId").is_none());
        assert!(encoded.get("publicKeyAlgorithm").is_none());
        assert!(encoded.get("publicKeyFingerprint").is_none());
    }

    #[test]
    fn private_key_file_import_request_hides_path_and_passphrase() {
        let request = PrivateKeyFileImportRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "private-key-file-import-once".to_owned(),
            identity_id: IdentityId::new(),
            passphrase: Some("key-passphrase".to_owned()),
            priority: 10,
            label: "AWS deploy key".to_owned(),
        };

        let debug = format!("{request:?}");
        assert!(!debug.contains("key-passphrase"));
        assert!(debug.contains("[REDACTED]"));

        let encoded = serde_json::to_value(&request).expect("serialize file import request");
        assert!(encoded.get("path").is_none());
        assert!(encoded.get("secret").is_none());
        assert!(encoded.get("secretRefId").is_none());
    }

    #[test]
    fn host_create_password_stage_debug_and_response_hide_secret_refs() {
        let request = super::HostCreatePasswordStageRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "configured-host-password-once".to_owned(),
            identity_label: "Dedicated identity".to_owned(),
            credential_label: "Password".to_owned(),
            password: "plaintext password".to_owned(),
        };
        assert!(!format!("{request:?}").contains("plaintext password"));
        let response = super::HostCreatePasswordStageResponse {
            staged_password_id: crate::HostCreatePasswordStageId::new(),
            identity_label: "Dedicated identity".to_owned(),
            credential_label: "Password".to_owned(),
            expires_at_unix_ms: 123,
        };
        let encoded = serde_json::to_string(&response).expect("serialize response");
        assert!(encoded.contains("stagedPasswordId"));
        assert!(!encoded.contains("secretRef"));
    }

    #[test]
    fn transient_credential_debug_redacts_secret_material() {
        let request = TransientCredentialPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "transient-credential-once".to_owned(),
            kind: CredentialKind::PrivateKey,
            secret: "private-key-material".to_owned(),
            passphrase: Some("key-passphrase".to_owned()),
        };

        let rendered = format!("{request:?}");
        assert!(!rendered.contains("private-key-material"));
        assert!(!rendered.contains("key-passphrase"));
        assert_eq!(rendered.matches("[REDACTED]").count(), 2);
    }

    #[test]
    fn automation_summary_omits_secret_refs_and_policy_debug_redacts_payloads() {
        let secret_ref_id = crate::LoginAutomationSecretStageId::new();
        let input = LoginAutomationStepInput::SendSecret {
            secret_ref_id: secret_ref_id.clone(),
            secret_label: "Escalation password".to_owned(),
            append_enter: true,
            timeout_seconds: 10,
        };
        let input_debug = format!("{input:?}");
        assert!(!input_debug.contains(secret_ref_id.as_str()));
        let input_json = serde_json::to_string(&input).expect("serialize automation input");
        assert!(input_json.contains("stagedSecretId"));
        assert!(!input_json.contains("secretRefId"));

        let create_request = LoginAutomationSecretCreateRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "stage-secret-once".to_owned(),
            host_id: HostId::new(),
            expected_automation_revision: WireSequence::new(1),
            label: "Escalation password".to_owned(),
            value: "plaintext secret".to_owned(),
        };
        assert!(!format!("{create_request:?}").contains("plaintext secret"));
        let response_json = serde_json::to_string(&LoginAutomationSecretCreateResponse {
            staged_secret_id: secret_ref_id.clone(),
            label: "Escalation password".to_owned(),
            expires_at_unix_ms: 123,
        })
        .expect("serialize stage response");
        let cancel_json = serde_json::to_string(&LoginAutomationSecretCancelRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "stage-secret-once".to_owned(),
        })
        .expect("serialize cancel request");
        assert!(response_json.contains("stagedSecretId"));
        assert!(cancel_json.contains("operationId"));
        assert!(cancel_json.contains("idempotencyKey"));
        assert!(!cancel_json.contains("stagedSecretId"));
        assert!(!response_json.contains("secretRefId"));
        assert!(!cancel_json.contains("secretRefId"));

        let summary = LoginAutomationSummary {
            host_id: HostId::new(),
            revision: WireSequence::new(2),
            confirmed_revision: Some(WireSequence::new(2)),
            enabled: true,
            steps: vec![
                LoginAutomationStepSummary::SendText {
                    text: "enable".to_owned(),
                    append_enter: true,
                    timeout_seconds: 5,
                },
                LoginAutomationStepSummary::SendSecret {
                    secret_label: "Escalation password".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                },
            ],
        };
        let encoded = serde_json::to_string(&summary).expect("serialize automation summary");
        assert!(!encoded.contains("secretRef"));
        assert!(!format!("{summary:?}").contains("text: \"enable\""));

        let heartbeat = HeartbeatPolicy::ShellHeartbeat {
            payload_text: "sensitive-looking plain text".to_owned(),
            line_ending: super::ShellHeartbeatLineEnding::Cr,
            interval_seconds: 30,
            user_idle_seconds: 5,
        };
        assert!(!format!("{heartbeat:?}").contains("sensitive-looking"));
    }

    #[test]
    fn host_metadata_wire_contract_is_camel_case_and_uses_string_ids() {
        let group_id = HostGroupId::new();
        let tag_id = HostTagId::new();
        let request = HostOrganizationReplaceRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            host_id: HostId::new(),
            expected_host_state_version: WireSequence::new(4),
            group_id: Some(group_id.clone()),
            tag_ids: vec![tag_id.clone()],
        };
        let encoded = serde_json::to_value(request).expect("serialize host metadata");
        assert_eq!(encoded["groupId"], group_id.as_str());
        assert_eq!(encoded["tagIds"][0], tag_id.as_str());
        assert_eq!(encoded["expectedHostStateVersion"], "4");
        assert_eq!(
            serde_json::to_string(&HostCatalogSort::RecentlyConnected)
                .expect("serialize catalog sort"),
            "\"recentlyConnected\""
        );
    }

    #[test]
    fn monitoring_resource_ids_reject_unknown_wire_values() {
        let valid = serde_json::json!({
            "enabled": true,
            "sampleIntervalMillis": 1500,
            "sampleTimeoutMillis": 5000,
            "diskMountIds": ["root"],
            "networkInterfaceIds": ["aggregateNonLoopback"]
        });
        let policy: super::MonitoringPolicy =
            serde_json::from_value(valid.clone()).expect("canonical monitoring policy");
        assert_eq!(policy.disk_mount_ids, [super::DiskResourceId::Root]);
        let mut unknown = valid;
        unknown["diskMountIds"] = serde_json::json!(["/"]);
        assert!(serde_json::from_value::<super::MonitoringPolicy>(unknown).is_err());
    }
}
