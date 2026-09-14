use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    error::{Result, SyncCodecError},
    schema::{BundleSchema, PortableBundleV1},
    secret::{RecoveryPassword, SyncKey},
};

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const SALT_BYTES: usize = 16;
const KDF_MEMORY_KIB: u32 = 65_536;
const KDF_ITERATIONS: u32 = 3;
const KDF_PARALLELISM: u32 = 1;
const KDF_OUTPUT_BYTES: u32 = 32;
const MAX_BUNDLE_PLAINTEXT_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENVELOPE_BYTES: usize = 96 * 1024 * 1024;
const MAX_OWNER_TEXT_BYTES: usize = 512;
const MAX_ETAG_BYTES: usize = 1_024;
pub const SERVICE_SYNC_ALGORITHM: &str = "xchacha20poly1305-ietf";
pub const SERVICE_RECOVERY_ALGORITHM: &str = "xchacha20poly1305-recovery-v1";
pub const SERVICE_RECOVERY_KDF: &str = "argon2id-v1:m=65536,t=3,p=1,l=32";
pub const SERVICE_RECOVERY_AAD_PREFIX: &str = "norishell:ssh-recovery:v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SyncEnvelopeFormat {
    #[serde(rename = "norishell-ssh-profile-sync-object-v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RecoveryEnvelopeFormat {
    #[serde(rename = "norishell-ssh-profile-recovery-envelope-v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum CipherId {
    #[serde(rename = "xchacha20poly1305-v1")]
    XChaCha20Poly1305V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RecoveryKdfId {
    #[serde(rename = "argon2id-sync-recovery-v1")]
    Argon2idSyncRecoveryV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncObjectKind {
    SshProfileBundle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SyncObjectBinding {
    pub application_id: String,
    pub account_id: String,
    pub schema: BundleSchema,
    pub object_kind: SyncObjectKind,
    pub revision: u64,
    pub base_revision: Option<u64>,
    pub base_etag: Option<String>,
    pub key_version: u32,
}

impl SyncObjectBinding {
    pub fn validate(&self) -> Result<()> {
        validate_owner_text(&self.application_id, "application ID")?;
        validate_owner_text(&self.account_id, "account ID")?;
        if self.revision == 0 {
            return Err(SyncCodecError::InvalidBundle("revision must be positive"));
        }
        if self.base_revision.is_some_and(|base| base >= self.revision) {
            return Err(SyncCodecError::InvalidBundle(
                "base revision must be positive and lower than revision",
            ));
        }
        if self.key_version == 0 {
            return Err(SyncCodecError::InvalidBundle(
                "sync key version must be positive",
            ));
        }
        if let Some(etag) = &self.base_etag
            && (etag.is_empty()
                || etag.len() > MAX_ETAG_BYTES
                || etag.chars().any(char::is_control))
        {
            return Err(SyncCodecError::InvalidBundle("invalid base etag"));
        }
        Ok(())
    }
}

/// Exact outer fields accepted by the Norixor opaque sync-object service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedSyncObject {
    pub algorithm: &'static str,
    pub nonce_base64url: String,
    pub ciphertext: Vec<u8>,
    pub ciphertext_sha256: String,
}

/// Exact non-secret metadata and opaque ciphertext accepted by the Norixor
/// recovery-envelope service. The recovery password and sync key are never
/// represented by this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEnvelopeUpload {
    pub algorithm: &'static str,
    pub kdf: &'static str,
    pub salt_base64url: String,
    pub nonce_base64url: String,
    pub aad: String,
    pub ciphertext: Vec<u8>,
    pub ciphertext_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RecoveryOwnerBinding {
    pub application_id: String,
    pub account_id: String,
    pub key_version: u32,
}

impl RecoveryOwnerBinding {
    pub fn validate(&self) -> Result<()> {
        validate_owner_text(&self.application_id, "application ID")?;
        validate_owner_text(&self.account_id, "account ID")?;
        if self.key_version == 0 {
            return Err(SyncCodecError::InvalidBundle(
                "sync key version must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BundleDigest([u8; 32]);

impl BundleDigest {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl fmt::Debug for BundleDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SyncEnvelope {
    format: SyncEnvelopeFormat,
    cipher: CipherId,
    nonce: String,
    binding: SyncObjectBinding,
    ciphertext: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecoveryKdfHeader {
    id: RecoveryKdfId,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output_bytes: u32,
    salt: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecoveryEnvelope {
    format: RecoveryEnvelopeFormat,
    owner: RecoveryOwnerBinding,
    kdf: RecoveryKdfHeader,
    cipher: CipherId,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecoveryAad<'a> {
    format: RecoveryEnvelopeFormat,
    owner: &'a RecoveryOwnerBinding,
    kdf: &'a RecoveryKdfHeader,
    cipher: CipherId,
    nonce: &'a str,
}

pub fn canonical_bundle_bytes(bundle: &PortableBundleV1) -> Result<Zeroizing<Vec<u8>>> {
    let canonical = bundle.canonicalized()?;
    let bytes = Zeroizing::new(serde_json::to_vec(&canonical)?);
    if bytes.len() > MAX_BUNDLE_PLAINTEXT_BYTES {
        return Err(SyncCodecError::BoundExceeded("canonical bundle plaintext"));
    }
    Ok(bytes)
}

pub fn decode_bundle(bytes: Zeroizing<Vec<u8>>) -> Result<PortableBundleV1> {
    if bytes.len() > MAX_BUNDLE_PLAINTEXT_BYTES {
        return Err(SyncCodecError::BoundExceeded("bundle plaintext"));
    }
    let bundle: PortableBundleV1 = serde_json::from_slice(bytes.as_slice())?;
    bundle.validate()?;
    Ok(bundle)
}

pub fn canonical_bundle_digest(bundle: &PortableBundleV1) -> Result<BundleDigest> {
    let bytes = canonical_bundle_bytes(bundle)?;
    Ok(BundleDigest(Sha256::digest(bytes.as_slice()).into()))
}

pub fn encrypt_bundle(
    bundle: &PortableBundleV1,
    key: &SyncKey,
    nonce: [u8; NONCE_BYTES],
    binding: &SyncObjectBinding,
) -> Result<Vec<u8>> {
    binding.validate()?;
    if binding.revision != bundle.revision || binding.schema != bundle.schema {
        return Err(SyncCodecError::BindingMismatch);
    }
    let plaintext = canonical_bundle_bytes(bundle)?;
    let aad = Zeroizing::new(serde_json::to_vec(binding)?);
    let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext.as_slice(),
                aad: aad.as_slice(),
            },
        )
        .map_err(|_| SyncCodecError::AuthenticationFailed)?;
    let envelope = SyncEnvelope {
        format: SyncEnvelopeFormat::V1,
        cipher: CipherId::XChaCha20Poly1305V1,
        nonce: BASE64.encode(nonce),
        binding: binding.clone(),
        ciphertext: BASE64.encode(ciphertext),
    };
    Ok(serde_json::to_vec(&envelope)?)
}

pub fn encrypt_bundle_random(
    bundle: &PortableBundleV1,
    key: &SyncKey,
    binding: &SyncObjectBinding,
) -> Result<Vec<u8>> {
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| SyncCodecError::Random)?;
    encrypt_bundle(bundle, key, nonce, binding)
}

pub fn encrypt_bundle_for_service(
    bundle: &PortableBundleV1,
    key: &SyncKey,
    binding: &SyncObjectBinding,
) -> Result<EncryptedSyncObject> {
    binding.validate()?;
    if binding.revision != bundle.revision || binding.schema != bundle.schema {
        return Err(SyncCodecError::BindingMismatch);
    }
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| SyncCodecError::Random)?;
    let plaintext = canonical_bundle_bytes(bundle)?;
    let aad = Zeroizing::new(serde_json::to_vec(binding)?);
    let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext.as_slice(),
                aad: aad.as_slice(),
            },
        )
        .map_err(|_| SyncCodecError::AuthenticationFailed)?;
    Ok(EncryptedSyncObject {
        algorithm: SERVICE_SYNC_ALGORITHM,
        nonce_base64url: URL_SAFE_NO_PAD.encode(nonce),
        ciphertext_sha256: sha256_hex(&ciphertext),
        ciphertext,
    })
}

pub fn decrypt_bundle_from_service(
    encrypted: &EncryptedSyncObject,
    key: &SyncKey,
    expected_binding: &SyncObjectBinding,
) -> Result<PortableBundleV1> {
    expected_binding.validate()?;
    if encrypted.algorithm != SERVICE_SYNC_ALGORITHM
        || encrypted.ciphertext.len() > MAX_BUNDLE_PLAINTEXT_BYTES + 16
        || sha256_hex(&encrypted.ciphertext) != encrypted.ciphertext_sha256
    {
        return Err(SyncCodecError::AuthenticationFailed);
    }
    let nonce = decode_base64url_fixed::<NONCE_BYTES>(&encrypted.nonce_base64url, "sync nonce")?;
    let aad = Zeroizing::new(serde_json::to_vec(expected_binding)?);
    let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &encrypted.ciphertext,
                    aad: aad.as_slice(),
                },
            )
            .map_err(|_| SyncCodecError::AuthenticationFailed)?,
    );
    let bundle = decode_bundle(plaintext)?;
    if bundle.revision != expected_binding.revision || bundle.schema != expected_binding.schema {
        return Err(SyncCodecError::BindingMismatch);
    }
    Ok(bundle)
}

