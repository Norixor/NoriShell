use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
    net::IpAddr,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::{Result, SyncCodecError},
    secret::SecretBytes,
};

pub const BUNDLE_SCHEMA: &str = "norishell-ssh-profile-bundle-v1";
pub const BUNDLE_SCHEMA_V2: &str = "norishell-ssh-profile-bundle-v2";
pub const BUNDLE_SCHEMA_V3: &str = "norishell-ssh-profile-bundle-v3";
pub const BUNDLE_SCHEMA_V4: &str = "norishell-ssh-profile-bundle-v4";
pub const BUNDLE_SCHEMA_V5: &str = "norishell-ssh-profile-bundle-v5";
const MAX_OBJECTS_PER_KIND: usize = 10_000;
const MAX_SMALL_TEXT_BYTES: usize = 1_024;
const MAX_LARGE_TEXT_BYTES: usize = 64 * 1024;
const MAX_COLLECTION_ITEMS: usize = 1_024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BundleSchema {
    #[serde(rename = "norishell-ssh-profile-bundle-v1")]
    V1,
    #[serde(rename = "norishell-ssh-profile-bundle-v2")]
    V2,
    #[serde(rename = "norishell-ssh-profile-bundle-v3")]
    V3,
    #[serde(rename = "norishell-ssh-profile-bundle-v4")]
    V4,
    #[serde(rename = "norishell-ssh-profile-bundle-v5")]
    V5,
}

impl fmt::Debug for BundleSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::V1 => BUNDLE_SCHEMA,
            Self::V2 => BUNDLE_SCHEMA_V2,
            Self::V3 => BUNDLE_SCHEMA_V3,
            Self::V4 => BUNDLE_SCHEMA_V4,
            Self::V5 => BUNDLE_SCHEMA_V5,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PortableObjectId(Uuid);

