//! Portable encrypted secret storage for NoriShell.
//!
//! A password-derived key-encryption key wraps a random vault master key; only
//! that random key encrypts the payload. An optional, vault-bound device slot
//! may wrap the same master key with a separate random key held by the
//! operating-system secure store; the Vault password is never stored there.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

mod approval_key;
pub use approval_key::ApprovalFingerprintKey;

const MAGIC: &str = "NORISHELL_VAULT";
const DEVICE_UNLOCK_MAGIC: &str = "NORISHELL_DEVICE_UNLOCK";
const SYNC_KEY_ENVELOPE_MAGIC: &str = "NORISHELL_SYNC_VAULT_KEY";
const FORMAT_VERSION: u32 = 1;
const DEVICE_UNLOCK_FORMAT_VERSION: u32 = 1;
const SYNC_KEY_ENVELOPE_FORMAT_VERSION: u32 = 1;
const PAYLOAD_SCHEMA_VERSION: u32 = 1;
const KDF_ID: &str = "argon2id-v1";
const KDF_PROFILE: &str = "interactive-v1";
const CIPHER_ID: &str = "xchacha20poly1305-v1";
const KEY_BYTES: usize = 32;
const SALT_BYTES: usize = 16;
const NONCE_BYTES: usize = 24;
const MAX_VAULT_FILE_BYTES: u64 = 64 * 1024 * 1024;
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_PARALLELISM: u32 = 1;
const MIN_PASSWORD_BYTES: usize = 12;
pub const MAX_SECRET_BATCH_ENTRIES: usize = 1_024;
pub const MAX_SECRET_BATCH_VALUE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PASSWORD_BYTES: usize = 64 * 1024;
const MAX_PASSPHRASE_BYTES: usize = 64 * 1024;
const MAX_PRIVATE_KEY_BYTES: usize = 16 * 1024 * 1024;
const MAX_CERTIFICATE_BYTES: usize = 16 * 1024 * 1024;
const MAX_OAUTH_REFRESH_TOKEN_BYTES: usize = 256 * 1024;
const SYNC_KEY_BYTES: usize = 32;
const MAX_SYNC_RECOVERY_SECRET_BYTES: usize = 64 * 1024;
pub const COMMAND_HISTORY_SCHEMA_VERSION: u32 = 1;
pub const MAX_COMMAND_HISTORY_ENTRIES: usize = 2_000;
const MAX_COMMAND_HISTORY_COMMAND_BYTES: usize = 16 * 1024;
const MAX_COMMAND_HISTORY_SCOPE_BYTES: usize = 160;
/// Command-history storage is deliberately bounded independently of entry
/// count. The terminal Core uses the same budget before submitting a Vault
/// replacement, so normal capture cannot repeatedly create invalid payloads.
pub const MAX_COMMAND_HISTORY_TOTAL_BYTES: usize = 8 * 1024 * 1024;

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivateObjectKind {
    Directory,
    File,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsAclPrincipal {
    CurrentUser,
    LocalSystem,
}

#[cfg(any(windows, test))]
const WINDOWS_PRIVATE_ACL_PRINCIPALS: [WindowsAclPrincipal; 2] = [
    WindowsAclPrincipal::CurrentUser,
    WindowsAclPrincipal::LocalSystem,
];

#[cfg(any(windows, test))]
const fn windows_acl_inheritance(kind: PrivateObjectKind) -> u32 {
    match kind {
        PrivateObjectKind::Directory => 0x3,
        PrivateObjectKind::File => 0,
    }
}

pub type Result<T> = std::result::Result<T, VaultError>;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault already exists")]
    AlreadyExists,
    #[error("vault is locked by another process")]
    Locked,
    #[error("vault password is incorrect or the ciphertext was modified")]
    AuthenticationFailed,
    #[error("vault password must contain at least 12 UTF-8 bytes")]
    WeakPassword,
    #[error("vault directory permissions are not private")]
    InsecurePermissions,
    #[error("vault commit state is unknown; close and unlock the vault again")]
    CommitStateUnknown(#[source] std::io::Error),
    #[error("vault must be closed and unlocked again before it can be used")]
    ReloadRequired,
    #[error("vault envelope is invalid: {0}")]
    InvalidEnvelope(&'static str),
    #[error("vault format version {0} is not supported")]
    UnsupportedFormat(u32),
    #[error("vault file exceeds the supported size limit")]
    TooLarge,
    #[error("vault key derivation failed")]
    KeyDerivation,
    #[error("device unlock key must contain exactly 32 bytes")]
    InvalidDeviceUnlockKey,
    #[error("device unlock slot belongs to a different vault")]
    DeviceUnlockVaultMismatch,
    #[error("device unlock slot commit state is unknown")]
    DeviceUnlockCommitStateUnknown(#[source] std::io::Error),
    #[error("secret reference must be a non-nil UUID")]
    InvalidSecretRef,
    #[error("secret batch must contain at least one entry")]
    EmptySecretBatch,
    #[error("secret batch exceeds the limit of {max_entries} entries")]
    SecretBatchTooLarge { max_entries: usize },
    #[error("secret batch plaintext exceeds the limit of {max_bytes} bytes")]
    SecretBatchValueTooLarge { max_bytes: usize },
    #[error("secret reference {0} already exists in the vault or batch")]
    DuplicateSecretRef(SecretRef),
    #[error("{kind:?} secret must not be empty")]
    EmptySecret { kind: SecretKind },
    #[error("{kind:?} secret exceeds the limit of {max_bytes} bytes")]
    SecretTooLarge { kind: SecretKind, max_bytes: usize },
    #[error("{kind:?} secret must contain exactly {expected_bytes} bytes")]
    InvalidSecretLength {
        kind: SecretKind,
        expected_bytes: usize,
    },
    #[error("secret kind mismatch: expected {expected:?}, found {actual:?}")]
    SecretKindMismatch {
        expected: SecretKind,
        actual: SecretKind,
    },
    #[error("vault serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("vault I/O failed")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretKind {
    Password,
    PrivateKey,
    Passphrase,
    Certificate,
    LoginAutomation,
    OAuthRefreshToken,
    PluginCredential,
    SshSyncKey,
    SshSyncRecoverySecret,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretRef(Uuid);

impl SecretRef {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(value: Uuid) -> Result<Self> {
        if value.is_nil() {
            return Err(VaultError::InvalidSecretRef);
        }
        Ok(Self(value))
    }

    pub fn parse(value: impl AsRef<str>) -> Result<Self> {
        let value = Uuid::parse_str(value.as_ref()).map_err(|_| VaultError::InvalidSecretRef)?;
        Self::from_uuid(value)
    }

    #[must_use]
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for SecretRef {
    fn default() -> Self {
        Self::new()
    }
}

impl TryFrom<Uuid> for SecretRef {
    type Error = VaultError;

    fn try_from(value: Uuid) -> Result<Self> {
        Self::from_uuid(value)
    }
}

impl FromStr for SecretRef {
    type Err = VaultError;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl std::fmt::Display for SecretRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for SecretRef {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Uuid::deserialize(deserializer)?;
        Self::from_uuid(value).map_err(de::Error::custom)
    }
}

impl std::fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("SecretRef").field(&self.0).finish()
    }
}

/// A caller-minted secret reference paired with plaintext for one atomic write.
///
/// The caller remains responsible for clearing the borrowed source buffer. The
/// vault's owned copy is zeroized when it is rolled back, replaced, or dropped.
pub struct SecretInsert<'a> {
    secret_ref: SecretRef,
    kind: SecretKind,
    value: &'a [u8],
}

impl<'a> SecretInsert<'a> {
    #[must_use]
    pub const fn new(secret_ref: SecretRef, kind: SecretKind, value: &'a [u8]) -> Self {
        Self {
            secret_ref,
            kind,
            value,
        }
    }

    #[must_use]
    pub const fn secret_ref(&self) -> SecretRef {
        self.secret_ref
    }
}

impl std::fmt::Debug for SecretInsert<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretInsert")
            .field("secret_ref", &self.secret_ref)
            .field("kind", &self.kind)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct SecretEntry {
    #[zeroize(skip)]
    kind: SecretKind,
    value: Vec<u8>,
}

/// Encrypted command-history data stored inside the existing Vault payload.
/// It has no relationship to a `SecretRef`, and is intentionally excluded from
/// the portable SSH-sync envelope.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandHistoryEntry {
    #[zeroize(skip)]
    pub entry_id: Uuid,
    pub scope: String,
    pub command: String,
    #[zeroize(skip)]
    pub completed_at_unix_ms: i64,
    #[zeroize(skip)]
    pub elapsed_millis: u64,
    #[zeroize(skip)]
    pub exit_code: Option<i32>,
}

impl std::fmt::Debug for CommandHistoryEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandHistoryEntry")
            .field("entry_id", &self.entry_id)
            .field("scope", &self.scope)
            .field("command", &"[REDACTED]")
            .field("completed_at_unix_ms", &self.completed_at_unix_ms)
            .field("elapsed_millis", &self.elapsed_millis)
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandHistoryPayload {
    pub schema_version: u32,
    pub entries: Vec<CommandHistoryEntry>,
}

impl Default for CommandHistoryPayload {
    fn default() -> Self {
        Self {
            schema_version: COMMAND_HISTORY_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

impl std::fmt::Debug for CommandHistoryPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandHistoryPayload")
            .field("schema_version", &self.schema_version)
            .field("entry_count", &self.entries.len())
            .finish()
    }
}

impl std::fmt::Debug for SecretEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretEntry")
            .field("kind", &self.kind)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultPayloadV1 {
    schema_version: u32,
    revision: u64,
    parent_envelope_sha256: Option<String>,
    entries: BTreeMap<SecretRef, SecretEntry>,
    #[serde(default)]
    command_history: CommandHistoryPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KdfHeader {
    id: String,
    profile: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CiphertextSection {
    cipher: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultEnvelope {
    magic: String,
    format_version: u32,
    vault_id: Uuid,
    kdf: KdfHeader,
    wrapped_vault_key: CiphertextSection,
    payload: CiphertextSection,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceUnlockEnvelope {
    magic: String,
    format_version: u32,
    vault_id: Uuid,
    wrapped_vault_key: CiphertextSection,
}

/// Password-wrapped copy of a Vault master key used only as the stable key for
/// an account-scoped SSH sync profile. It deliberately excludes the Vault
/// payload and every device auto-unlock slot.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultSyncKeyEnvelope {
    magic: String,
    format_version: u32,
    vault_id: Uuid,
    kdf: KdfHeader,
    wrapped_vault_key: CiphertextSection,
}

pub struct VaultSyncKeyMaterial {
    vault_id: Uuid,
    key: Zeroizing<[u8; KEY_BYTES]>,
    envelope: Vec<u8>,
}

impl VaultSyncKeyMaterial {
    #[must_use]
    pub fn vault_id(&self) -> Uuid {
        self.vault_id
    }

    #[must_use]
    pub fn key_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.key
    }

    #[must_use]
    pub fn envelope(&self) -> &[u8] {
        &self.envelope
    }
}

impl std::fmt::Debug for VaultSyncKeyMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaultSyncKeyMaterial")
            .field("vault_id", &self.vault_id)
            .field("key", &"[REDACTED]")
            .field("envelope_bytes", &self.envelope.len())
            .finish()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyWrapAad<'a> {
    magic: &'a str,
    format_version: u32,
    vault_id: Uuid,
    kdf: &'a KdfHeader,
    cipher: &'a str,
    nonce: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PayloadAad<'a> {
    magic: &'a str,
    format_version: u32,
    vault_id: Uuid,
    cipher: &'a str,
    nonce: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceUnlockAad<'a> {
    magic: &'a str,
    format_version: u32,
    vault_id: Uuid,
    cipher: &'a str,
    nonce: &'a str,
}

pub struct DeviceUnlockKey(Zeroizing<[u8; KEY_BYTES]>);

impl DeviceUnlockKey {
    pub fn generate() -> Result<Self> {
        let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
        fill_random(key.as_mut())?;
        Ok(Self(key))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != KEY_BYTES {
            return Err(VaultError::InvalidDeviceUnlockKey);
        }
        let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
        key.copy_from_slice(bytes);
        Ok(Self(key))
    }

    #[must_use]
    pub fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl std::fmt::Debug for DeviceUnlockKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DeviceUnlockKey([REDACTED])")
    }
}

pub struct SecretValue(Zeroizing<Vec<u8>>);

impl SecretValue {
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

struct VaultLock {
    file: File,
}

impl VaultLock {
    fn acquire(vault_path: &Path) -> Result<Self> {
        let lock_path = lock_path(vault_path)?;
        let file = open_private_file(&lock_path, false)?;
        if let Err(error) = FileExt::try_lock(&file) {
            return match error {
                TryLockError::WouldBlock => Err(VaultError::Locked),
                TryLockError::Error(error) => Err(VaultError::Io(error)),
            };
        }
        Ok(Self { file })
    }
}

impl Drop for VaultLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub struct UnlockedVault {
    path: PathBuf,
    vault_id: Uuid,
    kdf: KdfHeader,
    wrapped_vault_key: CiphertextSection,
    vault_master_key: Zeroizing<[u8; KEY_BYTES]>,
    payload: VaultPayloadV1,
    poisoned: bool,
    _lock: VaultLock,
}

impl std::fmt::Debug for UnlockedVault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UnlockedVault")
            .field("path", &self.path)
            .field("vault_id", &self.vault_id)
            .field("revision", &self.payload.revision)
            .field("entry_count", &self.payload.entries.len())
            .field("poisoned", &self.poisoned)
            .finish_non_exhaustive()
    }
}

impl UnlockedVault {
    pub fn create(path: impl AsRef<Path>, password: &[u8]) -> Result<Self> {
        validate_new_password(password)?;
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            return Err(VaultError::AlreadyExists);
        }
        ensure_private_parent_directory(&path)?;
        let lock = VaultLock::acquire(&path)?;
        if path.exists() {
            return Err(VaultError::AlreadyExists);
        }

        let kdf = new_kdf_header()?;
        let mut key_encryption_key = Zeroizing::new([0_u8; KEY_BYTES]);
        derive_key(password, &kdf, &mut key_encryption_key)?;
        let vault_id = Uuid::new_v4();
        let mut vault_master_key = Zeroizing::new([0_u8; KEY_BYTES]);
        fill_random(vault_master_key.as_mut())?;
        let wrapped_vault_key =
            wrap_vault_key(vault_id, &kdf, &key_encryption_key, &vault_master_key)?;
        let payload = VaultPayloadV1 {
            schema_version: PAYLOAD_SCHEMA_VERSION,
            revision: 0,
            parent_envelope_sha256: None,
            entries: BTreeMap::new(),
            command_history: CommandHistoryPayload::default(),
        };
        let mut vault = Self {
            path,
            vault_id,
            kdf,
            wrapped_vault_key,
            vault_master_key,
            payload,
            poisoned: false,
            _lock: lock,
        };
        vault.persist()?;
        Ok(vault)
    }

    pub fn unlock(path: impl AsRef<Path>, password: &[u8]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        ensure_private_parent_directory(&path)?;
        let lock = VaultLock::acquire(&path)?;
        let envelope = read_envelope(&path)?;
        validate_envelope(&envelope)?;
        let mut key_encryption_key = Zeroizing::new([0_u8; KEY_BYTES]);
        derive_key(password, &envelope.kdf, &mut key_encryption_key)?;
        let vault_master_key = unwrap_vault_key(&envelope, &key_encryption_key)?;
        let payload = decrypt_payload(&envelope, &vault_master_key)?;
        if payload.schema_version != PAYLOAD_SCHEMA_VERSION {
            return Err(VaultError::InvalidEnvelope("unsupported payload schema"));
        }
        validate_payload_entries(&payload)?;
        Ok(Self {
            path,
            vault_id: envelope.vault_id,
            kdf: envelope.kdf,
            wrapped_vault_key: envelope.wrapped_vault_key,
            vault_master_key,
            payload,
            poisoned: false,
            _lock: lock,
        })
    }

    pub fn unlock_with_device_key(
        path: impl AsRef<Path>,
        slot_path: impl AsRef<Path>,
        device_key: &DeviceUnlockKey,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        ensure_private_parent_directory(&path)?;
        let lock = VaultLock::acquire(&path)?;
        let envelope = read_envelope(&path)?;
        validate_envelope(&envelope)?;
        let slot: DeviceUnlockEnvelope =
            serde_json::from_slice(&read_bounded(slot_path.as_ref())?)?;
        validate_device_unlock_envelope(&slot)?;
        if slot.vault_id != envelope.vault_id {
            return Err(VaultError::DeviceUnlockVaultMismatch);
        }
        let vault_master_key = unwrap_device_vault_key(&slot, device_key)?;
        let payload = decrypt_payload(&envelope, &vault_master_key)?;
        if payload.schema_version != PAYLOAD_SCHEMA_VERSION {
            return Err(VaultError::InvalidEnvelope("unsupported payload schema"));
        }
        validate_payload_entries(&payload)?;
        Ok(Self {
            path,
            vault_id: envelope.vault_id,
            kdf: envelope.kdf,
            wrapped_vault_key: envelope.wrapped_vault_key,
            vault_master_key,
            payload,
            poisoned: false,
            _lock: lock,
        })
    }

    #[must_use]
    pub fn vault_id(&self) -> Uuid {
        self.vault_id
    }

    /// Exports only the existing password wrap and a temporary in-memory copy
    /// of the VMK. The encrypted Vault payload and device unlock material are
    /// never part of the returned envelope.
    pub fn export_sync_key_material(&self) -> Result<VaultSyncKeyMaterial> {
        self.ensure_usable()?;
        let envelope = serde_json::to_vec(&VaultSyncKeyEnvelope {
            magic: SYNC_KEY_ENVELOPE_MAGIC.to_owned(),
            format_version: SYNC_KEY_ENVELOPE_FORMAT_VERSION,
            vault_id: self.vault_id,
            kdf: self.kdf.clone(),
            wrapped_vault_key: self.wrapped_vault_key.clone(),
        })?;
        Ok(VaultSyncKeyMaterial {
            vault_id: self.vault_id,
            key: Zeroizing::new(*self.vault_master_key),
            envelope,
        })
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.payload.revision
    }

    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.payload.entries.len()
    }

    #[must_use]
    pub fn requires_reload(&self) -> bool {
        self.poisoned
    }

    pub fn verify_password(&self, password: &[u8]) -> Result<()> {
        self.ensure_usable()?;
        let mut key_encryption_key = Zeroizing::new([0_u8; KEY_BYTES]);
        derive_key(password, &self.kdf, &mut key_encryption_key)?;
        let candidate = unwrap_vault_key_parts(
            self.vault_id,
            &self.kdf,
            &self.wrapped_vault_key,
            &key_encryption_key,
        )?;
        if !bool::from(candidate[..].ct_eq(&self.vault_master_key[..])) {
            return Err(VaultError::AuthenticationFailed);
        }
        Ok(())
    }

    pub fn write_device_unlock_slot(
        &self,
        path: impl AsRef<Path>,
        device_key: &DeviceUnlockKey,
    ) -> Result<()> {
        self.ensure_usable()?;
        let envelope = DeviceUnlockEnvelope {
            magic: DEVICE_UNLOCK_MAGIC.to_owned(),
            format_version: DEVICE_UNLOCK_FORMAT_VERSION,
            vault_id: self.vault_id,
            wrapped_vault_key: wrap_device_vault_key(
                self.vault_id,
                device_key,
                &self.vault_master_key,
            )?,
        };
        let encoded = serde_json::to_vec(&envelope)?;
        match atomic_write(path.as_ref(), &encoded) {
            Ok(()) => Ok(()),
            Err(AtomicWriteError::BeforeCommit(error)) => Err(error),
            Err(AtomicWriteError::CommitStateUnknown(error)) => {
                Err(VaultError::DeviceUnlockCommitStateUnknown(error))
            }
        }
    }

    pub fn insert_secret(&mut self, kind: SecretKind, value: &[u8]) -> Result<SecretRef> {
        let secret_ref = SecretRef::new();
        self.insert_secrets_with_refs(&[SecretInsert::new(secret_ref, kind, value)])?;
        Ok(secret_ref)
    }

    /// Inserts a non-empty batch using caller-minted stable references.
    ///
    /// Every entry is committed in one payload revision and one envelope
    /// replacement. Validation and definite pre-commit failures leave the
    /// in-memory payload unchanged. If durability becomes uncertain after the
    /// replacement, the supplied references still identify the possibly
    /// committed entries and this instance requires a reload.
    pub fn insert_secrets_with_refs(&mut self, secrets: &[SecretInsert<'_>]) -> Result<()> {
        self.insert_secrets_with_refs_using(secrets, Self::persist)
    }

    /// Idempotently deletes a non-empty batch in one payload revision. Missing
    /// references are already-clean facts. A post-replacement durability error
    /// poisons the instance because deletion may have committed.
    pub fn delete_secrets(&mut self, secret_refs: &[SecretRef]) -> Result<()> {
        self.delete_secrets_using(secret_refs, Self::persist)
    }

    fn delete_secrets_using<F>(&mut self, secret_refs: &[SecretRef], persist: F) -> Result<()>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        self.ensure_usable()?;
        if secret_refs.is_empty() {
            return Err(VaultError::EmptySecretBatch);
        }
        if secret_refs.len() > MAX_SECRET_BATCH_ENTRIES {
            return Err(VaultError::SecretBatchTooLarge {
                max_entries: MAX_SECRET_BATCH_ENTRIES,
            });
        }
        let mut unique = BTreeSet::new();
        for secret_ref in secret_refs.iter().copied() {
            if !unique.insert(secret_ref) {
                return Err(VaultError::DuplicateSecretRef(secret_ref));
            }
        }
        let removed = unique
            .into_iter()
            .filter_map(|secret_ref| {
                self.payload
                    .entries
                    .remove(&secret_ref)
                    .map(|entry| (secret_ref, entry))
            })
            .collect::<Vec<_>>();
        if removed.is_empty() {
            return Ok(());
        }
        let previous_revision = self.payload.revision;
        self.payload.revision = previous_revision
            .checked_add(1)
            .ok_or(VaultError::InvalidEnvelope("revision overflow"))?;
        if let Err(error) = persist(self) {
            if matches!(error, VaultError::CommitStateUnknown(_)) {
                self.poisoned = true;
            } else {
                for (secret_ref, entry) in removed {
                    self.payload.entries.insert(secret_ref, entry);
                }
                self.payload.revision = previous_revision;
            }
            return Err(error);
        }
        Ok(())
    }

    fn insert_secrets_with_refs_using<F>(
        &mut self,
        secrets: &[SecretInsert<'_>],
        persist: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        self.ensure_usable()?;
        validate_secret_batch(&self.payload.entries, secrets)?;
        let previous_revision = self.payload.revision;
        let next_revision = previous_revision
            .checked_add(1)
            .ok_or(VaultError::InvalidEnvelope("revision overflow"))?;

        for secret in secrets {
            self.payload.entries.insert(
                secret.secret_ref,
                SecretEntry {
                    kind: secret.kind,
                    value: secret.value.to_vec(),
                },
            );
        }
        self.payload.revision = next_revision;

        if let Err(error) = persist(self) {
            if matches!(error, VaultError::CommitStateUnknown(_)) {
                self.poisoned = true;
            } else {
                for secret in secrets {
                    self.payload.entries.remove(&secret.secret_ref);
                }
                self.payload.revision = previous_revision;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn read_secret(
        &self,
        secret_ref: SecretRef,
        expected: SecretKind,
    ) -> Result<Option<SecretValue>> {
        self.ensure_usable()?;
        let Some(entry) = self.payload.entries.get(&secret_ref) else {
            return Ok(None);
        };
        if entry.kind != expected {
            return Err(VaultError::SecretKindMismatch {
                expected,
                actual: entry.kind,
            });
        }
        Ok(Some(SecretValue(Zeroizing::new(entry.value.clone()))))
    }

    /// Returns the bounded encrypted command-history payload. The caller is
    /// responsible for exposing entries only through a deliberately separate
    /// history API; this method is never part of SSH sync material.
    pub fn command_history(&self) -> Result<CommandHistoryPayload> {
        self.ensure_usable()?;
        Ok(self.payload.command_history.clone())
    }

    /// Replaces the complete bounded history projection in one Vault revision.
    /// This uses the same file lock, atomic replacement, and commit-unknown
    /// poison behavior as secret writes. It does not touch any SecretRef.
    pub fn replace_command_history(&mut self, history: CommandHistoryPayload) -> Result<()> {
        self.replace_command_history_using(history, Self::persist)
    }

    fn replace_command_history_using<F>(
        &mut self,
        history: CommandHistoryPayload,
        persist: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        self.ensure_usable()?;
        validate_command_history(&history)?;
        let previous_revision = self.payload.revision;
        let next_revision = previous_revision
            .checked_add(1)
            .ok_or(VaultError::InvalidEnvelope("revision overflow"))?;
        let previous_history = std::mem::replace(&mut self.payload.command_history, history);
        self.payload.revision = next_revision;
        if let Err(error) = persist(self) {
            if matches!(error, VaultError::CommitStateUnknown(_)) {
                self.poisoned = true;
            } else {
                self.payload.command_history = previous_history;
                self.payload.revision = previous_revision;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn change_password(&mut self, current_password: &[u8], new_password: &[u8]) -> Result<()> {
        self.ensure_usable()?;
        validate_new_password(new_password)?;

        let mut current_key_encryption_key = Zeroizing::new([0_u8; KEY_BYTES]);
        derive_key(current_password, &self.kdf, &mut current_key_encryption_key)?;
        let current_vault_master_key = unwrap_vault_key_parts(
            self.vault_id,
            &self.kdf,
            &self.wrapped_vault_key,
            &current_key_encryption_key,
        )?;
        if !bool::from(current_vault_master_key[..].ct_eq(&self.vault_master_key[..])) {
            return Err(VaultError::AuthenticationFailed);
        }

        let old_kdf = self.kdf.clone();
        let old_wrapped_vault_key = self.wrapped_vault_key.clone();
        let old_revision = self.payload.revision;

        let kdf = new_kdf_header()?;
        let mut key_encryption_key = Zeroizing::new([0_u8; KEY_BYTES]);
        derive_key(new_password, &kdf, &mut key_encryption_key)?;
        let wrapped_vault_key = wrap_vault_key(
            self.vault_id,
            &kdf,
            &key_encryption_key,
            &self.vault_master_key,
        )?;
        self.kdf = kdf;
        self.wrapped_vault_key = wrapped_vault_key;
        self.payload.revision = self
            .payload
            .revision
            .checked_add(1)
            .ok_or(VaultError::InvalidEnvelope("revision overflow"))?;

        if let Err(error) = self.persist() {
            if !matches!(error, VaultError::CommitStateUnknown(_)) {
                self.kdf = old_kdf;
                self.wrapped_vault_key = old_wrapped_vault_key;
                self.payload.revision = old_revision;
            }
            return Err(error);
        }
        Ok(())
    }

    fn persist(&mut self) -> Result<()> {
        self.ensure_usable()?;
        let previous_parent = self.payload.parent_envelope_sha256.clone();
        self.payload.parent_envelope_sha256 = current_envelope_digest(&self.path)?;
        let encoded = match self.encode_current_envelope() {
            Ok(encoded) => encoded,
            Err(error) => {
                self.payload.parent_envelope_sha256 = previous_parent;
                return Err(error);
            }
        };
        if encoded.len() as u64 > MAX_VAULT_FILE_BYTES {
            self.payload.parent_envelope_sha256 = previous_parent;
            return Err(VaultError::TooLarge);
        }
        match atomic_write(&self.path, &encoded) {
            Ok(()) => Ok(()),
            Err(AtomicWriteError::BeforeCommit(error)) => {
                self.payload.parent_envelope_sha256 = previous_parent;
                Err(error)
            }
            Err(AtomicWriteError::CommitStateUnknown(error)) => {
                self.poisoned = true;
                Err(VaultError::CommitStateUnknown(error))
            }
        }
    }

    fn encode_current_envelope(&self) -> Result<Vec<u8>> {
        let plaintext = Zeroizing::new(serde_json::to_vec(&self.payload)?);
        let payload = encrypt_payload(self.vault_id, &self.vault_master_key, &plaintext)?;
        let envelope = VaultEnvelope {
            magic: MAGIC.to_owned(),
            format_version: FORMAT_VERSION,
            vault_id: self.vault_id,
            kdf: self.kdf.clone(),
            wrapped_vault_key: self.wrapped_vault_key.clone(),
            payload,
        };
        Ok(serde_json::to_vec(&envelope)?)
    }

    fn ensure_usable(&self) -> Result<()> {
        if self.poisoned {
            return Err(VaultError::ReloadRequired);
        }
        Ok(())
    }
}

/// Opens a sync key envelope with the same password used by the originating
/// Vault. This does not create, replace or unlock a local Vault.
pub fn open_sync_key_material(
    envelope_bytes: &[u8],
    password: &[u8],
) -> Result<VaultSyncKeyMaterial> {
    if envelope_bytes.is_empty() || envelope_bytes.len() as u64 > MAX_VAULT_FILE_BYTES {
        return Err(VaultError::InvalidEnvelope(
            "invalid sync key envelope size",
        ));
    }
    let envelope: VaultSyncKeyEnvelope = serde_json::from_slice(envelope_bytes)?;
    if envelope.magic != SYNC_KEY_ENVELOPE_MAGIC
        || envelope.format_version != SYNC_KEY_ENVELOPE_FORMAT_VERSION
    {
        return Err(VaultError::InvalidEnvelope("invalid sync key envelope"));
    }
    validate_kdf_parameters(&envelope.kdf)?;
    validate_ciphertext_section(&envelope.wrapped_vault_key)?;
    let mut key_encryption_key = Zeroizing::new([0_u8; KEY_BYTES]);
    derive_key(password, &envelope.kdf, &mut key_encryption_key)?;
    let key = unwrap_vault_key_parts(
        envelope.vault_id,
        &envelope.kdf,
        &envelope.wrapped_vault_key,
        &key_encryption_key,
    )?;
    Ok(VaultSyncKeyMaterial {
        vault_id: envelope.vault_id,
        key,
        envelope: envelope_bytes.to_vec(),
    })
}

pub fn sync_key_envelope_vault_id(envelope_bytes: &[u8]) -> Result<Uuid> {
    let envelope: VaultSyncKeyEnvelope = serde_json::from_slice(envelope_bytes)?;
    if envelope.magic != SYNC_KEY_ENVELOPE_MAGIC
        || envelope.format_version != SYNC_KEY_ENVELOPE_FORMAT_VERSION
    {
        return Err(VaultError::InvalidEnvelope("invalid sync key envelope"));
    }
    validate_kdf_parameters(&envelope.kdf)?;
    validate_ciphertext_section(&envelope.wrapped_vault_key)?;
    Ok(envelope.vault_id)
}

fn new_kdf_header() -> Result<KdfHeader> {
    let mut salt = [0_u8; SALT_BYTES];
    fill_random(&mut salt)?;
    Ok(KdfHeader {
        id: KDF_ID.to_owned(),
        profile: KDF_PROFILE.to_owned(),
        memory_kib: KDF_MEMORY_KIB,
        iterations: KDF_ITERATIONS,
        parallelism: KDF_PARALLELISM,
        salt: BASE64.encode(salt),
    })
}

fn validate_new_password(password: &[u8]) -> Result<()> {
    if password.len() < MIN_PASSWORD_BYTES {
        return Err(VaultError::WeakPassword);
    }
    Ok(())
}

fn validate_payload_entries(payload: &VaultPayloadV1) -> Result<()> {
    for entry in payload.entries.values() {
        validate_secret_value(entry.kind, &entry.value)?;
    }
    validate_command_history(&payload.command_history)?;
    Ok(())
}

fn validate_command_history(history: &CommandHistoryPayload) -> Result<()> {
    if history.schema_version != COMMAND_HISTORY_SCHEMA_VERSION {
        return Err(VaultError::InvalidEnvelope(
            "unsupported command history schema",
        ));
    }
    if history.entries.len() > MAX_COMMAND_HISTORY_ENTRIES {
        return Err(VaultError::InvalidEnvelope(
            "command history exceeds entry limit",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut total_bytes = 0_usize;
    for entry in &history.entries {
        if entry.entry_id.is_nil() || !ids.insert(entry.entry_id) {
            return Err(VaultError::InvalidEnvelope(
                "invalid command history entry id",
            ));
        }
        if entry.scope.is_empty()
            || entry.scope.len() > MAX_COMMAND_HISTORY_SCOPE_BYTES
            || entry.scope.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(VaultError::InvalidEnvelope("invalid command history scope"));
        }
        if entry.command.is_empty()
            || entry.command.len() > MAX_COMMAND_HISTORY_COMMAND_BYTES
            || entry.command.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(VaultError::InvalidEnvelope(
                "invalid command history command",
            ));
        }
        total_bytes = total_bytes
            .checked_add(entry.scope.len())
            .and_then(|value| value.checked_add(entry.command.len()))
            .ok_or(VaultError::InvalidEnvelope("command history is too large"))?;
        if total_bytes > MAX_COMMAND_HISTORY_TOTAL_BYTES {
            return Err(VaultError::InvalidEnvelope("command history is too large"));
        }
    }
    Ok(())
}

fn validate_secret_batch(
    existing_entries: &BTreeMap<SecretRef, SecretEntry>,
    secrets: &[SecretInsert<'_>],
) -> Result<()> {
    if secrets.is_empty() {
        return Err(VaultError::EmptySecretBatch);
    }
    if secrets.len() > MAX_SECRET_BATCH_ENTRIES {
        return Err(VaultError::SecretBatchTooLarge {
            max_entries: MAX_SECRET_BATCH_ENTRIES,
        });
    }

    let mut batch_refs = BTreeSet::new();
    let mut total_value_bytes = 0_usize;
    for secret in secrets {
        if existing_entries.contains_key(&secret.secret_ref)
            || !batch_refs.insert(secret.secret_ref)
        {
            return Err(VaultError::DuplicateSecretRef(secret.secret_ref));
        }
        validate_secret_value(secret.kind, secret.value)?;
        total_value_bytes = total_value_bytes.checked_add(secret.value.len()).ok_or(
            VaultError::SecretBatchValueTooLarge {
                max_bytes: MAX_SECRET_BATCH_VALUE_BYTES,
            },
        )?;
        if total_value_bytes > MAX_SECRET_BATCH_VALUE_BYTES {
            return Err(VaultError::SecretBatchValueTooLarge {
                max_bytes: MAX_SECRET_BATCH_VALUE_BYTES,
            });
        }
    }
    Ok(())
}

fn validate_secret_value(kind: SecretKind, value: &[u8]) -> Result<()> {
    if value.is_empty() {
        return Err(VaultError::EmptySecret { kind });
    }
    let max_bytes = match kind {
        SecretKind::Password => MAX_PASSWORD_BYTES,
        SecretKind::PrivateKey => MAX_PRIVATE_KEY_BYTES,
        SecretKind::Passphrase => MAX_PASSPHRASE_BYTES,
        SecretKind::Certificate => MAX_CERTIFICATE_BYTES,
        SecretKind::LoginAutomation => MAX_PASSWORD_BYTES,
        SecretKind::OAuthRefreshToken => MAX_OAUTH_REFRESH_TOKEN_BYTES,
        SecretKind::PluginCredential => MAX_PASSWORD_BYTES,
        SecretKind::SshSyncKey => SYNC_KEY_BYTES,
        SecretKind::SshSyncRecoverySecret => MAX_SYNC_RECOVERY_SECRET_BYTES,
    };
    if value.len() > max_bytes {
        return Err(VaultError::SecretTooLarge { kind, max_bytes });
    }
    if kind == SecretKind::SshSyncKey && value.len() != SYNC_KEY_BYTES {
        return Err(VaultError::InvalidSecretLength {
            kind,
            expected_bytes: SYNC_KEY_BYTES,
        });
    }
    Ok(())
}

fn read_envelope(path: &Path) -> Result<VaultEnvelope> {
    let bytes = read_bounded(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = open_existing_private_file(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > MAX_VAULT_FILE_BYTES {
        return Err(VaultError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_VAULT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_VAULT_FILE_BYTES {
        return Err(VaultError::TooLarge);
    }
    Ok(bytes)
}

fn current_envelope_digest(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(hex::encode(Sha256::digest(read_bounded(path)?))))
}

fn validate_envelope(envelope: &VaultEnvelope) -> Result<()> {
    if envelope.magic != MAGIC {
        return Err(VaultError::InvalidEnvelope("wrong magic"));
    }
    if envelope.format_version != FORMAT_VERSION {
        return Err(VaultError::UnsupportedFormat(envelope.format_version));
    }
    validate_kdf_parameters(&envelope.kdf)?;
    validate_ciphertext_section(&envelope.wrapped_vault_key)?;
    validate_ciphertext_section(&envelope.payload)
}

fn validate_device_unlock_envelope(envelope: &DeviceUnlockEnvelope) -> Result<()> {
    if envelope.magic != DEVICE_UNLOCK_MAGIC {
        return Err(VaultError::InvalidEnvelope("wrong device unlock magic"));
    }
    if envelope.format_version != DEVICE_UNLOCK_FORMAT_VERSION {
        return Err(VaultError::InvalidEnvelope(
            "unsupported device unlock format",
        ));
    }
    validate_ciphertext_section(&envelope.wrapped_vault_key)
}

fn validate_ciphertext_section(section: &CiphertextSection) -> Result<()> {
    if section.cipher != CIPHER_ID {
        return Err(VaultError::InvalidEnvelope("unsupported cipher"));
    }
    let _ = decode_nonce(section)?;
    let ciphertext = BASE64
        .decode(&section.ciphertext)
        .map_err(|_| VaultError::InvalidEnvelope("invalid ciphertext"))?;
    if ciphertext.is_empty() {
        return Err(VaultError::InvalidEnvelope("empty ciphertext"));
    }
    Ok(())
}

fn validate_kdf_parameters(kdf: &KdfHeader) -> Result<()> {
    if kdf.id != KDF_ID
        || kdf.profile != KDF_PROFILE
        || kdf.memory_kib != KDF_MEMORY_KIB
        || kdf.iterations != KDF_ITERATIONS
        || kdf.parallelism != KDF_PARALLELISM
    {
        return Err(VaultError::InvalidEnvelope("unsupported KDF profile"));
    }
    let salt = BASE64
        .decode(&kdf.salt)
        .map_err(|_| VaultError::InvalidEnvelope("invalid salt"))?;
    if salt.len() != SALT_BYTES {
        return Err(VaultError::InvalidEnvelope("invalid salt length"));
    }
    Ok(())
}

fn derive_key(password: &[u8], kdf: &KdfHeader, output: &mut [u8; KEY_BYTES]) -> Result<()> {
    validate_kdf_parameters(kdf)?;
    let salt = Zeroizing::new(
        BASE64
            .decode(&kdf.salt)
            .map_err(|_| VaultError::InvalidEnvelope("invalid salt"))?,
    );
    let params = Params::new(
        kdf.memory_kib,
        kdf.iterations,
        kdf.parallelism,
        Some(KEY_BYTES),
    )
    .map_err(|_| VaultError::KeyDerivation)?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt.as_slice(), output)
        .map_err(|_| VaultError::KeyDerivation)
}

fn wrap_vault_key(
    vault_id: Uuid,
    kdf: &KdfHeader,
    key_encryption_key: &[u8; KEY_BYTES],
    vault_master_key: &[u8; KEY_BYTES],
) -> Result<CiphertextSection> {
    let mut nonce = [0_u8; NONCE_BYTES];
    fill_random(&mut nonce)?;
    let nonce_encoded = BASE64.encode(nonce);
    let aad = serde_json::to_vec(&KeyWrapAad {
        magic: MAGIC,
        format_version: FORMAT_VERSION,
        vault_id,
        kdf,
        cipher: CIPHER_ID,
        nonce: &nonce_encoded,
    })?;
    let cipher = XChaCha20Poly1305::new_from_slice(key_encryption_key)
        .map_err(|_| VaultError::InvalidEnvelope("invalid key encryption key"))?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: vault_master_key,
                aad: &aad,
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    Ok(CiphertextSection {
        cipher: CIPHER_ID.to_owned(),
        nonce: nonce_encoded,
        ciphertext: BASE64.encode(ciphertext),
    })
}

fn unwrap_vault_key(
    envelope: &VaultEnvelope,
    key_encryption_key: &[u8; KEY_BYTES],
) -> Result<Zeroizing<[u8; KEY_BYTES]>> {
    unwrap_vault_key_parts(
        envelope.vault_id,
        &envelope.kdf,
        &envelope.wrapped_vault_key,
        key_encryption_key,
    )
}

fn unwrap_vault_key_parts(
    vault_id: Uuid,
    kdf: &KdfHeader,
    wrapped_vault_key: &CiphertextSection,
    key_encryption_key: &[u8; KEY_BYTES],
) -> Result<Zeroizing<[u8; KEY_BYTES]>> {
    let nonce = decode_nonce(wrapped_vault_key)?;
    let ciphertext = BASE64
        .decode(&wrapped_vault_key.ciphertext)
        .map_err(|_| VaultError::InvalidEnvelope("invalid wrapped vault key"))?;
    let aad = serde_json::to_vec(&KeyWrapAad {
        magic: MAGIC,
        format_version: FORMAT_VERSION,
        vault_id,
        kdf,
        cipher: &wrapped_vault_key.cipher,
        nonce: &wrapped_vault_key.nonce,
    })?;
    let cipher = XChaCha20Poly1305::new_from_slice(key_encryption_key)
        .map_err(|_| VaultError::InvalidEnvelope("invalid key encryption key"))?;
    let unwrapped = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::AuthenticationFailed)?,
    );
    if unwrapped.len() != KEY_BYTES {
        return Err(VaultError::AuthenticationFailed);
    }
    let mut vault_master_key = Zeroizing::new([0_u8; KEY_BYTES]);
    vault_master_key.copy_from_slice(&unwrapped);
    Ok(vault_master_key)
}

fn wrap_device_vault_key(
    vault_id: Uuid,
    device_key: &DeviceUnlockKey,
    vault_master_key: &[u8; KEY_BYTES],
) -> Result<CiphertextSection> {
    let mut nonce = [0_u8; NONCE_BYTES];
    fill_random(&mut nonce)?;
    let nonce_encoded = BASE64.encode(nonce);
    let aad = serde_json::to_vec(&DeviceUnlockAad {
        magic: DEVICE_UNLOCK_MAGIC,
        format_version: DEVICE_UNLOCK_FORMAT_VERSION,
        vault_id,
        cipher: CIPHER_ID,
        nonce: &nonce_encoded,
    })?;
    let cipher = XChaCha20Poly1305::new_from_slice(device_key.expose())
        .map_err(|_| VaultError::InvalidDeviceUnlockKey)?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: vault_master_key,
                aad: &aad,
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    Ok(CiphertextSection {
        cipher: CIPHER_ID.to_owned(),
        nonce: nonce_encoded,
        ciphertext: BASE64.encode(ciphertext),
    })
}

fn unwrap_device_vault_key(
    envelope: &DeviceUnlockEnvelope,
    device_key: &DeviceUnlockKey,
) -> Result<Zeroizing<[u8; KEY_BYTES]>> {
    let nonce = decode_nonce(&envelope.wrapped_vault_key)?;
    let ciphertext = BASE64
        .decode(&envelope.wrapped_vault_key.ciphertext)
        .map_err(|_| VaultError::InvalidEnvelope("invalid wrapped device vault key"))?;
    let aad = serde_json::to_vec(&DeviceUnlockAad {
        magic: &envelope.magic,
        format_version: envelope.format_version,
        vault_id: envelope.vault_id,
        cipher: &envelope.wrapped_vault_key.cipher,
        nonce: &envelope.wrapped_vault_key.nonce,
    })?;
    let cipher = XChaCha20Poly1305::new_from_slice(device_key.expose())
        .map_err(|_| VaultError::InvalidDeviceUnlockKey)?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::AuthenticationFailed)?,
    );
    if plaintext.len() != KEY_BYTES {
        return Err(VaultError::AuthenticationFailed);
    }
    let mut vault_master_key = Zeroizing::new([0_u8; KEY_BYTES]);
    vault_master_key.copy_from_slice(&plaintext);
    Ok(vault_master_key)
}

fn encrypt_payload(
    vault_id: Uuid,
    vault_master_key: &[u8; KEY_BYTES],
    plaintext: &[u8],
) -> Result<CiphertextSection> {
    let mut nonce = [0_u8; NONCE_BYTES];
    fill_random(&mut nonce)?;
    let nonce_encoded = BASE64.encode(nonce);
    let aad = serde_json::to_vec(&PayloadAad {
        magic: MAGIC,
        format_version: FORMAT_VERSION,
        vault_id,
        cipher: CIPHER_ID,
        nonce: &nonce_encoded,
    })?;
    let cipher = XChaCha20Poly1305::new_from_slice(vault_master_key)
        .map_err(|_| VaultError::InvalidEnvelope("invalid vault master key"))?;
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| VaultError::AuthenticationFailed)?;
    Ok(CiphertextSection {
        cipher: CIPHER_ID.to_owned(),
        nonce: nonce_encoded,
        ciphertext: BASE64.encode(ciphertext),
    })
}

fn decrypt_payload(
    envelope: &VaultEnvelope,
    vault_master_key: &[u8; KEY_BYTES],
) -> Result<VaultPayloadV1> {
    let nonce = decode_nonce(&envelope.payload)?;
    let ciphertext = BASE64
        .decode(&envelope.payload.ciphertext)
        .map_err(|_| VaultError::InvalidEnvelope("invalid ciphertext"))?;
    let aad = serde_json::to_vec(&PayloadAad {
        magic: &envelope.magic,
        format_version: envelope.format_version,
        vault_id: envelope.vault_id,
        cipher: &envelope.payload.cipher,
        nonce: &envelope.payload.nonce,
    })?;
    let cipher = XChaCha20Poly1305::new_from_slice(vault_master_key)
        .map_err(|_| VaultError::InvalidEnvelope("invalid vault master key"))?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::AuthenticationFailed)?,
    );
    serde_json::from_slice(&plaintext).map_err(VaultError::Serialization)
}

fn decode_nonce(section: &CiphertextSection) -> Result<XNonce> {
    let nonce = BASE64
        .decode(&section.nonce)
        .map_err(|_| VaultError::InvalidEnvelope("invalid nonce"))?;
    XNonce::try_from(nonce.as_slice())
        .map_err(|_| VaultError::InvalidEnvelope("invalid nonce length"))
}

fn fill_random(output: &mut [u8]) -> Result<()> {
    getrandom::fill(output).map_err(|error| {
        VaultError::Io(std::io::Error::other(format!(
            "operating system randomness unavailable: {error}"
        )))
    })
}

fn ensure_private_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or(VaultError::InvalidEnvelope("vault path has no parent"))?;
    let created = !parent.exists();
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if created {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let mode = fs::metadata(parent)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(VaultError::InsecurePermissions);
        }
    }
    #[cfg(windows)]
    windows_acl::secure_directory(parent)?;
    Ok(())
}

fn lock_path(vault_path: &Path) -> Result<PathBuf> {
    let name = vault_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(VaultError::InvalidEnvelope("vault file name is invalid"))?;
    Ok(vault_path.with_file_name(format!(".{name}.lock")))
}

fn open_private_file(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, READ_CONTROL,
            WRITE_DAC, WRITE_OWNER,
        };

        options
            .access_mode(
                FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
            )
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
            drop(file);
            if create_new {
                let _ = fs::remove_file(path);
            }
            return Err(VaultError::Io(error));
        }
    }
    #[cfg(windows)]
    if let Err(error) = windows_acl::secure_file(&file) {
        drop(file);
        if create_new {
            let _ = fs::remove_file(path);
        }
        return Err(VaultError::Io(error));
    }
    Ok(file)
}

