use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    BundleSchema, EncryptedSyncObject, PortableBundleV1, RecoveryOwnerBinding, RecoveryPassword,
    Result, SyncCodecError, SyncKey, SyncObjectBinding, SyncObjectKind,
    create_recovery_envelope_random, decrypt_bundle_from_service, encrypt_bundle_for_service,
    open_recovery_envelope,
};

const EXCHANGE_FORMAT: &str = "norishell-ssh-sync-exchange-v2";
const VAULT_EXCHANGE_FORMAT: &str = "norishell-ssh-vault-exchange-v3";
const MAX_EXCHANGE_BYTES: usize = 96 * 1024 * 1024;

/// Stable data namespace for new exchanges. Package hashes remain separate
/// authorization identities and must never be substituted by this value there.
pub fn stable_plugin_data_owner(plugin_id: &str) -> Result<String> {
    validate_identifier(plugin_id, 160)?;
    Ok(sha256_hex(
        format!("norishell:ssh-sync-owner:v1\0{plugin_id}").as_bytes(),
    ))
}

/// Public, non-secret Core binding for one sync plugin and provider profile.
/// The signer-shaped field is the stable data owner for new exchanges and the
/// original package hash for legacy exchanges. It is never an authorization
/// identity; both formats bind it into the authenticated ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginExchangeBinding {
    pub plugin_id: String,
    pub signer_fingerprint_sha256: String,
    pub profile_id: String,
    pub revision: u64,
    pub base_revision: Option<u64>,
    pub base_etag: Option<String>,
}

impl PluginExchangeBinding {
    pub fn validate(&self) -> Result<()> {
        validate_identifier(&self.plugin_id, 160)?;
        if !valid_sha256(&self.signer_fingerprint_sha256) {
            return Err(SyncCodecError::InvalidBundle("invalid signer fingerprint"));
        }
        validate_identifier(&self.profile_id, 160)?;
        if self.revision == 0
            || self
                .base_revision
                .is_some_and(|value| value >= self.revision)
        {
            return Err(SyncCodecError::InvalidBundle("invalid exchange revision"));
        }
        if self.base_etag.as_ref().is_some_and(|value| {
            value.is_empty() || value.len() > 1_024 || value.chars().any(char::is_control)
        }) {
            return Err(SyncCodecError::InvalidBundle("invalid exchange base etag"));
        }
        Ok(())
    }

    fn owner(&self) -> ExchangeOwnerWire {
        ExchangeOwnerWire {
            plugin_id: self.plugin_id.clone(),
            signer_fingerprint_sha256: self.signer_fingerprint_sha256.clone(),
            profile_id: self.profile_id.clone(),
        }
    }