pub fn decrypt_bundle(
    envelope_bytes: &[u8],
    key: &SyncKey,
    expected_binding: &SyncObjectBinding,
) -> Result<PortableBundleV1> {
    if envelope_bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(SyncCodecError::BoundExceeded("sync envelope"));
    }
    expected_binding.validate()?;
    let envelope: SyncEnvelope = serde_json::from_slice(envelope_bytes)?;
    if envelope.binding != *expected_binding {
        return Err(SyncCodecError::BindingMismatch);
    }
    let nonce = decode_fixed::<NONCE_BYTES>(&envelope.nonce, "sync nonce")?;
    let ciphertext = Zeroizing::new(decode_bounded_base64(
        &envelope.ciphertext,
        MAX_BUNDLE_PLAINTEXT_BYTES + 16,
        "sync ciphertext",
    )?);
    let aad = Zeroizing::new(serde_json::to_vec(expected_binding)?);
    let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: ciphertext.as_slice(),
                    aad: aad.as_slice(),
                },
            )
            .map_err(|_| SyncCodecError::AuthenticationFailed)?,
    );
    let bundle = decode_bundle(plaintext)?;
    if bundle.revision != expected_binding.revision || bundle.schema != expected_binding.schema {
        return Err(SyncCodecError::BindingMismatch);
    }
    Ok(bundle)
}

