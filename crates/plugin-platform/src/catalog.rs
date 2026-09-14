use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use norishell_core_api::{PluginCapability, PluginCompatibility, PluginId};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{PluginPlatformError, Result};

const CATALOG_SCHEMA: &str = "norishell.plugin-catalog.v1";
const PUBLISHER_DOMAIN: &[u8] = b"NoriShell.PluginPackage.v1\0";
const ROOT_ROTATION_DOMAIN: &[u8] = b"NoriShell.CatalogRootRotation.v1\0";

#[derive(Debug, Clone)]
pub struct CatalogTrustRoots {
    keys: BTreeMap<String, VerifyingKey>,
}

impl CatalogTrustRoots {
    /// Production starts fail-closed until the application embeds reviewed roots.
    #[must_use]
    pub fn production() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Explicit fixture constructor. Application production code must not feed
    /// untrusted catalog content into this constructor.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn from_fixture_keys(keys: impl IntoIterator<Item = (String, [u8; 32])>) -> Result<Self> {
        let mut roots = BTreeMap::new();
        for (key_id, bytes) in keys {
            if key_id.is_empty() || key_id.len() > 120 || roots.contains_key(&key_id) {
                return Err(PluginPlatformError::InvalidCatalogEnvelope);
            }
            let key = VerifyingKey::from_bytes(&bytes)
                .map_err(|_| PluginPlatformError::InvalidCatalogEnvelope)?;
            roots.insert(key_id, key);
        }
        Ok(Self { keys: roots })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CatalogLimits {
    pub max_envelope_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_entries: usize,
    pub max_future_issued_skew_ms: i64,
    pub max_catalog_lifetime_ms: i64,
    pub allow_fixture_file_urls: bool,
}

impl Default for CatalogLimits {
    fn default() -> Self {
        Self {
            max_envelope_bytes: 2 * 1024 * 1024,
            max_payload_bytes: 1024 * 1024,
            max_entries: 2_000,
            max_future_issued_skew_ms: 5 * 60 * 1_000,
            max_catalog_lifetime_ms: 31 * 24 * 60 * 60 * 1_000,
            allow_fixture_file_urls: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTrustState {
    pub root_key_id: String,
    pub sequence: u64,
    pub payload_sha256: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct VerifiedCatalog {
    pub schema: String,
    pub sequence: u64,
    pub revision: String,
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub root_key_id: String,
    pub payload_sha256: [u8; 32],
    pub catalog_signature_base64: String,
    pub entries: Vec<VerifiedCatalogEntry>,
}

impl VerifiedCatalog {
    #[must_use]
    pub fn trust_state(&self) -> CatalogTrustState {
        CatalogTrustState {
            root_key_id: self.root_key_id.clone(),
            sequence: self.sequence,
            payload_sha256: self.payload_sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedCatalogEntry {
    pub plugin_id: PluginId,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub platform: String,
    pub architectures: Vec<String>,
    pub package_url: String,
    pub package_size: u64,
    pub package_sha256: String,
    pub publisher_key_base64: String,
    pub publisher_signature_base64: String,
    pub capabilities: Vec<PluginCapability>,
    pub minimum_app_version: String,
    pub published_at_unix_ms: i64,
}

impl VerifiedCatalogEntry {
    pub fn compatibility(&self, current_app_version: &Version) -> Result<PluginCompatibility> {
        if !norishell_core_api::plugin_package_protocol_is_compatible(
            self.protocol_major,
            self.protocol_minor,
        ) || (self.protocol_minor == norishell_core_api::PLUGIN_THEME_PROTOCOL_MINOR
            && !self.capabilities.is_empty())
            || self.capabilities.iter().any(|capability| {
                norishell_core_api::plugin_capability_min_protocol_minor(*capability)
                    > self.protocol_minor
            })
        {
            return Ok(PluginCompatibility::ProtocolIncompatible);
        }
        let minimum = Version::parse(&self.minimum_app_version)
            .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
        if current_app_version < &minimum {
            return Ok(PluginCompatibility::AppVersionIncompatible);
        }
        Ok(PluginCompatibility::Compatible)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogEnvelope {
    key_id: String,
    payload_base64: String,
    signature_base64: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogPayload {
    schema: String,
    sequence: u64,
    revision: String,
    issued_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    root_rotation: Option<RootRotationAuthorization>,
    entries: Vec<VerifiedCatalogEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RootRotationAuthorization {
    previous_root_key_id: String,
    previous_sequence: u64,
    previous_payload_sha256: String,
    new_root_key_id: String,
    activation_sequence: u64,
    authorization_signature_base64: String,
}

pub fn verify_catalog(
    envelope_json: &[u8],
    trust_roots: &CatalogTrustRoots,
    current_app_version: &Version,
    now_unix_ms: i64,
    previous: Option<&CatalogTrustState>,
    limits: CatalogLimits,
) -> Result<VerifiedCatalog> {
    if trust_roots.is_empty() {
        return Err(PluginPlatformError::TrustRootsUnavailable);
    }
    if envelope_json.len() > limits.max_envelope_bytes
        || limits.max_future_issued_skew_ms < 0
        || limits.max_catalog_lifetime_ms <= 0
    {
        return Err(PluginPlatformError::InvalidCatalogEnvelope);
    }
    let envelope: CatalogEnvelope = serde_json::from_slice(envelope_json)
        .map_err(|_| PluginPlatformError::InvalidCatalogEnvelope)?;
    let root = trust_roots
        .keys
        .get(&envelope.key_id)
        .ok_or(PluginPlatformError::InvalidCatalogSignature)?;
    let payload = BASE64
        .decode(envelope.payload_base64)
        .map_err(|_| PluginPlatformError::InvalidCatalogEnvelope)?;
    if payload.len() > limits.max_payload_bytes {
        return Err(PluginPlatformError::InvalidCatalogPayload);
    }
    let signature = decode_signature(&envelope.signature_base64)
        .map_err(|_| PluginPlatformError::InvalidCatalogEnvelope)?;
    root.verify_strict(&payload, &signature)
        .map_err(|_| PluginPlatformError::InvalidCatalogSignature)?;
    let payload_sha256: [u8; 32] = Sha256::digest(&payload).into();
    let parsed: CatalogPayload =
        serde_json::from_slice(&payload).map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
    if parsed.schema != CATALOG_SCHEMA
        || parsed.sequence == 0
        || parsed.revision.is_empty()
        || parsed.revision.len() > 120
        || parsed.issued_at_unix_ms <= 0
        || parsed.issued_at_unix_ms > now_unix_ms.saturating_add(limits.max_future_issued_skew_ms)
        || parsed.expires_at_unix_ms <= parsed.issued_at_unix_ms
        || parsed
            .expires_at_unix_ms
            .saturating_sub(parsed.issued_at_unix_ms)
            > limits.max_catalog_lifetime_ms
        || parsed.expires_at_unix_ms <= now_unix_ms
        || parsed.entries.len() > limits.max_entries
    {
        return Err(if parsed.expires_at_unix_ms <= now_unix_ms {
            PluginPlatformError::CatalogExpired
        } else {
            PluginPlatformError::InvalidCatalogPayload
        });
    }
    verify_root_transition(
        &envelope.key_id,
        parsed.sequence,
        parsed.root_rotation.as_ref(),
        previous,
        trust_roots,
    )?;
    if let Some(previous) = previous
        && (parsed.sequence < previous.sequence
            || (parsed.sequence == previous.sequence
                && (envelope.key_id != previous.root_key_id
                    || payload_sha256 != previous.payload_sha256)))
    {
        return Err(PluginPlatformError::CatalogRollback);
    }
    let mut identities = BTreeSet::new();
    for entry in &parsed.entries {
        validate_entry(entry, current_app_version, limits.allow_fixture_file_urls)?;
        if !identities.insert((entry.plugin_id.clone(), entry.version.clone())) {
            return Err(PluginPlatformError::InvalidCatalogPayload);
        }
    }
    Ok(VerifiedCatalog {
        schema: parsed.schema,
        sequence: parsed.sequence,
        revision: parsed.revision,
        issued_at_unix_ms: parsed.issued_at_unix_ms,
        expires_at_unix_ms: parsed.expires_at_unix_ms,
        root_key_id: envelope.key_id,
        payload_sha256,
        catalog_signature_base64: envelope.signature_base64,
        entries: parsed.entries,
    })
}

fn verify_root_transition(
    envelope_root_key_id: &str,
    sequence: u64,
    rotation: Option<&RootRotationAuthorization>,
    previous: Option<&CatalogTrustState>,
    trust_roots: &CatalogTrustRoots,
) -> Result<()> {
    let Some(previous) = previous else {
        return if rotation.is_none() {
            Ok(())
        } else {
            Err(PluginPlatformError::CatalogRollback)
        };
    };
    if envelope_root_key_id == previous.root_key_id {
        return if rotation.is_none() {
            Ok(())
        } else {
            Err(PluginPlatformError::CatalogRollback)
        };
    }
    let rotation = rotation.ok_or(PluginPlatformError::CatalogRollback)?;
    let expected_activation = previous
        .sequence
        .checked_add(1)
        .ok_or(PluginPlatformError::CatalogRollback)?;
    if sequence != expected_activation
        || rotation.previous_root_key_id != previous.root_key_id
        || rotation.previous_sequence != previous.sequence
        || rotation.previous_payload_sha256 != crate::lower_hex(&previous.payload_sha256)
        || rotation.new_root_key_id != envelope_root_key_id
        || rotation.activation_sequence != sequence
    {
        return Err(PluginPlatformError::CatalogRollback);
    }
    let old_root = trust_roots
        .keys
        .get(&previous.root_key_id)
        .ok_or(PluginPlatformError::InvalidCatalogSignature)?;
    let signature = decode_signature(&rotation.authorization_signature_base64)
        .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
    old_root
        .verify_strict(
            &root_rotation_signature_message(previous, envelope_root_key_id, sequence)?,
            &signature,
        )
        .map_err(|_| PluginPlatformError::InvalidCatalogSignature)
}

fn root_rotation_signature_message(
    previous: &CatalogTrustState,
    new_root_key_id: &str,
    activation_sequence: u64,
) -> Result<Vec<u8>> {
    let mut message = ROOT_ROTATION_DOMAIN.to_vec();
    push_field(&mut message, previous.root_key_id.as_bytes())?;
    push_field(&mut message, &previous.sequence.to_be_bytes())?;
    push_field(&mut message, &previous.payload_sha256)?;
    push_field(&mut message, new_root_key_id.as_bytes())?;
    push_field(&mut message, &activation_sequence.to_be_bytes())?;
    Ok(message)
}

fn validate_entry(
    entry: &VerifiedCatalogEntry,
    current_app_version: &Version,
    allow_fixture_file_urls: bool,
) -> Result<()> {
    if entry.name.is_empty()
        || entry.name.len() > 160
        || entry.publisher.is_empty()
        || entry.publisher.len() > 160
        || entry.version.len() > 80
        || entry.package_size == 0
        || entry.package_size > 256 * 1024 * 1024
        || entry.package_sha256.len() != 64
        || hex_bytes(&entry.package_sha256).is_err()
        || entry.architectures.is_empty()
        || entry.architectures.len() > 8
        || entry.capabilities.len() > 5
        || entry.platform.is_empty()
        || !valid_package_url(&entry.package_url, allow_fixture_file_urls)
        || Version::parse(&entry.version).is_err()
        || entry.compatibility(current_app_version).is_err()
    {
        return Err(PluginPlatformError::InvalidCatalogPayload);
    }
    let unique_capabilities = entry.capabilities.iter().collect::<BTreeSet<_>>();
    let unique_architectures = entry.architectures.iter().collect::<BTreeSet<_>>();
    if unique_capabilities.len() != entry.capabilities.len()
        || unique_architectures.len() != entry.architectures.len()
        || decode_key(&entry.publisher_key_base64).is_err()
        || decode_signature(&entry.publisher_signature_base64).is_err()
    {
        return Err(PluginPlatformError::InvalidCatalogPayload);
    }
    Ok(())
}

pub(crate) fn publisher_signature_message(entry: &VerifiedCatalogEntry) -> Result<Vec<u8>> {
    let hash = hex_bytes(&entry.package_sha256)?;
    let publisher_key = BASE64
        .decode(&entry.publisher_key_base64)
        .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
    let mut message = PUBLISHER_DOMAIN.to_vec();
    for field in [
        entry.plugin_id.as_str().as_bytes(),
        entry.name.as_bytes(),
        entry.publisher.as_bytes(),
        entry.version.as_bytes(),
        entry.platform.as_bytes(),
        entry.package_url.as_bytes(),
        entry.minimum_app_version.as_bytes(),
    ] {
        push_field(&mut message, field)?;
    }
    push_field(&mut message, &entry.protocol_major.to_be_bytes())?;
    push_field(&mut message, &entry.protocol_minor.to_be_bytes())?;
    push_field(&mut message, &entry.package_size.to_be_bytes())?;
    push_field(&mut message, &hash)?;
    push_field(&mut message, &publisher_key)?;
    push_field(
        &mut message,
        &u32::try_from(entry.architectures.len())
            .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?
            .to_be_bytes(),
    )?;
    for architecture in &entry.architectures {
        push_field(&mut message, architecture.as_bytes())?;
    }
    push_field(
        &mut message,
        &u32::try_from(entry.capabilities.len())
            .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?
            .to_be_bytes(),
    )?;
    for capability in &entry.capabilities {
        let value = serde_json::to_vec(capability)
            .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
        push_field(&mut message, &value)?;
    }
    Ok(message)
}

fn valid_package_url(value: &str, allow_fixture_file_urls: bool) -> bool {
    if value.is_empty()
        || value.len() > 2_048
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return false;
    }
    if allow_fixture_file_urls && value.starts_with("file:///") {
        return true;
    }
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty() && !authority.contains('@') && !authority.starts_with('.')
}

fn push_field(target: &mut Vec<u8>, field: &[u8]) -> Result<()> {
    let length =
        u32::try_from(field.len()).map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(field);
    Ok(())
}

pub(crate) fn decode_key(value: &str) -> Result<VerifyingKey> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| PluginPlatformError::InvalidCatalogPayload)
}

pub(crate) fn decode_signature(value: &str) -> Result<Signature> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| PluginPlatformError::InvalidCatalogPayload)?;
    Signature::from_slice(&decoded).map_err(|_| PluginPlatformError::InvalidCatalogPayload)
}

pub(crate) fn hex_bytes(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(PluginPlatformError::InvalidCatalogPayload);
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| PluginPlatformError::InvalidCatalogPayload)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use ed25519_dalek::{Signer as _, SigningKey};
    use norishell_core_api::{
        PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginCapability, PluginCompatibility,
        PluginId,
    };
    use semver::Version;

    use super::{CatalogLimits, CatalogTrustRoots, VerifiedCatalogEntry, verify_catalog};

    #[test]
    fn production_roots_are_deliberately_empty_until_release_configuration() {
        assert!(CatalogTrustRoots::production().is_empty());
    }

    #[test]
    fn catalog_compatibility_accepts_only_current_minor() {
        let mut entry = VerifiedCatalogEntry {
            plugin_id: PluginId::parse("com.norishell.fixture").expect("plugin id"),
            name: "Fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            version: "1.0.0".to_owned(),
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            platform: "desktop".to_owned(),
            architectures: vec!["universal".to_owned()],
            package_url: "https://plugins.example.test/fixture.zip".to_owned(),
            package_size: 42,
            package_sha256: "a".repeat(64),
            publisher_key_base64: String::new(),
            publisher_signature_base64: String::new(),
            capabilities: vec![PluginCapability::UiPanel],
            minimum_app_version: "0.1.0".to_owned(),
            published_at_unix_ms: 1,
        };
        entry.protocol_minor = PLUGIN_PROTOCOL_MINOR;
        assert_eq!(
            entry
                .compatibility(&Version::new(0, 1, 0))
                .expect("current minor compatibility"),
            PluginCompatibility::Compatible
        );

        entry.protocol_minor = PLUGIN_PROTOCOL_MINOR.saturating_sub(1);
        assert_eq!(
            entry
                .compatibility(&Version::new(0, 1, 0))
                .expect("non-current minor compatibility"),
            PluginCompatibility::ProtocolIncompatible
        );
    }

    #[test]
    fn catalog_signature_covers_raw_payload_and_sequence_prevents_rollback() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let roots = CatalogTrustRoots::from_fixture_keys([(
            "test-root".to_owned(),
            signing.verifying_key().to_bytes(),
        )])
        .expect("fixture roots");
        let entry = VerifiedCatalogEntry {
            plugin_id: PluginId::parse("com.norishell.fixture").expect("plugin id"),
            name: "Fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            version: "1.0.0".to_owned(),
            protocol_major: 1,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            platform: "desktop".to_owned(),
            architectures: vec!["universal".to_owned()],
            package_url: "https://plugins.example.test/fixture.zip".to_owned(),
            package_size: 42,
            package_sha256: "a".repeat(64),
            publisher_key_base64: BASE64.encode(signing.verifying_key().as_bytes()),
            publisher_signature_base64: BASE64.encode([0_u8; 64]),
            capabilities: vec![PluginCapability::UiPanel],
            minimum_app_version: "0.1.0".to_owned(),
            published_at_unix_ms: 1,
        };
        let payload = serde_json::to_vec(&serde_json::json!({
            "schema": "norishell.plugin-catalog.v1",
            "sequence": 5,
            "revision": "fixture-5",
            "issuedAtUnixMs": 1,
            "expiresAtUnixMs": 10_000,
            "entries": [entry],
        }))
        .expect("payload");
        let envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "test-root",
            "payloadBase64": BASE64.encode(&payload),
            "signatureBase64": BASE64.encode(signing.sign(&payload).to_bytes()),
        }))
        .expect("envelope");
        let verified = verify_catalog(
            &envelope,
            &roots,
            &Version::new(0, 1, 0),
            100,
            None,
            CatalogLimits::default(),
        )
        .expect("verified catalog");
        assert_eq!(verified.sequence, 5);
        assert!(
            verify_catalog(
                &envelope,
                &roots,
                &Version::new(0, 1, 0),
                100,
                Some(&super::CatalogTrustState {
                    root_key_id: "test-root".to_owned(),
                    sequence: 6,
                    payload_sha256: [9; 32],
                }),
                CatalogLimits::default(),
            )
            .is_err()
        );

        let mut tampered: serde_json::Value = serde_json::from_slice(&envelope).expect("json");
        tampered["payloadBase64"] = serde_json::Value::String(BASE64.encode(b"{}"));
        assert!(
            verify_catalog(
                &serde_json::to_vec(&tampered).expect("tampered"),
                &roots,
                &Version::new(0, 1, 0),
                100,
                None,
                CatalogLimits::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn issued_at_is_bounded_and_root_rotation_requires_exact_old_root_authorization() {
        let old = SigningKey::from_bytes(&[7; 32]);
        let new = SigningKey::from_bytes(&[8; 32]);
        let roots = CatalogTrustRoots::from_fixture_keys([
            ("old-root".to_owned(), old.verifying_key().to_bytes()),
            ("new-root".to_owned(), new.verifying_key().to_bytes()),
        ])
        .expect("fixture roots");
        let initial_payload = serde_json::to_vec(&serde_json::json!({
            "schema": "norishell.plugin-catalog.v1",
            "sequence": 5,
            "revision": "old-5",
            "issuedAtUnixMs": 100,
            "expiresAtUnixMs": 10_000,
            "entries": [],
        }))
        .expect("initial payload");
        let initial_envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "old-root",
            "payloadBase64": BASE64.encode(&initial_payload),
            "signatureBase64": BASE64.encode(old.sign(&initial_payload).to_bytes()),
        }))
        .expect("initial envelope");
        let initial = verify_catalog(
            &initial_envelope,
            &roots,
            &Version::new(0, 1, 0),
            100,
            None,
            CatalogLimits::default(),
        )
        .expect("initial catalog");
        let previous = initial.trust_state();
        let authorization = old.sign(
            &super::root_rotation_signature_message(&previous, "new-root", 6)
                .expect("rotation message"),
        );
        let rotation = serde_json::json!({
            "previousRootKeyId": "old-root",
            "previousSequence": 5,
            "previousPayloadSha256": crate::lower_hex(&previous.payload_sha256),
            "newRootKeyId": "new-root",
            "activationSequence": 6,
            "authorizationSignatureBase64": BASE64.encode(authorization.to_bytes()),
        });
        let rotated_payload = serde_json::to_vec(&serde_json::json!({
            "schema": "norishell.plugin-catalog.v1",
            "sequence": 6,
            "revision": "new-6",
            "issuedAtUnixMs": 101,
            "expiresAtUnixMs": 10_000,
            "rootRotation": rotation,
            "entries": [],
        }))
        .expect("rotated payload");
        let rotated_envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "new-root",
            "payloadBase64": BASE64.encode(&rotated_payload),
            "signatureBase64": BASE64.encode(new.sign(&rotated_payload).to_bytes()),
        }))
        .expect("rotated envelope");
        let rotated = verify_catalog(
            &rotated_envelope,
            &roots,
            &Version::new(0, 1, 0),
            101,
            Some(&previous),
            CatalogLimits::default(),
        )
        .expect("authorized rotation");
        assert_eq!(rotated.root_key_id, "new-root");
        let mismatched_previous = super::CatalogTrustState {
            payload_sha256: [0; 32],
            ..previous.clone()
        };
        assert!(
            verify_catalog(
                &rotated_envelope,
                &roots,
                &Version::new(0, 1, 0),
                101,
                Some(&mismatched_previous),
                CatalogLimits::default(),
            )
            .is_err(),
            "rotation authorization must match the exact prior trust state"
        );
        assert!(
            verify_catalog(
                &rotated_envelope,
                &roots,
                &Version::new(0, 1, 0),
                101,
                Some(&rotated.trust_state()),
                CatalogLimits::default(),
            )
            .is_err(),
            "a consumed rotation cannot be replayed"
        );

