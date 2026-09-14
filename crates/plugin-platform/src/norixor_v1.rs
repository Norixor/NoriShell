//! Verification for the Norixor NoriShell plugin marketplace v1 wire.
//!
//! This protocol is intentionally separate from the legacy optional catalog in
//! [`crate::catalog`]. Trust starts at a root key embedded by the client build;
//! the public key repeated by the server is checked for equality and is never
//! used to bootstrap trust.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read as _},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PLUGIN_THEME_PROTOCOL_MINOR, PluginCapability,
    PluginId, PluginPackageKind, ThemeDefinition,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive};

use crate::{
    InspectedFile, InspectedPackage, InspectedPluginProtocols, InspectedPluginWorkflows,
    PackageLimits, PluginManifest, PluginPlatformError, Result,
    inspect_protocols_asset_from_package_snapshot, inspect_theme_asset_from_package_snapshot,
    inspect_workflows_asset_from_package_snapshot, lower_hex,
};

const SCHEMA_VERSION: u16 = 1;
const MAX_WIRE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CATALOG_ITEMS: usize = 2_000;
const MAX_DOWNLOAD_TTL_SECONDS: u32 = 600;
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;

/// Client-owned trust anchor. Construct this only from reviewed bytes embedded
/// in the application build, never from `trust-root.json`.
#[derive(Debug, Clone)]
pub struct NorixorV1EmbeddedRoot {
    key_id: String,
    key: VerifyingKey,
}