/// Wraps a sync key with a dedicated recovery password. This API intentionally
/// has no Vault password, KEK, VMK, envelope, or auto-unlock input.
pub fn create_recovery_envelope(
    sync_key: &SyncKey,
    recovery_password: &RecoveryPassword,
    owner: &RecoveryOwnerBinding,
    salt: [u8; SALT_BYTES],
    nonce: [u8; NONCE_BYTES],
) -> Result<Vec<u8>> {
    owner.validate()?;
    let kdf = RecoveryKdfHeader {
        id: RecoveryKdfId::Argon2idSyncRecoveryV1,
        memory_kib: KDF_MEMORY_KIB,
        iterations: KDF_ITERATIONS,
        parallelism: KDF_PARALLELISM,
        output_bytes: KDF_OUTPUT_BYTES,
        salt: BASE64.encode(salt),
    };
    let nonce_encoded = BASE64.encode(nonce);
    let aad = Zeroizing::new(serde_json::to_vec(&RecoveryAad {
        format: RecoveryEnvelopeFormat::V1,
        owner,
        kdf: &kdf,
        cipher: CipherId::XChaCha20Poly1305V1,
        nonce: &nonce_encoded,
    })?);
    let mut recovery_key = Zeroizing::new([0_u8; KEY_BYTES]);
    derive_recovery_key(recovery_password, &kdf, &mut recovery_key)?;
    let cipher = XChaCha20Poly1305::new_from_slice(recovery_key.as_slice())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: sync_key.expose(),
                aad: aad.as_slice(),
            },
        )
        .map_err(|_| SyncCodecError::AuthenticationFailed)?;
    Ok(serde_json::to_vec(&RecoveryEnvelope {
        format: RecoveryEnvelopeFormat::V1,
        owner: owner.clone(),
        kdf,
        cipher: CipherId::XChaCha20Poly1305V1,
        nonce: nonce_encoded,
        ciphertext: BASE64.encode(ciphertext),
    })?)
}

pub fn create_recovery_envelope_random(
    sync_key: &SyncKey,
    recovery_password: &RecoveryPassword,
    owner: &RecoveryOwnerBinding,
) -> Result<Vec<u8>> {
    let mut salt = [0_u8; SALT_BYTES];
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut salt).map_err(|_| SyncCodecError::Random)?;
    getrandom::fill(&mut nonce).map_err(|_| SyncCodecError::Random)?;
    create_recovery_envelope(sync_key, recovery_password, owner, salt, nonce)
}