        let missing_authorization_payload = serde_json::to_vec(&serde_json::json!({
            "schema": "norishell.plugin-catalog.v1",
            "sequence": 6,
            "revision": "new-6-missing",
            "issuedAtUnixMs": 101,
            "expiresAtUnixMs": 10_000,
            "entries": [],
        }))
        .expect("missing authorization payload");
        let missing_authorization_envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "new-root",
            "payloadBase64": BASE64.encode(&missing_authorization_payload),
            "signatureBase64": BASE64.encode(new.sign(&missing_authorization_payload).to_bytes()),
        }))
        .expect("missing authorization envelope");
        assert!(
            verify_catalog(
                &missing_authorization_envelope,
                &roots,
                &Version::new(0, 1, 0),
                101,
                Some(&previous),
                CatalogLimits::default(),
            )
            .is_err()
        );

        let jump_authorization = old.sign(
            &super::root_rotation_signature_message(&previous, "new-root", 7)
                .expect("jump message"),
        );
        let jump_payload = serde_json::to_vec(&serde_json::json!({
            "schema": "norishell.plugin-catalog.v1",
            "sequence": 7,
            "revision": "jump-7",
            "issuedAtUnixMs": 101,
            "expiresAtUnixMs": 10_000,
            "rootRotation": {
                "previousRootKeyId": "old-root",
                "previousSequence": 5,
                "previousPayloadSha256": crate::lower_hex(&previous.payload_sha256),
                "newRootKeyId": "new-root",
                "activationSequence": 7,
                "authorizationSignatureBase64": BASE64.encode(jump_authorization.to_bytes()),
            },
            "entries": [],
        }))
        .expect("jump payload");
        let jump_envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "new-root",
            "payloadBase64": BASE64.encode(&jump_payload),
            "signatureBase64": BASE64.encode(new.sign(&jump_payload).to_bytes()),
        }))
        .expect("jump envelope");
        assert!(
            verify_catalog(
                &jump_envelope,
                &roots,
                &Version::new(0, 1, 0),
                101,
                Some(&previous),
                CatalogLimits::default(),
            )
            .is_err(),
            "a rotation activation sequence cannot jump"
        );

        let same_root_payload = serde_json::to_vec(&serde_json::json!({
            "schema": "norishell.plugin-catalog.v1",
            "sequence": 6,
            "revision": "same-root-rotation",
            "issuedAtUnixMs": 101,
            "expiresAtUnixMs": 10_000,
            "rootRotation": {
                "previousRootKeyId": "old-root",
                "previousSequence": 5,
                "previousPayloadSha256": crate::lower_hex(&previous.payload_sha256),
                "newRootKeyId": "new-root",
                "activationSequence": 6,
                "authorizationSignatureBase64": BASE64.encode(authorization.to_bytes()),
            },
            "entries": [],
        }))
        .expect("same-root payload");
        let same_root_envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "old-root",
            "payloadBase64": BASE64.encode(&same_root_payload),
            "signatureBase64": BASE64.encode(old.sign(&same_root_payload).to_bytes()),
        }))
        .expect("same-root envelope");
        assert!(
            verify_catalog(
                &same_root_envelope,
                &roots,
                &Version::new(0, 1, 0),
                101,
                Some(&previous),
                CatalogLimits::default(),
            )
            .is_err(),
            "same-root catalogs must not carry rotation authorization"
        );

        let unknown_root_envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "unknown-root",
            "payloadBase64": BASE64.encode(&rotated_payload),
            "signatureBase64": BASE64.encode(new.sign(&rotated_payload).to_bytes()),
        }))
        .expect("unknown-root envelope");
        assert!(
            verify_catalog(
                &unknown_root_envelope,
                &roots,
                &Version::new(0, 1, 0),
                101,
                Some(&previous),
                CatalogLimits::default(),
            )
            .is_err(),
            "unknown embedded roots must fail closed"
        );

        let future_payload = serde_json::to_vec(&serde_json::json!({
            "schema": "norishell.plugin-catalog.v1",
            "sequence": 6,
            "revision": "future",
            "issuedAtUnixMs": 1_000_000,
            "expiresAtUnixMs": 1_001_000,
            "entries": [],
        }))
        .expect("future payload");
        let future_envelope = serde_json::to_vec(&serde_json::json!({
            "keyId": "old-root",
            "payloadBase64": BASE64.encode(&future_payload),
            "signatureBase64": BASE64.encode(old.sign(&future_payload).to_bytes()),
        }))
        .expect("future envelope");
        assert!(
            verify_catalog(
                &future_envelope,
                &roots,
                &Version::new(0, 1, 0),
                100,
                Some(&previous),
                CatalogLimits::default(),
            )
            .is_err(),
            "issued-at beyond the configured clock skew must fail closed"
        );
    }
}