impl NorixorV1EmbeddedRoot {
    pub fn from_embedded_bytes(key_id: &'static str, public_key: [u8; 32]) -> Result<Self> {
        if !valid_identifier(key_id, 100) {
            return Err(PluginPlatformError::InvalidNorixorRoot);
        }
        let key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| PluginPlatformError::InvalidNorixorRoot)?;
        Ok(Self {
            key_id: key_id.to_owned(),
            key,
        })
    }

    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NorixorV1SignatureAlgorithm {
    Ed25519,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1Signature {
    pub key_id: String,
    pub algorithm: NorixorV1SignatureAlgorithm,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1RoleKeys {
    pub keys: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1PublisherRole {
    pub keys: BTreeMap<String, String>,
    pub revoked_key_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1RootRoles {
    pub catalog: NorixorV1RoleKeys,
    pub publishers: NorixorV1PublisherRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1RootMetadata {
    pub schema_version: u16,
    pub version: u64,
    pub expires_at: i64,
    pub root_key_id: String,
    pub roles: NorixorV1RootRoles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1TrustRootData {
    pub root_key_id: String,
    pub root_public_key: String,
    pub metadata: NorixorV1RootMetadata,
    pub signature: NorixorV1Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1TrustRootWire {
    pub data: NorixorV1TrustRootData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NorixorV1RootTrustState {
    pub version: u64,
    pub fingerprint_sha256: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedNorixorV1Root {
    pub metadata: NorixorV1RootMetadata,
    pub fingerprint_sha256: String,
    catalog_keys: BTreeMap<String, VerifyingKey>,
    publisher_keys: BTreeMap<String, VerifyingKey>,
    revoked_publisher_key_ids: BTreeSet<String>,
}

impl VerifiedNorixorV1Root {
    #[must_use]
    pub fn trust_state(&self) -> NorixorV1RootTrustState {
        NorixorV1RootTrustState {
            version: self.metadata.version,
            fingerprint_sha256: self.fingerprint_sha256.clone(),
        }
    }

    /// Returns a delegated publisher key only after the root metadata has
    /// verified against the client-embedded trust anchor.
    #[must_use]
    pub fn publisher_key_base64(&self, key_id: &str) -> Option<String> {
        self.publisher_keys
            .get(key_id)
            .filter(|_| !self.revoked_publisher_key_ids.contains(key_id))
            .map(|key| BASE64.encode(key.as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NorixorV1Capability {
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
    CredentialsPlugin,
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
    SftpRead,
    SftpWrite,
    MetricsRead,
    SshSync,
    #[serde(untagged)]
    Unknown(String),
}

impl NorixorV1Capability {
    #[must_use]
    pub const fn core_capability(&self) -> Option<PluginCapability> {
        Some(match self {
            Self::UiPanel => PluginCapability::UiPanel,
            Self::UiNavigation => PluginCapability::UiNavigation,
            Self::UiPage => PluginCapability::UiPage,
            Self::UiWebviewIsolated => PluginCapability::UiWebviewIsolated,
            Self::UiHostDomObserve => PluginCapability::UiHostDomObserve,
            Self::UiHostDomMutate => PluginCapability::UiHostDomMutate,
            Self::UiHostCss => PluginCapability::UiHostCss,
            Self::ClipboardWrite => PluginCapability::ClipboardWrite,
            Self::TerminalProvider => PluginCapability::TerminalProvider,
            Self::DeviceSerial => PluginCapability::DeviceSerial,
            Self::CredentialsPlugin => PluginCapability::CredentialsPlugin,
            Self::TerminalMetadata => PluginCapability::TerminalMetadata,
            Self::TerminalObserve => PluginCapability::TerminalObserve,
            Self::TerminalAnnotation => PluginCapability::TerminalAnnotation,
            Self::TerminalProposeInput => PluginCapability::TerminalProposeInput,
            Self::TerminalRequestInput => PluginCapability::TerminalRequestInput,
            Self::HostMetadataRead => PluginCapability::HostMetadataRead,
            Self::HostMutationPropose => PluginCapability::HostMutationPropose,
            Self::HostSessionRequest => PluginCapability::HostSessionRequest,
            Self::RemoteInspect => PluginCapability::RemoteInspect,
            Self::RemoteExecRequest => PluginCapability::RemoteExecRequest,
            Self::NetworkDomain => PluginCapability::NetworkDomain,
            Self::LocalFiles => PluginCapability::LocalFiles,
            Self::LocalProcess => PluginCapability::LocalProcess,
            Self::StoragePlugin => PluginCapability::StoragePlugin,
            Self::SftpRead => PluginCapability::SftpRead,
            Self::SftpWrite => PluginCapability::SftpWrite,
            Self::MetricsRead => PluginCapability::MetricsRead,
            Self::SshSync => PluginCapability::SshSync,
            Self::Unknown(_) => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NorixorV1UnsignedManifest {
    pub plugin_id: PluginId,
    pub name: String,
    pub publisher: String,
    pub publisher_key_id: String,
    pub version: String,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub platform: String,
    pub architectures: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_url: Option<String>,
    pub capabilities: Vec<NorixorV1Capability>,
    pub minimum_app_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NorixorV1PackageManifest {
    pub plugin_id: PluginId,
    pub name: String,
    pub publisher: String,
    pub publisher_key_id: String,
    pub publisher_signature: String,
    pub version: String,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub platform: String,
    pub architectures: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_url: Option<String>,
    pub capabilities: Vec<NorixorV1Capability>,
    pub minimum_app_version: String,
}

impl NorixorV1PackageManifest {
    fn unsigned(&self) -> NorixorV1UnsignedManifest {
        NorixorV1UnsignedManifest {
            plugin_id: self.plugin_id.clone(),
            name: self.name.clone(),
            publisher: self.publisher.clone(),
            publisher_key_id: self.publisher_key_id.clone(),
            version: self.version.clone(),
            protocol_major: self.protocol_major,
            protocol_minor: self.protocol_minor,
            platform: self.platform.clone(),
            architectures: self.architectures.clone(),
            package_url: self.package_url.clone(),
            capabilities: self.capabilities.clone(),
            minimum_app_version: self.minimum_app_version.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]

pub struct NorixorV1PublisherFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]

pub struct NorixorV1PublisherPayload {
    pub schema_version: u16,
    pub manifest: NorixorV1UnsignedManifest,
    pub files: Vec<NorixorV1PublisherFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1PublisherWire {
    pub payload: NorixorV1PublisherPayload,
    pub signature: NorixorV1Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]

pub struct NorixorV1CatalogDetails {
    pub description: String,
    pub release_notes: Vec<String>,
    pub extension_targets: Vec<String>,
    pub release_published_at_unix_ms: Option<i64>,
}

impl NorixorV1CatalogDetails {
    pub fn to_core(&self) -> norishell_core_api::PluginCatalogReleaseDetails {
        norishell_core_api::PluginCatalogReleaseDetails {
            description: self.description.clone(),
            release_notes: self.release_notes.clone(),
            extension_targets: self.extension_targets.clone(),
            release_published_at_unix_ms: self.release_published_at_unix_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]

pub struct NorixorV1CatalogItem {
    pub plugin_id: PluginId,
    pub name: String,
    pub version: String,
    pub manifest_hash: String,
    pub package_sha256: String,
    pub package_size: u64,
    pub publisher_key_id: String,
    pub publisher_signature: String,
    pub publisher_payload: NorixorV1PublisherPayload,
    pub publisher_payload_hash: String,
    pub capabilities: Vec<NorixorV1Capability>,
    pub architectures: Vec<String>,
    pub minimum_app_version: String,
    pub download: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<NorixorV1CatalogDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]

pub struct NorixorV1CatalogPayload {
    pub schema_version: u16,
    pub sequence: u64,
    pub generated_at: i64,
    pub expires_at: i64,
    pub root_version: u64,
    pub root_fingerprint: String,
    pub items: Vec<NorixorV1CatalogItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1CatalogData {
    pub catalog: NorixorV1CatalogPayload,
    pub signature: NorixorV1Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1CatalogWire {
    pub data: NorixorV1CatalogData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NorixorV1CatalogTrustState {
    pub root_version: u64,
    pub root_fingerprint_sha256: String,
    pub sequence: u64,
    pub catalog_sha256: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedNorixorV1Catalog {
    pub payload: NorixorV1CatalogPayload,
    pub catalog_sha256: String,
    pub unsupported_contracts: BTreeSet<(PluginId, String)>,
}

impl VerifiedNorixorV1Catalog {
    #[must_use]
    pub fn trust_state(&self) -> NorixorV1CatalogTrustState {
        NorixorV1CatalogTrustState {
            root_version: self.payload.root_version,
            root_fingerprint_sha256: self.payload.root_fingerprint.clone(),
            sequence: self.payload.sequence,
            catalog_sha256: self.catalog_sha256.clone(),
        }
    }

    #[must_use]
    pub fn item(&self, plugin_id: &PluginId, version: &str) -> Option<&NorixorV1CatalogItem> {
        self.payload
            .items
            .iter()
            .find(|item| &item.plugin_id == plugin_id && item.version == version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1DownloadData {
    pub url: String,
    pub expires_in: u32,
    pub sha256: String,
    pub size: u64,
    pub capabilities: Vec<NorixorV1Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NorixorV1DownloadWire {
    pub data: NorixorV1DownloadData,
}

#[derive(Debug, Clone)]
pub struct VerifiedNorixorV1Package {
    pub manifest: NorixorV1PackageManifest,
    pub manifest_size: u64,
    pub package_sha256: String,
    pub package_size: u64,
    pub files: Vec<NorixorV1PublisherFile>,
    pub package_kind: PluginPackageKind,
    pub theme_definition: Option<ThemeDefinition>,
    pub theme_definition_sha256: Option<[u8; 32]>,
    pub protocols: Option<InspectedPluginProtocols>,
    pub workflows: Option<InspectedPluginWorkflows>,
}

impl VerifiedNorixorV1Package {
    pub fn into_inspected_package(self) -> Result<InspectedPackage> {
        let package_sha256 = decode_sha256(&self.package_sha256)?;
        let capabilities = self
            .manifest
            .capabilities
            .iter()
            .map(NorixorV1Capability::core_capability)
            .collect::<Option<Vec<_>>>()
            .ok_or(PluginPlatformError::IncompatibleCatalogEntry)?;
        let manifest = PluginManifest {
            plugin_id: self.manifest.plugin_id,
            name: self.manifest.name,
            publisher: self.manifest.publisher,
            publisher_key_id: Some(self.manifest.publisher_key_id),
            publisher_signature: Some(self.manifest.publisher_signature),
            version: self.manifest.version,
            protocol_major: self.manifest.protocol_major,
            protocol_minor: self.manifest.protocol_minor,
            platform: self.manifest.platform,
            architectures: self.manifest.architectures,
            package_url: self.manifest.package_url,
            capabilities,
            minimum_app_version: self.manifest.minimum_app_version,
        };
        let mut files = self
            .files
            .into_iter()
            .map(|file| InspectedFile {
                relative_path: file.path.into(),
                uncompressed_size: file.size,
            })
            .collect::<Vec<_>>();
        files.push(InspectedFile {
            relative_path: "manifest.json".into(),
            uncompressed_size: self.manifest_size,
        });
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(InspectedPackage {
            manifest,
            package_size: self.package_size,
            package_sha256,
            files,
            package_kind: self.package_kind,
            theme_definition: self.theme_definition,
            theme_definition_sha256: self.theme_definition_sha256,
            settings: None,
            protocols: self.protocols,
            workflows: self.workflows,
        })
    }
}

fn decode_sha256(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PluginPlatformError::PackageHashMismatch);
    }

    let mut decoded = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_hex_nibble(pair[0]).ok_or(PluginPlatformError::PackageHashMismatch)?;
        let low = decode_hex_nibble(pair[1]).ok_or(PluginPlatformError::PackageHashMismatch)?;
        decoded[index] = (high << 4) | low;
    }
    Ok(decoded)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn verify_norixor_v1_root(
    wire_json: &[u8],
    embedded_root: &NorixorV1EmbeddedRoot,
    now_unix_seconds: i64,
    previous: Option<&NorixorV1RootTrustState>,
) -> Result<VerifiedNorixorV1Root> {
    let wire: NorixorV1TrustRootWire = parse_wire(wire_json)?;
    let data = wire.data;
    if data.root_key_id != embedded_root.key_id
        || data.metadata.root_key_id != embedded_root.key_id
        || data.signature.key_id != embedded_root.key_id
        || data.root_public_key != BASE64.encode(embedded_root.key.as_bytes())
        || data.metadata.schema_version != SCHEMA_VERSION
        || data.metadata.version == 0
    {
        return Err(PluginPlatformError::InvalidNorixorRoot);
    }
    verify_signature(
        &embedded_root.key,
        &canonical_json(&data.metadata, PluginPlatformError::InvalidNorixorRoot)?,
        &data.signature.value,
        PluginPlatformError::InvalidNorixorRoot,
    )?;
    if data.metadata.expires_at <= now_unix_seconds {
        return Err(PluginPlatformError::CatalogExpired);
    }
    let fingerprint_sha256 = sha256_hex(&canonical_json(
        &data.metadata,
        PluginPlatformError::InvalidNorixorRoot,
    )?);
    if let Some(previous) = previous
        && (data.metadata.version < previous.version
            || (data.metadata.version == previous.version
                && fingerprint_sha256 != previous.fingerprint_sha256))
    {
        return Err(PluginPlatformError::CatalogRollback);
    }

    let catalog_keys = decode_role_keys(&data.metadata.roles.catalog.keys)?;
    let publisher_keys = decode_role_keys(&data.metadata.roles.publishers.keys)?;
    if catalog_keys.is_empty() || publisher_keys.is_empty() {
        return Err(PluginPlatformError::InvalidNorixorRoot);
    }
    let revoked_publisher_key_ids = data
        .metadata
        .roles
        .publishers
        .revoked_key_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if revoked_publisher_key_ids.len() != data.metadata.roles.publishers.revoked_key_ids.len()
        || revoked_publisher_key_ids
            .iter()
            .any(|key_id| !publisher_keys.contains_key(key_id))
    {
        return Err(PluginPlatformError::InvalidNorixorRoot);
    }
    Ok(VerifiedNorixorV1Root {
        metadata: data.metadata,
        fingerprint_sha256,
        catalog_keys,
        publisher_keys,
        revoked_publisher_key_ids,
    })
}

pub fn verify_norixor_v1_catalog(
    wire_json: &[u8],
    root: &VerifiedNorixorV1Root,
    _current_app_version: &Version,
    _current_architecture: &str,
    now_unix_seconds: i64,
    previous: Option<&NorixorV1CatalogTrustState>,
) -> Result<VerifiedNorixorV1Catalog> {
    let raw: Value = parse_wire(wire_json)?;
    let wire: NorixorV1CatalogWire =
        serde_json::from_value(raw.clone()).map_err(|_| PluginPlatformError::InvalidNorixorWire)?;
    let signature = &wire.data.signature;
    let catalog_key = root
        .catalog_keys
        .get(&signature.key_id)
        .ok_or(PluginPlatformError::InvalidCatalogSignature)?;
    let canonical = canonical_json(
        &raw["data"]["catalog"],
        PluginPlatformError::InvalidNorixorCatalog,
    )?;
    verify_signature(
        catalog_key,
        &canonical,
        &signature.value,
        PluginPlatformError::InvalidCatalogSignature,
    )?;
    let catalog = wire.data.catalog;
    if catalog.schema_version != SCHEMA_VERSION
        || catalog.sequence == 0
        || catalog.generated_at <= 0
        || catalog.generated_at > now_unix_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS)
        || catalog.expires_at <= catalog.generated_at
        || catalog.root_version != root.metadata.version
        || catalog.root_fingerprint != root.fingerprint_sha256
        || catalog.items.len() > MAX_CATALOG_ITEMS
    {
        return Err(PluginPlatformError::InvalidNorixorCatalog);
    }
    if catalog.expires_at <= now_unix_seconds {
        return Err(PluginPlatformError::CatalogExpired);
    }
    let catalog_sha256 = sha256_hex(&canonical);
    if let Some(previous) = previous
        && (catalog.root_version < previous.root_version
            || (catalog.root_version == previous.root_version
                && catalog.root_fingerprint != previous.root_fingerprint_sha256)
            || catalog.sequence < previous.sequence
            || (catalog.sequence == previous.sequence && catalog_sha256 != previous.catalog_sha256))
    {
        return Err(PluginPlatformError::CatalogRollback);
    }

    let mut identities = BTreeSet::new();
    let mut unsupported_contracts = BTreeSet::new();
    for (index, item) in catalog.items.iter().enumerate() {
        if let Some(details) = &item.details {
            validate_catalog_details(details, catalog.generated_at)?;
        }
        if !identities.insert((item.plugin_id.clone(), item.version.clone())) {
            return Err(PluginPlatformError::InvalidNorixorCatalog);
        }
        let raw_payload = &raw["data"]["catalog"]["items"][index]["publisher_payload"];
        validate_catalog_item(item, root, raw_payload)?;
        // A lossy contract projection is safe to display, never safe to execute.
        if serde_json::to_value(&item.publisher_payload)
            .map_err(|_| PluginPlatformError::InvalidNorixorPublisher)?
            != *raw_payload
        {
            unsupported_contracts.insert((item.plugin_id.clone(), item.version.clone()));
        }
    }
    Ok(VerifiedNorixorV1Catalog {
        payload: catalog,
        catalog_sha256,
        unsupported_contracts,
    })
}

pub fn verify_norixor_v1_publisher_wire(
    wire_json: &[u8],
    root: &VerifiedNorixorV1Root,
    _current_app_version: &Version,
    _current_architecture: &str,
) -> Result<NorixorV1PublisherPayload> {
    let raw: Value = parse_wire(wire_json)?;
    let wire: NorixorV1PublisherWire =
        serde_json::from_value(raw.clone()).map_err(|_| PluginPlatformError::InvalidNorixorWire)?;
    validate_publisher_payload(&wire.payload)?;
    verify_publisher_signature(
        root,
        &wire.signature.key_id,
        &wire.signature.value,
        &raw["payload"],
    )?;
    if wire.signature.key_id != wire.payload.manifest.publisher_key_id {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    Ok(wire.payload)
}

pub fn verify_norixor_v1_download(
    wire_json: &[u8],
    item: &NorixorV1CatalogItem,
) -> Result<NorixorV1DownloadData> {
    let wire: NorixorV1DownloadWire = parse_wire(wire_json)?;
    let data = wire.data;
    if !valid_https_url(&data.url)
        || data.expires_in == 0
        || data.expires_in > MAX_DOWNLOAD_TTL_SECONDS
        || data.sha256 != item.package_sha256
        || data.size != item.package_size
        || data.capabilities != item.capabilities
    {
        return Err(PluginPlatformError::InvalidNorixorDownload);
    }
    Ok(data)
}

pub fn verify_norixor_v1_package(
    package_bytes: &[u8],
    item: &NorixorV1CatalogItem,
    download: &NorixorV1DownloadData,
    limits: PackageLimits,
) -> Result<VerifiedNorixorV1Package> {
    if !norishell_core_api::plugin_package_protocol_is_compatible(
        item.publisher_payload.manifest.protocol_major,
        item.publisher_payload.manifest.protocol_minor,
    ) || item.capabilities.iter().any(|capability| {
        capability.core_capability().is_none_or(|capability| {
            norishell_core_api::plugin_capability_min_protocol_minor(capability)
                > item.publisher_payload.manifest.protocol_minor
        })
    }) {
        return Err(PluginPlatformError::IncompatibleCatalogEntry);
    }
    let package_size =
        u64::try_from(package_bytes.len()).map_err(|_| PluginPlatformError::PackageTooLarge)?;
    if package_size == 0 || package_size > limits.max_archive_bytes {
        return Err(PluginPlatformError::PackageTooLarge);
    }
    if package_size != item.package_size || package_size != download.size {
        return Err(PluginPlatformError::PackageSizeMismatch);
    }
    let package_sha256 = sha256_hex(package_bytes);
    if package_sha256 != item.package_sha256 || package_sha256 != download.sha256 {
        return Err(PluginPlatformError::PackageHashMismatch);
    }

    let mut archive = ZipArchive::new(Cursor::new(package_bytes))
        .map_err(|_| PluginPlatformError::InvalidArchive)?;
    if archive.is_empty() || archive.len() > limits.max_files {
        return Err(PluginPlatformError::ExtractionLimitExceeded);
    }
    let mut names = BTreeSet::new();
    let mut expanded = 0_u64;
    let mut manifest_bytes = None;
    let mut wasm_seen = false;
    let mut directories = BTreeSet::new();
    let mut observed_files = Vec::new();
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .map_err(|_| PluginPlatformError::InvalidArchive)?;
        let name = file.name().to_owned();
        let is_directory = file.is_dir();
        if file.encrypted()
            || file.is_symlink()
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
            || !valid_archive_package_path(&name, is_directory)
            || !names.insert(name.to_ascii_lowercase())
            || file.size() > limits.max_single_file_bytes
        {
            return Err(PluginPlatformError::InvalidArchive);
        }
        expanded = expanded
            .checked_add(file.size())
            .ok_or(PluginPlatformError::ExtractionLimitExceeded)?;
        if expanded > limits.max_uncompressed_bytes {
            return Err(PluginPlatformError::ExtractionLimitExceeded);
        }
        if is_directory {
            if file.size() != 0 || !directories.insert(name) {
                return Err(PluginPlatformError::InvalidArchive);
            }
            continue;
        }
        let expected_size = file.size();
        let mut contents = Vec::new();
        file.take(limits.max_single_file_bytes.saturating_add(1))
            .read_to_end(&mut contents)?;
        if u64::try_from(contents.len()).ok() != Some(expected_size) {
            return Err(PluginPlatformError::InvalidArchive);
        }
        if name == "manifest.json" {
            if contents.len() as u64 > limits.max_manifest_bytes || manifest_bytes.is_some() {
                return Err(PluginPlatformError::InvalidArchive);
            }
            manifest_bytes = Some(contents);
        } else {
            if name == "plugin.wasm" {
                if contents.get(..4) != Some(b"\0asm") {
                    return Err(PluginPlatformError::InvalidArchive);
                }
                wasm_seen = true;
            }
            observed_files.push(NorixorV1PublisherFile {
                path: name,
                size: u64::try_from(contents.len())
                    .map_err(|_| PluginPlatformError::ExtractionLimitExceeded)?,
                sha256: sha256_hex(&contents),
            });
        }
    }
    let manifest_bytes = manifest_bytes.ok_or(PluginPlatformError::InvalidArchive)?;
    let manifest_size = u64::try_from(manifest_bytes.len())
        .map_err(|_| PluginPlatformError::ExtractionLimitExceeded)?;
    let manifest: NorixorV1PackageManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|_| PluginPlatformError::InvalidArchive)?;
    observed_files.sort_by(|left, right| left.path.cmp(&right.path));
    if manifest.unsigned() != item.publisher_payload.manifest
        || manifest.publisher_key_id != item.publisher_key_id
        || manifest.publisher_signature != item.publisher_signature
        || observed_files != item.publisher_payload.files
        || sha256_hex(&canonical_json(
            &manifest,
            PluginPlatformError::ManifestMismatch,
        )?) != item.manifest_hash
    {
        return Err(PluginPlatformError::ManifestMismatch);
    }
    let package_kind = publisher_package_kind(&manifest.unsigned(), &observed_files)
        .map_err(|_| PluginPlatformError::InvalidArchive)?;
    let (theme_definition, theme_definition_sha256, protocols, workflows) = match package_kind {
        PluginPackageKind::Wasm => {
            if !wasm_seen || !directories.is_empty() {
                return Err(PluginPlatformError::InvalidArchive);
            }
            // These assets are read only from the same package bytes that have
            // just passed the signed size and SHA-256 checks above.
            (
                None,
                None,
                inspect_protocols_asset_from_package_snapshot(package_bytes)?,
                inspect_workflows_asset_from_package_snapshot(package_bytes)?,
            )
        }
        PluginPackageKind::Theme => {
            let (definition, definition_sha256) =
                inspect_theme_asset_from_package_snapshot(package_bytes, limits)?;
            (Some(definition), Some(definition_sha256), None, None)
        }
    };
    Ok(VerifiedNorixorV1Package {
        manifest,
        manifest_size,
        package_sha256,
        package_size,
        files: observed_files,
        package_kind,
        theme_definition,
        theme_definition_sha256,
        protocols,
        workflows,
    })
}

fn validate_catalog_details(details: &NorixorV1CatalogDetails, generated_at: i64) -> Result<()> {
    if !valid_text(&details.description, 4000)
        || details.release_notes.len() > 24
        || details
            .release_notes
            .iter()
            .any(|note| !valid_text(note, 1000))
        || details.extension_targets.len() > 32
        || details
            .extension_targets
            .iter()
            .any(|target| !valid_identifier(target, 160))
        || details
            .extension_targets
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != details.extension_targets.len()
        || details
            .release_published_at_unix_ms
            .is_some_and(|timestamp| {
                timestamp <= 0
                    || timestamp > generated_at.saturating_mul(1000)
                    || timestamp > 9_007_199_254_740_991
            })
        || serde_json::to_vec(details).map_or(true, |bytes| bytes.len() > 64 * 1024)
    {
        return Err(PluginPlatformError::InvalidNorixorCatalog);
    }
    Ok(())
}

fn validate_catalog_item(
    item: &NorixorV1CatalogItem,
    root: &VerifiedNorixorV1Root,
    raw_publisher_payload: &Value,
) -> Result<()> {
    validate_publisher_payload(&item.publisher_payload)?;
    let manifest = &item.publisher_payload.manifest;
    let expected_download = format!(
        "/apps/norishell/plugins/{}/versions/{}/download",
        item.plugin_id, item.version
    );
    if !valid_text(&item.name, 160)
        || !canonical_semver(&item.version)
        || !lower_hex_64(&item.manifest_hash)
        || !lower_hex_64(&item.package_sha256)
        || item.package_size == 0
        || item.package_size > 256 * 1024 * 1024
        || !valid_identifier(&item.publisher_key_id, 100)
        || decode_signature(&item.publisher_signature).is_err()
        || !lower_hex_64(&item.publisher_payload_hash)
        || item.download != expected_download
        || item.plugin_id != manifest.plugin_id
        || item.name != manifest.name
        || item.version != manifest.version
        || item.publisher_key_id != manifest.publisher_key_id
        || item.capabilities != manifest.capabilities
        || item.architectures != manifest.architectures
        || item.minimum_app_version != manifest.minimum_app_version
    {
        return Err(PluginPlatformError::InvalidNorixorCatalog);
    }
    let publisher_canonical = canonical_json(
        raw_publisher_payload,
        PluginPlatformError::InvalidNorixorPublisher,
    )?;
    if sha256_hex(&publisher_canonical) != item.publisher_payload_hash {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    verify_publisher_signature(
        root,
        &item.publisher_key_id,
        &item.publisher_signature,
        raw_publisher_payload,
    )
}

fn validate_publisher_payload(payload: &NorixorV1PublisherPayload) -> Result<()> {
    if payload.schema_version != SCHEMA_VERSION {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    validate_manifest(&payload.manifest)?;
    if payload.files.is_empty() || payload.files.len() > 511 {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    let mut previous = None;
    let mut has_wasm = false;
    let mut has_theme = false;
    for file in &payload.files {
        if !valid_package_path(&file.path)
            || file.path == "manifest.json"
            || file.size > 32 * 1024 * 1024
            || !lower_hex_64(&file.sha256)
            || previous.is_some_and(|value: &str| value >= file.path.as_str())
        {
            return Err(PluginPlatformError::InvalidNorixorPublisher);
        }
        has_wasm |= file.path == "plugin.wasm";
        has_theme |= file.path == "assets/theme.json";
        previous = Some(file.path.as_str());
    }
    if has_theme {
        publisher_package_kind(&payload.manifest, &payload.files)?;
    } else if !has_wasm {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    Ok(())
}

fn publisher_package_kind(
    manifest: &NorixorV1UnsignedManifest,
    files: &[NorixorV1PublisherFile],
) -> Result<PluginPackageKind> {
    let has_wasm = files.iter().any(|file| file.path == "plugin.wasm");
    let has_theme = files.iter().any(|file| file.path == "assets/theme.json");
    if manifest.protocol_major != PLUGIN_PROTOCOL_MAJOR {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    if has_theme {
        if has_wasm
            || files.len() != 1
            || files[0].path != "assets/theme.json"
            || files[0].size == 0
            || files[0].size > norishell_core_api::MAX_THEME_DEFINITION_BYTES as u64
            || manifest.protocol_minor != PLUGIN_THEME_PROTOCOL_MINOR
            || !manifest.capabilities.is_empty()
        {
            return Err(PluginPlatformError::InvalidNorixorPublisher);
        }
        return Ok(PluginPackageKind::Theme);
    }
    if !has_wasm
        || manifest.protocol_minor != PLUGIN_PROTOCOL_MINOR
        || files.iter().any(|file| file.path == "assets/theme.json")
    {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    Ok(PluginPackageKind::Wasm)
}

fn validate_manifest(manifest: &NorixorV1UnsignedManifest) -> Result<()> {
    Version::parse(&manifest.minimum_app_version)
        .map_err(|_| PluginPlatformError::InvalidNorixorPublisher)?;
    let unique_architectures = manifest.architectures.iter().collect::<BTreeSet<_>>();
    let unique_capabilities = manifest.capabilities.iter().collect::<BTreeSet<_>>();
    if !valid_text(&manifest.name, 160)
        || !valid_text(&manifest.publisher, 160)
        || !valid_identifier(&manifest.publisher_key_id, 100)
        || !canonical_semver(&manifest.version)
        || !valid_text(&manifest.platform, 80)
        || manifest.architectures.is_empty()
        || manifest.architectures.len() > 8
        || unique_architectures.len() != manifest.architectures.len()
        || manifest
            .architectures
            .iter()
            .any(|architecture| !valid_architecture(architecture))
        || manifest.capabilities.len() > 32
        || unique_capabilities.len() != manifest.capabilities.len()
        || manifest.capabilities.iter().any(|capability| matches!(capability, NorixorV1Capability::Unknown(name) if !valid_identifier(name, 100)))
        || manifest
            .package_url
            .as_ref()
            .is_some_and(|url| !valid_https_url(url))
    {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    Ok(())
}

fn verify_publisher_signature<T: Serialize>(
    root: &VerifiedNorixorV1Root,
    key_id: &str,
    signature: &str,
    payload: &T,
) -> Result<()> {
    if root.revoked_publisher_key_ids.contains(key_id) {
        return Err(PluginPlatformError::InvalidNorixorPublisher);
    }
    let key = root
        .publisher_keys
        .get(key_id)
        .ok_or(PluginPlatformError::InvalidNorixorPublisher)?;
    verify_signature(
        key,
        &canonical_json(payload, PluginPlatformError::InvalidNorixorPublisher)?,
        signature,
        PluginPlatformError::InvalidNorixorPublisher,
    )
}

fn parse_wire<T>(wire_json: &[u8]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    if wire_json.is_empty() || wire_json.len() > MAX_WIRE_BYTES {
        return Err(PluginPlatformError::InvalidNorixorWire);
    }
    serde_json::from_slice(wire_json).map_err(|_| PluginPlatformError::InvalidNorixorWire)
}

fn canonical_json<T: Serialize>(value: &T, error: PluginPlatformError) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(value).map_err(|_| error)?;
    sort_json(&mut value);
    serde_json::to_vec(&value).map_err(|_| PluginPlatformError::InvalidNorixorWire)
}

fn sort_json(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(sort_json),
        Value::Object(values) => {
            let mut sorted = std::mem::take(values).into_iter().collect::<Vec<_>>();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, mut value) in sorted {
                sort_json(&mut value);
                values.insert(key, value);
            }
        }
        _ => {}
    }
}

fn decode_role_keys(values: &BTreeMap<String, String>) -> Result<BTreeMap<String, VerifyingKey>> {
    let mut decoded = BTreeMap::new();
    for (key_id, value) in values {
        if !valid_identifier(key_id, 100) {
            return Err(PluginPlatformError::InvalidNorixorRoot);
        }
        decoded.insert(key_id.clone(), decode_key(value)?);
    }
    Ok(decoded)
}

fn decode_key(value: &str) -> Result<VerifyingKey> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| PluginPlatformError::InvalidNorixorRoot)?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| PluginPlatformError::InvalidNorixorRoot)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| PluginPlatformError::InvalidNorixorRoot)
}

fn decode_signature(value: &str) -> Result<Signature> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| PluginPlatformError::InvalidNorixorPublisher)?;
    Signature::from_slice(&decoded).map_err(|_| PluginPlatformError::InvalidNorixorPublisher)
}

fn verify_signature(
    key: &VerifyingKey,
    payload: &[u8],
    signature: &str,
    error: PluginPlatformError,
) -> Result<()> {
    let Ok(decoded) = BASE64.decode(signature) else {
        return Err(error);
    };
    let Ok(signature) = Signature::from_slice(&decoded) else {
        return Err(error);
    };
    if key.verify_strict(payload, &signature).is_err() {
        return Err(error);
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

fn lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= maximum
        && !value.chars().any(char::is_control)
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.is_ascii()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'-'))
        })
}

fn canonical_semver(value: &str) -> bool {
    Version::parse(value).is_ok_and(|version| version.to_string() == value)
}

fn valid_architecture(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.is_ascii()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn valid_package_path(value: &str) -> bool {
    if value.is_empty()
        || !value.is_ascii()
        || value.contains(['\\', '\0'])
        || value.starts_with('/')
        || value.ends_with('/')
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return false;
    }
    value == "manifest.json" || value == "plugin.wasm" || value.starts_with("assets/")
}

fn valid_archive_package_path(value: &str, is_directory: bool) -> bool {
    if is_directory {
        value == "assets/"
    } else {
        valid_package_path(value)
    }
}

fn valid_https_url(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 2_048
        || !value.is_ascii()
        || value.contains('#')
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return false;
    }
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && !authority.starts_with('.')
        && !authority.ends_with('.')
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use ed25519_dalek::{Signer as _, SigningKey};
    use semver::Version;
    use serde::Serialize;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use super::{
        NorixorV1CatalogWire, NorixorV1EmbeddedRoot, NorixorV1PublisherWire,
        NorixorV1RootTrustState, NorixorV1TrustRootWire, canonical_json, verify_norixor_v1_catalog,
        verify_norixor_v1_download, verify_norixor_v1_package, verify_norixor_v1_publisher_wire,
        verify_norixor_v1_root,
    };
    use crate::{PackageLimits, PluginPlatformError};
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    const ROOT_JSON: &[u8] = include_bytes!("../tests/fixtures/norixor-v1/trust-root.json");
    const CATALOG_JSON: &[u8] = include_bytes!("../tests/fixtures/norixor-v1/catalog.json");
    const PUBLISHER_JSON: &[u8] = include_bytes!("../tests/fixtures/norixor-v1/publisher.json");
    const DOWNLOAD_JSON: &[u8] = include_bytes!("../tests/fixtures/norixor-v1/download.json");
    const ROOT_PUBLIC: [u8; 32] = [
        0x3a, 0x58, 0xd2, 0x8e, 0x7c, 0x66, 0xe1, 0x86, 0x52, 0x9e, 0x3f, 0x1e, 0x9f, 0x7e, 0x23,
        0x43, 0x5e, 0xdf, 0x8c, 0x79, 0x0f, 0x24, 0xb6, 0x82, 0xa1, 0x2b, 0x5c, 0x85, 0x55, 0x06,
        0x81, 0x34,
    ];
    const NOW: i64 = 1_788_192_000;

    fn anchor() -> NorixorV1EmbeddedRoot {
        NorixorV1EmbeddedRoot::from_embedded_bytes("norishell-test-fixture-root-v1", ROOT_PUBLIC)
            .expect("fixture root")
    }

    fn signing_key(role: &str) -> SigningKey {
        let seed: [u8; 32] =
            Sha256::digest(format!("TEST-ONLY:norishell-plugin-trust-v1:{role}").as_bytes()).into();
        SigningKey::from_bytes(&seed)
    }

    fn signed_json<T: Serialize>(value: &T, key: &SigningKey) -> String {
        BASE64.encode(
            key.sign(
                &canonical_json(value, PluginPlatformError::InvalidNorixorWire).expect("canonical"),
            )
            .to_bytes(),
        )
    }

    /// Re-sign the fixture with the current protocol. The fixture keys are test-only,
    /// so this keeps the trust-chain tests focused on current production policy.
    fn current_fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut catalog: NorixorV1CatalogWire =
            serde_json::from_slice(CATALOG_JSON).expect("catalog");
        let item = &mut catalog.data.catalog.items[0];
        item.publisher_payload.manifest.protocol_minor = norishell_core_api::PLUGIN_PROTOCOL_MINOR;
        item.publisher_signature = signed_json(&item.publisher_payload, &signing_key("publisher"));
        item.publisher_payload_hash = super::sha256_hex(
            &canonical_json(
                &item.publisher_payload,
                PluginPlatformError::InvalidNorixorPublisher,
            )
            .expect("publisher canonical"),
        );

        let manifest = super::NorixorV1PackageManifest {
            plugin_id: item.publisher_payload.manifest.plugin_id.clone(),
            name: item.publisher_payload.manifest.name.clone(),
            publisher: item.publisher_payload.manifest.publisher.clone(),
            publisher_key_id: item.publisher_payload.manifest.publisher_key_id.clone(),
            publisher_signature: item.publisher_signature.clone(),
            version: item.publisher_payload.manifest.version.clone(),
            protocol_major: item.publisher_payload.manifest.protocol_major,
            protocol_minor: item.publisher_payload.manifest.protocol_minor,
            platform: item.publisher_payload.manifest.platform.clone(),
            architectures: item.publisher_payload.manifest.architectures.clone(),
            package_url: item.publisher_payload.manifest.package_url.clone(),
            capabilities: item.publisher_payload.manifest.capabilities.clone(),
            minimum_app_version: item.publisher_payload.manifest.minimum_app_version.clone(),
        };
        item.manifest_hash = super::sha256_hex(
            &canonical_json(&manifest, PluginPlatformError::ManifestMismatch)
                .expect("manifest canonical"),
        );
        let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest json");
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        archive
            .start_file("manifest.json", options)
            .expect("manifest entry");
        archive.write_all(&manifest_bytes).expect("manifest bytes");
        archive
            .start_file("plugin.wasm", options)
            .expect("wasm entry");
        archive.write_all(b"\0asm\x01\0\0\0").expect("wasm bytes");
        let package = archive.finish().expect("finish package").into_inner();
        item.package_size = package.len().try_into().expect("package size");
        item.package_sha256 = super::sha256_hex(&package);
        let package_sha256 = item.package_sha256.clone();
        let package_size = item.package_size;
        let _ = item;
        catalog.data.signature.value =
            signed_json(&catalog.data.catalog, &signing_key("online-catalog"));
        let catalog_bytes = serde_json::to_vec(&catalog).expect("catalog json");

        let mut download: Value = serde_json::from_slice(DOWNLOAD_JSON).expect("download");
        download["data"]["sha256"] = json!(package_sha256);
        download["data"]["size"] = json!(package_size);
        (
            catalog_bytes,
            serde_json::to_vec(&download).expect("download json"),
            package,
        )
    }

    fn current_theme_fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut catalog: NorixorV1CatalogWire =
            serde_json::from_slice(CATALOG_JSON).expect("catalog");
        let theme_bytes = include_bytes!("../../../examples/theme-plugins/clear/assets/theme.json");
        let (package_sha256, package_size, package) = {
            let item = &mut catalog.data.catalog.items[0];
            item.publisher_payload.manifest.protocol_minor =
                norishell_core_api::PLUGIN_THEME_PROTOCOL_MINOR;
            item.publisher_payload.manifest.capabilities.clear();
            item.capabilities.clear();
            item.publisher_payload.files = vec![super::NorixorV1PublisherFile {
                path: "assets/theme.json".to_owned(),
                size: theme_bytes.len().try_into().expect("theme size"),
                sha256: super::sha256_hex(theme_bytes),
            }];
            item.publisher_signature =
                signed_json(&item.publisher_payload, &signing_key("publisher"));
            item.publisher_payload_hash = super::sha256_hex(
                &canonical_json(
                    &item.publisher_payload,
                    PluginPlatformError::InvalidNorixorPublisher,
                )
                .expect("publisher canonical"),
            );

            let manifest = super::NorixorV1PackageManifest {
                plugin_id: item.publisher_payload.manifest.plugin_id.clone(),
                name: item.publisher_payload.manifest.name.clone(),
                publisher: item.publisher_payload.manifest.publisher.clone(),
                publisher_key_id: item.publisher_payload.manifest.publisher_key_id.clone(),
                publisher_signature: item.publisher_signature.clone(),
                version: item.publisher_payload.manifest.version.clone(),
                protocol_major: item.publisher_payload.manifest.protocol_major,
                protocol_minor: item.publisher_payload.manifest.protocol_minor,
                platform: item.publisher_payload.manifest.platform.clone(),
                architectures: item.publisher_payload.manifest.architectures.clone(),
                package_url: item.publisher_payload.manifest.package_url.clone(),
                capabilities: item.publisher_payload.manifest.capabilities.clone(),
                minimum_app_version: item.publisher_payload.manifest.minimum_app_version.clone(),
            };
            item.manifest_hash = super::sha256_hex(
                &canonical_json(&manifest, PluginPlatformError::ManifestMismatch)
                    .expect("manifest canonical"),
            );
            let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest json");
            let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            archive
                .start_file("manifest.json", options)
                .expect("manifest entry");
            archive.write_all(&manifest_bytes).expect("manifest bytes");
            archive
                .add_directory("assets/", options)
                .expect("assets directory");
            archive
                .start_file("assets/theme.json", options)
                .expect("theme entry");
            archive.write_all(theme_bytes).expect("theme bytes");
            let package = archive.finish().expect("finish package").into_inner();
            item.package_size = package.len().try_into().expect("package size");
            item.package_sha256 = super::sha256_hex(&package);
            (item.package_sha256.clone(), item.package_size, package)
        };
        catalog.data.signature.value =
            signed_json(&catalog.data.catalog, &signing_key("online-catalog"));
        let mut download: Value = serde_json::from_slice(DOWNLOAD_JSON).expect("download");
        download["data"]["sha256"] = json!(package_sha256);
        download["data"]["size"] = json!(package_size);
        download["data"]["capabilities"] = json!([]);
        (
            serde_json::to_vec(&catalog).expect("catalog json"),
            serde_json::to_vec(&download).expect("download json"),
            package,
        )
    }

    fn verified_chain() -> (
        super::VerifiedNorixorV1Root,
        super::VerifiedNorixorV1Catalog,
        super::NorixorV1DownloadData,
    ) {
        let (catalog_bytes, download_bytes, _) = current_fixture();
        let root = verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, None).expect("root");
        let catalog = verify_norixor_v1_catalog(
            &catalog_bytes,
            &root,
            &Version::new(1, 0, 0),
            "aarch64",
            NOW,
            None,
        )
        .expect("catalog");
        let download = verify_norixor_v1_download(&download_bytes, &catalog.payload.items[0])
            .expect("download");
        (root, catalog, download)
    }

    fn resign_raw_catalog(wire: &mut Value) {
        for item in wire["data"]["catalog"]["items"].as_array_mut().unwrap() {
            item["publisher_signature"] = json!(signed_json(
                &item["publisher_payload"],
                &signing_key("publisher")
            ));
            item["publisher_payload_hash"] = json!(super::sha256_hex(
                &canonical_json(
                    &item["publisher_payload"],
                    PluginPlatformError::InvalidNorixorPublisher
                )
                .unwrap()
            ));
        }
        wire["data"]["signature"]["value"] = json!(signed_json(
            &wire["data"]["catalog"],
            &signing_key("online-catalog")
        ));
    }

    #[test]
    fn mixed_abi_catalog_and_future_contracts_remain_signed_and_visible() {
        let (bytes, _, _) = current_fixture();
        let root = verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, None).unwrap();
        let mut wire: Value = serde_json::from_slice(&bytes).unwrap();
        let template = wire["data"]["catalog"]["items"][0].clone();
        let items = [9, 12, 13, 14]
            .into_iter()
            .map(|minor| {
                let mut item = template.clone();
                let version = format!("1.0.{minor}");
                item["version"] = json!(version);
                item["publisher_payload"]["manifest"]["version"] = json!(version);
                item["publisher_payload"]["manifest"]["protocolMinor"] = json!(minor);
                item["download"] = json!(format!(
                    "/apps/norishell/plugins/{}/versions/{version}/download",
                    item["plugin_id"].as_str().unwrap()
                ));
                if minor == 14 {
                    item["capabilities"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!("futureCapability"));
                    item["publisher_payload"]["manifest"]["capabilities"] =
                        item["capabilities"].clone();
                    item["publisher_payload"]["manifest"]["futureField"] = json!({"required":true});
                    item["publisher_payload"]["futurePublisherField"] = json!(42);
                    item["futureDisplayField"] = json!("metadata");
                }
                item
            })
            .collect::<Vec<_>>();
        wire["data"]["catalog"]["items"] = json!(items);
        wire["data"]["catalog"]["futureMetadata"] = json!("signed");
        resign_raw_catalog(&mut wire);
        let verify = |wire: &Value, previous: Option<&super::NorixorV1CatalogTrustState>| {
            verify_norixor_v1_catalog(
                &serde_json::to_vec(wire).unwrap(),
                &root,
                &Version::new(1, 0, 0),
                "aarch64",
                NOW,
                previous,
            )
        };
        let catalog = verify(&wire, None).unwrap();
        assert_eq!(catalog.payload.items.len(), 4);
        for item in &catalog.payload.items {
            let manifest = &item.publisher_payload.manifest;
            assert_eq!(
                norishell_core_api::plugin_protocol_is_compatible(
                    manifest.protocol_major,
                    manifest.protocol_minor
                ),
                manifest.protocol_minor == 13
            );
        }
        assert_eq!(catalog.unsupported_contracts.len(), 1);
        assert_eq!(
            catalog.payload.items[3]
                .capabilities
                .last()
                .unwrap()
                .core_capability(),
            None
        );
        let previous = catalog.trust_state();
        let mut changed = wire.clone();
        changed["data"]["catalog"]["futureMetadata"] = json!("tampered");
        assert!(verify(&changed, None).is_err());
        resign_raw_catalog(&mut changed);
        assert!(matches!(
            verify(&changed, Some(&previous)),
            Err(PluginPlatformError::CatalogRollback)
        ));
        let mut changed = wire.clone();
        changed["data"]["catalog"]["items"][3]["publisher_payload"]["manifest"]["futureField"] =
            json!({"required":false});
        // Even a valid new catalog signature cannot launder modified publisher fields.
        changed["data"]["signature"]["value"] = json!(signed_json(
            &changed["data"]["catalog"],
            &signing_key("online-catalog")
        ));
        assert!(matches!(
            verify(&changed, None),
            Err(PluginPlatformError::InvalidNorixorPublisher)
        ));
    }

    #[test]
    fn unknown_current_capability_and_contract_cannot_become_installable() {
        let (bytes, download_bytes, package) = current_fixture();
        let root = verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, None).unwrap();
        let mut wire: Value = serde_json::from_slice(&bytes).unwrap();
        let item = &mut wire["data"]["catalog"]["items"][0];
        item["capabilities"]
            .as_array_mut()
            .unwrap()
            .push(json!("futureCapability"));
        item["publisher_payload"]["manifest"]["capabilities"] = item["capabilities"].clone();
        item["publisher_payload"]["manifest"]["futureRequiredContract"] = json!(true);
        resign_raw_catalog(&mut wire);
        let catalog = verify_norixor_v1_catalog(
            &serde_json::to_vec(&wire).unwrap(),
            &root,
            &Version::new(1, 0, 0),
            "aarch64",
            NOW,
            None,
        )
        .unwrap();
        assert_eq!(catalog.unsupported_contracts.len(), 1);
        let item = &catalog.payload.items[0];
        assert!(
            item.capabilities
                .iter()
                .map(super::NorixorV1Capability::core_capability)
                .collect::<Option<Vec<_>>>()
                .is_none()
        );
        let download: super::NorixorV1DownloadWire =
            serde_json::from_slice(&download_bytes).unwrap();
        assert!(matches!(
            verify_norixor_v1_package(&package, item, &download.data, PackageLimits::default()),
            Err(PluginPlatformError::IncompatibleCatalogEntry)
        ));
    }

    #[test]
    fn catalog_details_are_signed_bounded_and_preserve_rollback_checks() {
        let (root, previous_catalog, _) = verified_chain();
        let (catalog_bytes, _, _) = current_fixture();
        let mut wire: NorixorV1CatalogWire = serde_json::from_slice(&catalog_bytes).unwrap();
        wire.data.catalog.sequence += 1;
        wire.data.catalog.items[0].details = Some(super::NorixorV1CatalogDetails {
            description: "Service management".into(),
            release_notes: vec!["Keep inventory during diagnosis".into()],
            extension_targets: vec!["terminal.tools".into()],
            release_published_at_unix_ms: Some(wire.data.catalog.generated_at * 1000),
        });
        wire.data.signature.value = signed_json(&wire.data.catalog, &signing_key("online-catalog"));
        let previous = super::NorixorV1CatalogTrustState {
            root_version: previous_catalog.payload.root_version,
            root_fingerprint_sha256: previous_catalog.payload.root_fingerprint.clone(),
            sequence: previous_catalog.payload.sequence,
            catalog_sha256: previous_catalog.catalog_sha256.clone(),
        };
        let verify = |bytes: &[u8]| {
            super::verify_norixor_v1_catalog(
                bytes,
                &root,
                &Version::new(1, 0, 0),
                "aarch64",
                NOW,
                Some(&previous),
            )
        };
        let verified = verify(&serde_json::to_vec(&wire).unwrap()).unwrap();
        assert_eq!(
            verified.payload.items[0]
                .details
                .as_ref()
                .unwrap()
                .description,
            "Service management"
        );
        let mut tampered = wire.clone();
        tampered.data.catalog.items[0]
            .details
            .as_mut()
            .unwrap()
            .description = "tampered".into();
        assert!(verify(&serde_json::to_vec(&tampered).unwrap()).is_err());
        for mutation in ["duplicate-target", "future-date", "long-note"] {
            let mut invalid = wire.clone();
            let details = invalid.data.catalog.items[0].details.as_mut().unwrap();
            match mutation {
                "duplicate-target" => details.extension_targets.push("terminal.tools".into()),
                "future-date" => details.release_published_at_unix_ms = Some(i64::MAX),
                _ => details.release_notes.push("x".repeat(1001)),
            }
            invalid.data.signature.value =
                signed_json(&invalid.data.catalog, &signing_key("online-catalog"));
            assert!(
                verify(&serde_json::to_vec(&invalid).unwrap()).is_err(),
                "{mutation}"
            );
        }
        wire.data.catalog.sequence = previous_catalog.payload.sequence;
        wire.data.signature.value = signed_json(&wire.data.catalog, &signing_key("online-catalog"));
        assert!(matches!(
            verify(&serde_json::to_vec(&wire).unwrap()),
            Err(PluginPlatformError::CatalogRollback)
        ));
    }

    #[test]
    fn golden_wire_verifies_the_complete_chain() {
        let (catalog_bytes, _, package) = current_fixture();
        let (root, catalog, download) = verified_chain();
        let catalog_wire: NorixorV1CatalogWire =
            serde_json::from_slice(&catalog_bytes).expect("catalog");
        let publisher_wire = super::NorixorV1PublisherWire {
            payload: catalog_wire.data.catalog.items[0].publisher_payload.clone(),
            signature: super::NorixorV1Signature {
                key_id: catalog_wire.data.catalog.items[0].publisher_key_id.clone(),
                algorithm: super::NorixorV1SignatureAlgorithm::Ed25519,
                value: catalog_wire.data.catalog.items[0]
                    .publisher_signature
                    .clone(),
            },
        };
        let publisher = verify_norixor_v1_publisher_wire(
            &serde_json::to_vec(&publisher_wire).expect("publisher wire"),
            &root,
            &Version::new(1, 0, 0),
            "aarch64",
        )
        .expect("publisher wire");
        assert_eq!(publisher, catalog.payload.items[0].publisher_payload);
        let verified = verify_norixor_v1_package(
            &package,
            &catalog.payload.items[0],
            &download,
            PackageLimits::default(),
        )
        .expect("package");
        assert_eq!(verified.package_size, package.len() as u64);
        assert_eq!(verified.package_sha256, super::sha256_hex(&package));
        let inspected = verified
            .into_inspected_package()
            .expect("inspected package");
        assert!(
            inspected
                .files
                .iter()
                .any(|file| file.relative_path == std::path::Path::new("manifest.json"))
        );
    }

    #[test]
    fn signed_theme_package_is_verified_as_exact_data_only_content() {
        let (catalog_bytes, download_bytes, package) = current_theme_fixture();
        let root = verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, None).expect("root");
        let catalog = verify_norixor_v1_catalog(
            &catalog_bytes,
            &root,
            &Version::new(1, 0, 0),
            "aarch64",
            NOW,
            None,
        )
        .expect("theme catalog");
        assert!(catalog.unsupported_contracts.is_empty());
        let download = verify_norixor_v1_download(&download_bytes, &catalog.payload.items[0])
            .expect("theme download");
        let verified = verify_norixor_v1_package(
            &package,
            &catalog.payload.items[0],
            &download,
            PackageLimits::default(),
        )
        .expect("signed theme package");
        assert_eq!(
            verified.package_kind,
            norishell_core_api::PluginPackageKind::Theme
        );
        assert_eq!(
            verified
                .theme_definition
                .as_ref()
                .expect("theme definition")
                .id,
            "clear"
        );
        let inspected = verified.into_inspected_package().expect("inspected theme");
        assert_eq!(
            inspected.package_kind,
            norishell_core_api::PluginPackageKind::Theme
        );
        assert!(inspected.settings.is_none());
        assert!(inspected.protocols.is_none());
        assert!(inspected.workflows.is_none());
    }

    #[test]
    fn expiration_and_rollback_fail_closed() {
        assert!(matches!(
            verify_norixor_v1_root(ROOT_JSON, &anchor(), 2_145_916_800, None),
            Err(PluginPlatformError::CatalogExpired)
        ));
        let (catalog_bytes, _, _) = current_fixture();
        let root = verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, None).expect("root");
        assert!(matches!(
            verify_norixor_v1_catalog(
                &catalog_bytes,
                &root,
                &Version::new(1, 0, 0),
                "aarch64",
                2_145_916_800,
                None,
            ),
            Err(PluginPlatformError::CatalogExpired)
        ));
        let catalog = verify_norixor_v1_catalog(
            &catalog_bytes,
            &root,
            &Version::new(1, 0, 0),
            "aarch64",
            NOW,
            None,
        )
        .expect("catalog");
        let mut state = catalog.trust_state();
        state.sequence += 1;
        assert!(matches!(
            verify_norixor_v1_catalog(
                &catalog_bytes,
                &root,
                &Version::new(1, 0, 0),
                "aarch64",
                NOW,
                Some(&state),
            ),
            Err(PluginPlatformError::CatalogRollback)
        ));
        let root_state = NorixorV1RootTrustState {
            version: 2,
            fingerprint_sha256: root.fingerprint_sha256,
        };
        assert!(matches!(
            verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, Some(&root_state)),
            Err(PluginPlatformError::CatalogRollback)
        ));
    }

    #[test]
    fn revoked_publisher_is_rejected_after_valid_root_resigning() {
        let mut wire: NorixorV1TrustRootWire =
            serde_json::from_slice(ROOT_JSON).expect("root json");
        wire.data
            .metadata
            .roles
            .publishers
            .revoked_key_ids
            .push("norishell-test-fixture-publisher-v1".to_owned());
        wire.data.signature.value = signed_json(&wire.data.metadata, &signing_key("offline-root"));
        let root = verify_norixor_v1_root(
            &serde_json::to_vec(&wire).expect("wire"),
            &anchor(),
            NOW,
            None,
        )
        .expect("valid resigned root");
        let (catalog_bytes, _, _) = current_fixture();
        let mut catalog_wire: NorixorV1CatalogWire =
            serde_json::from_slice(&catalog_bytes).expect("catalog json");
        catalog_wire.data.catalog.root_fingerprint = root.fingerprint_sha256.clone();
        catalog_wire.data.signature.value =
            signed_json(&catalog_wire.data.catalog, &signing_key("online-catalog"));
        assert!(matches!(
            verify_norixor_v1_catalog(
                &serde_json::to_vec(&catalog_wire).expect("catalog wire"),
                &root,
                &Version::new(1, 0, 0),
                "aarch64",
                NOW,
                None,
            ),
            Err(PluginPlatformError::InvalidNorixorPublisher)
        ));
    }

    #[test]
    fn signed_catalog_capability_and_download_route_tampering_are_rejected() {
        let (catalog_bytes, _, _) = current_fixture();
        let root = verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, None).expect("root");
        for mutate in ["capability", "download"] {
            let mut wire: NorixorV1CatalogWire =
                serde_json::from_slice(&catalog_bytes).expect("catalog json");
            if mutate == "capability" {
                wire.data.catalog.items[0].capabilities.pop();
            } else {
                wire.data.catalog.items[0].download.push_str("/tampered");
            }
            wire.data.signature.value =
                signed_json(&wire.data.catalog, &signing_key("online-catalog"));
            assert!(
                verify_norixor_v1_catalog(
                    &serde_json::to_vec(&wire).expect("wire"),
                    &root,
                    &Version::new(1, 0, 0),
                    "aarch64",
                    NOW,
                    None,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn signed_catalog_cannot_launder_tampered_publisher_proof() {
        let (catalog_bytes, _, _) = current_fixture();
        let root = verify_norixor_v1_root(ROOT_JSON, &anchor(), NOW, None).expect("root");
        for mutate in ["payload", "payload_hash", "signature"] {
            let mut wire: NorixorV1CatalogWire =
                serde_json::from_slice(&catalog_bytes).expect("catalog json");
            let item = &mut wire.data.catalog.items[0];
            match mutate {
                "payload" => item
                    .publisher_payload
                    .manifest
                    .publisher
                    .push_str(" tampered"),
                "payload_hash" => item.publisher_payload_hash.replace_range(..1, "f"),
                "signature" => item.publisher_signature.replace_range(..1, "A"),
                _ => unreachable!(),
            }
            wire.data.signature.value =
                signed_json(&wire.data.catalog, &signing_key("online-catalog"));
            assert!(matches!(
                verify_norixor_v1_catalog(
                    &serde_json::to_vec(&wire).expect("wire"),
                    &root,
                    &Version::new(1, 0, 0),
                    "aarch64",
                    NOW,
                    None,
                ),
                Err(PluginPlatformError::InvalidNorixorCatalog)
                    | Err(PluginPlatformError::InvalidNorixorPublisher)
            ));
        }
    }

    #[test]
    fn package_and_download_tampering_are_rejected() {
        let (_, download_bytes, package) = current_fixture();
        let (_, catalog, download) = verified_chain();
        let item = &catalog.payload.items[0];
        let mut package = package;
        package[100] ^= 1;
        assert!(matches!(
            verify_norixor_v1_package(&package, item, &download, PackageLimits::default()),
            Err(PluginPlatformError::PackageHashMismatch)
        ));
        let mut wire: Value = serde_json::from_slice(&download_bytes).expect("download json");
        wire["data"]["capabilities"] = json!(["storagePlugin"]);
        assert!(matches!(
            verify_norixor_v1_download(&serde_json::to_vec(&wire).expect("wire"), item),
            Err(PluginPlatformError::InvalidNorixorDownload)
        ));
        let mut wire: Value = serde_json::from_slice(&download_bytes).expect("download json");
        wire["data"]["sha256"] = json!("0".repeat(64));
        assert!(matches!(
            verify_norixor_v1_download(&serde_json::to_vec(&wire).expect("wire"), item),
            Err(PluginPlatformError::InvalidNorixorDownload)
        ));
        let mut wire: Value = serde_json::from_slice(&download_bytes).expect("download json");
        wire["data"]["url"] = json!("http://example.invalid/plugin.zip");
        assert!(matches!(
            verify_norixor_v1_download(&serde_json::to_vec(&wire).expect("wire"), item),
            Err(PluginPlatformError::InvalidNorixorDownload)
        ));
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_wire_boundary() {
        for source in [ROOT_JSON, CATALOG_JSON, PUBLISHER_JSON, DOWNLOAD_JSON] {
            let mut value: Value = serde_json::from_slice(source).expect("json");
            value
                .as_object_mut()
                .expect("object")
                .insert("unknown".to_owned(), Value::Bool(true));
            let bytes = serde_json::to_vec(&value).expect("wire");
            assert!(serde_json::from_slice::<NorixorV1TrustRootWire>(&bytes).is_err());
            assert!(serde_json::from_slice::<NorixorV1CatalogWire>(&bytes).is_err());
            assert!(serde_json::from_slice::<NorixorV1PublisherWire>(&bytes).is_err());
            assert!(serde_json::from_slice::<super::NorixorV1DownloadWire>(&bytes).is_err());
        }
        let mut manifest: Value = serde_json::from_slice(
            br#"{"pluginId":"org.example.fixture","name":"Fixture","publisher":"Fixture","publisherKeyId":"fixture","publisherSignature":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==","version":"1.0.0","protocolMajor":1,"protocolMinor":13,"platform":"desktop","architectures":["universal"],"capabilities":[],"minimumAppVersion":"1.0.0"}"#,
        )
        .expect("manifest");
        manifest["unknown"] = Value::Bool(true);
        assert!(serde_json::from_value::<super::NorixorV1PackageManifest>(manifest).is_err());
    }
}