pub fn open_recovery_envelope(
    envelope_bytes: &[u8],
    recovery_password: &RecoveryPassword,
    expected_owner: &RecoveryOwnerBinding,
) -> Result<SyncKey> {
    if envelope_bytes.len() > 64 * 1024 {
        return Err(SyncCodecError::BoundExceeded("recovery envelope"));
    }
    expected_owner.validate()?;
    let envelope: RecoveryEnvelope = serde_json::from_slice(envelope_bytes)?;
    if envelope.owner != *expected_owner {
        return Err(SyncCodecError::BindingMismatch);
    }
    validate_recovery_kdf(&envelope.kdf)?;
    let nonce = decode_fixed::<NONCE_BYTES>(&envelope.nonce, "recovery nonce")?;
    let ciphertext = Zeroizing::new(decode_bounded_base64(
        &envelope.ciphertext,
        KEY_BYTES + 16,
        "wrapped sync key",
    )?);
    let aad = Zeroizing::new(serde_json::to_vec(&RecoveryAad {
        format: envelope.format,
        owner: expected_owner,
        kdf: &envelope.kdf,
        cipher: envelope.cipher,
        nonce: &envelope.nonce,
    })?);
    let mut recovery_key = Zeroizing::new([0_u8; KEY_BYTES]);
    derive_recovery_key(recovery_password, &envelope.kdf, &mut recovery_key)?;
    let cipher = XChaCha20Poly1305::new_from_slice(recovery_key.as_slice())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: ciphertext.as_slice(),
                    aad: aad.as_slice(),
                },
            )
            .map_err(|_| SyncCodecError::AuthenticationFailed)?,
    );
    if plaintext.len() != KEY_BYTES {
        return Err(SyncCodecError::InvalidSyncKey);
    }
    let mut key = [0_u8; KEY_BYTES];
    key.copy_from_slice(plaintext.as_slice());
    Ok(SyncKey::from_bytes(key))
}

pub fn create_recovery_envelope_for_service(
    sync_key: &SyncKey,
    recovery_password: &RecoveryPassword,
    owner: &RecoveryOwnerBinding,
) -> Result<RecoveryEnvelopeUpload> {
    owner.validate()?;
    let account_id = positive_account_id(&owner.account_id)?;
    let mut salt = [0_u8; SALT_BYTES];
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut salt).map_err(|_| SyncCodecError::Random)?;
    getrandom::fill(&mut nonce).map_err(|_| SyncCodecError::Random)?;
    let aad = format!(
        "{SERVICE_RECOVERY_AAD_PREFIX}|application={}|account={account_id}|key_version={}",
        owner.application_id, owner.key_version
    );
    let mut recovery_key = Zeroizing::new([0_u8; KEY_BYTES]);
    derive_service_recovery_key(recovery_password, &salt, &mut recovery_key)?;
    let cipher = XChaCha20Poly1305::new_from_slice(recovery_key.as_slice())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: sync_key.expose(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| SyncCodecError::AuthenticationFailed)?;
    Ok(RecoveryEnvelopeUpload {
        algorithm: SERVICE_RECOVERY_ALGORITHM,
        kdf: SERVICE_RECOVERY_KDF,
        salt_base64url: URL_SAFE_NO_PAD.encode(salt),
        nonce_base64url: URL_SAFE_NO_PAD.encode(nonce),
        aad,
        ciphertext_sha256: sha256_hex(&ciphertext),
        ciphertext,
    })
}

pub fn open_recovery_envelope_from_service(
    envelope: &RecoveryEnvelopeUpload,
    recovery_password: &RecoveryPassword,
    expected_owner: &RecoveryOwnerBinding,
) -> Result<SyncKey> {
    expected_owner.validate()?;
    let account_id = positive_account_id(&expected_owner.account_id)?;
    let expected_aad = format!(
        "{SERVICE_RECOVERY_AAD_PREFIX}|application={}|account={account_id}|key_version={}",
        expected_owner.application_id, expected_owner.key_version
    );
    if envelope.algorithm != SERVICE_RECOVERY_ALGORITHM
        || envelope.kdf != SERVICE_RECOVERY_KDF
        || envelope.aad != expected_aad
        || envelope.ciphertext.len() != KEY_BYTES + 16
        || sha256_hex(&envelope.ciphertext) != envelope.ciphertext_sha256
    {
        return Err(SyncCodecError::BindingMismatch);
    }
    let salt = decode_base64url_fixed::<SALT_BYTES>(&envelope.salt_base64url, "recovery salt")?;
    let nonce = decode_base64url_fixed::<NONCE_BYTES>(&envelope.nonce_base64url, "recovery nonce")?;
    let mut recovery_key = Zeroizing::new([0_u8; KEY_BYTES]);
    derive_service_recovery_key(recovery_password, &salt, &mut recovery_key)?;
    let cipher = XChaCha20Poly1305::new_from_slice(recovery_key.as_slice())
        .map_err(|_| SyncCodecError::InvalidSyncKey)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &envelope.ciphertext,
                    aad: expected_aad.as_bytes(),
                },
            )
            .map_err(|_| SyncCodecError::AuthenticationFailed)?,
    );
    if plaintext.len() != KEY_BYTES {
        return Err(SyncCodecError::InvalidSyncKey);
    }
    let mut key = [0_u8; KEY_BYTES];
    key.copy_from_slice(&plaintext);
    Ok(SyncKey::from_bytes(key))
}