fn open_existing_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
        };

        options
            .access_mode(FILE_GENERIC_READ | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    windows_acl::secure_file(&file)?;
    Ok(file)
}

#[cfg(windows)]
mod windows_acl {
    use std::{
        ffi::c_void,
        fs::{File, OpenOptions},
        io,
        mem::{size_of, zeroed},
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        path::Path,
        ptr::{addr_of, null, null_mut},
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
            Authorization::{
                EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS,
                SetEntriesInAclW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
                TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
            },
            CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
            GetSecurityDescriptorControl, GetTokenInformation, INHERITED_ACE,
            OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
            PSID, SE_DACL_PROTECTED, SECURITY_MAX_SID_SIZE, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
        },
        Storage::FileSystem::{
            FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
            FileAttributeTagInfo, GetFileInformationByHandleEx, READ_CONTROL, WRITE_DAC,
            WRITE_OWNER,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    use super::{
        PrivateObjectKind, WINDOWS_PRIVATE_ACL_PRINCIPALS, WindowsAclPrincipal,
        windows_acl_inheritance,
    };

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct LocalAllocation(*mut c_void);

    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    struct CurrentUserSid {
        buffer: Vec<usize>,
    }

    impl CurrentUserSid {
        fn query() -> io::Result<Self> {
            let mut token = null_mut();
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = OwnedHandle(token);
            let mut required = 0_u32;
            unsafe {
                GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required);
            }
            if required < size_of::<TOKEN_USER>() as u32 {
                return Err(io::Error::last_os_error());
            }
            let words = (required as usize).div_ceil(size_of::<usize>());
            let mut buffer = vec![0_usize; words];
            if unsafe {
                GetTokenInformation(
                    token.0,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { buffer })
        }

        fn as_ptr(&self) -> PSID {
            let token_user = self.buffer.as_ptr().cast::<TOKEN_USER>();
            unsafe { (*token_user).User.Sid }
        }
    }

    pub(super) fn secure_directory(path: &Path) -> io::Result<()> {
        let directory = OpenOptions::new()
            .read(true)
            .access_mode(FILE_GENERIC_READ | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        secure_handle(&directory, PrivateObjectKind::Directory)
    }

    pub(super) fn secure_file(file: &File) -> io::Result<()> {
        secure_handle(file, PrivateObjectKind::File)
    }

    fn secure_handle(file: &File, kind: PrivateObjectKind) -> io::Result<()> {
        let handle: HANDLE = file.as_raw_handle();
        reject_reparse_point(handle)?;
        let current_user = CurrentUserSid::query()?;
        let mut system_sid =
            [0_usize; (SECURITY_MAX_SID_SIZE as usize).div_ceil(size_of::<usize>())];
        let mut system_sid_bytes = SECURITY_MAX_SID_SIZE;
        if unsafe {
            CreateWellKnownSid(
                WinLocalSystemSid,
                null_mut(),
                system_sid.as_mut_ptr().cast(),
                &mut system_sid_bytes,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let system_sid = system_sid.as_mut_ptr().cast::<c_void>();
        let inheritance = windows_acl_inheritance(kind);
        debug_assert_eq!(
            inheritance,
            if kind == PrivateObjectKind::Directory {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                0
            }
        );
        let entries = WINDOWS_PRIVATE_ACL_PRINCIPALS.map(|principal| {
            let (sid, trustee_type) = match principal {
                WindowsAclPrincipal::CurrentUser => (current_user.as_ptr(), TRUSTEE_IS_USER),
                WindowsAclPrincipal::LocalSystem => (system_sid, TRUSTEE_IS_WELL_KNOWN_GROUP),
            };
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_ALL_ACCESS,
                grfAccessMode: SET_ACCESS,
                grfInheritance: inheritance,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: null_mut(),
                    MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: trustee_type,
                    ptstrName: sid.cast(),
                },
            }
        });
        let mut acl = null_mut::<ACL>();
        let status =
            unsafe { SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), null(), &mut acl) };
        if status != ERROR_SUCCESS {
            return Err(win32_error(status));
        }
        let _acl_allocation = LocalAllocation(acl.cast());
        let security_information = OWNER_SECURITY_INFORMATION
            | DACL_SECURITY_INFORMATION
            | PROTECTED_DACL_SECURITY_INFORMATION;
        let status = unsafe {
            SetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                security_information,
                current_user.as_ptr(),
                null_mut(),
                acl,
                null(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(win32_error(status));
        }
        verify_handle(handle, kind, current_user.as_ptr(), system_sid)
    }

    fn reject_reparse_point(handle: HANDLE) -> io::Result<()> {
        let mut information = FILE_ATTRIBUTE_TAG_INFO::default();
        if unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                addr_of!(information).cast_mut().cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if information.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "vault paths must not be Windows reparse points",
            ));
        }
        Ok(())
    }

    fn verify_handle(
        handle: HANDLE,
        kind: PrivateObjectKind,
        current_user_sid: PSID,
        system_sid: PSID,
    ) -> io::Result<()> {
        let mut owner = null_mut();
        let mut dacl = null_mut::<ACL>();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let status = unsafe {
            windows_sys::Win32::Security::Authorization::GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(win32_error(status));
        }
        let _descriptor = LocalAllocation(descriptor);
        if owner.is_null() || unsafe { EqualSid(owner, current_user_sid) } == 0 {
            return Err(private_acl_verification_error());
        }
        if dacl.is_null() {
            return Err(private_acl_verification_error());
        }
        let mut control = 0_u16;
        let mut revision = 0_u32;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
        {
            return Err(private_acl_verification_error());
        }
        let mut acl_information: ACL_SIZE_INFORMATION = unsafe { zeroed() };
        if unsafe {
            GetAclInformation(
                dacl,
                addr_of!(acl_information).cast_mut().cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
            || acl_information.AceCount != WINDOWS_PRIVATE_ACL_PRINCIPALS.len() as u32
        {
            return Err(private_acl_verification_error());
        }
        let expected_flags = windows_acl_inheritance(kind) as u8;
        let mut current_user_aces = 0_u8;
        let mut system_aces = 0_u8;
        for index in 0..acl_information.AceCount {
            let mut raw_ace = null_mut::<c_void>();
            if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 || raw_ace.is_null() {
                return Err(private_acl_verification_error());
            }
            let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
            let ace = unsafe { &*ace };
            if ace.Header.AceType != 0
                || ace.Header.AceFlags != expected_flags
                || u32::from(ace.Header.AceFlags) & INHERITED_ACE != 0
                || ace.Mask != FILE_ALL_ACCESS
            {
                return Err(private_acl_verification_error());
            }
            let ace_sid = addr_of!(ace.SidStart).cast_mut().cast::<c_void>();
            if unsafe { EqualSid(ace_sid, current_user_sid) } != 0 {
                current_user_aces += 1;
            } else if unsafe { EqualSid(ace_sid, system_sid) } != 0 {
                system_aces += 1;
            } else {
                return Err(private_acl_verification_error());
            }
        }
        if current_user_aces != 1 || system_aces != 1 {
            return Err(private_acl_verification_error());
        }
        Ok(())
    }

    fn private_acl_verification_error() -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "vault Windows ACL could not be restricted to the current user and LocalSystem",
        )
    }

    fn win32_error(status: u32) -> io::Error {
        io::Error::from_raw_os_error(status as i32)
    }
}