impl PortableObjectId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn from_uuid(value: Uuid) -> Result<Self> {
        if value.is_nil() {
            return Err(SyncCodecError::InvalidBundle(
                "portable object ID must not be nil",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for PortableObjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PortableObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableBundleV1 {
    pub schema: BundleSchema,
    pub revision: u64,
    pub objects: PortableObjects,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferences: Option<PortablePreferencesV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<PortableSecret>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_machine_bound: Vec<SkippedMachineBoundObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tombstones: Vec<PortableTombstone>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub update_times: Vec<PortableItemUpdateTime>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub preference_update_times: BTreeMap<String, i64>,
}

/// Authenticated per-item wall-clock time in the encrypted portable bundle.
/// A missing entry means unknown, never the time at which a snapshot was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableItemUpdateTime {
    pub kind: PortableObjectKind,
    pub id: PortableObjectId,
    pub update_time_unix_ms: i64,
}

/// A bounded copy of the eight host-validated global preference groups. The
/// frontend adapters own each group's semantic validation and CAS application;
/// this type also rejects unknown groups and unexpected top-level fields before
/// the values enter an encrypted exchange or a Core-private pending restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePreferencesV1 {
    pub product: String,
    pub version: u16,
    pub groups: BTreeMap<String, serde_json::Value>,
}

impl PortablePreferencesV1 {
    pub fn validate(&self) -> Result<()> {
        const GROUPS: [(&str, &[&str]); 8] = [
            (
                "application",
                &[
                    "themePreference",
                    "locale",
                    "uiZoom",
                    "terminalStartupBehavior",
                    "newTerminalBehavior",
                    "singlePaneTabCloseBehavior",
                ],
            ),
            (
                "appearance",
                &[
                    "terminalThemeMode",
                    "terminalFontFamily",
                    "terminalFontSize",
                    "terminalFontWeight",
                    "terminalBoldFontWeight",
                    "terminalLineHeight",
                    "terminalLetterSpacing",
                    "terminalCursorStyle",
                    "terminalCursorBlink",
                    "customTerminalPalette",
                    "customTerminalPaletteName",
                ],
            ),
            ("interaction", &["interaction", "pasteWarning"]),
            ("highlights", &["enabled", "rules"]),
            ("shortcuts", &["version", "bindings"]),
            ("files", &["browser", "rememberLastDirectory"]),
            (
                "desktop",
                &[
                    "windowCloseBehavior",
                    "trayShowStatus",
                    "trayRecentLimit",
                    "trayShowHostNames",
                    "notificationBackgroundOnly",
                    "notificationFailureOnly",
                    "notifyTransferCompleted",
                    "notifyTransferFailed",
                    "notifyDisconnected",
                ],
            ),
            (
                "commandNotifications",
                &["notificationsEnabled", "notificationThresholdSeconds"],
            ),
        ];
        if self.product != "NoriShell" || self.version != 1 || self.groups.len() != GROUPS.len() {
            return Err(SyncCodecError::InvalidBundle(
                "invalid preferences transfer header",
            ));
        }
        let encoded = serde_json::to_vec(self)?;
        if encoded.len() > 256 * 1024 {
            return Err(SyncCodecError::BoundExceeded("preferences"));
        }
        for (group, keys) in GROUPS {
            let Some(value) = self
                .groups
                .get(group)
                .and_then(serde_json::Value::as_object)
            else {
                return Err(SyncCodecError::InvalidBundle("missing preferences group"));
            };
            let optional_app_theme = group == "appearance" && value.contains_key("appTheme");
            if value.len() != keys.len() + usize::from(optional_app_theme)
                || value.keys().any(|key| {
                    !keys.contains(&key.as_str()) && !(group == "appearance" && key == "appTheme")
                })
            {
                return Err(SyncCodecError::InvalidBundle(
                    "invalid preferences group shape",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortableObjectKind {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableTombstone {
    pub kind: PortableObjectKind,
    pub id: PortableObjectId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableObjects {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<PortableHost>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desktop_profiles: Vec<PortableDesktopProfile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<PortableIdentity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credentials: Vec<PortableCredential>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<PortableRoute>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authentication_plans: Vec<PortableAuthenticationPlan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub algorithm_policies: Vec<PortableAlgorithmPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heartbeat_policies: Vec<PortableHeartbeatPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitoring_policies: Vec<PortableMonitoringPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub login_automations: Vec<PortableLoginAutomation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableHost {
    pub id: PortableObjectId,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    pub identity_id: Option<PortableObjectId>,
    pub route_id: PortableObjectId,
    pub authentication_plan_id: PortableObjectId,
    pub algorithm_policy_id: PortableObjectId,
    pub heartbeat_policy_id: PortableObjectId,
    pub monitoring_policy_id: PortableObjectId,
    pub login_automation_id: Option<PortableObjectId>,
}

/// Portable, non-secret remote desktop configuration. Local profile revisions,
/// local IDs, trust decisions, and runtime/session state are intentionally not
/// representable here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableDesktopProfile {
    pub id: PortableObjectId,
    pub label: String,
    pub protocol: PortableDesktopProtocol,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub host_id: Option<PortableObjectId>,
    pub gateway_host_id: Option<PortableObjectId>,
    pub credential_id: Option<PortableObjectId>,
    pub width: u16,
    pub height: u16,
    pub clipboard_enabled: bool,
    pub audio_playback_enabled: bool,
    #[serde(default, skip_serializing_if = "PortableVncProtocolVersion::is_auto")]
    pub vnc_protocol_version: PortableVncProtocolVersion,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortableVncProtocolVersion {
    #[default]
    Auto,
    Rfb33,
    Rfb37,
    Rfb38,
}

impl PortableVncProtocolVersion {
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortableDesktopProtocol {
    Rdp,
    Vnc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableIdentity {
    pub id: PortableObjectId,
    pub label: String,
    pub username: Option<String>,
    pub credential_ids: Vec<PortableObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableCredential {
    pub id: PortableObjectId,
    pub identity_id: PortableObjectId,
    pub label: String,
    pub material: PortableCredentialMaterial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PortableCredentialMaterial {
    Password {
        password_secret_id: PortableObjectId,
    },
    PrivateKey {
        algorithm: PublicKeyAlgorithm,
        public_key_fingerprint: String,
        private_key_secret_id: PortableObjectId,
        passphrase_secret_id: Option<PortableObjectId>,
    },
    Certificate {
        algorithm: PublicKeyAlgorithm,
        public_key_fingerprint: String,
        certificate_secret_id: PortableObjectId,
        private_key_secret_id: Option<PortableObjectId>,
        passphrase_secret_id: Option<PortableObjectId>,
    },
    KeyboardInteractive {
        max_rounds: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicKeyAlgorithm {
    #[serde(rename = "ssh-ed25519")]
    Ed25519,
    #[serde(rename = "rsa-sha2-256")]
    RsaSha256,
    #[serde(rename = "rsa-sha2-512")]
    RsaSha512,
    #[serde(rename = "ecdsa-sha2-nistp256")]
    EcdsaP256,
    #[serde(rename = "ecdsa-sha2-nistp384")]
    EcdsaP384,
    #[serde(rename = "ecdsa-sha2-nistp521")]
    EcdsaP521,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableRoute {
    pub id: PortableObjectId,
    pub ingress: RouteIngress,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jump_hops: Vec<JumpHop>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RouteIngress {
    Direct,
    HttpConnect {
        host: String,
        port: u16,
        username: Option<String>,
        credential_id: Option<PortableObjectId>,
    },
    Socks5 {
        host: String,
        port: u16,
        username: Option<String>,
        credential_id: Option<PortableObjectId>,
        resolve_dns_remotely: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct JumpHop {
    pub host_id: PortableObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableAuthenticationPlan {
    pub id: PortableObjectId,
    pub credential_ids: Vec<PortableObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableAlgorithmPolicy {
    pub id: PortableObjectId,
    pub policy_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compatibility_exceptions: Vec<PortableAlgorithmCompatibilityException>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableAlgorithmCompatibilityException {
    pub category: PortableAlgorithmCategory,
    pub exception_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortableAlgorithmCategory {
    KeyExchange,
    HostKey,
    Cipher,
    Mac,
}

macro_rules! algorithm_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $wire)] $variant),+ }
    };
}

algorithm_enum!(KeyExchangeAlgorithm {
    Curve25519Sha256 => "curve25519-sha256",
    EcdhSha2Nistp256 => "ecdh-sha2-nistp256",
    EcdhSha2Nistp384 => "ecdh-sha2-nistp384",
    EcdhSha2Nistp521 => "ecdh-sha2-nistp521",
    DiffieHellmanGroup16Sha512 => "diffie-hellman-group16-sha512",
    DiffieHellmanGroup14Sha256 => "diffie-hellman-group14-sha256",
});
algorithm_enum!(HostKeyAlgorithm {
    SshEd25519 => "ssh-ed25519",
    RsaSha2512 => "rsa-sha2-512",
    RsaSha2256 => "rsa-sha2-256",
    EcdsaSha2Nistp256 => "ecdsa-sha2-nistp256",
    EcdsaSha2Nistp384 => "ecdsa-sha2-nistp384",
    EcdsaSha2Nistp521 => "ecdsa-sha2-nistp521",
});
algorithm_enum!(CipherAlgorithm {
    Chacha20Poly1305 => "chacha20-poly1305@openssh.com",
    Aes256Gcm => "aes256-gcm@openssh.com",
    Aes128Gcm => "aes128-gcm@openssh.com",
    Aes256Ctr => "aes256-ctr",
    Aes192Ctr => "aes192-ctr",
    Aes128Ctr => "aes128-ctr",
});
algorithm_enum!(MacAlgorithm {
    HmacSha2512Etm => "hmac-sha2-512-etm@openssh.com",
    HmacSha2256Etm => "hmac-sha2-256-etm@openssh.com",
    HmacSha2512 => "hmac-sha2-512",
    HmacSha2256 => "hmac-sha2-256",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableHeartbeatPolicy {
    pub id: PortableObjectId,
    pub mode: HeartbeatMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum HeartbeatMode {
    Disabled,
    TransportKeepalive {
        interval_seconds: u32,
        reply_timeout_seconds: u32,
        max_missed_replies: u8,
    },
    ShellHeartbeat {
        interval_seconds: u32,
        idle_seconds: u32,
        payload: String,
        line_ending: PortableShellHeartbeatLineEnding,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortableShellHeartbeatLineEnding {
    None,
    Cr,
    Lf,
    Crlf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableMonitoringPolicy {
    pub id: PortableObjectId,
    pub enabled: bool,
    pub interval_seconds: u32,
    pub timeout_seconds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_millis: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_millis: Option<u32>,
    pub resources: BTreeSet<MonitoringResource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MonitoringResource {
    Cpu,
    Memory,
    RootDisk,
    AggregateNonLoopbackNetwork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableLoginAutomation {
    pub id: PortableObjectId,
    pub steps: Vec<LoginAutomationStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LoginAutomationStep {
    Expect {
        pattern: String,
        timeout_seconds: u32,
    },
    SendText {
        text: String,
        append_enter: bool,
        timeout_seconds: u32,
    },
    SendSecret {
        secret_id: PortableObjectId,
        secret_label: String,
        append_enter: bool,
        timeout_seconds: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortableSecretKind {
    Password,
    PrivateKey,
    Passphrase,
    Certificate,
    LoginAutomation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortableSecret {
    pub id: PortableObjectId,
    pub kind: PortableSecretKind,
    pub selected_by_user: bool,
    pub payload: SecretBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MachineBoundKind {
    SshAgent,
    Fido,
    HardwareKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MachineBoundSkipReason {
    MachineBound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SkippedMachineBoundObject {
    pub id: PortableObjectId,
    pub kind: MachineBoundKind,
    pub reason: MachineBoundSkipReason,
}

impl PortableBundleV1 {
    pub fn validate(&self) -> Result<()> {
        if self.revision == 0 {
            return Err(SyncCodecError::InvalidBundle("revision must be positive"));
        }
        validate_count(self.secrets.len(), "secrets")?;
        validate_count(self.skipped_machine_bound.len(), "skipped objects")?;
        validate_count(self.tombstones.len(), "tombstones")?;
        if self.schema == BundleSchema::V1 && !self.tombstones.is_empty() {
            return Err(SyncCodecError::InvalidBundle(
                "bundle v1 cannot contain tombstones",
            ));
        }
        if !matches!(
            self.schema,
            BundleSchema::V3 | BundleSchema::V4 | BundleSchema::V5
        ) && !self.objects.desktop_profiles.is_empty()
        {
            return Err(SyncCodecError::InvalidBundle(
                "desktop profiles require bundle v3",
            ));
        }
        if !matches!(
            self.schema,
            BundleSchema::V3 | BundleSchema::V4 | BundleSchema::V5
        ) && self
            .tombstones
            .iter()
            .any(|item| item.kind == PortableObjectKind::DesktopProfile)
        {
            return Err(SyncCodecError::InvalidBundle(
                "desktop profile tombstones require bundle v3",
            ));
        }
        match (self.schema, &self.preferences) {
            (BundleSchema::V4 | BundleSchema::V5, Some(preferences)) => preferences.validate()?,
            (BundleSchema::V4 | BundleSchema::V5, None) => {
                return Err(SyncCodecError::InvalidBundle(
                    "bundle v4/v5 requires preferences",
                ));
            }
            (_, Some(_)) => {
                return Err(SyncCodecError::InvalidBundle(
                    "preferences require bundle v4",
                ));
            }
            (_, None) => {}
        }
        self.objects.validate()?;

        if self.schema != BundleSchema::V5
            && (!self.update_times.is_empty() || !self.preference_update_times.is_empty())
        {
            return Err(SyncCodecError::InvalidBundle(
                "update times require bundle v5",
            ));
        }
        if self.update_times.len() > 12 * MAX_OBJECTS_PER_KIND {
            return Err(SyncCodecError::BoundExceeded("update times"));
        }
        let present = self
            .objects
            .all_nodes()
            .chain(
                self.secrets
                    .iter()
                    .map(|item| (PortableObjectKind::Secret, item.id)),
            )
            .chain(self.tombstones.iter().map(|item| (item.kind, item.id)))
            .collect::<HashSet<_>>();
        let mut timed = HashSet::new();
        for item in &self.update_times {
            if item.update_time_unix_ms <= 0
                || !present.contains(&(item.kind, item.id))
                || !timed.insert((item.kind, item.id))
            {
                return Err(SyncCodecError::InvalidBundle("invalid item update time"));
            }
        }
        for (group, time) in &self.preference_update_times {
            if *time <= 0
                || !self
                    .preferences
                    .as_ref()
                    .is_some_and(|value| value.groups.contains_key(group))
            {
                return Err(SyncCodecError::InvalidBundle(
                    "invalid preference update time",
                ));
            }
        }

        let mut ids = HashSet::new();
        for id in self.all_ids() {
            if id.as_uuid().is_nil() {
                return Err(SyncCodecError::InvalidBundle(
                    "portable object ID must not be nil",
                ));
            }
            if !ids.insert(id) {
                return Err(SyncCodecError::DuplicateObjectId);
            }
        }

        for secret in &self.secrets {
            if !secret.selected_by_user {
                return Err(SyncCodecError::InvalidBundle(
                    "secret payload was not explicitly selected by the user",
                ));
            }
        }
        self.validate_references()
    }

    pub(crate) fn canonicalized(&self) -> Result<Self> {
        self.validate()?;
        let mut value = self.clone();
        value.objects.sort_by_id();
        value.secrets.sort_by_key(|item| item.id);
        value.skipped_machine_bound.sort_by_key(|item| item.id);
        value.tombstones.sort_by_key(|item| (item.kind, item.id));
        value.update_times.sort_by_key(|item| (item.kind, item.id));
        Ok(value)
    }

    fn all_ids(&self) -> impl Iterator<Item = PortableObjectId> + '_ {
        self.objects
            .all_ids()
            .chain(self.secrets.iter().map(|item| item.id))
            .chain(self.skipped_machine_bound.iter().map(|item| item.id))
            .chain(self.tombstones.iter().map(|item| item.id))
    }

    fn validate_references(&self) -> Result<()> {
        let hosts = id_set(&self.objects.hosts, |item| item.id);
        let identities = id_set(&self.objects.identities, |item| item.id);
        let credentials = id_set(&self.objects.credentials, |item| item.id);
        let credentials_by_id = self
            .objects
            .credentials
            .iter()
            .map(|item| (item.id, item))
            .collect::<std::collections::HashMap<_, _>>();
        let identity_credential_ids = self
            .objects
            .identities
            .iter()
            .map(|item| (item.id, &item.credential_ids))
            .collect::<std::collections::HashMap<_, _>>();
        let routes = id_set(&self.objects.routes, |item| item.id);
        let authentication_plans = id_set(&self.objects.authentication_plans, |item| item.id);
        let algorithm_policies = id_set(&self.objects.algorithm_policies, |item| item.id);
        let heartbeat_policies = id_set(&self.objects.heartbeat_policies, |item| item.id);
        let monitoring_policies = id_set(&self.objects.monitoring_policies, |item| item.id);
        let login_automations = id_set(&self.objects.login_automations, |item| item.id);
        let secrets = self
            .secrets
            .iter()
            .map(|item| (item.id, item.kind))
            .collect::<std::collections::HashMap<_, _>>();

        for host in &self.objects.hosts {
            if !routes.contains(&host.route_id)
                || !authentication_plans.contains(&host.authentication_plan_id)
                || !algorithm_policies.contains(&host.algorithm_policy_id)
                || !heartbeat_policies.contains(&host.heartbeat_policy_id)
                || !monitoring_policies.contains(&host.monitoring_policy_id)
                || host.identity_id.is_some_and(|id| !identities.contains(&id))
                || host
                    .login_automation_id
                    .is_some_and(|id| !login_automations.contains(&id))
            {
                return Err(SyncCodecError::DanglingObjectReference);
            }
        }
        for desktop in &self.objects.desktop_profiles {
            if desktop.host_id.is_some_and(|id| !hosts.contains(&id))
                || desktop
                    .gateway_host_id
                    .is_some_and(|id| !hosts.contains(&id))
            {
                return Err(SyncCodecError::DanglingObjectReference);
            }
            if let Some(credential_id) = desktop.credential_id {
                let Some(credential) = credentials_by_id.get(&credential_id) else {
                    return Err(SyncCodecError::DanglingObjectReference);
                };
                if !matches!(
                    &credential.material,
                    PortableCredentialMaterial::Password { .. }
                ) || !identity_credential_ids
                    .get(&credential.identity_id)
                    .is_some_and(|credential_ids| credential_ids.contains(&credential_id))
                {
                    return Err(SyncCodecError::DanglingObjectReference);
                }
            }
        }
        for identity in &self.objects.identities {
            if identity
                .credential_ids
                .iter()
                .any(|id| !credentials.contains(id))
            {
                return Err(SyncCodecError::DanglingObjectReference);
            }
        }
        for credential in &self.objects.credentials {
            if !identities.contains(&credential.identity_id) {
                return Err(SyncCodecError::DanglingObjectReference);
            }
            let refs: Vec<(PortableObjectId, PortableSecretKind)> = match credential.material {
                PortableCredentialMaterial::Password { password_secret_id } => {
                    vec![(password_secret_id, PortableSecretKind::Password)]
                }
                PortableCredentialMaterial::PrivateKey {
                    private_key_secret_id,
                    passphrase_secret_id,
                    ..
                } => [
                    Some((private_key_secret_id, PortableSecretKind::PrivateKey)),
                    passphrase_secret_id.map(|id| (id, PortableSecretKind::Passphrase)),
                ]
                .into_iter()
                .flatten()
                .collect(),
                PortableCredentialMaterial::Certificate {
                    certificate_secret_id,
                    private_key_secret_id,
                    passphrase_secret_id,
                    ..
                } => [
                    Some((certificate_secret_id, PortableSecretKind::Certificate)),
                    private_key_secret_id.map(|id| (id, PortableSecretKind::PrivateKey)),
                    passphrase_secret_id.map(|id| (id, PortableSecretKind::Passphrase)),
                ]
                .into_iter()
                .flatten()
                .collect(),
                PortableCredentialMaterial::KeyboardInteractive { .. } => Vec::new(),
            };
            if refs
                .iter()
                .any(|(id, expected)| secrets.get(id) != Some(expected))
            {
                return Err(SyncCodecError::DanglingObjectReference);
            }
        }
        for route in &self.objects.routes {
            for hop in &route.jump_hops {
                if !hosts.contains(&hop.host_id) {
                    return Err(SyncCodecError::DanglingObjectReference);
                }
            }
            let credential_id = match route.ingress {
                RouteIngress::Direct => None,
                RouteIngress::HttpConnect { credential_id, .. }
                | RouteIngress::Socks5 { credential_id, .. } => credential_id,
            };
            if credential_id.is_some_and(|id| {
                !matches!(
                    credentials_by_id
                        .get(&id)
                        .map(|credential| &credential.material),
                    Some(PortableCredentialMaterial::Password { .. })
                )
            }) {
                return Err(SyncCodecError::DanglingObjectReference);
            }
        }
        for plan in &self.objects.authentication_plans {
            if plan
                .credential_ids
                .iter()
                .any(|id| !credentials.contains(id))
            {
                return Err(SyncCodecError::DanglingObjectReference);
            }
        }
        for automation in &self.objects.login_automations {
            for step in &automation.steps {
                if let LoginAutomationStep::SendSecret { secret_id, .. } = step
                    && secrets.get(secret_id) != Some(&PortableSecretKind::LoginAutomation)
                {
                    return Err(SyncCodecError::DanglingObjectReference);
                }
            }
        }
        Ok(())
    }
}

fn id_set<T>(items: &[T], id: impl Fn(&T) -> PortableObjectId) -> HashSet<PortableObjectId> {
    items.iter().map(id).collect()
}

impl PortableObjects {
    fn validate(&self) -> Result<()> {
        for (count, name) in [
            (self.hosts.len(), "hosts"),
            (self.desktop_profiles.len(), "desktop profiles"),
            (self.identities.len(), "identities"),
            (self.credentials.len(), "credentials"),
            (self.routes.len(), "routes"),
            (self.authentication_plans.len(), "authentication plans"),
            (self.algorithm_policies.len(), "algorithm policies"),
            (self.heartbeat_policies.len(), "heartbeat policies"),
            (self.monitoring_policies.len(), "monitoring policies"),
            (self.login_automations.len(), "login automations"),
        ] {
            validate_count(count, name)?;
        }
        for host in &self.hosts {
            validate_text(&host.label, MAX_SMALL_TEXT_BYTES, "host label")?;
            validate_text(&host.address, MAX_SMALL_TEXT_BYTES, "host address")?;
            if host.port == 0 {
                return Err(SyncCodecError::InvalidBundle("host port must be positive"));
            }
            validate_optional_text(host.username.as_deref(), "username")?;
            validate_collection(host.tags.len(), "host tags")?;
            for tag in &host.tags {
                validate_text(tag, MAX_SMALL_TEXT_BYTES, "host tag")?;
            }
        }
        for desktop in &self.desktop_profiles {
            validate_desktop_label(&desktop.label)?;
            validate_desktop_endpoint(&desktop.address, desktop.port)?;
            validate_desktop_text(&desktop.username, "desktop username")?;
            validate_desktop_text(&desktop.domain, "desktop domain")?;
            if desktop.protocol == PortableDesktopProtocol::Vnc && desktop.audio_playback_enabled {
                return Err(SyncCodecError::InvalidBundle(
                    "VNC desktop profiles cannot enable audio playback",
                ));
            }
            if desktop.protocol == PortableDesktopProtocol::Rdp
                && desktop.vnc_protocol_version != PortableVncProtocolVersion::Auto
            {
                return Err(SyncCodecError::InvalidBundle(
                    "RDP cannot select a VNC protocol version",
                ));
            }
            if desktop.width == 0
                || desktop.height == 0
                || desktop.width > 8_192
                || desktop.height > 8_192
                || u32::from(desktop.width) * u32::from(desktop.height) > 16_777_216
            {
                return Err(SyncCodecError::InvalidBundle("invalid desktop dimensions"));
            }
        }
        for identity in &self.identities {
            validate_text(&identity.label, MAX_SMALL_TEXT_BYTES, "identity label")?;
            validate_optional_text(identity.username.as_deref(), "identity username")?;
            validate_unique_ids(&identity.credential_ids, "identity credentials")?;
        }
        for credential in &self.credentials {
            validate_text(&credential.label, MAX_SMALL_TEXT_BYTES, "credential label")?;
            match &credential.material {
                PortableCredentialMaterial::Password { .. } => {}
                PortableCredentialMaterial::PrivateKey {
                    public_key_fingerprint,
                    ..
                }
                | PortableCredentialMaterial::Certificate {
                    public_key_fingerprint,
                    ..
                } => validate_text(
                    public_key_fingerprint,
                    MAX_SMALL_TEXT_BYTES,
                    "public key fingerprint",
                )?,
                PortableCredentialMaterial::KeyboardInteractive { max_rounds } => {
                    if !(1..=32).contains(max_rounds) {
                        return Err(SyncCodecError::InvalidBundle(
                            "keyboard-interactive max rounds must be between 1 and 32",
                        ));
                    }
                }
            }
        }
        for route in &self.routes {
            validate_collection(route.jump_hops.len(), "jump hops")?;
            if route.jump_hops.len() > 5 {
                return Err(SyncCodecError::BoundExceeded("jump hops"));
            }
            match &route.ingress {
                RouteIngress::Direct => {}
                RouteIngress::HttpConnect {
                    host,
                    port,
                    username,
                    ..
                }
                | RouteIngress::Socks5 {
                    host,
                    port,
                    username,
                    ..
                } => {
                    validate_text(host, MAX_SMALL_TEXT_BYTES, "proxy host")?;
                    if *port == 0 {
                        return Err(SyncCodecError::InvalidBundle("proxy port must be positive"));
                    }
                    validate_optional_text(username.as_deref(), "proxy username")?;
                }
            }
        }
        for plan in &self.authentication_plans {
            validate_unique_ids(&plan.credential_ids, "authentication plan")?;
        }
        for policy in &self.algorithm_policies {
            validate_text(
                &policy.policy_id,
                MAX_SMALL_TEXT_BYTES,
                "algorithm policy ID",
            )?;
            validate_collection(
                policy.compatibility_exceptions.len(),
                "algorithm compatibility exceptions",
            )?;
            for exception in &policy.compatibility_exceptions {
                validate_text(
                    &exception.exception_id,
                    MAX_SMALL_TEXT_BYTES,
                    "algorithm compatibility exception ID",
                )?;
                validate_optional_text(exception.reason.as_deref(), "algorithm exception reason")?;
            }
        }
        for policy in &self.heartbeat_policies {
            match &policy.mode {
                HeartbeatMode::Disabled => {}
                HeartbeatMode::TransportKeepalive {
                    interval_seconds,
                    reply_timeout_seconds,
                    max_missed_replies,
                } => {
                    validate_seconds(*interval_seconds, "keepalive interval")?;
                    validate_seconds(*reply_timeout_seconds, "keepalive timeout")?;
                    if *max_missed_replies == 0 {
                        return Err(SyncCodecError::InvalidBundle(
                            "max missed replies must be positive",
                        ));
                    }
                }
                HeartbeatMode::ShellHeartbeat {
                    interval_seconds,
                    idle_seconds,
                    payload,
                    ..
                } => {
                    validate_seconds(*interval_seconds, "shell heartbeat interval")?;
                    validate_seconds(*idle_seconds, "shell heartbeat idle threshold")?;
                    validate_text(payload, MAX_LARGE_TEXT_BYTES, "shell heartbeat payload")?;
                }
            }
        }
        for policy in &self.monitoring_policies {
            validate_seconds(policy.interval_seconds, "monitoring interval")?;
            validate_seconds(policy.timeout_seconds, "monitoring timeout")?;
            match (policy.interval_millis, policy.timeout_millis) {
                (None, None) => {}
                (Some(interval), Some(timeout))
                    if (1_500..=300_000).contains(&interval)
                        && (500..=30_000).contains(&timeout)
                        && interval.div_ceil(1_000) == policy.interval_seconds
                        && timeout.div_ceil(1_000) == policy.timeout_seconds => {}
                _ => return Err(SyncCodecError::InvalidBundle("invalid monitoring duration")),
            }
            validate_collection(policy.resources.len(), "monitoring resources")?;
        }
        for automation in &self.login_automations {
            validate_collection(automation.steps.len(), "login automation steps")?;
            for step in &automation.steps {
                match step {
                    LoginAutomationStep::Expect {
                        pattern,
                        timeout_seconds,
                    } => {
                        validate_text(pattern, MAX_LARGE_TEXT_BYTES, "expect pattern")?;
                        validate_seconds(*timeout_seconds, "expect timeout")?;
                    }
                    LoginAutomationStep::SendText {
                        text,
                        timeout_seconds,
                        ..
                    } => {
                        validate_text(text, MAX_LARGE_TEXT_BYTES, "send text")?;
                        validate_seconds(*timeout_seconds, "send text timeout")?;
                    }
                    LoginAutomationStep::SendSecret {
                        secret_label,
                        timeout_seconds,
                        ..
                    } => {
                        validate_text(secret_label, MAX_SMALL_TEXT_BYTES, "send secret label")?;
                        validate_seconds(*timeout_seconds, "send secret timeout")?;
                    }
                }
            }
        }
        Ok(())
    }

    fn all_ids(&self) -> impl Iterator<Item = PortableObjectId> + '_ {
        self.hosts
            .iter()
            .map(|item| item.id)
            .chain(self.desktop_profiles.iter().map(|item| item.id))
            .chain(self.identities.iter().map(|item| item.id))
            .chain(self.credentials.iter().map(|item| item.id))
            .chain(self.routes.iter().map(|item| item.id))
            .chain(self.authentication_plans.iter().map(|item| item.id))
            .chain(self.algorithm_policies.iter().map(|item| item.id))
            .chain(self.heartbeat_policies.iter().map(|item| item.id))
            .chain(self.monitoring_policies.iter().map(|item| item.id))
            .chain(self.login_automations.iter().map(|item| item.id))
    }

    fn all_nodes(&self) -> impl Iterator<Item = (PortableObjectKind, PortableObjectId)> + '_ {
        self.hosts
            .iter()
            .map(|item| (PortableObjectKind::Host, item.id))
            .chain(
                self.desktop_profiles
                    .iter()
                    .map(|item| (PortableObjectKind::DesktopProfile, item.id)),
            )
            .chain(
                self.identities
                    .iter()
                    .map(|item| (PortableObjectKind::Identity, item.id)),
            )
            .chain(
                self.credentials
                    .iter()
                    .map(|item| (PortableObjectKind::Credential, item.id)),
            )
            .chain(
                self.routes
                    .iter()
                    .map(|item| (PortableObjectKind::Route, item.id)),
            )
            .chain(
                self.authentication_plans
                    .iter()
                    .map(|item| (PortableObjectKind::AuthenticationPlan, item.id)),
            )
            .chain(
                self.algorithm_policies
                    .iter()
                    .map(|item| (PortableObjectKind::AlgorithmPolicy, item.id)),
            )
            .chain(
                self.heartbeat_policies
                    .iter()
                    .map(|item| (PortableObjectKind::HeartbeatPolicy, item.id)),
            )
            .chain(
                self.monitoring_policies
                    .iter()
                    .map(|item| (PortableObjectKind::MonitoringPolicy, item.id)),
            )
            .chain(
                self.login_automations
                    .iter()
                    .map(|item| (PortableObjectKind::LoginAutomation, item.id)),
            )
    }

    fn sort_by_id(&mut self) {
        self.hosts.sort_by_key(|item| item.id);
        self.desktop_profiles.sort_by_key(|item| item.id);
        self.identities.sort_by_key(|item| item.id);
        self.credentials.sort_by_key(|item| item.id);
        self.routes.sort_by_key(|item| item.id);
        self.authentication_plans.sort_by_key(|item| item.id);
        self.algorithm_policies.sort_by_key(|item| item.id);
        self.heartbeat_policies.sort_by_key(|item| item.id);
        self.monitoring_policies.sort_by_key(|item| item.id);
        self.login_automations.sort_by_key(|item| item.id);
    }
}

fn validate_count(count: usize, name: &'static str) -> Result<()> {
    if count > MAX_OBJECTS_PER_KIND {
        return Err(SyncCodecError::BoundExceeded(name));
    }
    Ok(())
}

fn validate_collection(count: usize, name: &'static str) -> Result<()> {
    if count > MAX_COLLECTION_ITEMS {
        return Err(SyncCodecError::BoundExceeded(name));
    }
    Ok(())
}

fn validate_unique_ids(ids: &[PortableObjectId], name: &'static str) -> Result<()> {
    validate_collection(ids.len(), name)?;
    let unique = ids.iter().copied().collect::<HashSet<_>>();
    if unique.len() != ids.len() {
        return Err(SyncCodecError::InvalidBundle(
            "ordered object references must not contain duplicates",
        ));
    }
    Ok(())
}

fn validate_text(value: &str, max_bytes: usize, name: &'static str) -> Result<()> {
    if value.is_empty() {
        return Err(SyncCodecError::InvalidBundle(name));
    }
    if value.len() > max_bytes {
        return Err(SyncCodecError::BoundExceeded(name));
    }
    if value.chars().any(char::is_control) {
        return Err(SyncCodecError::InvalidBundle(
            "text fields must not contain control characters",
        ));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, name: &'static str) -> Result<()> {
    if let Some(value) = value {
        validate_text(value, MAX_SMALL_TEXT_BYTES, name)?;
    }
    Ok(())
}

fn validate_desktop_label(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(SyncCodecError::InvalidBundle("desktop label"));
    }
    validate_desktop_text(value, "desktop label")
}

fn validate_desktop_text(value: &str, name: &'static str) -> Result<()> {
    if value.len() > 256 {
        return Err(SyncCodecError::BoundExceeded(name));
    }
    if value.chars().any(char::is_control) {
        return Err(SyncCodecError::InvalidBundle(
            "desktop text fields must not contain control characters",
        ));
    }
    Ok(())
}

/// Mirrors the application desktop-profile endpoint grammar without taking a
/// persistence or SSH-domain dependency into this codec crate.
fn validate_desktop_endpoint(input: &str, port: u16) -> Result<()> {
    if port == 0 {
        return Err(SyncCodecError::InvalidBundle(
            "desktop port must be positive",
        ));
    }
    let input = input.trim();
    if input.is_empty() {
        return Err(SyncCodecError::InvalidBundle("desktop endpoint"));
    }
    let (address, bracketed) = match input
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        Some(address) => (address, true),
        None => (input, false),
    };
    if let Ok(ip) = address.parse::<IpAddr>() {
        if bracketed && !ip.is_ipv6() {
            return Err(SyncCodecError::InvalidBundle("desktop endpoint"));
        }
        return Ok(());
    }
    if bracketed || !address.is_ascii() {
        return Err(SyncCodecError::InvalidBundle("desktop endpoint"));
    }
    let hostname = address
        .strip_suffix('.')
        .unwrap_or(address)
        .to_ascii_lowercase();
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(SyncCodecError::InvalidBundle("desktop endpoint"));
    }
    let labels = hostname.split('.').collect::<Vec<_>>();
    if labels.len() == 4
        && labels
            .iter()
            .all(|label| label.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(SyncCodecError::InvalidBundle("desktop endpoint"));
    }
    if labels.iter().any(|label| {
        label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || !label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
    }) {
        return Err(SyncCodecError::InvalidBundle("desktop endpoint"));
    }
    Ok(())
}

fn validate_seconds(value: u32, name: &'static str) -> Result<()> {
    if !(1..=86_400).contains(&value) {
        return Err(SyncCodecError::InvalidBundle(name));
    }
    Ok(())
}