fn derive_service_recovery_key(
    password: &RecoveryPassword,
    salt: &[u8; SALT_BYTES],
    output: &mut [u8; KEY_BYTES],
) -> Result<()> {
    let params = Params::new(
        KDF_MEMORY_KIB,
        KDF_ITERATIONS,
        KDF_PARALLELISM,
        Some(KEY_BYTES),
    )
    .map_err(|_| SyncCodecError::KeyDerivation)?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.expose(), salt, output)
        .map_err(|_| SyncCodecError::KeyDerivation)
}

fn positive_account_id(value: &str) -> Result<u64> {
    let parsed = value
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0)
        .ok_or(SyncCodecError::InvalidBundle("account ID"))?;
    if parsed.to_string() != value {
        return Err(SyncCodecError::InvalidBundle("account ID"));
    }
    Ok(parsed)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_base64url_fixed<const N: usize>(value: &str, field: &'static str) -> Result<[u8; N]> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| SyncCodecError::InvalidBundle(field))?;
    if URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(SyncCodecError::InvalidBundle(field));
    }
    decoded
        .try_into()
        .map_err(|_| SyncCodecError::InvalidBundle(field))
}

fn derive_recovery_key(
    password: &RecoveryPassword,
    kdf: &RecoveryKdfHeader,
    output: &mut [u8; KEY_BYTES],
) -> Result<()> {
    validate_recovery_kdf(kdf)?;
    let salt = Zeroizing::new(decode_fixed::<SALT_BYTES>(&kdf.salt, "recovery salt")?);
    let params = Params::new(
        KDF_MEMORY_KIB,
        KDF_ITERATIONS,
        KDF_PARALLELISM,
        Some(KEY_BYTES),
    )
    .map_err(|_| SyncCodecError::KeyDerivation)?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.expose(), salt.as_slice(), output)
        .map_err(|_| SyncCodecError::KeyDerivation)
}

fn validate_recovery_kdf(kdf: &RecoveryKdfHeader) -> Result<()> {
    if kdf.id != RecoveryKdfId::Argon2idSyncRecoveryV1
        || kdf.memory_kib != KDF_MEMORY_KIB
        || kdf.iterations != KDF_ITERATIONS
        || kdf.parallelism != KDF_PARALLELISM
        || kdf.output_bytes != KDF_OUTPUT_BYTES
    {
        return Err(SyncCodecError::UnsupportedKdfProfile);
    }
    let _ = decode_fixed::<SALT_BYTES>(&kdf.salt, "recovery salt")?;
    Ok(())
}

fn validate_owner_text(value: &str, field: &'static str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_OWNER_TEXT_BYTES || value.chars().any(char::is_control)
    {
        return Err(SyncCodecError::InvalidBundle(field));
    }
    Ok(())
}

fn decode_fixed<const N: usize>(value: &str, field: &'static str) -> Result<[u8; N]> {
    let decoded = decode_bounded_base64(value, N, field)?;
    decoded
        .try_into()
        .map_err(|_| SyncCodecError::InvalidBundle(field))
}

fn decode_bounded_base64(value: &str, max_bytes: usize, field: &'static str) -> Result<Vec<u8>> {
    if value.len() > max_bytes.div_ceil(3) * 4 + 4 {
        return Err(SyncCodecError::BoundExceeded(field));
    }
    let decoded = BASE64
        .decode(value)
        .map_err(|_| SyncCodecError::InvalidBundle(field))?;
    if decoded.len() > max_bytes {
        return Err(SyncCodecError::BoundExceeded(field));
    }
    if BASE64.encode(&decoded) != value {
        return Err(SyncCodecError::InvalidBundle(field));
    }
    Ok(decoded)
}