#[derive(Debug)]
enum AtomicWriteError {
    BeforeCommit(VaultError),
    CommitStateUnknown(std::io::Error),
}

fn atomic_write(path: &Path, contents: &[u8]) -> std::result::Result<(), AtomicWriteError> {
    atomic_write_with(path, contents, sync_parent_directory)
}

fn atomic_write_with<F>(
    path: &Path,
    contents: &[u8],
    after_replace: F,
) -> std::result::Result<(), AtomicWriteError>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    ensure_private_parent_directory(path).map_err(AtomicWriteError::BeforeCommit)?;
    let parent = path.parent().ok_or({
        AtomicWriteError::BeforeCommit(VaultError::InvalidEnvelope("vault path has no parent"))
    })?;
    let name = path.file_name().and_then(|name| name.to_str()).ok_or({
        AtomicWriteError::BeforeCommit(VaultError::InvalidEnvelope("vault file name is invalid"))
    })?;
    let temporary_path = parent.join(format!(".{name}.{}.staging", Uuid::new_v4()));
    let mut temporary =
        open_private_file(&temporary_path, true).map_err(AtomicWriteError::BeforeCommit)?;
    if let Err(error) = (|| -> std::io::Result<()> {
        temporary.write_all(contents)?;
        temporary.flush()?;
        temporary.sync_all()
    })() {
        let _ = fs::remove_file(&temporary_path);
        return Err(AtomicWriteError::BeforeCommit(VaultError::Io(error)));
    }
    drop(temporary);

    if let Err(error) = replace_file(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(AtomicWriteError::BeforeCommit(VaultError::Io(error)));
    }
    after_replace(parent).map_err(AtomicWriteError::CommitStateUnknown)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &[u8] = b"correct horse battery staple";
    const NEW_PASSWORD: &[u8] = b"a different correct vault password";

    fn vault_path(directory: &tempfile::TempDir) -> PathBuf {
        directory.path().join("vault").join("vault.nvx")
    }

    #[test]
    fn creates_unlocks_and_persists_a_secret_without_plaintext_leakage() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let secret = b"PRIVATE KEY MATERIAL THAT MUST NOT APPEAR ON DISK";
        let (vault_id, secret_ref, empty_envelope) = {
            let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
            let vault_id = vault.vault_id();
            let empty_envelope = fs::read(&path).expect("read empty envelope");
            let secret_ref = vault
                .insert_secret(SecretKind::PrivateKey, secret)
                .expect("insert secret");
            assert_eq!(vault.revision(), 1);
            let bytes = fs::read(&path).expect("read envelope");
            assert!(!bytes.windows(secret.len()).any(|window| window == secret));
            (vault_id, secret_ref, empty_envelope)
        };

        let vault = UnlockedVault::unlock(&path, PASSWORD).expect("unlock vault");
        assert_eq!(vault.vault_id(), vault_id);
        assert_eq!(vault.entry_count(), 1);
        assert_eq!(
            vault
                .read_secret(secret_ref, SecretKind::PrivateKey)
                .expect("vault usable")
                .expect("read secret")
                .expose(),
            secret
        );
        assert_ne!(
            fs::read(path).expect("read rewritten envelope"),
            empty_envelope
        );
    }

    #[test]
    fn device_unlock_slot_reopens_only_its_bound_vault() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let slot_path = directory.path().join("vault").join("auto-unlock.nvx");
        let device_key = DeviceUnlockKey::generate().expect("device key");
        let secret_ref = {
            let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
            let secret_ref = vault
                .insert_secret(SecretKind::Password, b"server password")
                .expect("insert secret");
            vault
                .write_device_unlock_slot(&slot_path, &device_key)
                .expect("write slot");
            secret_ref
        };

        let vault = UnlockedVault::unlock_with_device_key(&path, &slot_path, &device_key)
            .expect("device unlock");
        assert_eq!(
            vault
                .read_secret(secret_ref, SecretKind::Password)
                .expect("vault usable")
                .expect("secret exists")
                .expose(),
            b"server password"
        );
    }

    #[test]
    fn device_unlock_slot_rejects_wrong_device_key_and_vault() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let slot_path = directory.path().join("vault").join("auto-unlock.nvx");
        let device_key = DeviceUnlockKey::generate().expect("device key");
        {
            let vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
            vault
                .write_device_unlock_slot(&slot_path, &device_key)
                .expect("write slot");
        }

        let wrong_key = DeviceUnlockKey::generate().expect("wrong key");
        assert!(matches!(
            UnlockedVault::unlock_with_device_key(&path, &slot_path, &wrong_key),
            Err(VaultError::AuthenticationFailed)
        ));

        let other_directory = tempfile::tempdir().expect("other tempdir");
        let other_path = vault_path(&other_directory);
        drop(UnlockedVault::create(&other_path, PASSWORD).expect("other vault"));
        assert!(matches!(
            UnlockedVault::unlock_with_device_key(&other_path, &slot_path, &device_key),
            Err(VaultError::DeviceUnlockVaultMismatch)
        ));
    }

    #[test]
    fn verifies_the_password_without_rewriting_or_reopening_the_vault() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        vault.verify_password(PASSWORD).expect("password accepted");
        assert!(matches!(
            vault.verify_password(b"incorrect vault password"),
            Err(VaultError::AuthenticationFailed)
        ));
    }

    #[test]
    fn constructs_and_parses_only_non_nil_secret_references() {
        let uuid = Uuid::new_v4();
        let from_uuid = SecretRef::from_uuid(uuid).expect("non-nil UUID");
        let parsed = SecretRef::parse(uuid.to_string()).expect("parse UUID");
        let from_str: SecretRef = uuid.to_string().parse().expect("FromStr UUID");

        assert_eq!(from_uuid, parsed);
        assert_eq!(parsed, from_str);
        assert_eq!(from_str.as_uuid(), uuid);
        assert!(matches!(
            SecretRef::from_uuid(Uuid::nil()),
            Err(VaultError::InvalidSecretRef)
        ));
        assert!(matches!(
            SecretRef::parse("not-a-uuid"),
            Err(VaultError::InvalidSecretRef)
        ));
        let error = serde_json::from_str::<SecretRef>(&format!("\"{}\"", Uuid::nil()))
            .expect_err("nil serialized reference rejected");
        assert!(error.to_string().contains("non-nil UUID"));
    }

    #[test]
    fn atomically_inserts_multiple_caller_minted_references_in_one_revision() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let private_key_ref = SecretRef::new();
        let passphrase_ref = SecretRef::new();
        let private_key = b"PRIVATE KEY MATERIAL";
        let passphrase = b"key passphrase";

        {
            let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
            vault
                .insert_secrets_with_refs(&[
                    SecretInsert::new(private_key_ref, SecretKind::PrivateKey, private_key),
                    SecretInsert::new(passphrase_ref, SecretKind::Passphrase, passphrase),
                ])
                .expect("insert batch");
            assert_eq!(vault.revision(), 1);
            assert_eq!(vault.entry_count(), 2);
        }

        let vault = UnlockedVault::unlock(&path, PASSWORD).expect("unlock vault");
        assert_eq!(vault.revision(), 1);
        assert_eq!(
            vault
                .read_secret(private_key_ref, SecretKind::PrivateKey)
                .expect("correct kind")
                .expect("private key exists")
                .expose(),
            private_key
        );
        assert_eq!(
            vault
                .read_secret(passphrase_ref, SecretKind::Passphrase)
                .expect("correct kind")
                .expect("passphrase exists")
                .expose(),
            passphrase
        );
    }

    #[test]
    fn rejects_invalid_batches_without_mutating_the_payload() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let existing_ref = vault
            .insert_secret(SecretKind::Password, b"existing")
            .expect("insert existing");
        let revision = vault.revision();
        let entry_count = vault.entry_count();

        assert!(matches!(
            vault.insert_secrets_with_refs(&[]),
            Err(VaultError::EmptySecretBatch)
        ));
        assert!(matches!(
            vault.insert_secrets_with_refs(&[SecretInsert::new(
                SecretRef::new(),
                SecretKind::Passphrase,
                b""
            )]),
            Err(VaultError::EmptySecret {
                kind: SecretKind::Passphrase
            })
        ));

        let oversized_password = vec![b'x'; MAX_PASSWORD_BYTES + 1];
        assert!(matches!(
            vault.insert_secrets_with_refs(&[SecretInsert::new(
                SecretRef::new(),
                SecretKind::Password,
                &oversized_password
            )]),
            Err(VaultError::SecretTooLarge {
                kind: SecretKind::Password,
                max_bytes: MAX_PASSWORD_BYTES
            })
        ));

        assert!(matches!(
            vault.insert_secrets_with_refs(&[SecretInsert::new(
                SecretRef::new(),
                SecretKind::SshSyncKey,
                &[7_u8; SYNC_KEY_BYTES - 1]
            )]),
            Err(VaultError::InvalidSecretLength {
                kind: SecretKind::SshSyncKey,
                expected_bytes: SYNC_KEY_BYTES
            })
        ));
        let duplicate_ref = SecretRef::new();
        assert!(matches!(
            vault.insert_secrets_with_refs(&[
                SecretInsert::new(duplicate_ref, SecretKind::PrivateKey, b"key"),
                SecretInsert::new(duplicate_ref, SecretKind::Passphrase, b"passphrase"),
            ]),
            Err(VaultError::DuplicateSecretRef(value)) if value == duplicate_ref
        ));
        assert!(matches!(
            vault.insert_secrets_with_refs(&[SecretInsert::new(
                existing_ref,
                SecretKind::Password,
                b"replacement"
            )]),
            Err(VaultError::DuplicateSecretRef(value)) if value == existing_ref
        ));

        assert_eq!(vault.revision(), revision);
        assert_eq!(vault.entry_count(), entry_count);
        assert_eq!(
            vault
                .read_secret(existing_ref, SecretKind::Password)
                .expect("correct kind")
                .expect("existing secret remains")
                .expose(),
            b"existing"
        );
    }

    #[test]
    fn stores_platform_secrets_under_dedicated_non_credential_kinds() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let refresh_ref = SecretRef::new();
        let plugin_credential_ref = SecretRef::new();
        let sync_key_ref = SecretRef::new();
        let recovery_ref = SecretRef::new();
        let sync_key = [9_u8; SYNC_KEY_BYTES];
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");

        vault
            .insert_secrets_with_refs(&[
                SecretInsert::new(
                    refresh_ref,
                    SecretKind::OAuthRefreshToken,
                    b"opaque refresh token",
                ),
                SecretInsert::new(sync_key_ref, SecretKind::SshSyncKey, &sync_key),
                SecretInsert::new(
                    recovery_ref,
                    SecretKind::SshSyncRecoverySecret,
                    b"independent recovery secret",
                ),
                SecretInsert::new(
                    plugin_credential_ref,
                    SecretKind::PluginCredential,
                    b"plugin API token",
                ),
            ])
            .expect("insert platform secrets");

        assert_eq!(
            vault
                .read_secret(sync_key_ref, SecretKind::SshSyncKey)
                .expect("correct kind")
                .expect("sync key exists")
                .expose(),
            sync_key
        );
        assert!(matches!(
            vault.read_secret(refresh_ref, SecretKind::Password),
            Err(VaultError::SecretKindMismatch {
                expected: SecretKind::Password,
                actual: SecretKind::OAuthRefreshToken,
            })
        ));
        assert!(matches!(
            vault.read_secret(plugin_credential_ref, SecretKind::OAuthRefreshToken),
            Err(VaultError::SecretKindMismatch {
                expected: SecretKind::OAuthRefreshToken,
                actual: SecretKind::PluginCredential,
            })
        ));
    }

    #[test]
    fn rolls_back_the_entire_batch_after_a_definite_commit_failure() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let first_ref = SecretRef::new();
        let second_ref = SecretRef::new();

        let error = vault
            .insert_secrets_with_refs_using(
                &[
                    SecretInsert::new(first_ref, SecretKind::PrivateKey, b"key"),
                    SecretInsert::new(second_ref, SecretKind::Passphrase, b"passphrase"),
                ],
                |_| Err(VaultError::Io(std::io::Error::other("injected failure"))),
            )
            .expect_err("definite failure");

        assert!(matches!(error, VaultError::Io(_)));
        assert_eq!(vault.revision(), 0);
        assert_eq!(vault.entry_count(), 0);
        assert!(!vault.payload.entries.contains_key(&first_ref));
        assert!(!vault.payload.entries.contains_key(&second_ref));
        assert!(!vault.requires_reload());
    }

    #[test]
    fn retains_fixed_references_and_requires_reload_when_commit_state_is_unknown() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let private_key_ref = SecretRef::new();
        let passphrase_ref = SecretRef::new();

        let error = vault
            .insert_secrets_with_refs_using(
                &[
                    SecretInsert::new(private_key_ref, SecretKind::PrivateKey, b"private key"),
                    SecretInsert::new(passphrase_ref, SecretKind::Passphrase, b"passphrase"),
                ],
                |_| {
                    Err(VaultError::CommitStateUnknown(std::io::Error::other(
                        "injected directory sync failure",
                    )))
                },
            )
            .expect_err("uncertain commit");

        assert!(matches!(error, VaultError::CommitStateUnknown(_)));
        assert_eq!(vault.revision(), 1);
        assert!(vault.payload.entries.contains_key(&private_key_ref));
        assert!(vault.payload.entries.contains_key(&passphrase_ref));
        assert!(vault.requires_reload());
        assert!(matches!(
            vault.read_secret(private_key_ref, SecretKind::PrivateKey),
            Err(VaultError::ReloadRequired)
        ));
    }

    #[test]
    fn batch_delete_is_atomic_and_missing_references_are_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let first_ref = vault
            .insert_secret(SecretKind::LoginAutomation, b"first")
            .expect("first secret");
        let second_ref = vault
            .insert_secret(SecretKind::LoginAutomation, b"second")
            .expect("second secret");
        let before_delete = vault.revision();

        vault
            .delete_secrets(&[first_ref, second_ref])
            .expect("delete batch");
        assert_eq!(vault.revision(), before_delete + 1);
        assert_eq!(vault.entry_count(), 0);

        let after_delete = vault.revision();
        vault
            .delete_secrets(&[first_ref, SecretRef::new()])
            .expect("missing refs are already clean");
        assert_eq!(vault.revision(), after_delete);
    }

    #[test]
    fn batch_delete_rolls_back_after_a_definite_commit_failure() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let secret_ref = vault
            .insert_secret(SecretKind::LoginAutomation, b"secret")
            .expect("secret");
        let revision = vault.revision();

        let error = vault
            .delete_secrets_using(&[secret_ref], |_| {
                Err(VaultError::Io(std::io::Error::other("injected failure")))
            })
            .expect_err("definite failure");

        assert!(matches!(error, VaultError::Io(_)));
        assert_eq!(vault.revision(), revision);
        assert!(vault.payload.entries.contains_key(&secret_ref));
        assert!(!vault.requires_reload());
    }

    #[test]
    fn batch_delete_poisoned_state_preserves_authoritative_unknown_outcome() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let secret_ref = vault
            .insert_secret(SecretKind::LoginAutomation, b"secret")
            .expect("secret");

        let error = vault
            .delete_secrets_using(&[secret_ref], |_| {
                Err(VaultError::CommitStateUnknown(std::io::Error::other(
                    "injected directory sync failure",
                )))
            })
            .expect_err("unknown commit");

        assert!(matches!(error, VaultError::CommitStateUnknown(_)));
        assert!(!vault.payload.entries.contains_key(&secret_ref));
        assert!(vault.requires_reload());
        assert!(matches!(
            vault.delete_secrets(&[secret_ref]),
            Err(VaultError::ReloadRequired)
        ));
    }

    #[test]
    fn distinguishes_missing_secrets_from_kind_mismatches() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let password_ref = vault
            .insert_secret(SecretKind::Password, b"server password")
            .expect("insert password");

        assert!(
            vault
                .read_secret(SecretRef::new(), SecretKind::Password)
                .expect("missing is not an error")
                .is_none()
        );
        assert!(matches!(
            vault.read_secret(password_ref, SecretKind::Passphrase),
            Err(VaultError::SecretKindMismatch {
                expected: SecretKind::Passphrase,
                actual: SecretKind::Password
            })
        ));
    }

    #[test]
    fn plaintext_is_redacted_from_public_debug_output() {
        let secret_ref = SecretRef::new();
        let plaintext = b"plaintext that must be redacted";
        let insert = SecretInsert::new(secret_ref, SecretKind::PrivateKey, plaintext);
        let value = SecretValue(Zeroizing::new(plaintext.to_vec()));

        assert!(!format!("{insert:?}").contains("plaintext"));
        assert!(!format!("{value:?}").contains("plaintext"));
    }

    #[test]
    fn command_history_is_versioned_encrypted_and_redacted() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let history = CommandHistoryPayload {
            schema_version: COMMAND_HISTORY_SCHEMA_VERSION,
            entries: vec![CommandHistoryEntry {
                entry_id: Uuid::now_v7(),
                scope: "local".to_owned(),
                command: "top secret command text".to_owned(),
                completed_at_unix_ms: 123,
                elapsed_millis: 456,
                exit_code: Some(0),
            }],
        };
        {
            let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
            vault
                .replace_command_history(history.clone())
                .expect("replace history");
            assert!(!format!("{vault:?}").contains("top secret command text"));
        }
        let vault = UnlockedVault::unlock(&path, PASSWORD).expect("unlock vault");
        let recovered = vault.command_history().expect("read history");
        assert_eq!(recovered.entries.len(), 1);
        assert_eq!(recovered.entries[0].command, "top secret command text");
        assert!(!format!("{:?}", recovered.entries[0]).contains("top secret command text"));
    }

    #[test]
    fn command_history_replacement_rolls_back_after_definite_failure() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let history = CommandHistoryPayload {
            schema_version: COMMAND_HISTORY_SCHEMA_VERSION,
            entries: vec![CommandHistoryEntry {
                entry_id: Uuid::now_v7(),
                scope: "local".to_owned(),
                command: "safe command".to_owned(),
                completed_at_unix_ms: 1,
                elapsed_millis: 1,
                exit_code: None,
            }],
        };
        let error = vault
            .replace_command_history_using(history, |_| {
                Err(VaultError::Io(std::io::Error::other("injected failure")))
            })
            .expect_err("definite failure");
        assert!(matches!(error, VaultError::Io(_)));
        assert!(
            vault
                .command_history()
                .expect("history remains usable")
                .entries
                .is_empty()
        );
        assert!(!vault.requires_reload());
    }

    #[test]
    fn wraps_a_random_vault_key_and_can_change_the_password() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let secret_ref = {
            let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
            let secret_ref = vault
                .insert_secret(SecretKind::Password, b"server password")
                .expect("insert secret");
            vault
                .change_password(PASSWORD, NEW_PASSWORD)
                .expect("change password");
            secret_ref
        };

        let old_error = UnlockedVault::unlock(&path, PASSWORD).expect_err("old password rejected");
        assert!(matches!(old_error, VaultError::AuthenticationFailed));
        let vault = UnlockedVault::unlock(&path, NEW_PASSWORD).expect("new password works");
        assert_eq!(
            vault
                .read_secret(secret_ref, SecretKind::Password)
                .expect("vault usable")
                .expect("read secret")
                .expose(),
            b"server password"
        );
    }

    #[test]
    fn rejects_an_incorrect_password_without_exposing_a_distinct_crypto_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        drop(UnlockedVault::create(&path, PASSWORD).expect("create vault"));
        let error = UnlockedVault::unlock(&path, b"wrong password").expect_err("must reject");
        assert!(matches!(error, VaultError::AuthenticationFailed));
    }

    #[test]
    fn rejects_password_change_when_the_current_password_is_incorrect() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let revision = vault.revision();
        let error = vault
            .change_password(b"incorrect current password", NEW_PASSWORD)
            .expect_err("must reject password change");
        assert!(matches!(error, VaultError::AuthenticationFailed));
        assert_eq!(vault.revision(), revision);
        drop(vault);
        UnlockedVault::unlock(&path, PASSWORD).expect("old password still works");
    }

    #[test]
    fn rejects_header_tampering() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        drop(UnlockedVault::create(&path, PASSWORD).expect("create vault"));
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read vault")).expect("parse vault");
        value["vaultId"] = serde_json::Value::String(Uuid::new_v4().to_string());
        fs::write(&path, serde_json::to_vec(&value).expect("encode tampering"))
            .expect("write tampering");
        let error = UnlockedVault::unlock(&path, PASSWORD).expect_err("must reject tampering");
        assert!(matches!(error, VaultError::AuthenticationFailed));
    }

    #[test]
    fn holds_an_exclusive_process_lock_while_unlocked() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let error = UnlockedVault::unlock(&path, PASSWORD).expect_err("must be locked");
        assert!(matches!(error, VaultError::Locked));
        drop(vault);
        UnlockedVault::unlock(&path, PASSWORD).expect("lock released");
    }

    #[test]
    fn distinguishes_uncertain_commit_after_replace() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        ensure_private_parent_directory(&path).expect("private directory");
        fs::write(&path, b"old").expect("old file");

        let error = atomic_write_with(&path, b"new", |_| {
            Err(std::io::Error::other("injected parent sync failure"))
        })
        .expect_err("must report uncertain commit");
        assert!(matches!(error, AtomicWriteError::CommitStateUnknown(_)));
        assert_eq!(fs::read(path).expect("read committed file"), b"new");
    }

    #[test]
    fn windows_acl_policy_allows_only_the_user_and_local_system() {
        assert_eq!(
            WINDOWS_PRIVATE_ACL_PRINCIPALS,
            [
                WindowsAclPrincipal::CurrentUser,
                WindowsAclPrincipal::LocalSystem,
            ]
        );
        assert_eq!(windows_acl_inheritance(PrivateObjectKind::File), 0);
        assert_eq!(windows_acl_inheritance(PrivateObjectKind::Directory), 0x3);
    }

    #[cfg(windows)]
    #[test]
    fn creates_persists_and_reopens_with_verified_windows_acls() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let secret_ref = {
            let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create private vault");
            let secret_ref = vault
                .insert_secret(SecretKind::Password, b"server password")
                .expect("persist through private staging file");
            assert!(lock_path(&path).expect("lock path").is_file());
            secret_ref
        };
        let vault = UnlockedVault::unlock(&path, PASSWORD).expect("reopen private vault");
        assert_eq!(
            vault
                .read_secret(secret_ref, SecretKind::Password)
                .expect("matching secret kind")
                .expect("persisted secret")
                .expose(),
            b"server password"
        );
    }

    #[cfg(unix)]
    #[test]
    fn creates_private_directory_and_files_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let vault_directory = path.parent().expect("vault directory");
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        let lock = lock_path(&path).expect("lock path");
        vault
            .insert_secret(SecretKind::Password, b"server password")
            .expect("persist through staging file");
        assert_eq!(
            fs::metadata(vault_directory)
                .expect("vault directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path)
                .expect("vault metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(lock)
                .expect("lock metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            fs::read_dir(vault_directory)
                .expect("vault directory entries")
                .all(|entry| !entry
                    .expect("vault directory entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".staging"))
        );
        drop(vault);
    }

    #[test]
    fn rejects_a_weak_new_password() {
        let directory = tempfile::tempdir().expect("tempdir");
        let error = UnlockedVault::create(vault_path(&directory), b"short")
            .expect_err("weak password rejected");
        assert!(matches!(error, VaultError::WeakPassword));
    }

    #[test]
    fn sync_key_envelope_reuses_the_vault_password_without_exporting_payload() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = vault_path(&directory);
        let secret = b"server password must stay out of the sync key envelope";
        let mut vault = UnlockedVault::create(&path, PASSWORD).expect("create vault");
        vault
            .insert_secret(SecretKind::Password, secret)
            .expect("insert secret");

        let exported = vault
            .export_sync_key_material()
            .expect("export sync material");
        assert_eq!(
            sync_key_envelope_vault_id(exported.envelope()).expect("inspect envelope"),
            vault.vault_id()
        );
        assert!(
            !exported
                .envelope()
                .windows(secret.len())
                .any(|window| window == secret)
        );

        let recovered = open_sync_key_material(exported.envelope(), PASSWORD)
            .expect("same Vault password opens sync key");
        assert_eq!(recovered.vault_id(), exported.vault_id());
        assert_eq!(recovered.key_bytes(), exported.key_bytes());
        assert!(matches!(
            open_sync_key_material(exported.envelope(), NEW_PASSWORD),
            Err(VaultError::AuthenticationFailed)
        ));
    }
}