    fn owner_id(&self) -> String {
        format!(
            "plugin:{}:{}:signer:{}:profile:{}:{}",
            self.plugin_id.len(),
            self.plugin_id,
            self.signer_fingerprint_sha256,
            self.profile_id.len(),
            self.profile_id,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginExchangeSummary {
    pub schema: BundleSchema,
    pub revision: u64,
    pub base_revision: Option<u64>,
    pub base_etag: Option<String>,
    pub key_version: u32,
    pub ciphertext_sha256: String,
    pub exchange_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExchangeWire {
    format: String,
    owner: ExchangeOwnerWire,
    binding: SyncObjectBinding,
    recovery: RecoveryWire,
    encrypted: EncryptedWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExchangeOwnerWire {
    plugin_id: String,
    signer_fingerprint_sha256: String,
    profile_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryWire {
    envelope: String,
    envelope_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptedWire {
    algorithm: String,
    nonce: String,
    ciphertext: String,
    ciphertext_sha256: String,
}

pub fn create_plugin_exchange(
    bundle: &PortableBundleV1,
    password: &RecoveryPassword,
    binding: &PluginExchangeBinding,
) -> Result<Vec<u8>> {
    binding.validate()?;
    if !matches!(
        bundle.schema,
        BundleSchema::V3 | BundleSchema::V4 | BundleSchema::V5 | BundleSchema::V6
    ) || bundle.revision != binding.revision
    {
        return Err(SyncCodecError::BindingMismatch);
    }
    let key_version = 1;
    let object_binding = new_object_binding(binding, key_version, bundle.schema);
    let owner = RecoveryOwnerBinding {
        application_id: "norishell".to_owned(),
        account_id: binding.owner_id(),
        key_version,
    };
    let key = SyncKey::generate()?;
    let recovery = create_recovery_envelope_random(&key, password, &owner)?;
    let encrypted = encrypt_bundle_for_service(bundle, &key, &object_binding)?;
    let wire = ExchangeWire {
        format: EXCHANGE_FORMAT.to_owned(),
        owner: binding.owner(),
        binding: object_binding,
        recovery: RecoveryWire {
            envelope_sha256: sha256_hex(&recovery),
            envelope: BASE64.encode(recovery),
        },
        encrypted: EncryptedWire {
            algorithm: encrypted.algorithm.to_owned(),
            nonce: encrypted.nonce_base64url,
            ciphertext: BASE64.encode(encrypted.ciphertext),
            ciphertext_sha256: encrypted.ciphertext_sha256,
        },
    };
    let bytes = serde_json::to_vec(&wire)?;
    if bytes.len() > MAX_EXCHANGE_BYTES {
        return Err(SyncCodecError::BoundExceeded("plugin exchange"));
    }
    Ok(bytes)
}

/// Creates an exchange with a stable account sync key whose recovery envelope
/// is the originating Vault's password-wrapped key material. The envelope is
/// opaque to this crate and never contains a Vault payload or device slot.
pub fn create_plugin_exchange_with_key(
    bundle: &PortableBundleV1,
    key: &SyncKey,
    vault_key_envelope: &[u8],
    binding: &PluginExchangeBinding,
) -> Result<Vec<u8>> {
    binding.validate()?;
    if !matches!(
        bundle.schema,
        BundleSchema::V3 | BundleSchema::V4 | BundleSchema::V5 | BundleSchema::V6
    ) || bundle.revision != binding.revision
        || vault_key_envelope.is_empty()
        || vault_key_envelope.len() > 64 * 1024
    {
        return Err(SyncCodecError::BindingMismatch);
    }
    let key_version = 1;
    let object_binding = new_object_binding(binding, key_version, bundle.schema);
    let encrypted = encrypt_bundle_for_service(bundle, key, &object_binding)?;
    let wire = ExchangeWire {
        format: VAULT_EXCHANGE_FORMAT.to_owned(),
        owner: binding.owner(),
        binding: object_binding,
        recovery: RecoveryWire {
            envelope_sha256: sha256_hex(vault_key_envelope),
            envelope: BASE64.encode(vault_key_envelope),
        },
        encrypted: EncryptedWire {
            algorithm: encrypted.algorithm.to_owned(),
            nonce: encrypted.nonce_base64url,
            ciphertext: BASE64.encode(encrypted.ciphertext),
            ciphertext_sha256: encrypted.ciphertext_sha256,
        },
    };
    let bytes = serde_json::to_vec(&wire)?;
    if bytes.len() > MAX_EXCHANGE_BYTES {
        return Err(SyncCodecError::BoundExceeded("plugin exchange"));
    }
    Ok(bytes)
}

pub fn inspect_plugin_exchange(
    bytes: &[u8],
    expected: &PluginExchangeBinding,
) -> Result<PluginExchangeSummary> {
    let wire = parse_wire(bytes)?;
    validate_wire(&wire, expected)?;
    Ok(PluginExchangeSummary {
        schema: wire.binding.schema,
        revision: wire.binding.revision,
        base_revision: wire.binding.base_revision,
        base_etag: wire.binding.base_etag.clone(),
        key_version: wire.binding.key_version,
        ciphertext_sha256: wire.encrypted.ciphertext_sha256.clone(),
        exchange_sha256: sha256_hex(bytes),
    })
}

/// Reads only authenticated-envelope metadata needed to discover a downloaded
/// exchange revision on a new device. The owner remains fixed to the requesting
/// plugin/profile; plaintext is not opened by this function.
pub fn inspect_plugin_exchange_owner(
    bytes: &[u8],
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
) -> Result<(PluginExchangeBinding, PluginExchangeSummary)> {
    let wire = parse_wire(bytes)?;
    inspect_wire_owner(
        wire,
        bytes,
        plugin_id,
        signer_fingerprint_sha256,
        profile_id,
    )
}

fn inspect_wire_owner(
    wire: ExchangeWire,
    bytes: &[u8],
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
) -> Result<(PluginExchangeBinding, PluginExchangeSummary)> {
    let binding = PluginExchangeBinding {
        plugin_id: plugin_id.to_owned(),
        signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
        profile_id: profile_id.to_owned(),
        revision: wire.binding.revision,
        base_revision: wire.binding.base_revision,
        base_etag: wire.binding.base_etag.clone(),
    };
    validate_wire_any_supported(&wire, &binding)?;
    let summary = PluginExchangeSummary {
        schema: wire.binding.schema,
        revision: wire.binding.revision,
        base_revision: wire.binding.base_revision,
        base_etag: wire.binding.base_etag,
        key_version: wire.binding.key_version,
        ciphertext_sha256: wire.encrypted.ciphertext_sha256,
        exchange_sha256: sha256_hex(bytes),
    };
    Ok((binding, summary))
}

/// Discovers the owner recorded by a legacy package-bound exchange. The
/// result is untrusted until the caller opens the encrypted body with the
/// sync key; only plugin and profile are selected by the caller here.
pub fn inspect_plugin_exchange_data_owner(
    bytes: &[u8],
    plugin_id: &str,
    profile_id: &str,
) -> Result<(PluginExchangeBinding, PluginExchangeSummary)> {
    let wire = parse_wire(bytes)?;
    let signer = wire.owner.signer_fingerprint_sha256.clone();
    inspect_wire_owner(wire, bytes, plugin_id, &signer, profile_id)
}

/// Returns the opaque Vault key envelope from a v3 exchange after validating
/// its owner binding and hashes. No ciphertext is decrypted here.
pub fn plugin_exchange_vault_key_envelope(
    bytes: &[u8],
    expected: &PluginExchangeBinding,
) -> Result<Vec<u8>> {
    let wire = parse_wire(bytes)?;
    validate_wire_with_format(&wire, expected, VAULT_EXCHANGE_FORMAT)?;
    let envelope = decode_base64(&wire.recovery.envelope, 64 * 1024)?;
    if sha256_hex(&envelope) != wire.recovery.envelope_sha256 {
        return Err(SyncCodecError::AuthenticationFailed);
    }
    Ok(envelope)
}

pub fn open_plugin_exchange_with_key(
    bytes: &[u8],
    key: &SyncKey,
    expected: &PluginExchangeBinding,
) -> Result<PortableBundleV1> {
    let wire = parse_wire(bytes)?;
    validate_wire_with_format(&wire, expected, VAULT_EXCHANGE_FORMAT)?;
    let encrypted = EncryptedSyncObject {
        algorithm: sync_algorithm(&wire.encrypted.algorithm)?,
        nonce_base64url: wire.encrypted.nonce.clone(),
        ciphertext: decode_base64(&wire.encrypted.ciphertext, 64 * 1024 * 1024 + 16)?,
        ciphertext_sha256: wire.encrypted.ciphertext_sha256,
    };
    decrypt_bundle_from_service(&encrypted, key, &wire.binding)
}

pub fn open_plugin_exchange(
    bytes: &[u8],
    password: &RecoveryPassword,
    expected: &PluginExchangeBinding,
) -> Result<PortableBundleV1> {
    let wire = parse_wire(bytes)?;
    validate_wire(&wire, expected)?;
    let recovery = decode_base64(&wire.recovery.envelope, 64 * 1024)?;
    if sha256_hex(&recovery) != wire.recovery.envelope_sha256 {
        return Err(SyncCodecError::AuthenticationFailed);
    }
    let owner = RecoveryOwnerBinding {
        application_id: "norishell".to_owned(),
        account_id: expected.owner_id(),
        key_version: wire.binding.key_version,
    };
    let key = open_recovery_envelope(&recovery, password, &owner)?;
    let encrypted = EncryptedSyncObject {
        algorithm: sync_algorithm(&wire.encrypted.algorithm)?,
        nonce_base64url: wire.encrypted.nonce.clone(),
        ciphertext: decode_base64(&wire.encrypted.ciphertext, 64 * 1024 * 1024 + 16)?,
        ciphertext_sha256: wire.encrypted.ciphertext_sha256,
    };
    decrypt_bundle_from_service(&encrypted, &key, &wire.binding)
}

fn parse_wire(bytes: &[u8]) -> Result<ExchangeWire> {
    if bytes.is_empty() || bytes.len() > MAX_EXCHANGE_BYTES {
        return Err(SyncCodecError::BoundExceeded("plugin exchange"));
    }
    Ok(serde_json::from_slice(bytes)?)
}

fn validate_wire(wire: &ExchangeWire, expected: &PluginExchangeBinding) -> Result<()> {
    validate_wire_with_format(wire, expected, EXCHANGE_FORMAT)
}

fn validate_wire_any_supported(
    wire: &ExchangeWire,
    expected: &PluginExchangeBinding,
) -> Result<()> {
    if wire.format != EXCHANGE_FORMAT && wire.format != VAULT_EXCHANGE_FORMAT {
        return Err(SyncCodecError::BindingMismatch);
    }
    validate_wire_common(wire, expected)
}

fn validate_wire_with_format(
    wire: &ExchangeWire,
    expected: &PluginExchangeBinding,
    format: &str,
) -> Result<()> {
    if wire.format != format {
        return Err(SyncCodecError::BindingMismatch);
    }
    validate_wire_common(wire, expected)
}

fn validate_wire_common(wire: &ExchangeWire, expected: &PluginExchangeBinding) -> Result<()> {
    expected.validate()?;
    let expected_binding = authenticated_object_binding(expected, 1, wire.binding.schema)?;
    if wire.owner != expected.owner() || wire.binding != expected_binding {
        return Err(SyncCodecError::BindingMismatch);
    }
    if wire.encrypted.algorithm != crate::crypto::SERVICE_SYNC_ALGORITHM
        || !canonical_base64(&wire.recovery.envelope)
        || !canonical_base64(&wire.encrypted.ciphertext)
        || !canonical_base64url(&wire.encrypted.nonce)
        || !valid_sha256(&wire.recovery.envelope_sha256)
        || !valid_sha256(&wire.encrypted.ciphertext_sha256)
    {
        return Err(SyncCodecError::AuthenticationFailed);
    }
    Ok(())
}

fn new_object_binding(
    binding: &PluginExchangeBinding,
    key_version: u32,
    schema: BundleSchema,
) -> SyncObjectBinding {
    object_binding_for_schema(binding, key_version, schema)
}

fn authenticated_object_binding(
    binding: &PluginExchangeBinding,
    key_version: u32,
    schema: BundleSchema,
) -> Result<SyncObjectBinding> {
    match schema {
        // Older authenticated exchanges remain readable, while current writes
        // bind the current schema into the AEAD associated data.
        BundleSchema::V2
        | BundleSchema::V3
        | BundleSchema::V4
        | BundleSchema::V5
        | BundleSchema::V6 => Ok(object_binding_for_schema(binding, key_version, schema)),
        BundleSchema::V1 => Err(SyncCodecError::BindingMismatch),
    }
}

fn object_binding_for_schema(
    binding: &PluginExchangeBinding,
    key_version: u32,
    schema: BundleSchema,
) -> SyncObjectBinding {
    SyncObjectBinding {
        application_id: "norishell".to_owned(),
        account_id: binding.owner_id(),
        schema,
        object_kind: SyncObjectKind::SshProfileBundle,
        revision: binding.revision,
        base_revision: binding.base_revision,
        base_etag: binding.base_etag.clone(),
        key_version,
    }
}

fn validate_identifier(value: &str, maximum: usize) -> Result<()> {
    if value.is_empty()
        || value.len() > maximum
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
    {
        return Err(SyncCodecError::InvalidBundle("invalid exchange identifier"));
    }
    Ok(())
}

fn decode_base64(value: &str, maximum: usize) -> Result<Vec<u8>> {
    if value.len() > maximum.div_ceil(3) * 4 + 4 {
        return Err(SyncCodecError::BoundExceeded("exchange ciphertext"));
    }
    let decoded = BASE64
        .decode(value.as_bytes())
        .map_err(|_| SyncCodecError::AuthenticationFailed)?;
    if decoded.len() > maximum || BASE64.encode(&decoded) != value {
        return Err(SyncCodecError::AuthenticationFailed);
    }
    Ok(decoded)
}

fn canonical_base64(value: &str) -> bool {
    BASE64
        .decode(value.as_bytes())
        .is_ok_and(|decoded| BASE64.encode(decoded) == value)
}

fn canonical_base64url(value: &str) -> bool {
    URL_SAFE_NO_PAD
        .decode(value.as_bytes())
        .is_ok_and(|decoded| URL_SAFE_NO_PAD.encode(decoded) == value)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sync_algorithm(value: &str) -> Result<&'static str> {
    match value {
        crate::crypto::SERVICE_SYNC_ALGORITHM => Ok(crate::crypto::SERVICE_SYNC_ALGORITHM),
        _ => Err(SyncCodecError::AuthenticationFailed),
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    use crate::{
        PluginExchangeBinding, RecoveryPassword, SyncKey, create_plugin_exchange,
        create_plugin_exchange_with_key, inspect_plugin_exchange,
        inspect_plugin_exchange_data_owner, inspect_plugin_exchange_owner, open_plugin_exchange,
        open_plugin_exchange_with_key, plugin_exchange_vault_key_envelope,
        stable_plugin_data_owner,
    };

    use super::PortableBundleV1;

    fn empty_bundle() -> PortableBundleV1 {
        PortableBundleV1 {
            schema: crate::BundleSchema::V3,
            selected_categories: None,
            revision: 1,
            objects: crate::PortableObjects::default(),
            preferences: None,
            update_times: Vec::new(),
            preference_update_times: Default::default(),
            secrets: Vec::new(),
            skipped_machine_bound: Vec::new(),
            tombstones: Vec::new(),
        }
    }

    fn binding() -> PluginExchangeBinding {
        PluginExchangeBinding {
            plugin_id: "org.example.sync".to_owned(),
            signer_fingerprint_sha256:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
            profile_id: "primary".to_owned(),
            revision: 1,
            base_revision: None,
            base_etag: None,
        }
    }

    #[test]
    fn exchange_round_trip_is_plugin_and_profile_bound() {
        let bundle = empty_bundle();
        let password =
            RecoveryPassword::new(b"correct horse battery staple".to_vec()).expect("password");
        let encoded = create_plugin_exchange(&bundle, &password, &binding()).expect("exchange");
        let summary = inspect_plugin_exchange(&encoded, &binding()).expect("summary");
        assert_eq!(summary.revision, 1);
        assert_eq!(summary.schema, crate::BundleSchema::V3);
        let (discovered, discovered_summary) = inspect_plugin_exchange_owner(
            &encoded,
            "org.example.sync",
            &binding().signer_fingerprint_sha256,
            "primary",
        )
        .expect("discover owner-bound metadata");
        assert_eq!(discovered, binding());
        assert_eq!(discovered_summary.revision, 1);
        assert_eq!(discovered_summary.schema, crate::BundleSchema::V3);
        assert_eq!(
            open_plugin_exchange(&encoded, &password, &binding()).expect("bundle"),
            bundle
        );

        let mut wrong = binding();
        wrong.plugin_id = "org.other.sync".to_owned();
        assert!(open_plugin_exchange(&encoded, &password, &wrong).is_err());

        let mut wrong_signer = binding();
        wrong_signer.signer_fingerprint_sha256 =
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_owned();
        assert!(inspect_plugin_exchange(&encoded, &wrong_signer).is_err());
        assert!(open_plugin_exchange(&encoded, &password, &wrong_signer).is_err());
    }

    #[test]
    fn exchange_rejects_unknown_fields_and_wrong_password() {
        let bundle = empty_bundle();
        let password =
            RecoveryPassword::new(b"correct horse battery staple".to_vec()).expect("password");
        let encoded = create_plugin_exchange(&bundle, &password, &binding()).expect("exchange");
        let mut value: serde_json::Value = serde_json::from_slice(&encoded).expect("json");
        value
            .as_object_mut()
            .expect("object")
            .insert("extra".into(), true.into());
        assert!(inspect_plugin_exchange(&serde_json::to_vec(&value).unwrap(), &binding()).is_err());

        let wrong = RecoveryPassword::new(b"this is the wrong recovery password".to_vec())
            .expect("password");
        assert!(open_plugin_exchange(&encoded, &wrong, &binding()).is_err());

        let mut owner_tamper: serde_json::Value = serde_json::from_slice(&encoded).expect("json");
        owner_tamper["owner"]["signerFingerprintSha256"] =
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".into();
        assert!(
            inspect_plugin_exchange(&serde_json::to_vec(&owner_tamper).unwrap(), &binding())
                .is_err()
        );
    }

    #[test]
    fn vault_exchange_round_trip_reuses_stable_key_and_opaque_wrap() {
        let bundle = empty_bundle();
        let key = SyncKey::from_bytes([7_u8; 32]);
        let vault_wrap = br#"{"opaque":"vault-password-wrap"}"#;
        let encoded = create_plugin_exchange_with_key(&bundle, &key, vault_wrap, &binding())
            .expect("vault exchange");
        let (discovered, summary) = inspect_plugin_exchange_owner(
            &encoded,
            "org.example.sync",
            &binding().signer_fingerprint_sha256,
            "primary",
        )
        .expect("discover v3 exchange");
        assert_eq!(discovered, binding());
        assert_eq!(summary.revision, 1);
        assert_eq!(summary.schema, crate::BundleSchema::V3);
        assert_eq!(
            plugin_exchange_vault_key_envelope(&encoded, &binding()).expect("vault wrap"),
            vault_wrap
        );
        assert_eq!(
            open_plugin_exchange_with_key(&encoded, &key, &binding()).expect("bundle"),
            bundle
        );
        let mut wrong_signer = binding();
        wrong_signer.signer_fingerprint_sha256 =
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_owned();
        assert!(plugin_exchange_vault_key_envelope(&encoded, &wrong_signer).is_err());
        assert!(open_plugin_exchange_with_key(&encoded, &key, &wrong_signer).is_err());
        assert!(
            open_plugin_exchange(
                &encoded,
                &RecoveryPassword::new(b"correct horse battery staple".to_vec()).unwrap(),
                &binding()
            )
            .is_err()
        );

        let mut legacy: serde_json::Value = serde_json::from_slice(&encoded).expect("json");
        legacy["format"] = "norishell-ssh-vault-exchange-v2".into();
        assert!(
            inspect_plugin_exchange_owner(
                &serde_json::to_vec(&legacy).unwrap(),
                "org.example.sync",
                &binding().signer_fingerprint_sha256,
                "primary",
            )
            .is_err()
        );
    }

    #[test]
    fn data_owner_is_stable_across_package_hashes_and_legacy_requires_key() {
        let stable = stable_plugin_data_owner("org.example.sync").expect("stable owner");
        assert_eq!(stable.len(), 64);
        assert_ne!(stable, binding().signer_fingerprint_sha256);
        assert_ne!(stable, stable_plugin_data_owner("org.other.sync").unwrap());

        let key = SyncKey::from_bytes([7_u8; 32]);
        let legacy = create_plugin_exchange_with_key(&empty_bundle(), &key, b"opaque", &binding())
            .expect("legacy package exchange");
        let (owner, _) = inspect_plugin_exchange_data_owner(&legacy, "org.example.sync", "primary")
            .expect("legacy metadata");
        assert_eq!(
            owner.signer_fingerprint_sha256,
            binding().signer_fingerprint_sha256
        );
        assert!(
            open_plugin_exchange_with_key(&legacy, &SyncKey::from_bytes([8_u8; 32]), &owner)
                .is_err()
        );
        assert_eq!(
            open_plugin_exchange_with_key(&legacy, &key, &owner).unwrap(),
            empty_bundle()
        );
        assert!(inspect_plugin_exchange_data_owner(&legacy, "org.other.sync", "primary").is_err());
        assert!(inspect_plugin_exchange_data_owner(&legacy, "org.example.sync", "other").is_err());
        let mut forged: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
        forged["owner"]["signerFingerprintSha256"] = "b".repeat(64).into();
        forged["binding"]["accountId"] = format!(
            "plugin:{}:{}:signer:{}:profile:{}:{}",
            "org.example.sync".len(),
            "org.example.sync",
            "b".repeat(64),
            "primary".len(),
            "primary",
        )
        .into();
        let forged_bytes = serde_json::to_vec(&forged).unwrap();
        let (forged_owner, _) =
            inspect_plugin_exchange_data_owner(&forged_bytes, "org.example.sync", "primary")
                .expect("self-consistent metadata is not cryptographic proof");
        assert!(open_plugin_exchange_with_key(&forged_bytes, &key, &forged_owner).is_err());

        let mut current = binding();
        current.signer_fingerprint_sha256 = stable;
        let stable_exchange =
            create_plugin_exchange_with_key(&empty_bundle(), &key, b"opaque", &current)
                .expect("stable exchange");
        let (selected, _) =
            inspect_plugin_exchange_data_owner(&stable_exchange, "org.example.sync", "primary")
                .expect("stable metadata");
        assert_eq!(selected, current);
        assert_eq!(
            open_plugin_exchange_with_key(&stable_exchange, &key, &selected).unwrap(),
            empty_bundle()
        );
    }

    #[test]
    fn authenticated_v2_exchange_remains_readable_but_new_writes_bind_v3_aad() {
        let key = SyncKey::from_bytes([7_u8; 32]);
        let vault_wrap = br#"{\"opaque\":\"vault-password-wrap\"}"#;
        let current =
            create_plugin_exchange_with_key(&empty_bundle(), &key, vault_wrap, &binding())
                .expect("v3 exchange");
        let current_value: serde_json::Value = serde_json::from_slice(&current).expect("json");
        assert_eq!(
            current_value["binding"]["schema"],
            "norishell-ssh-profile-bundle-v3"
        );

        let legacy_bundle = PortableBundleV1 {
            schema: crate::BundleSchema::V2,
            ..empty_bundle()
        };
        let legacy_binding =
            super::object_binding_for_schema(&binding(), 1, crate::BundleSchema::V2);
        let encrypted = crate::encrypt_bundle_for_service(&legacy_bundle, &key, &legacy_binding)
            .expect("legacy ciphertext");
        let legacy = super::ExchangeWire {
            format: super::VAULT_EXCHANGE_FORMAT.to_owned(),
            owner: binding().owner(),
            binding: legacy_binding,
            recovery: super::RecoveryWire {
                envelope: BASE64.encode(vault_wrap),
                envelope_sha256: super::sha256_hex(vault_wrap),
            },
            encrypted: super::EncryptedWire {
                algorithm: encrypted.algorithm.to_owned(),
                nonce: encrypted.nonce_base64url,
                ciphertext: BASE64.encode(encrypted.ciphertext),
                ciphertext_sha256: encrypted.ciphertext_sha256,
            },
        };
        let legacy = serde_json::to_vec(&legacy).expect("legacy wire");
        let (_, summary) = inspect_plugin_exchange_owner(
            &legacy,
            "org.example.sync",
            &binding().signer_fingerprint_sha256,
            "primary",
        )
        .expect("legacy summary");
        assert_eq!(summary.schema, crate::BundleSchema::V2);
        assert_eq!(
            open_plugin_exchange_with_key(&legacy, &key, &binding()).expect("legacy bundle"),
            legacy_bundle
        );

        let mut schema_tamper: serde_json::Value =
            serde_json::from_slice(&legacy).expect("legacy json");
        schema_tamper["binding"]["schema"] = "norishell-ssh-profile-bundle-v3".into();
        assert!(
            open_plugin_exchange_with_key(
                &serde_json::to_vec(&schema_tamper).expect("tampered wire"),
                &key,
                &binding(),
            )
            .is_err()
        );
    }
}
