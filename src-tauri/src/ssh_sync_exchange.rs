//! Generic, plugin-scoped SSH sync data exchange broker.
//!
//! Plugins own provider login configuration, HTTP(S) endpoints and their
//! declarative UI. Core owns secure selection, portable serialization,
//! end-to-end encryption, durable baselines, revision/strong-ETag CAS,
//! conflict detection and approved local apply. No provider ID or origin is
//! built into this module.

pub(crate) mod data_exchange;
mod difference;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    future::Future,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use hmac::{Hmac, Mac as _};
use norishell_core_api::{
    PluginApiErrorCode, PluginSshSyncAccountState, PluginSshSyncBrowserSnapshot,
    PluginSshSyncConflictPolicy, PluginSshSyncCredentialProfile, PluginSshSyncDeleteTarget,
    PluginSshSyncDifferenceState, PluginSshSyncDownloadSource, PluginSshSyncHttpMethod,
    PluginSshSyncOperationState, PluginSshSyncRequest, PluginSshSyncScopeMode,
    PluginSshSyncStableErrorCode, PluginSshSyncStatus, PluginSshSyncUploadTarget, PluginUiFieldId,
    PluginUiFieldValue, SecretRefId, WireSequence,
};
use norishell_ssh_profile_sync::{
    BundleConflictResolution, BundleMergeOutcome, BundleSchema, PluginExchangeBinding,
    PortableBundleV1, PortableObjects, SyncKey, canonical_bundle_bytes,
    create_plugin_exchange_with_key, inspect_plugin_exchange_data_owner,
    inspect_plugin_exchange_owner, merge_bundles_three_way_with_policies,
    open_plugin_exchange_with_key, plugin_exchange_vault_key_envelope, stable_plugin_data_owner,
};
use reqwest::{
    Client, StatusCode, Url,
    header::{CONTENT_TYPE, ETAG, IF_MATCH},
    redirect::Policy,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::plugin_credential_service::CredentialLease;
use crate::plugin_oauth::{
    NativeAccountState, NativeAuthError, PluginOAuthConfiguration, PluginOAuthService,
    delete_plugin_oauth_data,
};
use crate::ssh_sync_browser_cache::{
    SshSyncBrowserCache, SshSyncBrowserCacheBinding, empty_snapshot,
};
use crate::ssh_sync_browser_store::{self, BrowserDiskCacheBinding};
use crate::vault_service::SyncKeyRecoveryError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub(crate) type ActionFence = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshSyncInvocationBinding {
    pub(crate) package_sha256: String,
    pub(crate) instance_generation: WireSequence,
    pub(crate) authorization_epoch: WireSequence,
}

const MAX_DOWNLOAD_BYTES: usize = 96 * 1024 * 1024;
const MAX_ACK_BYTES: usize = 16 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const EXCHANGE_CONTENT_TYPE: &str = "application/vnd.norishell.ssh-sync-exchange+json;version=1";
const MAX_EXCHANGE_REVISION: u64 = 9_007_199_254_740_991;
type HttpExchangeData = (u16, Option<String>, Option<u64>, Option<i64>, Vec<u8>);

struct HttpExchangeHead {
    data: Option<HttpExchangeData>,
    next_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecureSelectionError {
    InteractionRequired,
    Cancelled,
    Unavailable,
}

#[derive(Clone)]
pub(crate) struct SecureActionContext {
    pub(crate) background: bool,
    pub(crate) plugin_id: String,
    pub(crate) data_owner_sha256: String,
    pub(crate) profile_id: String,
    pub(crate) remote_origin: Option<String>,
    pub(crate) fence: ActionFence,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SecureBackupSelection {
    token: String,
    plugin_id: String,
    signer_fingerprint_sha256: String,
    profile_id: String,
}

impl SecureBackupSelection {
    pub(crate) fn new(
        token: impl Into<String>,
        plugin_id: impl Into<String>,
        signer_fingerprint_sha256: impl Into<String>,
        profile_id: impl Into<String>,
    ) -> Self {
        Self {
            token: token.into(),
            plugin_id: plugin_id.into(),
            signer_fingerprint_sha256: signer_fingerprint_sha256.into(),
            profile_id: profile_id.into(),
        }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn owner(&self) -> (&str, &str, &str) {
        (
            &self.plugin_id,
            &self.signer_fingerprint_sha256,
            &self.profile_id,
        )
    }
}

impl std::fmt::Debug for SecureBackupSelection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecureBackupSelection([OPAQUE])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SecureApplyApproval {
    token: String,
    plugin_id: String,
    signer_fingerprint_sha256: String,
    profile_id: String,
}

impl SecureApplyApproval {
    pub(crate) fn new(
        token: impl Into<String>,
        plugin_id: impl Into<String>,
        signer_fingerprint_sha256: impl Into<String>,
        profile_id: impl Into<String>,
    ) -> Self {
        Self {
            token: token.into(),
            plugin_id: plugin_id.into(),
            signer_fingerprint_sha256: signer_fingerprint_sha256.into(),
            profile_id: profile_id.into(),
        }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl std::fmt::Debug for SecureApplyApproval {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecureApplyApproval([OPAQUE])")
    }
}

pub(crate) struct SyncDirectionReview {
    /// True only when the secure decision approves the displayed item-level merge plan.
    pub(crate) merge_plan: bool,
    pub(crate) local_desktop_profile_count: u32,
    pub(crate) remote_desktop_profile_count: u32,
    pub(crate) restore_handle: PendingRestoreHandle,
    pub(crate) local_host_count: u32,
    pub(crate) local_credential_count: u32,
    pub(crate) remote_host_count: u32,
    pub(crate) remote_credential_count: u32,
    pub(crate) conflict_count: u32,
    pub(crate) delete_count: u32,
    pub(crate) local_compared_at_unix_ms: i64,
    pub(crate) remote_updated_at_unix_ms: Option<i64>,
    pub(crate) differences: Vec<norishell_core_api::SshSyncSecureDifference>,
    pub(crate) difference_total_count: u32,
    pub(crate) difference_omitted_count: u32,
}

pub(crate) struct SyncConflictReview {
    pub(crate) local_desktop_profile_count: u32,
    pub(crate) remote_desktop_profile_count: u32,
    pub(crate) local_host_count: u32,
    pub(crate) local_credential_count: u32,
    pub(crate) remote_host_count: u32,
    pub(crate) remote_credential_count: u32,
    pub(crate) conflict_count: u32,
    pub(crate) local_compared_at_unix_ms: i64,
    pub(crate) remote_updated_at_unix_ms: Option<i64>,
    pub(crate) differences: Vec<norishell_core_api::SshSyncSecureDifference>,
    pub(crate) difference_total_count: u32,
    pub(crate) difference_omitted_count: u32,
}

pub(crate) trait SecureSshSyncUi: Send + Sync {
    fn create_vault(
        &self,
        context: SecureActionContext,
    ) -> BoxFuture<'_, Result<(), SecureSelectionError>>;

    fn select_backup(
        &self,
        context: SecureActionContext,
    ) -> BoxFuture<'_, Result<SecureBackupSelection, SecureSelectionError>>;
    fn vault_password(
        &self,
        context: SecureActionContext,
    ) -> BoxFuture<'_, Result<Zeroizing<Vec<u8>>, SecureSelectionError>>;
    fn choose_sync_direction(
        &self,
        context: SecureActionContext,
        review: SyncDirectionReview,
    ) -> BoxFuture<'_, Result<SecureSyncDirection, SecureSelectionError>>;
    fn approve_data_apply(
        &self,
        context: SecureActionContext,
        review: SyncDirectionReview,
        uploaded: bool,
    ) -> BoxFuture<'_, Result<SecureApplyApproval, SecureSelectionError>>;
    fn choose_conflict_side(
        &self,
        context: SecureActionContext,
        review: SyncConflictReview,
    ) -> BoxFuture<'_, Result<BundleConflictResolution, SecureSelectionError>>;
    fn approve_remote_reset(
        &self,
        context: SecureActionContext,
    ) -> BoxFuture<'_, Result<(), SecureSelectionError>>;
}

pub(crate) enum SecureSyncDirection {
    LocalOverRemote,
    RemoteOverLocal(SecureApplyApproval),
    ApplyMerged(SecureApplyApproval),
}

/// Core issues this only after verifying the three-way merge, policy, and
/// fresh local/remote snapshots. The store checks the exact staged payload
/// again before an unattended local apply.
pub(crate) struct VerifiedAutomaticMergeApproval {
    staged_bundle_sha256: String,
}

impl VerifiedAutomaticMergeApproval {
    fn new(bundle: &PortableBundleV1) -> Result<Self, BrokerError> {
        let bytes = canonical_bundle_bytes(bundle).map_err(|_| BrokerError::RemoteDataInvalid)?;
        Ok(Self {
            staged_bundle_sha256: sha256_hex(&bytes),
        })
    }

    pub(crate) fn matches(&self, bundle: &PortableBundleV1) -> bool {
        bundle.schema == BundleSchema::V6
            && canonical_bundle_bytes(bundle)
                .map(|bytes| sha256_hex(&bytes) == self.staged_bundle_sha256)
                .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortableStoreError {
    InvalidSelection,
    Stale,
    Rejected(&'static str),
    InvalidBundle(&'static str, &'static str),
    Internal(&'static str),
    Persistence(&'static str, &'static str, Option<i32>),
}

pub(crate) struct PortableSnapshot {
    pub(crate) desktop_profile_count: u32,
    pub(crate) bundle: PortableBundleV1,
    pub(crate) host_count: u32,
    pub(crate) credential_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortableProfileKeyBinding {
    pub(crate) secret_ref_id: SecretRefId,
    pub(crate) password_wrapped_envelope: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortableRemoteBaseline {
    pub(crate) revision: u64,
    pub(crate) etag: String,
    pub(crate) content_sha256: String,
    pub(crate) exchange_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortableProfileState {
    pub(crate) scope_mode: PluginSshSyncScopeMode,
    pub(crate) key_binding: Option<PortableProfileKeyBinding>,
    pub(crate) remote_baseline: Option<PortableRemoteBaseline>,
    pub(crate) state_version: WireSequence,
    pub(crate) last_successful_sync_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortableUploadAttemptState {
    Prepared,
    Sent,
    Verifying,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PortableUploadAttempt {
    pub(crate) canonical_url: String,
    pub(crate) method: PluginSshSyncHttpMethod,
    pub(crate) use_oauth: bool,
    pub(crate) authorization_revision: WireSequence,
    pub(crate) configuration_revision: WireSequence,
    pub(crate) base_revision: u64,
    pub(crate) base_etag: Option<String>,
    pub(crate) target_revision: u64,
    pub(crate) keyed_content_sha256: String,
    pub(crate) body_sha256: String,
    pub(crate) idempotency_key: String,
    pub(crate) state: PortableUploadAttemptState,
    pub(crate) state_version: WireSequence,
}

impl std::fmt::Debug for PortableUploadAttempt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PortableUploadAttempt")
            .field("canonical_url", &self.canonical_url)
            .field("method", &self.method)
            .field("use_oauth", &self.use_oauth)
            .field("authorization_revision", &self.authorization_revision)
            .field("configuration_revision", &self.configuration_revision)
            .field("base_revision", &self.base_revision)
            .field("base_etag", &self.base_etag)
            .field("target_revision", &self.target_revision)
            .field("keyed_content_sha256", &"[REDACTED]")
            .field("body_sha256", &"[REDACTED]")
            .field("idempotency_key", &"[REDACTED]")
            .field("state", &self.state)
            .field("state_version", &self.state_version)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SshSyncActionRevision {
    pub(crate) authorization: WireSequence,
    pub(crate) configuration: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableUploadFence {
    pub(crate) canonical_url: String,
    pub(crate) method: PluginSshSyncHttpMethod,
    pub(crate) use_oauth: bool,
    pub(crate) action_revision: SshSyncActionRevision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PortableUploadCompletionProof {
    ServerAcknowledged {
        acknowledged_revision: u64,
    },
    ExactBodyObserved {
        observed_revision: u64,
    },
    SupersededByNewerRemote {
        authenticated_remote_revision: u64,
    },
    ConflictingBodyObservedAtTargetRevision {
        authenticated_remote_revision: u64,
        authenticated_remote_body_sha256: String,
        authenticated_remote_etag: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PortableUploadAbandonProof {
    PreparedNotSent,
    AuthenticatedRemoteAtBase {
        remote_revision: u64,
        remote_etag: Option<String>,
        remote_body_sha256: Option<String>,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PendingRestoreHandle(String);

impl PendingRestoreHandle {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn token(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PendingRestoreHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PendingRestoreHandle([OPAQUE])")
    }
}

pub(crate) struct RestorePreview {
    pub(crate) desktop_profile_count: u32,
    pub(crate) handle: PendingRestoreHandle,
    pub(crate) host_count: u32,
    pub(crate) credential_count: u32,
    pub(crate) conflict_count: u32,
    pub(crate) delete_count: u32,
}

// Keep staged plaintext cleanup tied to ownership, including cancellation that drops the future.
struct StagedRestoreCleanup {
    profiles: Arc<dyn PortableSshProfileStore>,
    handle: PendingRestoreHandle,
}

impl Drop for StagedRestoreCleanup {
    fn drop(&mut self) {
        self.profiles.discard_staged_restore(&self.handle);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplyResult {
    pub(crate) desktop_profile_count: u32,
    pub(crate) host_count: u32,
    pub(crate) credential_count: u32,
    pub(crate) conflict_count: u32,
}

pub(crate) trait PortableSshProfileStore: Send + Sync {
    /// Release uncommitted staged plaintext on cancellation, upload selection, or validation failure.
    fn discard_staged_restore(&self, handle: &PendingRestoreHandle);
    /// Drops every short-lived selection, approval and decrypted restore owned by a plugin.
    /// Lifecycle callers invoke this only after they have serialized against in-flight broker
    /// operations, so an old generation cannot recreate state after the clear completes.
    fn clear_plugin(&self, plugin_id: &str);
    /// Unlocks the local Vault from a Core-owned secure prompt and runs the
    /// durable SSH-sync reconciliation required before the caller resumes.
    fn unlock_vault_for_operation(
        &self,
        password: Zeroizing<Vec<u8>>,
    ) -> BoxFuture<'_, Result<(), PortableStoreError>>;
    fn delete_plugin_data(
        &self,
        plugin_id: String,
    ) -> BoxFuture<'_, Result<(), PortableStoreError>>;
    fn profile_state(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
    ) -> BoxFuture<'_, Result<PortableProfileState, PortableStoreError>>;
    fn existing_profile_state(
        &self,
        plugin_id: String,
        data_owner_sha256: String,
        profile_id: String,
    ) -> BoxFuture<'_, Result<Option<PortableProfileState>, PortableStoreError>>;
    fn existing_data_owners(
        &self,
        plugin_id: String,
        profile_id: String,
    ) -> BoxFuture<'_, Result<Vec<String>, PortableStoreError>>;
    fn migrate_provisional_scope_owner(
        &self,
        plugin_id: String,
        source_owner_sha256: String,
        target_owner_sha256: String,
        profile_id: String,
        expected_state_version: WireSequence,
    ) -> BoxFuture<'_, Result<(), PortableStoreError>>;
    fn local_counts(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
    ) -> BoxFuture<'_, Result<(u32, u32, u32), PortableStoreError>>;
    fn snapshot_current(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        revision: u64,
    ) -> BoxFuture<'_, Result<PortableSnapshot, PortableStoreError>>;
    fn prepare_local_merge(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        base: PortableBundleV1,
        current: PortableBundleV1,
    ) -> BoxFuture<'_, Result<PortableBundleV1, PortableStoreError>>;
    fn configure_scope(
        &self,
        signer_fingerprint_sha256: String,
        selection: SecureBackupSelection,
    ) -> BoxFuture<'_, Result<PortableSnapshot, PortableStoreError>>;
    #[allow(clippy::too_many_arguments)]
    fn bind_sync_key(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: WireSequence,
        secret_ref_id: SecretRefId,
        key: SyncKey,
        password_wrapped_envelope: Vec<u8>,
    ) -> BoxFuture<'_, Result<PortableProfileState, PortableStoreError>>;
    fn update_remote_baseline(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: WireSequence,
        baseline: PortableRemoteBaseline,
    ) -> BoxFuture<'_, Result<PortableProfileState, PortableStoreError>>;
    /// Clears only the remote baseline and any superseded uncertain upload.
    /// Local scope, object mappings and the Vault-backed sync key remain.
    fn reset_remote_state(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: WireSequence,
        expected_upload_fence: DurableUploadFence,
    ) -> BoxFuture<'_, Result<PortableProfileState, PortableStoreError>>;
    fn mark_sync_succeeded(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: WireSequence,
    ) -> BoxFuture<'_, Result<PortableProfileState, PortableStoreError>>;
    fn pending_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
    ) -> BoxFuture<'_, Result<Option<PortableUploadAttempt>, PortableStoreError>>;
    #[allow(clippy::too_many_arguments)]
    fn ensure_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        canonical_url: String,
        method: PluginSshSyncHttpMethod,
        use_oauth: bool,
        action_revision: SshSyncActionRevision,
        base_revision: u64,
        base_etag: Option<String>,
        target_revision: u64,
        keyed_content_sha256: String,
        body_sha256: String,
        proposed_idempotency_key: String,
    ) -> BoxFuture<'_, Result<PortableUploadAttempt, PortableStoreError>>;
    fn advance_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: WireSequence,
        state: PortableUploadAttemptState,
    ) -> BoxFuture<'_, Result<PortableUploadAttempt, PortableStoreError>>;
    #[allow(clippy::too_many_arguments)]
    fn complete_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        idempotency_key: String,
        body_sha256: String,
        expected_fence: DurableUploadFence,
        proof: PortableUploadCompletionProof,
    ) -> BoxFuture<'_, Result<(), PortableStoreError>>;
    #[allow(clippy::too_many_arguments)]
    fn abandon_legacy_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        idempotency_key: String,
        body_sha256: String,
        expected_state_version: WireSequence,
        expected_fence: DurableUploadFence,
        proof: PortableUploadAbandonProof,
    ) -> BoxFuture<'_, Result<(), PortableStoreError>>;
    fn stage_restore(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        bundle: PortableBundleV1,
        key: SyncKey,
    ) -> BoxFuture<'_, Result<RestorePreview, PortableStoreError>>;
    fn apply_staged_restore(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        handle: PendingRestoreHandle,
        approval: Option<SecureApplyApproval>,
    ) -> BoxFuture<'_, Result<ApplyResult, PortableStoreError>>;
    fn apply_verified_merge(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        handle: PendingRestoreHandle,
        approval: VerifiedAutomaticMergeApproval,
        fence: ActionFence,
    ) -> BoxFuture<'_, Result<ApplyResult, PortableStoreError>>;
}

struct DownloadedExchange {
    http_status: u16,
    remote_origin: String,
    etag: String,
    remote_updated_at_unix_ms: Option<i64>,
    binding: PluginExchangeBinding,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct BrokerState {
    oauth: BTreeMap<String, PluginOAuthService>,
    account_epochs: BTreeMap<String, u64>,
    browser_sources: BTreeMap<String, String>,
    last: BTreeMap<String, PluginSshSyncStatus>,
}

#[derive(Default)]
struct BrokerOperations {
    locks: BTreeMap<String, Arc<AsyncMutex<()>>>,
    epochs: BTreeMap<String, u64>,
    stopping: BTreeSet<String>,
}

impl BrokerOperations {
    fn register(&mut self, plugin_id: &str, namespace: &str) -> Option<(Arc<AsyncMutex<()>>, u64)> {
        if self.stopping.contains(plugin_id) {
            return None;
        }
        let epoch = *self.epochs.entry(plugin_id.to_owned()).or_default();
        let operation = self
            .locks
            .entry(namespace.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        Some((operation, epoch))
    }

    fn begin_stop(&mut self, plugin_id: &str) -> Vec<Arc<AsyncMutex<()>>> {
        self.stopping.insert(plugin_id.to_owned());
        let epoch = self.epochs.entry(plugin_id.to_owned()).or_default();
        *epoch = epoch.saturating_add(1);
        let prefix = format!("{plugin_id}\0");
        self.locks
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(_, lock)| Arc::clone(lock))
            .collect()
    }

    fn finish_stop(&mut self, plugin_id: &str) {
        self.stopping.remove(plugin_id);
    }

    fn epoch_current(&self, plugin_id: &str, expected: u64) -> bool {
        !self.stopping.contains(plugin_id)
            && self.epochs.get(plugin_id).copied().unwrap_or_default() == expected
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum BrokerError {
    VaultMissing,
    VaultLocked,
    InteractionRequired,
    Cancelled,
    AuthorizationDenied,
    AuthorizationExpired,
    AccountNotConnected,
    NetworkUnavailable,
    StateConflict,
    LocalStateChanged,
    RetryLocalSnapshot,
    OwnerConflict,
    KeyBindingConflict,
    RevisionExhausted,
    RestoreConflict,
    MergeInvalid(&'static str),
    HttpFailure(u16),
    RemoteDataInvalid,
    RemoteFormatUnsupported,
    RecoveryRemoteKeyAuthenticationFailed,
    RecoveryActionExpired,
    OperationRejected(&'static str),
    LocalDataInvalid(&'static str, &'static str),
    LocalKeyUnavailable,
    OperationBusy,
    Internal(&'static str),
    Persistence(&'static str, &'static str, Option<i32>),
}

#[derive(Clone)]
pub(crate) struct SshSyncExchangeBroker {
    background: bool,
    package_signer_sha256: String,
    data_owner_sha256: String,
    authenticated_remote_key: Option<(String, SyncKey)>,
    app_data_directory: Arc<std::path::PathBuf>,
    vault: crate::vault_service::VaultService,
    secure_ui: Arc<dyn SecureSshSyncUi>,
    profiles: Arc<dyn PortableSshProfileStore>,
    http: Client,
    operations: Arc<Mutex<BrokerOperations>>,
    state: Arc<Mutex<BrokerState>>,
    browser_cache: SshSyncBrowserCache,
}

impl SshSyncExchangeBroker {
    pub(crate) fn new(
        app_data_directory: impl AsRef<std::path::Path>,
        vault: crate::vault_service::VaultService,
        secure_ui: Arc<dyn SecureSshSyncUi>,
        profiles: Arc<dyn PortableSshProfileStore>,
    ) -> Self {
        Self {
            background: false,
            package_signer_sha256: String::new(),
            data_owner_sha256: String::new(),
            authenticated_remote_key: None,
            app_data_directory: Arc::new(app_data_directory.as_ref().to_path_buf()),
            vault,
            secure_ui,
            profiles,
            http: Client::builder()
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(HTTP_TIMEOUT)
                .user_agent(concat!("NoriShell/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("generic SSH sync HTTP policy must be valid"),
            operations: Arc::new(Mutex::new(BrokerOperations::default())),
            state: Arc::new(Mutex::new(BrokerState::default())),
            browser_cache: SshSyncBrowserCache::default(),
        }
    }

    /// Disallow secure prompts for plugin lifecycle and other implicit calls.
    pub(crate) fn for_background(mut self) -> Self {
        self.background = true;
        self
    }

    pub(crate) fn browser_snapshot(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        invocation: &SshSyncInvocationBinding,
        fence: &dyn Fn() -> bool,
    ) -> PluginSshSyncBrowserSnapshot {
        if !self.vault.is_unlocked() {
            self.browser_cache.invalidate_profile(plugin_id, profile_id);
            return empty_snapshot(
                if self.vault.status().state == norishell_core_api::VaultState::Missing {
                    norishell_core_api::PluginSshSyncBrowserState::NeedsCreation
                } else {
                    norishell_core_api::PluginSshSyncBrowserState::NeedsUnlock
                },
                profile_id.to_owned(),
                WireSequence::new(0),
            );
        }
        let Some(binding) =
            self.browser_binding(plugin_id, signer_fingerprint_sha256, profile_id, invocation)
        else {
            return empty_snapshot(
                norishell_core_api::PluginSshSyncBrowserState::NotLoaded,
                profile_id.to_owned(),
                WireSequence::new(0),
            );
        };
        let guarded_fence =
            || fence() && browser_binding_current(&self.vault, &self.state, &binding);
        if !guarded_fence() {
            self.browser_cache.invalidate_profile(plugin_id, profile_id);
            return empty_snapshot(
                if self.vault.is_unlocked() {
                    norishell_core_api::PluginSshSyncBrowserState::NotLoaded
                } else if self.vault.status().state == norishell_core_api::VaultState::Missing {
                    norishell_core_api::PluginSshSyncBrowserState::NeedsCreation
                } else {
                    norishell_core_api::PluginSshSyncBrowserState::NeedsUnlock
                },
                profile_id.to_owned(),
                WireSequence::new(0),
            );
        }
        self.browser_cache.read(&binding, &guarded_fence)
    }

    pub(crate) fn invalidate_browser_profile(&self, plugin_id: &str, profile_id: &str) {
        self.browser_cache.invalidate_profile(plugin_id, profile_id);
    }

    pub(crate) fn invalidate_browser_plugin(&self, plugin_id: &str) {
        self.browser_cache.invalidate_plugin(plugin_id);
    }

    pub(crate) fn invalidate_all_browser_snapshots(&self) {
        self.browser_cache.invalidate_all();
    }

    fn browser_binding(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        invocation: &SshSyncInvocationBinding,
    ) -> Option<SshSyncBrowserCacheBinding> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let namespace = namespace(plugin_id, signer_fingerprint_sha256, profile_id);
        let oauth = state.oauth.get(&namespace)?.clone();
        let account_configuration_sha256 = oauth.configuration_digest();
        let account_epoch = state
            .account_epochs
            .get(&namespace)
            .copied()
            .unwrap_or_default();
        let source_url = state.browser_sources.get(&namespace).cloned();
        drop(state);
        Some(SshSyncBrowserCacheBinding {
            plugin_id: plugin_id.to_owned(),
            signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
            profile_id: profile_id.to_owned(),
            package_sha256: invocation.package_sha256.clone(),
            instance_generation: invocation.instance_generation,
            authorization_epoch: invocation.authorization_epoch,
            account_epoch,
            account_configuration_sha256,
            oauth_session_id: oauth.session_id(),
            source_url,
        })
    }

    fn publish_browser_snapshot(
        &self,
        binding: &SshSyncBrowserCacheBinding,
        bundle: &PortableBundleV1,
        remote_updated_at_unix_ms: Option<i64>,
        fence: &ActionFence,
    ) {
        let Some(binding) = self.current_browser_binding(binding) else {
            return;
        };
        let guarded_fence =
            || fence() && browser_binding_current(&self.vault, &self.state, &binding);
        self.browser_cache
            .publish(&binding, bundle, remote_updated_at_unix_ms, &guarded_fence);
    }

    fn current_browser_binding(
        &self,
        binding: &SshSyncBrowserCacheBinding,
    ) -> Option<SshSyncBrowserCacheBinding> {
        if browser_binding_current(&self.vault, &self.state, binding) {
            return Some(binding.clone());
        }
        // A legacy refresh-token record acquires its first persistent session
        // ID only after a successful refresh in this serialized operation.
        if binding.oauth_session_id.is_some() {
            return None;
        }
        let namespace = namespace(
            &binding.plugin_id,
            &binding.signer_fingerprint_sha256,
            &binding.profile_id,
        );
        let oauth = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .get(&namespace)
            .cloned()?;
        let mut upgraded = binding.clone();
        upgraded.oauth_session_id = Some(oauth.session_id()?);
        browser_binding_current(&self.vault, &self.state, &upgraded).then_some(upgraded)
    }

    fn mark_browser_failed(&self, binding: &SshSyncBrowserCacheBinding, fence: &ActionFence) {
        let guarded_fence =
            || fence() && browser_binding_current(&self.vault, &self.state, binding);
        self.browser_cache.mark_failed(binding, &guarded_fence);
    }

    fn retain_verified_remote_counts(
        &self,
        status: &mut PluginSshSyncStatus,
        binding: &SshSyncBrowserCacheBinding,
        fence: &ActionFence,
    ) {
        let guarded_fence =
            || fence() && browser_binding_current(&self.vault, &self.state, binding);
        let snapshot = self.browser_cache.read(binding, &guarded_fence);
        apply_verified_stale_counts(status, &snapshot);
    }

    fn browser_disk_binding(
        &self,
        binding: &SshSyncBrowserCacheBinding,
    ) -> Option<BrowserDiskCacheBinding> {
        let source_url = binding.source_url.as_ref()?.clone();
        let namespace = namespace(
            &binding.plugin_id,
            &binding.signer_fingerprint_sha256,
            &binding.profile_id,
        );
        let oauth = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .get(&namespace)
            .cloned()?;
        if oauth.configuration_digest() != binding.account_configuration_sha256 {
            return None;
        }
        let session_id = oauth.session_id()?;
        if binding.oauth_session_id.as_deref() != Some(session_id.as_str()) {
            return None;
        }
        Some(BrowserDiskCacheBinding {
            plugin_id: binding.plugin_id.clone(),
            signer_sha256: binding.signer_fingerprint_sha256.clone(),
            profile_id: binding.profile_id.clone(),
            oauth_configuration_sha256: binding.account_configuration_sha256.clone(),
            oauth_session_id: session_id,
            canonical_source_url: source_url,
        })
    }

    fn browser_cache_mac_key(&self) -> Option<Zeroizing<[u8; 32]>> {
        let material = self.vault.export_sync_key_material().ok()?;
        let mut mac = Hmac::<Sha256>::new_from_slice(material.key_bytes()).ok()?;
        mac.update(b"NoriShell/ssh-sync-browser-cache/mac-key/v1\0");
        let tag = mac.finalize().into_bytes();
        Some(Zeroizing::new(tag.as_slice().try_into().ok()?))
    }

    async fn remember_browser_exchange(
        &self,
        binding: Option<&SshSyncBrowserCacheBinding>,
        ciphertext: Option<&[u8]>,
        remote_updated_at_unix_ms: Option<i64>,
        fence: &ActionFence,
    ) {
        let Some(binding) = binding else { return };
        if !fence() {
            return;
        }
        let Some(binding) = self.current_browser_binding(binding) else {
            return;
        };
        let Some(disk_binding) = self.browser_disk_binding(&binding) else {
            return;
        };
        let Some(mac_key) = self.browser_cache_mac_key() else {
            return;
        };
        let root = self.app_data_directory.as_ref().clone();
        let bytes = ciphertext.map(<[u8]>::to_vec);
        let saved = tokio::task::spawn_blocking(move || {
            ssh_sync_browser_store::save(
                &root,
                &disk_binding,
                bytes.as_deref(),
                remote_updated_at_unix_ms,
                &mac_key,
            )
        })
        .await;
        match saved {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("verified SSH sync browser cache save failed: {error}"),
            Err(error) => eprintln!("verified SSH sync browser cache worker failed: {error}"),
        }
    }

    async fn restore_browser_exchange(
        &self,
        binding: &SshSyncBrowserCacheBinding,
        fence: &ActionFence,
    ) {
        if !fence() || !browser_binding_current(&self.vault, &self.state, binding) {
            return;
        }
        if matches!(
            self.browser_cache.read(binding, &|| fence()).state,
            norishell_core_api::PluginSshSyncBrowserState::Ready
                | norishell_core_api::PluginSshSyncBrowserState::Empty
                | norishell_core_api::PluginSshSyncBrowserState::Stale
        ) {
            return;
        }
        let Some(disk_binding) = self.browser_disk_binding(binding) else {
            return;
        };
        let Some(mac_key) = self.browser_cache_mac_key() else {
            return;
        };
        let root = self.app_data_directory.as_ref().clone();
        let loaded = tokio::task::spawn_blocking(move || {
            ssh_sync_browser_store::load(&root, &disk_binding, &mac_key)
        })
        .await;
        let Ok(Ok(Some(snapshot))) = loaded else {
            return;
        };
        let bundle = if let Some(bytes) = snapshot.ciphertext {
            let Some((exchange_binding, envelope)) =
                inspect_cached_browser_exchange(&bytes, &binding.plugin_id, &binding.profile_id)
            else {
                return;
            };
            let Ok(Some(profile)) = self
                .profiles
                .existing_profile_state(
                    binding.plugin_id.clone(),
                    exchange_binding.signer_fingerprint_sha256.clone(),
                    binding.profile_id.clone(),
                )
                .await
            else {
                return;
            };
            let Some(key_binding) = profile.key_binding else {
                return;
            };
            if key_binding.password_wrapped_envelope != envelope {
                return;
            }
            let Ok(value) = self.vault.read_secret(
                &key_binding.secret_ref_id,
                norishell_secret_vault::SecretKind::SshSyncKey,
            ) else {
                return;
            };
            let Ok(key_bytes) = <[u8; 32]>::try_from(value.expose()) else {
                return;
            };
            let key = SyncKey::from_bytes(key_bytes);
            let Ok(bundle) = open_plugin_exchange_with_key(&bytes, &key, &exchange_binding) else {
                return;
            };
            bundle
        } else {
            empty_portable_bundle(1)
        };
        let guarded_fence =
            || fence() && browser_binding_current(&self.vault, &self.state, binding);
        self.browser_cache.restore_stale(
            binding,
            &bundle,
            snapshot.remote_updated_at_unix_ms,
            snapshot.verified_at_unix_ms,
            &guarded_fence,
        );
    }

    pub(crate) async fn stop_plugin(&self, plugin_id: &str) {
        let operation_locks = {
            let mut operations = self
                .operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            operations.begin_stop(plugin_id)
        };
        let mut operation_guards = Vec::with_capacity(operation_locks.len());
        for operation in operation_locks {
            operation_guards.push(operation.lock_owned().await);
        }
        {
            let prefix = format!("{plugin_id}\0");
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (_, service) in state
                .oauth
                .iter()
                .filter(|(key, _)| key.starts_with(&prefix))
            {
                service.clear_runtime();
            }
            state.oauth.retain(|key, _| !key.starts_with(&prefix));
            state
                .account_epochs
                .retain(|key, _| !key.starts_with(&prefix));
            state
                .browser_sources
                .retain(|key, _| !key.starts_with(&prefix));
            state.last.retain(|key, _| !key.starts_with(&prefix));
        }
        self.browser_cache.invalidate_plugin(plugin_id);
        self.profiles.clear_plugin(plugin_id);
        self.operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish_stop(plugin_id);
        drop(operation_guards);
    }

    pub(crate) async fn delete_plugin_data(&self, plugin_id: &str) -> Result<(), NativeAuthError> {
        self.stop_plugin(plugin_id).await;
        let cache_root = self.app_data_directory.as_ref().clone();
        let cache_plugin_id = plugin_id.to_owned();
        tokio::task::spawn_blocking(move || {
            ssh_sync_browser_store::remove_plugin(&cache_root, &cache_plugin_id)
        })
        .await
        .map_err(|_| NativeAuthError::LocalCommit)?
        .map_err(|_| NativeAuthError::LocalCommit)?;
        self.profiles
            .delete_plugin_data(plugin_id.to_owned())
            .await
            .map_err(|_| NativeAuthError::LocalCommit)?;
        let baseline_root = self.app_data_directory.as_ref().clone();
        let plugin_id_for_cleanup = plugin_id.to_owned();
        tokio::task::spawn_blocking(move || {
            remove_plugin_baseline_files(&baseline_root, &plugin_id_for_cleanup)
        })
        .await
        .map_err(|_| NativeAuthError::LocalCommit)?
        .map_err(|_| NativeAuthError::LocalCommit)?;
        delete_plugin_oauth_data(self.app_data_directory.as_ref(), &self.vault, plugin_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn invoke(
        mut self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        request: PluginSshSyncRequest,
        fields: &[PluginUiFieldValue],
        action_revision: SshSyncActionRevision,
        invocation_binding: SshSyncInvocationBinding,
        background: bool,
        fence: ActionFence,
    ) -> PluginSshSyncStatus {
        self.background = background;
        self.package_signer_sha256 = signer_fingerprint_sha256.to_owned();
        self.data_owner_sha256.clear();
        self.authenticated_remote_key = None;
        let profile_id = request_profile_id(&request).to_owned();
        if !valid_identifier(&profile_id, 160)
            || !valid_identifier(plugin_id, 160)
            || !valid_sha256(signer_fingerprint_sha256)
        {
            return failed_status(
                profile_id,
                BrokerError::OperationRejected("broker.invoke.01"),
            );
        }
        if !fence() {
            return failed_status(
                profile_id,
                BrokerError::OperationRejected("broker.invoke.02"),
            );
        }
        if background
            && !matches!(
                request,
                PluginSshSyncRequest::Status { .. } | PluginSshSyncRequest::Refresh { .. }
            )
        {
            return failed_status(profile_id, BrokerError::InteractionRequired);
        }
        let namespace = namespace(plugin_id, signer_fingerprint_sha256, &profile_id);
        if matches!(request, PluginSshSyncRequest::Status { .. }) {
            if let Some(auth) = request_restore_profile(&request)
                && let Err(error) = self.ensure_credential_service(
                    plugin_id,
                    signer_fingerprint_sha256,
                    &profile_id,
                    auth.clone(),
                )
            {
                return failed_status(profile_id, error);
            }
            if let Some(error) = vault_access_error(self.vault.status().state) {
                let mut status = failed_status(profile_id.clone(), error);
                status.account_state =
                    self.account_state(plugin_id, signer_fingerprint_sha256, &profile_id);
                status.operation_state = PluginSshSyncOperationState::NeedsReview;
                return status;
            }
            return self.status(&namespace, &profile_id);
        }
        let (operation, operation_epoch) = {
            let mut operations = self
                .operations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(operation) = operations.register(plugin_id, &namespace) else {
                return failed_status(
                    profile_id,
                    BrokerError::OperationRejected("broker.invoke.03"),
                );
            };
            operation
        };
        let _operation = operation.lock().await;
        if !fence() || !self.operation_epoch_current(plugin_id, operation_epoch) {
            return failed_status(
                profile_id,
                BrokerError::OperationRejected("broker.invoke.04"),
            );
        }
        // Restore after the namespace operation lock: a queued request must not
        // replace the session with a stale refresh-token pointer before an
        // earlier request finishes rotating that token.
        if let Some(auth) = request_restore_profile(&request)
            && let Err(error) = self.ensure_credential_service(
                plugin_id,
                signer_fingerprint_sha256,
                &profile_id,
                auth.clone(),
            )
        {
            let mut status = failed_status(profile_id, error);
            status.account_state =
                self.account_state(plugin_id, signer_fingerprint_sha256, &status.profile_id);
            self.remember_status(&namespace, &status);
            return status;
        }
        // Background operations must not turn a missing/locked Vault into a prompt.
        if background && let Some(error) = vault_access_error(self.vault.status().state) {
            let mut status = failed_status(profile_id.clone(), error);
            status.account_state =
                self.account_state(plugin_id, signer_fingerprint_sha256, &profile_id);
            status.operation_state = PluginSshSyncOperationState::NeedsReview;
            self.remember_status(&namespace, &status);
            return status;
        }
        let caller_fence = fence;
        let operation_state = Arc::clone(&self.operations);
        let fence_plugin_id = plugin_id.to_owned();
        let operation_fence: ActionFence = Arc::new(move || {
            caller_fence()
                && operation_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .epoch_current(&fence_plugin_id, operation_epoch)
        });
        if !matches!(
            request,
            PluginSshSyncRequest::Authorize { .. } | PluginSshSyncRequest::Status { .. }
        ) {
            if background {
                // Background work must never enter the secure prompt chain,
                // including when the Vault locks after the first state check.
                if let Some(error) = vault_access_error(self.vault.status().state) {
                    let mut status = failed_status(profile_id.clone(), error);
                    status.account_state =
                        self.account_state(plugin_id, signer_fingerprint_sha256, &profile_id);
                    status.operation_state = PluginSshSyncOperationState::NeedsReview;
                    if operation_fence() {
                        self.remember_status(&namespace, &status);
                    }
                    return status;
                }
            } else if let Err(error) = self
                .ensure_vault_unlocked(
                    plugin_id,
                    signer_fingerprint_sha256,
                    &profile_id,
                    &operation_fence,
                )
                .await
            {
                let mut status = failed_status(profile_id.clone(), error);
                status.account_state =
                    self.account_state(plugin_id, signer_fingerprint_sha256, &profile_id);
                if operation_fence() {
                    self.remember_status(&namespace, &status);
                }
                return status;
            }
        }
        let browser_source_url = match &request {
            PluginSshSyncRequest::Refresh { source, .. }
            | PluginSshSyncRequest::Sync { source, .. } => sync_url(&source.url).ok(),
            PluginSshSyncRequest::ResetRemote { target, .. } => sync_url(&target.url).ok(),
            _ => None,
        }
        .map(|url| url.as_str().to_owned());
        if let Some(source_url) = &browser_source_url {
            let changed = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .browser_sources
                .insert(namespace.clone(), source_url.clone())
                .is_some_and(|previous| previous != *source_url);
            if changed {
                self.browser_cache
                    .invalidate_profile(plugin_id, &profile_id);
            }
        }
        let browser_binding = self.browser_binding(
            plugin_id,
            signer_fingerprint_sha256,
            &profile_id,
            &invocation_binding,
        );
        let browser_fetch_operation = matches!(
            &request,
            PluginSshSyncRequest::Refresh { .. }
                | PluginSshSyncRequest::Sync { .. }
                | PluginSshSyncRequest::ResetRemote { .. }
        );
        let invalidate_browser_on_success = matches!(
            &request,
            PluginSshSyncRequest::Authorize { .. }
                | PluginSshSyncRequest::Login { .. }
                | PluginSshSyncRequest::Register { .. }
                | PluginSshSyncRequest::VerifyEmail { .. }
                | PluginSshSyncRequest::CompleteMfa { .. }
                | PluginSshSyncRequest::Logout { .. }
        );
        if let Err(error) = self
            .select_data_owner(plugin_id, &profile_id, &request, &operation_fence)
            .await
        {
            if browser_fetch_operation && let Some(binding) = &browser_binding {
                self.mark_browser_failed(binding, &operation_fence);
                self.restore_browser_exchange(binding, &operation_fence)
                    .await;
            }
            let mut status = failed_status(profile_id, error);
            if browser_fetch_operation && let Some(binding) = &browser_binding {
                self.retain_verified_remote_counts(&mut status, binding, &operation_fence);
            }
            status.account_state = account_state_after_result(
                self.account_state(plugin_id, signer_fingerprint_sha256, &status.profile_id),
                status.stable_error_code,
            );
            if background
                && matches!(
                    status.stable_error_code,
                    Some(
                        PluginSshSyncStableErrorCode::InteractionRequired
                            | PluginSshSyncStableErrorCode::AuthorizationExpired
                    )
                )
            {
                status.operation_state = PluginSshSyncOperationState::NeedsReview;
            }
            if operation_fence() {
                self.remember_status(&namespace, &status);
            }
            return status;
        }
        let data_owner_sha256 = self.data_owner_sha256.clone();
        let result = match request {
            PluginSshSyncRequest::Status { .. } => unreachable!(),
            PluginSshSyncRequest::Authorize { .. } => {
                Err(BrokerError::OperationRejected("broker.invoke.05"))
            }
            PluginSshSyncRequest::Login {
                auth,
                username_field_id,
                password_field_id,
                ..
            } => {
                self.login(
                    plugin_id,
                    signer_fingerprint_sha256,
                    &profile_id,
                    auth,
                    username_field_id,
                    password_field_id,
                    fields,
                    &operation_fence,
                )
                .await
            }
            PluginSshSyncRequest::Register {
                auth,
                username_field_id,
                password_field_id,
                password_confirmation_field_id,
                display_name_field_id,
                ..
            } => {
                self.register(
                    plugin_id,
                    signer_fingerprint_sha256,
                    &profile_id,
                    auth,
                    username_field_id,
                    password_field_id,
                    password_confirmation_field_id,
                    display_name_field_id,
                    fields,
                    &operation_fence,
                )
                .await
            }
            PluginSshSyncRequest::VerifyEmail { code_field_id, .. } => {
                self.complete_direct_auth(
                    &namespace,
                    &profile_id,
                    code_field_id,
                    fields,
                    false,
                    &operation_fence,
                )
                .await
            }
            PluginSshSyncRequest::CompleteMfa { code_field_id, .. } => {
                self.complete_direct_auth(
                    &namespace,
                    &profile_id,
                    code_field_id,
                    fields,
                    true,
                    &operation_fence,
                )
                .await
            }
            PluginSshSyncRequest::Logout { .. } => {
                self.logout(&namespace, &profile_id, &operation_fence).await
            }
            PluginSshSyncRequest::Refresh { source, .. } => {
                self.refresh(
                    plugin_id,
                    &data_owner_sha256,
                    &profile_id,
                    source,
                    action_revision,
                    browser_binding.as_ref(),
                    &operation_fence,
                )
                .await
            }
            PluginSshSyncRequest::Sync {
                source,
                destination,
                conflict_policy,
                deletion_policy,
                ..
            } => {
                self.sync(
                    plugin_id,
                    &data_owner_sha256,
                    &profile_id,
                    source,
                    destination,
                    action_revision,
                    browser_binding.as_ref(),
                    &operation_fence,
                    false,
                    conflict_policy,
                    deletion_policy,
                )
                .await
            }
            PluginSshSyncRequest::ConfigureScope { .. } => {
                self.configure_scope(plugin_id, &data_owner_sha256, &profile_id, &operation_fence)
                    .await
            }
            PluginSshSyncRequest::ResetRemote { target, .. } => {
                self.reset_remote(
                    plugin_id,
                    &data_owner_sha256,
                    &profile_id,
                    target,
                    action_revision,
                    browser_binding.as_ref(),
                    &operation_fence,
                )
                .await
            }
        };
        if result.is_err()
            && browser_fetch_operation
            && let Some(binding) = &browser_binding
        {
            self.mark_browser_failed(binding, &operation_fence);
            self.restore_browser_exchange(binding, &operation_fence)
                .await;
        }
        if invalidate_browser_on_success
            && (result.is_ok()
                || browser_binding.as_ref().is_some_and(|binding| {
                    !browser_binding_current(&self.vault, &self.state, binding)
                }))
        {
            self.browser_cache
                .invalidate_profile(plugin_id, &profile_id);
        }
        let cancelled = matches!(&result, Err(BrokerError::Cancelled));
        let mut status = match result {
            Err(BrokerError::Cancelled) => cancelled_status(
                profile_id.clone(),
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .last
                    .get(&namespace)
                    .cloned(),
            ),
            result => result.unwrap_or_else(|error| {
                eprintln!("plugin SSH sync operation failed: {error:?}");
                failed_status(profile_id.clone(), error)
            }),
        };
        if status.stable_error_code.is_some()
            && browser_fetch_operation
            && let Some(binding) = &browser_binding
        {
            self.retain_verified_remote_counts(&mut status, binding, &operation_fence);
        }
        if !cancelled {
            status.account_state = account_state_after_result(
                self.account_state(plugin_id, signer_fingerprint_sha256, &profile_id),
                status.stable_error_code,
            );
        }
        if background
            && matches!(
                status.stable_error_code,
                Some(
                    PluginSshSyncStableErrorCode::InteractionRequired
                        | PluginSshSyncStableErrorCode::VaultMissing
                        | PluginSshSyncStableErrorCode::VaultLocked
                        | PluginSshSyncStableErrorCode::AuthorizationExpired
                )
            )
        {
            status.operation_state = PluginSshSyncOperationState::NeedsReview;
        }
        if operation_fence() {
            self.remember_status(&namespace, &status);
        }
        status
    }

    fn operation_epoch_current(&self, plugin_id: &str, expected: u64) -> bool {
        let operations = self
            .operations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operations.epoch_current(plugin_id, expected)
    }

    async fn select_data_owner(
        &mut self,
        plugin_id: &str,
        profile_id: &str,
        request: &PluginSshSyncRequest,
        fence: &ActionFence,
    ) -> Result<(), BrokerError> {
        let source = match request {
            PluginSshSyncRequest::Refresh { source, .. }
            | PluginSshSyncRequest::Sync { source, .. } => Some(source.clone()),
            // Reset has its own protected confirmation and ETag-conditional
            // DELETE. It must remain usable for a damaged remote exchange.
            PluginSshSyncRequest::ResetRemote { .. }
            | PluginSshSyncRequest::ConfigureScope { .. } => None,
            _ => {
                self.data_owner_sha256 = self.package_signer_sha256.clone();
                return Ok(());
            }
        };
        if let Some(source) = source
            && let Some(remote) = self
                .fetch_exchange(
                    plugin_id,
                    &self.package_signer_sha256,
                    profile_id,
                    source.clone(),
                    fence,
                )
                .await?
        {
            self.data_owner_sha256 = remote.binding.signer_fingerprint_sha256.clone();
            let key = self
                .authenticate_data_owner(plugin_id, profile_id, &remote, fence)
                .await?;
            let confirmed = self
                .fetch_exchange(
                    plugin_id,
                    &self.package_signer_sha256,
                    profile_id,
                    source,
                    fence,
                )
                .await?
                .ok_or(BrokerError::StateConflict)?;
            ensure_same_remote(&remote, &confirmed)?;
            self.authenticated_remote_key = Some((sha256_hex(&remote.bytes), key));
            self.reconcile_local_data_owner(
                plugin_id,
                profile_id,
                &remote.binding.signer_fingerprint_sha256,
            )
            .await?;
            return Ok(());
        }
        let owners = self
            .profiles
            .existing_data_owners(plugin_id.to_owned(), profile_id.to_owned())
            .await
            .map_err(map_store_error)?;
        if owners.len() > 1 {
            let mut established = None;
            for owner in &owners {
                let profile = self
                    .profiles
                    .existing_profile_state(
                        plugin_id.to_owned(),
                        owner.clone(),
                        profile_id.to_owned(),
                    )
                    .await
                    .map_err(map_store_error)?;
                if profile.is_some_and(|profile| profile.remote_baseline.is_some())
                    && established.replace(owner.clone()).is_some()
                {
                    return Err(BrokerError::OwnerConflict);
                }
            }
            self.data_owner_sha256 = established.ok_or(BrokerError::OwnerConflict)?;
            return Ok(());
        }
        self.data_owner_sha256 = select_local_data_owner(plugin_id, &owners)?;
        Ok(())
    }

    async fn authenticate_data_owner(
        &self,
        plugin_id: &str,
        profile_id: &str,
        remote: &DownloadedExchange,
        fence: &ActionFence,
    ) -> Result<SyncKey, BrokerError> {
        let owner = &remote.binding.signer_fingerprint_sha256;
        let envelope = plugin_exchange_vault_key_envelope(&remote.bytes, &remote.binding)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        let existing = self
            .profiles
            .existing_profile_state(plugin_id.to_owned(), owner.clone(), profile_id.to_owned())
            .await
            .map_err(map_store_error)?;
        let key = if let Some(binding) = existing
            .as_ref()
            .and_then(|state| state.key_binding.as_ref())
        {
            if binding.password_wrapped_envelope != envelope {
                return Err(BrokerError::KeyBindingConflict);
            }
            let secret = self
                .vault
                .read_secret(
                    &binding.secret_ref_id,
                    norishell_secret_vault::SecretKind::SshSyncKey,
                )
                .map_err(|_| BrokerError::LocalKeyUnavailable)?;
            let bytes: [u8; 32] = secret
                .expose()
                .try_into()
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            SyncKey::from_bytes(bytes)
        } else {
            let local = self
                .vault
                .export_sync_key_material()
                .map_err(|_| BrokerError::LocalKeyUnavailable)?;
            let local_key = SyncKey::from_bytes(*local.key_bytes());
            if open_plugin_exchange_with_key(&remote.bytes, &local_key, &remote.binding).is_ok() {
                local_key
            } else {
                if self.background {
                    return Err(BrokerError::InteractionRequired);
                }
                let password = self
                    .secure_ui
                    .vault_password(self.secure_context(
                        plugin_id,
                        owner,
                        profile_id,
                        Some(remote.remote_origin.clone()),
                        fence,
                    ))
                    .await
                    .map_err(map_selection_error)?;
                if !fence() {
                    return Err(BrokerError::RecoveryActionExpired);
                }
                let material = self
                    .vault
                    .open_synchronized_key_material(&envelope, password.as_slice())
                    .map_err(map_sync_key_recovery_error)?;
                SyncKey::from_bytes(*material.key_bytes())
            }
        };
        open_plugin_exchange_with_key(&remote.bytes, &key, &remote.binding)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        if !fence() {
            return Err(BrokerError::RecoveryActionExpired);
        }
        // Authentication alone does not create or bind a local profile.
        Ok(key)
    }

    async fn reconcile_local_data_owner(
        &self,
        plugin_id: &str,
        profile_id: &str,
        owner: &str,
    ) -> Result<(), BrokerError> {
        let owners = self
            .profiles
            .existing_data_owners(plugin_id.to_owned(), profile_id.to_owned())
            .await
            .map_err(map_store_error)?;
        // An authenticated remote owner may coexist with a provisional owner
        // created by an earlier package. Keep the latter intact, but do not
        // let it block the already bound remote profile.
        if owners.iter().any(|existing_owner| existing_owner == owner) {
            return Ok(());
        }
        match owners.as_slice() {
            [] => {}
            [existing_owner] => {
                let source = self
                    .profiles
                    .existing_profile_state(
                        plugin_id.to_owned(),
                        existing_owner.clone(),
                        profile_id.to_owned(),
                    )
                    .await
                    .map_err(map_store_error)?
                    .ok_or(BrokerError::StateConflict)?;
                self.profiles
                    .migrate_provisional_scope_owner(
                        plugin_id.to_owned(),
                        existing_owner.clone(),
                        owner.to_owned(),
                        profile_id.to_owned(),
                        source.state_version,
                    )
                    .await
                    .map_err(map_store_error)?;
            }
            _ => return Err(BrokerError::OwnerConflict),
        }
        Ok(())
    }

    async fn ensure_vault_unlocked(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        fence: &ActionFence,
    ) -> Result<(), BrokerError> {
        if self.vault.is_unlocked() {
            return Ok(());
        }
        if self.vault.status().state == norishell_core_api::VaultState::Missing {
            self.secure_ui
                .create_vault(self.secure_context(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    None,
                    fence,
                ))
                .await
                .map_err(map_selection_error)?;
            if !fence() || !self.vault.is_unlocked() {
                return Err(BrokerError::OperationRejected(
                    "broker.ensure_vault_unlocked.01",
                ));
            }
            return Ok(());
        }
        let password = self
            .secure_ui
            .vault_password(self.secure_context(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                None,
                fence,
            ))
            .await
            .map_err(map_selection_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected(
                "broker.ensure_vault_unlocked.02",
            ));
        }
        self.profiles
            .unlock_vault_for_operation(password)
            .await
            .map_err(map_store_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected(
                "broker.ensure_vault_unlocked.03",
            ));
        }
        Ok(())
    }

    async fn persist_baseline_exchange(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        bytes: &[u8],
    ) -> Result<String, BrokerError> {
        let root = self.app_data_directory.as_ref().clone();
        let plugin_id = plugin_id.to_owned();
        let signer = signer_fingerprint_sha256.to_owned();
        let profile_id = profile_id.to_owned();
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || {
            persist_baseline_exchange_file(&root, &plugin_id, &signer, &profile_id, &bytes)
        })
        .await
        .map_err(|_| BrokerError::Internal("broker.persist_baseline_exchange.internal01"))?
    }

    async fn load_baseline_exchange(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        exchange_sha256: &str,
    ) -> Result<Vec<u8>, BrokerError> {
        let root = self.app_data_directory.as_ref().clone();
        let plugin_id = plugin_id.to_owned();
        let signer = signer_fingerprint_sha256.to_owned();
        let profile_id = profile_id.to_owned();
        let exchange_sha256 = exchange_sha256.to_owned();
        tokio::task::spawn_blocking(move || {
            load_baseline_exchange_file(&root, &plugin_id, &signer, &profile_id, &exchange_sha256)
        })
        .await
        .map_err(|_| BrokerError::Internal("broker.load_baseline_exchange.internal01"))?
    }

    async fn reset_local_remote_state(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        expected_state_version: WireSequence,
        expected_upload_fence: DurableUploadFence,
    ) -> Result<PortableProfileState, BrokerError> {
        let profile = self
            .profiles
            .reset_remote_state(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                expected_state_version,
                expected_upload_fence,
            )
            .await
            .map_err(map_store_error)?;
        let root = self.app_data_directory.as_ref().clone();
        let plugin_id = plugin_id.to_owned();
        let signer = signer_fingerprint_sha256.to_owned();
        let profile_id = profile_id.to_owned();
        tokio::task::spawn_blocking(move || {
            ssh_sync_browser_store::remove_profile(&root, &plugin_id, &profile_id)
                .map_err(|_| BrokerError::Internal("broker.reset_local_remote_state.browser01"))?;
            remove_profile_baseline_files(&root, &plugin_id, &signer, &profile_id)
        })
        .await
        .map_err(|_| BrokerError::Internal("broker.reset_local_remote_state.internal01"))??;
        Ok(profile)
    }

    fn ensure_credential_service(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        auth: PluginSshSyncCredentialProfile,
    ) -> Result<(), BrokerError> {
        let configuration =
            credential_configuration(plugin_id, signer_fingerprint_sha256, profile_id, auth)?;
        let key = namespace(plugin_id, signer_fingerprint_sha256, profile_id);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !reuse_or_restore_credential_service(
            &mut state.oauth,
            &key,
            self.app_data_directory.as_ref(),
            &self.vault,
            configuration,
        )
        .map_err(map_oauth_error)?
        {
            return Ok(());
        }
        state.last.remove(&key);
        state.browser_sources.remove(&key);
        let account_epoch = state.account_epochs.entry(key).or_default();
        *account_epoch = account_epoch.saturating_add(1).max(1);
        drop(state);
        self.browser_cache.invalidate_profile(plugin_id, profile_id);
        Ok(())
    }

    fn remember_status(&self, namespace: &str, status: &PluginSshSyncStatus) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last
            .insert(namespace.to_owned(), status.clone());
    }

    fn status(&self, namespace: &str, profile_id: &str) -> PluginSshSyncStatus {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(status) = state.last.get(namespace) {
            let mut status = status.clone();
            status.account_state = state
                .oauth
                .get(namespace)
                .map_or(PluginSshSyncAccountState::Disconnected, |oauth| {
                    account_state(oauth.status().account_state)
                });
            return status;
        }
        idle_status(
            profile_id.to_owned(),
            state
                .oauth
                .get(namespace)
                .map_or(PluginSshSyncAccountState::Disconnected, |oauth| {
                    account_state(oauth.status().account_state)
                }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn login(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        auth: PluginSshSyncCredentialProfile,
        username_field_id: PluginUiFieldId,
        password_field_id: PluginUiFieldId,
        fields: &[PluginUiFieldValue],
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let username = credential_text(fields, &username_field_id, 320)?;
        let password = credential_password(fields, &password_field_id)?;
        let configuration =
            credential_configuration(plugin_id, signer_fingerprint_sha256, profile_id, auth)?;
        let service = PluginOAuthService::start(
            self.app_data_directory.as_ref(),
            self.vault.clone(),
            configuration,
        )
        .map_err(map_oauth_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.login.01"));
        }
        let status = await_fenced(service.login(username, password), fence)
            .await?
            .map_err(map_oauth_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.login.02"));
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .insert(
                namespace(plugin_id, signer_fingerprint_sha256, profile_id),
                service,
            );
        Ok(idle_status(
            profile_id.to_owned(),
            account_state(status.account_state),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn register(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        auth: PluginSshSyncCredentialProfile,
        username_field_id: PluginUiFieldId,
        password_field_id: PluginUiFieldId,
        password_confirmation_field_id: PluginUiFieldId,
        display_name_field_id: Option<PluginUiFieldId>,
        fields: &[PluginUiFieldValue],
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let username = credential_text(fields, &username_field_id, 320)?;
        let password = credential_password(fields, &password_field_id)?;
        let password_confirmation = credential_password(fields, &password_confirmation_field_id)?;
        if password.len() != password_confirmation.len()
            || !bool::from(password.as_bytes().ct_eq(password_confirmation.as_bytes()))
        {
            return Err(BrokerError::OperationRejected("broker.register.01"));
        }
        let display_name = display_name_field_id
            .as_ref()
            .map(|field_id| credential_text(fields, field_id, 160))
            .transpose()?;
        let configuration =
            credential_configuration(plugin_id, signer_fingerprint_sha256, profile_id, auth)?;
        let service = PluginOAuthService::start(
            self.app_data_directory.as_ref(),
            self.vault.clone(),
            configuration,
        )
        .map_err(map_oauth_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.register.02"));
        }
        let status = await_fenced(service.register(username, password, display_name), fence)
            .await?
            .map_err(map_oauth_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.register.03"));
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .insert(
                namespace(plugin_id, signer_fingerprint_sha256, profile_id),
                service,
            );
        Ok(idle_status(
            profile_id.to_owned(),
            account_state(status.account_state),
        ))
    }

    async fn complete_direct_auth(
        &self,
        namespace: &str,
        profile_id: &str,
        code_field_id: PluginUiFieldId,
        fields: &[PluginUiFieldValue],
        mfa: bool,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let code = credential_code(fields, &code_field_id)?;
        let service = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .get(namespace)
            .cloned()
            .ok_or(BrokerError::AuthorizationExpired)?;
        let status = if mfa {
            await_fenced(service.complete_mfa(code), fence).await?
        } else {
            await_fenced(service.verify_email(code), fence).await?
        }
        .map_err(map_oauth_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected(
                "broker.complete_direct_auth.01",
            ));
        }
        Ok(idle_status(
            profile_id.to_owned(),
            account_state(status.account_state),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn refresh(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        source: PluginSshSyncDownloadSource,
        action_revision: SshSyncActionRevision,
        browser_binding: Option<&SshSyncBrowserCacheBinding>,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let completion_fence = durable_upload_fence(
            &source.url,
            PluginSshSyncHttpMethod::Put,
            source.use_oauth,
            action_revision,
        )?;
        let mut local = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(map_store_error)?;
        let mut profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?;
        let remote = self
            .fetch_exchange(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                source,
                fence,
            )
            .await?;
        let Some(remote) = remote else {
            if !fence() {
                return Err(BrokerError::OperationRejected("broker.refresh.01"));
            }
            profile = self
                .reset_local_remote_state(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    profile.state_version,
                    completion_fence,
                )
                .await?;
            if let Some(binding) = browser_binding {
                self.publish_browser_snapshot(binding, &empty_portable_bundle(1), None, fence);
            }
            self.remember_browser_exchange(browser_binding, None, None, fence)
                .await;
            return Ok(aggregate_status(
                profile_id,
                self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                PluginSshSyncOperationState::Succeeded,
                &local,
                Some((0, 0, 0)),
                if local.host_count == 0
                    && local.credential_count == 0
                    && local.desktop_profile_count == 0
                {
                    PluginSshSyncDifferenceState::Equal
                } else {
                    PluginSshSyncDifferenceState::LocalOnly
                },
                profile.scope_mode,
                profile.last_successful_sync_at_unix_ms,
                None,
                None,
                None,
            ));
        };
        let (remote_bundle_raw, next_profile, key) = self
            .open_downloaded_exchange(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                &remote,
                profile,
                fence,
            )
            .await?;
        self.reconcile_upload_attempt_with_remote(
            plugin_id,
            signer_fingerprint_sha256,
            profile_id,
            &remote,
            &completion_fence,
            &key,
        )
        .await?;
        profile = next_profile;
        let base_bundle = self
            .open_profile_baseline(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                &profile,
                &key,
            )
            .await?
            .map(business_only_bundle)
            .transpose()?;
        if let Some(base) = base_bundle.as_ref() {
            local.bundle = self
                .profiles
                .prepare_local_merge(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    base.clone(),
                    local.bundle,
                )
                .await
                .map_err(map_store_error)?;
        }
        let local_sha256 = portable_content_sha256(&local.bundle, &key)?;
        let remote_counts = (
            bounded_count(remote_bundle_raw.objects.hosts.len())?,
            bounded_count(remote_bundle_raw.objects.credentials.len())?,
            bounded_count(remote_bundle_raw.objects.desktop_profiles.len())?,
        );
        let remote_sha256_raw = portable_content_sha256(&remote_bundle_raw, &key)?;
        verify_remote_baseline_invariants(
            &profile,
            remote.binding.revision,
            &remote.etag,
            &remote_sha256_raw,
            &remote.bytes,
        )?;
        if let Some(binding) = browser_binding {
            self.publish_browser_snapshot(
                binding,
                &remote_bundle_raw,
                remote.remote_updated_at_unix_ms,
                fence,
            );
        }
        self.remember_browser_exchange(
            browser_binding,
            Some(&remote.bytes),
            remote.remote_updated_at_unix_ms,
            fence,
        )
        .await;
        let migration_needed = remote_bundle_raw.schema != BundleSchema::V6
            || remote_bundle_raw.preferences.is_some()
            || !remote_bundle_raw.preference_update_times.is_empty();
        let remote_bundle = business_only_bundle(remote_bundle_raw)?;
        let remote_sha256 = portable_content_sha256(&remote_bundle, &key)?;
        let logical_profile = profile_with_business_baseline(&profile, base_bundle.as_ref(), &key)?;
        let mut difference = difference_state(
            &logical_profile,
            &local_sha256,
            remote.binding.revision,
            &remote.etag,
            &remote_sha256,
            local.host_count,
            local.credential_count,
            local.desktop_profile_count,
            remote_counts,
        );
        if migration_needed && difference == PluginSshSyncDifferenceState::Equal {
            difference = PluginSshSyncDifferenceState::Different;
        }
        if difference != PluginSshSyncDifferenceState::Equal
            && !migration_needed
            && logical_profile
                .remote_baseline
                .as_ref()
                .is_some_and(|baseline| {
                    baseline.content_sha256 == local_sha256
                        && remote.binding.revision > baseline.revision
                })
            && remote_only_historical_tombstones(&local.bundle, &remote_bundle, &key)?
        {
            let mut fresh_local = self
                .profiles
                .snapshot_current(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    1,
                )
                .await
                .map_err(map_store_error)?;
            if let Some(base) = base_bundle.as_ref() {
                fresh_local.bundle = self
                    .profiles
                    .prepare_local_merge(
                        plugin_id.to_owned(),
                        signer_fingerprint_sha256.to_owned(),
                        profile_id.to_owned(),
                        base.clone(),
                        fresh_local.bundle,
                    )
                    .await
                    .map_err(map_store_error)?;
            }
            if portable_content_sha256(&fresh_local.bundle, &key)? != local_sha256 {
                return Err(BrokerError::LocalStateChanged);
            }
            difference = PluginSshSyncDifferenceState::Equal;
        }
        if difference == PluginSshSyncDifferenceState::Equal {
            if !fence() {
                return Err(BrokerError::OperationRejected("broker.refresh.02"));
            }
            let exchange_sha256 = self
                .persist_baseline_exchange(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    &remote.bytes,
                )
                .await?;
            if !fence() {
                return Err(BrokerError::OperationRejected("broker.refresh.03"));
            }
            profile = self
                .profiles
                .update_remote_baseline(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    profile.state_version,
                    PortableRemoteBaseline {
                        revision: remote.binding.revision,
                        etag: remote.etag.clone(),
                        content_sha256: remote_sha256,
                        exchange_sha256,
                    },
                )
                .await
                .map_err(map_store_error)?;
        }
        Ok(aggregate_status(
            profile_id,
            self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
            if difference == PluginSshSyncDifferenceState::Conflict {
                PluginSshSyncOperationState::NeedsReview
            } else {
                PluginSshSyncOperationState::Succeeded
            },
            &local,
            Some(remote_counts),
            difference,
            profile.scope_mode,
            profile.last_successful_sync_at_unix_ms,
            Some(remote.http_status),
            Some(remote.binding.revision),
            Some(remote.etag),
        ))
    }

    async fn configure_scope(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let selection = self
            .secure_ui
            .select_backup(self.secure_context(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                None,
                fence,
            ))
            .await
            .map_err(map_selection_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.configure_scope.01"));
        }
        let local = self
            .profiles
            .configure_scope(signer_fingerprint_sha256.to_owned(), selection)
            .await
            .map_err(map_store_error)?;
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?;
        Ok(aggregate_status(
            profile_id,
            self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
            PluginSshSyncOperationState::Succeeded,
            &local,
            None,
            PluginSshSyncDifferenceState::Unavailable,
            profile.scope_mode,
            profile.last_successful_sync_at_unix_ms,
            None,
            profile.remote_baseline.as_ref().map(|value| value.revision),
            profile.remote_baseline.map(|value| value.etag),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn reset_remote(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        target: PluginSshSyncDeleteTarget,
        action_revision: SshSyncActionRevision,
        browser_binding: Option<&SshSyncBrowserCacheBinding>,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let remote_url = sync_url(&target.url)?;
        let remote_origin = origin(&remote_url);
        let upload_fence = durable_upload_fence(
            &target.url,
            PluginSshSyncHttpMethod::Put,
            target.use_oauth,
            action_revision,
        )?;
        let local_counts = self
            .profiles
            .local_counts(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?;
        let mut profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?;
        if let Some(attempt) = self
            .profiles
            .pending_upload_attempt(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?
            && !upload_attempt_matches_fence(&attempt, &upload_fence)
        {
            return Err(BrokerError::StateConflict);
        }
        let token = self
            .token_for(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                target.use_oauth,
                &remote_url,
                fence,
            )
            .await?;
        let remote = await_fenced(
            self.download_optional(
                PluginSshSyncDownloadSource {
                    url: target.url.clone(),
                    use_oauth: target.use_oauth,
                },
                token.as_deref().map(String::as_str),
            ),
            fence,
        )
        .await??;
        let etag = remote
            .as_ref()
            .map(|(_, etag, _, _, _)| {
                etag.as_ref()
                    .filter(|value| valid_strong_etag(value))
                    .cloned()
                    .ok_or(BrokerError::RemoteDataInvalid)
            })
            .transpose()?;
        self.secure_ui
            .approve_remote_reset(self.secure_context(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                Some(remote_origin),
                fence,
            ))
            .await
            .map_err(map_selection_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.reset_remote.01"));
        }
        // The warning can remain open for an arbitrary amount of time. Ask
        // the account service for a fresh token before checking or mutating
        // the remote state.
        let token = self
            .token_for(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                target.use_oauth,
                &remote_url,
                fence,
            )
            .await?;
        let Some(etag) = etag else {
            let rechecked_remote = await_fenced(
                self.download_optional(
                    PluginSshSyncDownloadSource {
                        url: target.url.clone(),
                        use_oauth: target.use_oauth,
                    },
                    token.as_deref().map(String::as_str),
                ),
                fence,
            )
            .await??;
            validate_remote_still_absent(rechecked_remote.as_ref())?;
            profile = self
                .reset_local_remote_state(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    profile.state_version,
                    upload_fence,
                )
                .await?;
            if let Some(binding) = browser_binding {
                self.publish_browser_snapshot(binding, &empty_portable_bundle(1), None, fence);
            }
            self.remember_browser_exchange(browser_binding, None, None, fence)
                .await;
            return Ok(remote_reset_status(
                profile_id,
                self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                local_counts,
                &profile,
                None,
            ));
        };
        let http_status = await_fenced(
            self.delete_remote(
                target,
                token.as_deref().map(String::as_str),
                &etag,
                &Uuid::new_v4().to_string(),
            ),
            fence,
        )
        .await??;
        profile = self
            .reset_local_remote_state(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                profile.state_version,
                upload_fence,
            )
            .await?;
        if let Some(binding) = browser_binding {
            self.publish_browser_snapshot(binding, &empty_portable_bundle(1), None, fence);
        }
        self.remember_browser_exchange(browser_binding, None, None, fence)
            .await;
        Ok(remote_reset_status(
            profile_id,
            self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
            local_counts,
            &profile,
            Some(http_status),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn sync(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        source: PluginSshSyncDownloadSource,
        destination: PluginSshSyncUploadTarget,
        action_revision: SshSyncActionRevision,
        browser_binding: Option<&SshSyncBrowserCacheBinding>,
        fence: &ActionFence,
        automatic: bool,
        conflict_policy: PluginSshSyncConflictPolicy,
        deletion_policy: PluginSshSyncConflictPolicy,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        for attempt in 0..2 {
            let result = self
                .sync_once(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    source.clone(),
                    destination.clone(),
                    action_revision,
                    browser_binding,
                    fence,
                    automatic,
                    conflict_policy,
                    deletion_policy,
                )
                .await;
            if matches!(result, Err(BrokerError::RetryLocalSnapshot)) {
                if attempt == 0 && fence() {
                    continue;
                }
                return Err(BrokerError::LocalStateChanged);
            }
            return result;
        }
        unreachable!("bounded sync retry returns from its final attempt")
    }

    #[allow(clippy::too_many_arguments)]
    async fn sync_once(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        source: PluginSshSyncDownloadSource,
        destination: PluginSshSyncUploadTarget,
        action_revision: SshSyncActionRevision,
        browser_binding: Option<&SshSyncBrowserCacheBinding>,
        fence: &ActionFence,
        automatic: bool,
        conflict_policy: PluginSshSyncConflictPolicy,
        deletion_policy: PluginSshSyncConflictPolicy,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        if destination.method != PluginSshSyncHttpMethod::Put
            || destination.if_match.is_some()
            || destination.url != source.url
            || destination.use_oauth != source.use_oauth
        {
            return Err(BrokerError::OperationRejected("broker.sync.01"));
        }
        let completion_fence = durable_upload_fence(
            &source.url,
            destination.method,
            source.use_oauth,
            action_revision,
        )?;
        let source_for_recheck = source.clone();
        let mut local = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(map_store_error)?;
        let mut profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?;
        let (remote, absent_next_revision) = self
            .fetch_exchange_head(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                source,
                fence,
            )
            .await?;
        if automatic && profile.remote_baseline.is_none() {
            return Ok(aggregate_status(
                profile_id,
                self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                PluginSshSyncOperationState::NeedsReview,
                &local,
                None,
                PluginSshSyncDifferenceState::Different,
                profile.scope_mode,
                profile.last_successful_sync_at_unix_ms,
                remote.as_ref().map(|value| value.http_status),
                remote.as_ref().map(|value| value.binding.revision),
                remote.as_ref().map(|value| value.etag.clone()),
            ));
        }
        let Some(remote) = remote else {
            if automatic {
                return Ok(aggregate_status(
                    profile_id,
                    self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                    PluginSshSyncOperationState::NeedsReview,
                    &local,
                    Some((0, 0, 0)),
                    PluginSshSyncDifferenceState::LocalOnly,
                    profile.scope_mode,
                    profile.last_successful_sync_at_unix_ms,
                    Some(StatusCode::NOT_FOUND.as_u16()),
                    None,
                    None,
                ));
            }
            profile = self
                .reset_local_remote_state(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    profile.state_version,
                    completion_fence,
                )
                .await?;
            if let Some(binding) = browser_binding {
                self.publish_browser_snapshot(binding, &empty_portable_bundle(1), None, fence);
            }
            self.remember_browser_exchange(browser_binding, None, None, fence)
                .await;
            if local.host_count == 0
                && local.credential_count == 0
                && local.desktop_profile_count == 0
            {
                profile = self
                    .profiles
                    .mark_sync_succeeded(
                        plugin_id.to_owned(),
                        signer_fingerprint_sha256.to_owned(),
                        profile_id.to_owned(),
                        profile.state_version,
                    )
                    .await
                    .map_err(map_store_error)?;
                return Ok(aggregate_status(
                    profile_id,
                    self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                    PluginSshSyncOperationState::Succeeded,
                    &local,
                    Some((0, 0, 0)),
                    PluginSshSyncDifferenceState::Equal,
                    profile.scope_mode,
                    profile.last_successful_sync_at_unix_ms,
                    Some(StatusCode::NOT_FOUND.as_u16()),
                    None,
                    None,
                ));
            }
            let (key, _envelope, next_profile) = self
                .sync_key_for_upload(plugin_id, signer_fingerprint_sha256, profile_id, profile)
                .await?;
            profile = next_profile;
            let empty_remote = empty_portable_bundle(1);
            let report = difference::compare_bundles(&local.bundle, &empty_remote);
            let expected_local_sha256 = portable_content_sha256(&local.bundle, &key)?;
            let restore = self
                .profiles
                .stage_restore(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    difference::with_tombstones_for_missing(&empty_remote, &local.bundle)
                        .map_err(|_| BrokerError::RemoteDataInvalid)?,
                    key.clone(),
                )
                .await
                .map_err(map_store_error)?;
            let _restore_cleanup = StagedRestoreCleanup {
                profiles: Arc::clone(&self.profiles),
                handle: restore.handle.clone(),
            };
            if restore.conflict_count > 0 {
                return Err(BrokerError::RestoreConflict);
            }
            let direction = self
                .secure_ui
                .choose_sync_direction(
                    self.secure_context(
                        plugin_id,
                        signer_fingerprint_sha256,
                        profile_id,
                        Some(origin(&sync_url(&source_for_recheck.url)?)),
                        fence,
                    ),
                    SyncDirectionReview {
                        merge_plan: false,
                        restore_handle: restore.handle.clone(),
                        local_desktop_profile_count: local.desktop_profile_count,
                        remote_desktop_profile_count: 0,
                        local_host_count: local.host_count,
                        local_credential_count: local.credential_count,
                        remote_host_count: 0,
                        remote_credential_count: 0,
                        conflict_count: report.changed_count,
                        delete_count: restore.delete_count,
                        local_compared_at_unix_ms: crate::time::unix_time_ms(),
                        remote_updated_at_unix_ms: None,
                        differences: report.differences,
                        difference_total_count: report.total_count,
                        difference_omitted_count: report.omitted_count,
                    },
                )
                .await
                .map_err(map_selection_error)?;
            if !fence() {
                return Err(BrokerError::OperationRejected("broker.sync.02"));
            }
            let (rechecked, next_revision) = self
                .fetch_exchange_head(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    source_for_recheck,
                    fence,
                )
                .await?;
            if rechecked.is_some() || next_revision != absent_next_revision {
                return Err(BrokerError::StateConflict);
            }
            let fresh_local = self
                .profiles
                .snapshot_current(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    1,
                )
                .await
                .map_err(map_store_error)?;
            if portable_content_sha256(&fresh_local.bundle, &key)? != expected_local_sha256 {
                return Err(BrokerError::LocalStateChanged);
            }
            return match direction {
                SecureSyncDirection::LocalOverRemote => {
                    self.push_local(
                        plugin_id,
                        signer_fingerprint_sha256,
                        profile_id,
                        fresh_local,
                        profile,
                        destination,
                        None,
                        absent_next_revision,
                        action_revision,
                        browser_binding,
                        fence,
                    )
                    .await
                }
                SecureSyncDirection::RemoteOverLocal(approval) => {
                    self.apply_absent_remote(
                        plugin_id,
                        signer_fingerprint_sha256,
                        profile_id,
                        restore,
                        empty_remote,
                        profile,
                        key,
                        approval,
                        fence,
                    )
                    .await
                }
                SecureSyncDirection::ApplyMerged(_) => {
                    Err(BrokerError::OperationRejected("broker.sync.03"))
                }
            };
        };
        let (remote_bundle_raw, profile, key) = self
            .open_downloaded_exchange(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                &remote,
                profile,
                fence,
            )
            .await?;
        self.reconcile_upload_attempt_with_remote(
            plugin_id,
            signer_fingerprint_sha256,
            profile_id,
            &remote,
            &completion_fence,
            &key,
        )
        .await?;
        let remote_counts = (
            bounded_count(remote_bundle_raw.objects.hosts.len())?,
            bounded_count(remote_bundle_raw.objects.credentials.len())?,
            bounded_count(remote_bundle_raw.objects.desktop_profiles.len())?,
        );
        let remote_sha256_raw = portable_content_sha256(&remote_bundle_raw, &key)?;
        verify_remote_baseline_invariants(
            &profile,
            remote.binding.revision,
            &remote.etag,
            &remote_sha256_raw,
            &remote.bytes,
        )?;
        if let Some(binding) = browser_binding {
            self.publish_browser_snapshot(
                binding,
                &remote_bundle_raw,
                remote.remote_updated_at_unix_ms,
                fence,
            );
        }
        self.remember_browser_exchange(
            browser_binding,
            Some(&remote.bytes),
            remote.remote_updated_at_unix_ms,
            fence,
        )
        .await;
        let base_bundle = self
            .open_profile_baseline(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                &profile,
                &key,
            )
            .await?
            .map(business_only_bundle)
            .transpose()?;
        if let Some(base) = &base_bundle {
            local.bundle = self
                .profiles
                .prepare_local_merge(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    base.clone(),
                    local.bundle,
                )
                .await
                .map_err(map_store_error)?;
        }
        let migration_needed = remote_bundle_raw.schema != BundleSchema::V6
            || remote_bundle_raw.preferences.is_some()
            || !remote_bundle_raw.preference_update_times.is_empty();
        let remote_bundle = business_only_bundle(remote_bundle_raw)?;
        let remote_sha256 = portable_content_sha256(&remote_bundle, &key)?;
        let local_sha256 = portable_content_sha256(&local.bundle, &key)?;
        let logical_profile = profile_with_business_baseline(&profile, base_bundle.as_ref(), &key)?;
        let mut difference = difference_state(
            &logical_profile,
            &local_sha256,
            remote.binding.revision,
            &remote.etag,
            &remote_sha256,
            local.host_count,
            local.credential_count,
            local.desktop_profile_count,
            remote_counts,
        );
        if migration_needed && difference == PluginSshSyncDifferenceState::Equal {
            difference = PluginSshSyncDifferenceState::Different;
        }
        if difference == PluginSshSyncDifferenceState::Equal {
            if !fence() {
                return Err(BrokerError::OperationRejected("broker.sync.04"));
            }
            let exchange_sha256 = self
                .persist_baseline_exchange(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    &remote.bytes,
                )
                .await?;
            if !fence() {
                return Err(BrokerError::OperationRejected("broker.sync.05"));
            }
            let profile = self
                .profiles
                .update_remote_baseline(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    profile.state_version,
                    PortableRemoteBaseline {
                        revision: remote.binding.revision,
                        etag: remote.etag.clone(),
                        content_sha256: local_sha256,
                        exchange_sha256,
                    },
                )
                .await
                .map_err(map_store_error)?;
            let profile = self
                .profiles
                .mark_sync_succeeded(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    profile.state_version,
                )
                .await
                .map_err(map_store_error)?;
            return Ok(aggregate_status(
                profile_id,
                self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                PluginSshSyncOperationState::Succeeded,
                &local,
                Some(remote_counts),
                PluginSshSyncDifferenceState::Equal,
                profile.scope_mode,
                profile.last_successful_sync_at_unix_ms,
                Some(remote.http_status),
                Some(remote.binding.revision),
                Some(remote.etag),
            ));
        }
        if migration_needed && automatic {
            return Ok(aggregate_status(
                profile_id,
                self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                PluginSshSyncOperationState::NeedsReview,
                &local,
                Some(remote_counts),
                difference,
                profile.scope_mode,
                profile.last_successful_sync_at_unix_ms,
                Some(remote.http_status),
                Some(remote.binding.revision),
                Some(remote.etag),
            ));
        }
        if let Some(base) = base_bundle.as_ref()
            && base.schema == BundleSchema::V6
            && local.bundle.schema == BundleSchema::V6
            && remote_bundle.schema == BundleSchema::V6
        {
            if remote.binding.revision >= MAX_EXCHANGE_REVISION {
                return Err(BrokerError::RevisionExhausted);
            }
            let resolution = (conflict_policy == PluginSshSyncConflictPolicy::Newest)
                .then_some(BundleConflictResolution::Newest);
            let deletion_resolution = (deletion_policy == PluginSshSyncConflictPolicy::Newest)
                .then_some(BundleConflictResolution::Newest);
            let needs_review = |count| {
                let mut status = aggregate_status(
                    profile_id,
                    self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                    PluginSshSyncOperationState::NeedsReview,
                    &local,
                    Some(remote_counts),
                    PluginSshSyncDifferenceState::Conflict,
                    profile.scope_mode,
                    profile.last_successful_sync_at_unix_ms,
                    Some(remote.http_status),
                    Some(remote.binding.revision),
                    Some(remote.etag.clone()),
                );
                status.conflict_count = count;
                status
            };
            let mut merged = match merge_bundles_three_way_with_policies(
                base,
                &local.bundle,
                &remote_bundle,
                remote.binding.revision + 1,
                resolution,
                deletion_resolution,
            ) {
                Ok(value) => value,
                Err(_) => return Err(BrokerError::MergeInvalid("broker.sync.merge_initial")),
            };
            if let BundleMergeOutcome::Conflicts { count } = &merged
                && !automatic
            {
                let report = difference::compare_bundles(&local.bundle, &remote_bundle);
                let side = self
                    .secure_ui
                    .choose_conflict_side(
                        self.secure_context(
                            plugin_id,
                            signer_fingerprint_sha256,
                            profile_id,
                            Some(remote.remote_origin.clone()),
                            fence,
                        ),
                        SyncConflictReview {
                            local_desktop_profile_count: local.desktop_profile_count,
                            remote_desktop_profile_count: remote_counts.2,
                            local_host_count: local.host_count,
                            local_credential_count: local.credential_count,
                            remote_host_count: remote_counts.0,
                            remote_credential_count: remote_counts.1,
                            conflict_count: *count,
                            local_compared_at_unix_ms: crate::time::unix_time_ms(),
                            remote_updated_at_unix_ms: remote.remote_updated_at_unix_ms,
                            differences: report.differences,
                            difference_total_count: report.total_count,
                            difference_omitted_count: report.omitted_count,
                        },
                    )
                    .await
                    .map_err(map_selection_error)?;
                if !fence() {
                    return Err(BrokerError::OperationRejected("broker.sync.06"));
                }
                merged = match merge_bundles_three_way_with_policies(
                    base,
                    &local.bundle,
                    &remote_bundle,
                    remote.binding.revision + 1,
                    Some(side),
                    Some(side),
                ) {
                    Ok(value) => value,
                    Err(_) => return Err(BrokerError::MergeInvalid("broker.sync.merge_selected")),
                };
            }
            match merged {
                BundleMergeOutcome::Merged(merged) => {
                    return self
                        .sync_merged_bundle(
                            plugin_id,
                            signer_fingerprint_sha256,
                            profile_id,
                            source_for_recheck,
                            destination,
                            action_revision,
                            browser_binding,
                            fence,
                            automatic,
                            deletion_policy,
                            local,
                            profile,
                            remote,
                            remote_counts,
                            key,
                            base.clone(),
                            local_sha256,
                            *merged,
                        )
                        .await;
                }
                BundleMergeOutcome::Conflicts { count } => {
                    if automatic {
                        return Ok(needs_review(count));
                    }
                    return Err(BrokerError::MergeInvalid("broker.sync.merge_unresolved"));
                }
            }
        }
        if automatic {
            return Ok(aggregate_status(
                profile_id,
                self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                PluginSshSyncOperationState::NeedsReview,
                &local,
                Some(remote_counts),
                difference,
                profile.scope_mode,
                profile.last_successful_sync_at_unix_ms,
                Some(remote.http_status),
                Some(remote.binding.revision),
                Some(remote.etag),
            ));
        }
        let report = difference::compare_bundles(&local.bundle, &remote_bundle);
        let restore = self
            .profiles
            .stage_restore(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                difference::with_tombstones_for_missing(&remote_bundle, &local.bundle)
                    .map_err(|_| BrokerError::RemoteDataInvalid)?,
                key.clone(),
            )
            .await
            .map_err(map_store_error)?;
        let _restore_cleanup = StagedRestoreCleanup {
            profiles: Arc::clone(&self.profiles),
            handle: restore.handle.clone(),
        };
        if restore.conflict_count > 0 {
            return Err(BrokerError::RestoreConflict);
        }
        let direction = self
            .secure_ui
            .choose_sync_direction(
                self.secure_context(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    Some(remote.remote_origin.clone()),
                    fence,
                ),
                SyncDirectionReview {
                    merge_plan: false,
                    restore_handle: restore.handle.clone(),
                    local_desktop_profile_count: local.desktop_profile_count,
                    remote_desktop_profile_count: remote_counts.2,
                    local_host_count: local.host_count,
                    local_credential_count: local.credential_count,
                    remote_host_count: remote_counts.0,
                    remote_credential_count: remote_counts.1,
                    conflict_count: report.changed_count,
                    delete_count: restore.delete_count,
                    local_compared_at_unix_ms: crate::time::unix_time_ms(),
                    remote_updated_at_unix_ms: remote.remote_updated_at_unix_ms,
                    differences: report.differences,
                    difference_total_count: report.total_count,
                    difference_omitted_count: report.omitted_count,
                },
            )
            .await
            .map_err(map_selection_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.sync.07"));
        }
        let rechecked_remote = self
            .fetch_exchange(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                source_for_recheck,
                fence,
            )
            .await?
            .ok_or(BrokerError::StateConflict)?;
        ensure_same_remote(&remote, &rechecked_remote)?;
        let mut fresh_local = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(map_store_error)?;
        if let Some(base) = base_bundle {
            fresh_local.bundle = self
                .profiles
                .prepare_local_merge(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    base,
                    fresh_local.bundle,
                )
                .await
                .map_err(map_store_error)?;
        }
        if portable_content_sha256(&fresh_local.bundle, &key)? != local_sha256 {
            return Err(BrokerError::LocalStateChanged);
        }
        match direction {
            SecureSyncDirection::LocalOverRemote => {
                self.push_local(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    fresh_local,
                    profile,
                    destination,
                    Some(rechecked_remote),
                    1,
                    action_revision,
                    browser_binding,
                    fence,
                )
                .await
            }
            SecureSyncDirection::RemoteOverLocal(approval) => {
                self.pull_remote(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    rechecked_remote,
                    remote_bundle,
                    restore,
                    profile,
                    key,
                    approval,
                    migration_needed,
                    destination,
                    action_revision,
                    browser_binding,
                    fence,
                )
                .await
            }
            SecureSyncDirection::ApplyMerged(_) => {
                Err(BrokerError::OperationRejected("broker.sync.08"))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn sync_merged_bundle(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        source: PluginSshSyncDownloadSource,
        destination: PluginSshSyncUploadTarget,
        action_revision: SshSyncActionRevision,
        browser_binding: Option<&SshSyncBrowserCacheBinding>,
        fence: &ActionFence,
        automatic: bool,
        deletion_policy: PluginSshSyncConflictPolicy,
        local: PortableSnapshot,
        profile: PortableProfileState,
        remote: DownloadedExchange,
        remote_counts: (u32, u32, u32),
        key: SyncKey,
        base: PortableBundleV1,
        local_sha256: String,
        merged: PortableBundleV1,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let old_tombstones = base
            .tombstones
            .iter()
            .map(|value| (value.kind, value.id))
            .collect::<BTreeSet<_>>();
        let merged_sha256 = portable_content_sha256(&merged, &key)?;
        let staged_bundle = difference::with_tombstones_for_missing(&merged, &local.bundle)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        let merged_tombstones = merged
            .tombstones
            .iter()
            .map(|value| (value.kind, value.id))
            .collect::<BTreeSet<_>>();
        if staged_bundle
            .tombstones
            .iter()
            .any(|value| !merged_tombstones.contains(&(value.kind, value.id)))
        {
            return Err(BrokerError::Internal("broker.sync.merge_tombstones"));
        }
        let restore = self
            .profiles
            .stage_restore(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                staged_bundle.clone(),
                key.clone(),
            )
            .await
            .map_err(map_store_error)?;
        let _restore_cleanup = StagedRestoreCleanup {
            profiles: Arc::clone(&self.profiles),
            handle: restore.handle.clone(),
        };
        let mut deleted_ids = merged
            .tombstones
            .iter()
            .filter(|value| !old_tombstones.contains(&(value.kind, value.id)))
            .map(|value| (value.kind, value.id))
            .collect::<BTreeSet<_>>();
        deleted_ids.extend(
            difference::object_keys(&local.bundle)
                .difference(&difference::object_keys(&staged_bundle))
                .copied(),
        );
        let delete_count = bounded_count(deleted_ids.len())?.max(restore.delete_count);
        let deletion_needs_prompt =
            delete_count > 0 && deletion_policy == PluginSshSyncConflictPolicy::Prompt;
        match staged_restore_blocker(restore.conflict_count, automatic, deletion_needs_prompt) {
            Some(StagedRestoreBlocker::RestoreConflict(conflict_count)) => {
                return Ok(restore_conflict_status(
                    aggregate_status(
                        profile_id,
                        self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                        PluginSshSyncOperationState::Failed,
                        &local,
                        Some(remote_counts),
                        PluginSshSyncDifferenceState::Conflict,
                        profile.scope_mode,
                        profile.last_successful_sync_at_unix_ms,
                        Some(remote.http_status),
                        Some(remote.binding.revision),
                        Some(remote.etag),
                    ),
                    conflict_count,
                ));
            }
            Some(StagedRestoreBlocker::DeletionApproval) => {
                return Ok(aggregate_status(
                    profile_id,
                    self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                    PluginSshSyncOperationState::NeedsReview,
                    &local,
                    Some(remote_counts),
                    PluginSshSyncDifferenceState::Conflict,
                    profile.scope_mode,
                    profile.last_successful_sync_at_unix_ms,
                    Some(remote.http_status),
                    Some(remote.binding.revision),
                    Some(remote.etag),
                ));
            }
            None => {}
        }
        let merge_approval = if deletion_needs_prompt {
            let report = difference::compare_bundles(&base, &merged);
            let direction = self
                .secure_ui
                .choose_sync_direction(
                    self.secure_context(
                        plugin_id,
                        signer_fingerprint_sha256,
                        profile_id,
                        Some(remote.remote_origin.clone()),
                        fence,
                    ),
                    SyncDirectionReview {
                        merge_plan: true,
                        restore_handle: restore.handle.clone(),
                        local_desktop_profile_count: bounded_count(
                            base.objects.desktop_profiles.len(),
                        )?,
                        remote_desktop_profile_count: bounded_count(
                            merged.objects.desktop_profiles.len(),
                        )?,
                        local_host_count: bounded_count(base.objects.hosts.len())?,
                        local_credential_count: bounded_count(base.objects.credentials.len())?,
                        remote_host_count: bounded_count(merged.objects.hosts.len())?,
                        remote_credential_count: bounded_count(merged.objects.credentials.len())?,
                        conflict_count: report.changed_count,
                        delete_count,
                        local_compared_at_unix_ms: crate::time::unix_time_ms(),
                        remote_updated_at_unix_ms: remote.remote_updated_at_unix_ms,
                        differences: report.differences,
                        difference_total_count: report.total_count,
                        difference_omitted_count: report.omitted_count,
                    },
                )
                .await
                .map_err(map_selection_error)?;
            match direction {
                SecureSyncDirection::ApplyMerged(approval) => Some(approval),
                _ => {
                    return Err(BrokerError::OperationRejected(
                        "broker.sync_merged_bundle.01",
                    ));
                }
            }
        } else {
            None
        };
        if !fence() {
            return Err(BrokerError::OperationRejected(
                "broker.sync_merged_bundle.02",
            ));
        }
        let rechecked_remote = self
            .fetch_exchange(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                source,
                fence,
            )
            .await?
            .ok_or(BrokerError::StateConflict)?;
        ensure_same_remote(&remote, &rechecked_remote)?;
        let mut fresh_local = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                merged.revision,
            )
            .await
            .map_err(map_store_error)?;
        fresh_local.bundle = self
            .profiles
            .prepare_local_merge(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                base,
                fresh_local.bundle,
            )
            .await
            .map_err(map_store_error)?;
        if portable_content_sha256(&fresh_local.bundle, &key)? != local_sha256 {
            return Err(BrokerError::LocalStateChanged);
        }
        if merged_sha256 != local_sha256 || merge_approval.is_some() {
            if let Some(approval) = merge_approval {
                self.profiles
                    .apply_staged_restore(
                        plugin_id.to_owned(),
                        signer_fingerprint_sha256.to_owned(),
                        profile_id.to_owned(),
                        restore.handle.clone(),
                        Some(approval),
                    )
                    .await
                    .map_err(map_store_error)?;
            } else {
                self.profiles
                    .apply_verified_merge(
                        plugin_id.to_owned(),
                        signer_fingerprint_sha256.to_owned(),
                        profile_id.to_owned(),
                        restore.handle.clone(),
                        VerifiedAutomaticMergeApproval::new(&staged_bundle)?,
                        Arc::clone(fence),
                    )
                    .await
                    .map_err(|error| match error {
                        PortableStoreError::Stale => BrokerError::RetryLocalSnapshot,
                        error => map_store_error(error),
                    })?;
            }
            if !fence() {
                return Err(BrokerError::OperationRejected(
                    "broker.sync_merged_bundle.03",
                ));
            }
            fresh_local = self
                .profiles
                .snapshot_current(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    merged.revision,
                )
                .await
                .map_err(map_store_error)?;
            fresh_local.bundle = self
                .profiles
                .prepare_local_merge(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    merged,
                    fresh_local.bundle,
                )
                .await
                .map_err(map_store_error)?;
            if portable_content_sha256(&fresh_local.bundle, &key)? != merged_sha256 {
                return Err(BrokerError::LocalStateChanged);
            }
        }
        let profile = self
            .profiles
            .profile_state(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?;
        self.push_local(
            plugin_id,
            signer_fingerprint_sha256,
            profile_id,
            fresh_local,
            profile,
            destination,
            Some(rechecked_remote),
            1,
            action_revision,
            browser_binding,
            fence,
        )
        .await
    }

    async fn fetch_exchange(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        source: PluginSshSyncDownloadSource,
        fence: &ActionFence,
    ) -> Result<Option<DownloadedExchange>, BrokerError> {
        self.fetch_exchange_head(
            plugin_id,
            signer_fingerprint_sha256,
            profile_id,
            source,
            fence,
        )
        .await
        .map(|(current, _)| current)
    }

    async fn fetch_exchange_head(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        source: PluginSshSyncDownloadSource,
        fence: &ActionFence,
    ) -> Result<(Option<DownloadedExchange>, u64), BrokerError> {
        let remote_url = sync_url(&source.url)?;
        let token = self
            .token_for(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                source.use_oauth,
                &remote_url,
                fence,
            )
            .await?;
        let downloaded = await_fenced(
            self.download_head(source, token.as_deref().map(String::as_str)),
            fence,
        )
        .await??;
        let Some((http_status, etag, header_revision, remote_updated_at_unix_ms, bytes)) =
            downloaded.data
        else {
            return Ok((None, downloaded.next_revision));
        };
        let etag = etag
            .filter(|value| valid_strong_etag(value))
            .ok_or_else(|| {
                eprintln!("provider exchange response did not include a valid strong ETag");
                BrokerError::RemoteDataInvalid
            })?;
        if is_legacy_exchange_format(&bytes) {
            eprintln!("provider exchange uses an unsupported legacy format");
            return Err(BrokerError::RemoteFormatUnsupported);
        }
        let (binding, _) = inspect_plugin_exchange_data_owner(&bytes, plugin_id, profile_id)
            .map_err(|error| {
                let wire_kind = serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("format")
                            .and_then(serde_json::Value::as_str)
                            .map(|format| match format {
                                "norishell-ssh-vault-exchange-v2" => "legacy-vault-v2".to_owned(),
                                "norishell-ssh-vault-exchange-v3" => "vault-v3".to_owned(),
                                "norishell-ssh-sync-exchange-v2" => "password-v2".to_owned(),
                                _ if format.len() <= 96
                                    && format.bytes().all(|byte| {
                                        byte.is_ascii_alphanumeric()
                                            || matches!(byte, b'-' | b'_' | b'.' | b':')
                                    }) =>
                                {
                                    format.to_owned()
                                }
                                _ => "unknown-json".to_owned(),
                            })
                    })
                    .unwrap_or_else(|| "not-json".to_owned());
                eprintln!("provider exchange ownership validation failed ({wire_kind}): {error}");
                BrokerError::RemoteDataInvalid
            })?;
        if !self.data_owner_sha256.is_empty()
            && binding.signer_fingerprint_sha256 != self.data_owner_sha256
        {
            return Err(BrokerError::StateConflict);
        }
        if header_revision != Some(binding.revision) {
            eprintln!("provider exchange revision header did not match its authenticated body");
            return Err(BrokerError::RemoteDataInvalid);
        }
        Ok((
            Some(DownloadedExchange {
                http_status,
                remote_origin: origin(&remote_url),
                etag,
                remote_updated_at_unix_ms,
                binding,
                bytes,
            }),
            downloaded.next_revision,
        ))
    }

    async fn reconcile_upload_attempt_with_remote(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        remote: &DownloadedExchange,
        expected_fence: &DurableUploadFence,
        key: &SyncKey,
    ) -> Result<(), BrokerError> {
        let Some(attempt) = self
            .profiles
            .pending_upload_attempt(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?
        else {
            return Ok(());
        };
        if !upload_attempt_matches_fence(&attempt, expected_fence) {
            return Err(BrokerError::StateConflict);
        }
        open_plugin_exchange_with_key(&remote.bytes, key, &remote.binding)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        let remote_body_sha256 = sha256_hex(&remote.bytes);
        let Some(proof) = authenticated_upload_completion_proof(
            &attempt,
            remote.binding.revision,
            &remote_body_sha256,
            Some(&remote.etag),
        ) else {
            return Ok(());
        };
        self.profiles
            .complete_upload_attempt(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                attempt.idempotency_key,
                attempt.body_sha256,
                expected_fence.clone(),
                proof,
            )
            .await
            .map_err(map_store_error)
    }

    async fn open_downloaded_exchange(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        remote: &DownloadedExchange,
        mut profile: PortableProfileState,
        fence: &ActionFence,
    ) -> Result<(PortableBundleV1, PortableProfileState, SyncKey), BrokerError> {
        let envelope = plugin_exchange_vault_key_envelope(&remote.bytes, &remote.binding)
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
        if let Some(binding) = &profile.key_binding {
            if binding.password_wrapped_envelope != envelope {
                return Err(BrokerError::KeyBindingConflict);
            }
            let value = self
                .vault
                .read_secret(
                    &binding.secret_ref_id,
                    norishell_secret_vault::SecretKind::SshSyncKey,
                )
                .map_err(|_| BrokerError::LocalKeyUnavailable)?;
            let bytes: [u8; 32] = value
                .expose()
                .try_into()
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            let key = SyncKey::from_bytes(bytes);
            let bundle = open_plugin_exchange_with_key(&remote.bytes, &key, &remote.binding)
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            return Ok((bundle, profile, key));
        }

        // Owner authentication and import share one invocation. Reuse its key
        // only for the exact authenticated ciphertext, never across actions.
        let (bundle, key) = if let Some((digest, key)) = &self.authenticated_remote_key {
            if !fence() || !self.vault.is_unlocked() {
                return Err(BrokerError::RecoveryActionExpired);
            }
            if digest != &sha256_hex(&remote.bytes) {
                return Err(BrokerError::StateConflict);
            }
            let bundle = open_plugin_exchange_with_key(&remote.bytes, key, &remote.binding)
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            (bundle, key.clone())
        } else {
            let local_material = self
                .vault
                .export_sync_key_material()
                .map_err(|_| BrokerError::LocalKeyUnavailable)?;
            let local_key = SyncKey::from_bytes(*local_material.key_bytes());
            match open_plugin_exchange_with_key(&remote.bytes, &local_key, &remote.binding) {
                Ok(bundle) => (bundle, local_key),
                Err(_) => {
                    if self.background {
                        return Err(BrokerError::InteractionRequired);
                    }
                    let password = self
                        .secure_ui
                        .vault_password(self.secure_context(
                            plugin_id,
                            signer_fingerprint_sha256,
                            profile_id,
                            Some(remote.remote_origin.clone()),
                            fence,
                        ))
                        .await
                        .map_err(map_selection_error)?;
                    if !fence() {
                        return Err(BrokerError::RecoveryActionExpired);
                    }
                    let material = self
                        .vault
                        .open_synchronized_key_material(&envelope, password.as_slice())
                        .map_err(map_sync_key_recovery_error)?;
                    let key = SyncKey::from_bytes(*material.key_bytes());
                    let bundle =
                        open_plugin_exchange_with_key(&remote.bytes, &key, &remote.binding)
                            .map_err(|_| BrokerError::RemoteDataInvalid)?;
                    (bundle, key)
                }
            }
        };
        let secret_ref_id = new_secret_ref_id()?;
        profile = self
            .profiles
            .bind_sync_key(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                profile.state_version,
                secret_ref_id,
                key.clone(),
                envelope,
            )
            .await
            .map_err(map_store_error)?;
        Ok((bundle, profile, key))
    }

    async fn sync_key_for_upload(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        mut profile: PortableProfileState,
    ) -> Result<(SyncKey, Vec<u8>, PortableProfileState), BrokerError> {
        if let Some(binding) = &profile.key_binding {
            let value = self
                .vault
                .read_secret(
                    &binding.secret_ref_id,
                    norishell_secret_vault::SecretKind::SshSyncKey,
                )
                .map_err(|_| BrokerError::LocalKeyUnavailable)?;
            let bytes: [u8; 32] = value
                .expose()
                .try_into()
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            return Ok((
                SyncKey::from_bytes(bytes),
                binding.password_wrapped_envelope.clone(),
                profile,
            ));
        }
        let material = self
            .vault
            .export_sync_key_material()
            .map_err(|_| BrokerError::LocalKeyUnavailable)?;
        let key = SyncKey::from_bytes(*material.key_bytes());
        let envelope = material.envelope().to_vec();
        profile = self
            .profiles
            .bind_sync_key(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                profile.state_version,
                new_secret_ref_id()?,
                key.clone(),
                envelope.clone(),
            )
            .await
            .map_err(map_store_error)?;
        Ok((key, envelope, profile))
    }

    async fn open_profile_baseline(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        profile: &PortableProfileState,
        key: &SyncKey,
    ) -> Result<Option<PortableBundleV1>, BrokerError> {
        let Some(baseline) = &profile.remote_baseline else {
            return Ok(None);
        };
        let bytes = self
            .load_baseline_exchange(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                &baseline.exchange_sha256,
            )
            .await?;
        let (binding, _) =
            inspect_plugin_exchange_owner(&bytes, plugin_id, signer_fingerprint_sha256, profile_id)
                .map_err(|_| BrokerError::LocalDataInvalid("open_profile_baseline", "binding"))?;
        if binding.revision != baseline.revision {
            return Err(BrokerError::LocalDataInvalid(
                "open_profile_baseline",
                "revision",
            ));
        }
        let bundle = open_plugin_exchange_with_key(&bytes, key, &binding)
            .map_err(|_| BrokerError::LocalDataInvalid("open_profile_baseline", "decrypt"))?;
        if portable_content_sha256(&bundle, key)? != baseline.content_sha256 {
            return Err(BrokerError::LocalDataInvalid(
                "open_profile_baseline",
                "content_sha256",
            ));
        }
        Ok(Some(bundle))
    }

    #[allow(clippy::too_many_arguments)]
    async fn push_local(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        mut local: PortableSnapshot,
        profile: PortableProfileState,
        mut destination: PluginSshSyncUploadTarget,
        remote: Option<DownloadedExchange>,
        absent_next_revision: u64,
        action_revision: SshSyncActionRevision,
        browser_binding: Option<&SshSyncBrowserCacheBinding>,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let url = sync_url(&destination.url)?;
        let canonical_url = url.as_str().to_owned();
        let completion_fence = DurableUploadFence {
            canonical_url: canonical_url.clone(),
            method: destination.method,
            use_oauth: destination.use_oauth,
            action_revision,
        };
        let (revision, base_revision, base_etag) =
            remote
                .as_ref()
                .map_or((absent_next_revision, None, None), |value| {
                    (
                        value.binding.revision.saturating_add(1),
                        Some(value.binding.revision),
                        Some(value.etag.clone()),
                    )
                });
        if !(1..=MAX_EXCHANGE_REVISION).contains(&revision) {
            return Err(BrokerError::RevisionExhausted);
        }
        local.bundle.revision = revision;
        let binding = exchange_binding(
            plugin_id,
            signer_fingerprint_sha256,
            profile_id,
            revision,
            base_revision,
            base_etag.clone(),
        )?;
        let (key, envelope, profile) = self
            .sync_key_for_upload(plugin_id, signer_fingerprint_sha256, profile_id, profile)
            .await?;
        let local_sha256 = portable_content_sha256(&local.bundle, &key)?;
        let mut pending = self
            .profiles
            .pending_upload_attempt(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
            )
            .await
            .map_err(map_store_error)?;
        if let (Some(attempt), Some(remote)) = (pending.as_ref(), remote.as_ref()) {
            if !upload_attempt_matches_fence(attempt, &completion_fence) {
                return Err(BrokerError::StateConflict);
            }
            open_plugin_exchange_with_key(&remote.bytes, &key, &remote.binding)
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            let remote_body_sha256 = sha256_hex(&remote.bytes);
            let proof = authenticated_upload_completion_proof(
                attempt,
                remote.binding.revision,
                &remote_body_sha256,
                Some(&remote.etag),
            );
            if let Some(proof) = proof {
                self.profiles
                    .complete_upload_attempt(
                        plugin_id.to_owned(),
                        signer_fingerprint_sha256.to_owned(),
                        profile_id.to_owned(),
                        attempt.idempotency_key.clone(),
                        attempt.body_sha256.clone(),
                        completion_fence.clone(),
                        proof,
                    )
                    .await
                    .map_err(map_store_error)?;
                pending = None;
            }
        }
        if let Some(attempt) = pending.as_ref() {
            if !upload_attempt_matches_fence(attempt, &completion_fence) {
                return Err(BrokerError::StateConflict);
            }
            let old_bytes = self
                .load_baseline_exchange(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    &attempt.body_sha256,
                )
                .await?;
            let (old_binding, _) = inspect_plugin_exchange_owner(
                &old_bytes,
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
            )
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
            let old_bundle = open_plugin_exchange_with_key(&old_bytes, &key, &old_binding)
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            if old_binding.revision != attempt.target_revision {
                return Err(BrokerError::RemoteDataInvalid);
            }
            if old_bundle.validate_current_business_exchange().is_err() {
                let proof = if attempt.state == PortableUploadAttemptState::Prepared {
                    PortableUploadAbandonProof::PreparedNotSent
                } else {
                    PortableUploadAbandonProof::AuthenticatedRemoteAtBase {
                        remote_revision: remote.as_ref().map_or(0, |value| value.binding.revision),
                        remote_etag: remote.as_ref().map(|value| value.etag.clone()),
                        remote_body_sha256: remote.as_ref().map(|value| sha256_hex(&value.bytes)),
                    }
                };
                self.profiles
                    .abandon_legacy_upload_attempt(
                        plugin_id.to_owned(),
                        signer_fingerprint_sha256.to_owned(),
                        profile_id.to_owned(),
                        attempt.idempotency_key.clone(),
                        attempt.body_sha256.clone(),
                        attempt.state_version,
                        completion_fence.clone(),
                        proof,
                    )
                    .await
                    .map_err(map_store_error)?;
                pending = None;
            }
        }
        let expected_base_revision = base_revision.unwrap_or(0);
        let (bytes, mut attempt) = if let Some(attempt) = pending {
            if attempt.canonical_url != canonical_url
                || attempt.method != destination.method
                || attempt.use_oauth != destination.use_oauth
                || attempt.authorization_revision != action_revision.authorization
                || attempt.configuration_revision != action_revision.configuration
                || attempt.target_revision != revision
                || attempt.base_revision != expected_base_revision
                || attempt.base_etag != base_etag
                || attempt.keyed_content_sha256 != local_sha256
            {
                return Err(BrokerError::StateConflict);
            }
            let bytes = self
                .load_baseline_exchange(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    &attempt.body_sha256,
                )
                .await?;
            let (stored_binding, _) = inspect_plugin_exchange_owner(
                &bytes,
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
            )
            .map_err(|_| BrokerError::RemoteDataInvalid)?;
            let stored_bundle = open_plugin_exchange_with_key(&bytes, &key, &stored_binding)
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
            if stored_binding != binding
                || portable_content_sha256(&stored_bundle, &key)? != local_sha256
            {
                return Err(BrokerError::RemoteDataInvalid);
            }
            (bytes, attempt)
        } else {
            let bytes = create_plugin_exchange_with_key(&local.bundle, &key, &envelope, &binding)
                .map_err(|_| BrokerError::Internal("broker.push_local.internal01"))?;
            let body_sha256 = self
                .persist_baseline_exchange(plugin_id, signer_fingerprint_sha256, profile_id, &bytes)
                .await?;
            let attempt = self
                .profiles
                .ensure_upload_attempt(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    canonical_url,
                    destination.method,
                    destination.use_oauth,
                    action_revision,
                    expected_base_revision,
                    base_etag.clone(),
                    revision,
                    local_sha256.clone(),
                    body_sha256,
                    Uuid::now_v7().to_string(),
                )
                .await
                .map_err(map_store_error)?;
            (bytes, attempt)
        };
        let persisted_exchange_sha256 = attempt.body_sha256.clone();
        destination.if_match = base_etag;
        let token = self
            .token_for(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                destination.use_oauth,
                &url,
                fence,
            )
            .await?;
        if attempt.state == PortableUploadAttemptState::Prepared {
            attempt = self
                .profiles
                .advance_upload_attempt(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    attempt.state_version,
                    PortableUploadAttemptState::Sent,
                )
                .await
                .map_err(map_store_error)?;
        }
        let idempotency_key = attempt.idempotency_key.clone();
        let first_upload = await_fenced(
            self.upload(
                destination.clone(),
                token.as_deref().map(String::as_str),
                bytes.clone(),
                &idempotency_key,
                Some(attempt.target_revision),
            ),
            fence,
        )
        .await?;
        let (http_status, etag, acknowledged_revision) = match first_upload {
            Ok(result) => result,
            Err(BrokerError::HttpFailure(409 | 412)) if remote.is_some() => {
                let current = await_fenced(
                    self.download_optional(
                        PluginSshSyncDownloadSource {
                            url: destination.url.clone(),
                            use_oauth: destination.use_oauth,
                        },
                        token.as_deref().map(String::as_str),
                    ),
                    fence,
                )
                .await??
                .ok_or(BrokerError::StateConflict)?;
                let (
                    current_http_status,
                    current_etag,
                    current_revision,
                    current_updated_at_unix_ms,
                    current_bytes,
                ) = current;
                let current_etag = current_etag
                    .filter(|value| valid_strong_etag(value))
                    .ok_or(BrokerError::RemoteDataInvalid)?;
                let prior = remote.as_ref().expect("guarded by remote.is_some");
                let (current_binding, _) = inspect_plugin_exchange_owner(
                    &current_bytes,
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                )
                .map_err(|_| BrokerError::RemoteDataInvalid)?;
                if current_revision != Some(current_binding.revision) {
                    return Err(BrokerError::RemoteDataInvalid);
                }
                if current_binding.revision != prior.binding.revision
                    || current_bytes != prior.bytes
                {
                    self.reconcile_upload_attempt_with_remote(
                        plugin_id,
                        signer_fingerprint_sha256,
                        profile_id,
                        &DownloadedExchange {
                            http_status: current_http_status,
                            remote_origin: origin(&url),
                            etag: current_etag,
                            remote_updated_at_unix_ms: current_updated_at_unix_ms,
                            binding: current_binding,
                            bytes: current_bytes,
                        },
                        &completion_fence,
                        &key,
                    )
                    .await?;
                    return Err(BrokerError::StateConflict);
                }
                destination.if_match = Some(current_etag);
                await_fenced(
                    self.upload(
                        destination,
                        token.as_deref().map(String::as_str),
                        bytes.clone(),
                        &idempotency_key,
                        Some(attempt.target_revision),
                    ),
                    fence,
                )
                .await??
            }
            Err(error) => return Err(error),
        };
        if acknowledged_revision != Some(revision) {
            return Err(BrokerError::RemoteDataInvalid);
        }
        let etag = etag
            .filter(|value| valid_strong_etag(value))
            .ok_or(BrokerError::RemoteDataInvalid)?;
        let profile = self
            .profiles
            .update_remote_baseline(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                profile.state_version,
                PortableRemoteBaseline {
                    revision,
                    etag: etag.clone(),
                    content_sha256: local_sha256,
                    exchange_sha256: persisted_exchange_sha256,
                },
            )
            .await
            .map_err(map_store_error)?;
        let profile = self
            .profiles
            .mark_sync_succeeded(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                profile.state_version,
            )
            .await
            .map_err(map_store_error)?;
        self.profiles
            .complete_upload_attempt(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                attempt.idempotency_key,
                attempt.body_sha256,
                completion_fence,
                PortableUploadCompletionProof::ServerAcknowledged {
                    acknowledged_revision: revision,
                },
            )
            .await
            .map_err(map_store_error)?;
        if let Some(binding) = browser_binding {
            self.publish_browser_snapshot(binding, &local.bundle, None, fence);
        }
        self.remember_browser_exchange(browser_binding, Some(&bytes), None, fence)
            .await;
        Ok(aggregate_status(
            profile_id,
            self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
            PluginSshSyncOperationState::Succeeded,
            &local,
            Some((
                local.host_count,
                local.credential_count,
                local.desktop_profile_count,
            )),
            PluginSshSyncDifferenceState::Equal,
            profile.scope_mode,
            profile.last_successful_sync_at_unix_ms,
            Some(http_status),
            Some(revision),
            Some(etag),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_absent_remote(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        preview: RestorePreview,
        empty_remote: PortableBundleV1,
        profile: PortableProfileState,
        key: SyncKey,
        approval: SecureApplyApproval,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        if preview.conflict_count > 0 {
            return Err(BrokerError::RestoreConflict);
        }
        if !fence() {
            return Err(BrokerError::OperationRejected(
                "broker.apply_absent_remote.01",
            ));
        }
        self.profiles
            .apply_staged_restore(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                preview.handle,
                Some(approval),
            )
            .await
            .map_err(map_store_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected(
                "broker.apply_absent_remote.02",
            ));
        }
        let mut local = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                empty_remote.revision,
            )
            .await
            .map_err(map_store_error)?;
        local.bundle = self
            .profiles
            .prepare_local_merge(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                empty_remote.clone(),
                local.bundle,
            )
            .await
            .map_err(map_store_error)?;
        if portable_content_sha256(&local.bundle, &key)?
            != portable_content_sha256(&empty_remote, &key)?
        {
            return Err(BrokerError::LocalStateChanged);
        }
        let profile = self
            .profiles
            .mark_sync_succeeded(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                profile.state_version,
            )
            .await
            .map_err(map_store_error)?;
        Ok(aggregate_status(
            profile_id,
            self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
            PluginSshSyncOperationState::Succeeded,
            &local,
            Some((0, 0, 0)),
            PluginSshSyncDifferenceState::Equal,
            profile.scope_mode,
            profile.last_successful_sync_at_unix_ms,
            Some(StatusCode::NOT_FOUND.as_u16()),
            None,
            None,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn pull_remote(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        remote: DownloadedExchange,
        remote_bundle: PortableBundleV1,
        preview: RestorePreview,
        profile: PortableProfileState,
        key: SyncKey,
        approval: SecureApplyApproval,
        migration_needed: bool,
        destination: PluginSshSyncUploadTarget,
        action_revision: SshSyncActionRevision,
        browser_binding: Option<&SshSyncBrowserCacheBinding>,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.pull_remote.01"));
        }
        if preview.conflict_count > 0 {
            let local = self
                .profiles
                .snapshot_current(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                    1,
                )
                .await
                .map_err(map_store_error)?;
            return Ok(restore_conflict_status(
                aggregate_status(
                    profile_id,
                    self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
                    PluginSshSyncOperationState::Failed,
                    &local,
                    Some((
                        preview.host_count,
                        preview.credential_count,
                        preview.desktop_profile_count,
                    )),
                    PluginSshSyncDifferenceState::Conflict,
                    profile.scope_mode,
                    profile.last_successful_sync_at_unix_ms,
                    Some(remote.http_status),
                    Some(remote.binding.revision),
                    Some(remote.etag),
                ),
                preview.conflict_count,
            ));
        }
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.pull_remote.02"));
        }
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.pull_remote.03"));
        }
        self.profiles
            .apply_staged_restore(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                preview.handle,
                Some(approval),
            )
            .await
            .map_err(map_store_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.pull_remote.04"));
        }
        let mut local = self
            .profiles
            .snapshot_current(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                1,
            )
            .await
            .map_err(map_store_error)?;
        local.bundle = self
            .profiles
            .prepare_local_merge(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                remote_bundle.clone(),
                local.bundle,
            )
            .await
            .map_err(map_store_error)?;
        let local_sha256 = portable_content_sha256(&local.bundle, &key)?;
        if local_sha256 != portable_content_sha256(&remote_bundle, &key)? {
            return Err(BrokerError::LocalStateChanged);
        }
        if migration_needed {
            let profile = self
                .profiles
                .profile_state(
                    plugin_id.to_owned(),
                    signer_fingerprint_sha256.to_owned(),
                    profile_id.to_owned(),
                )
                .await
                .map_err(map_store_error)?;
            return self
                .push_local(
                    plugin_id,
                    signer_fingerprint_sha256,
                    profile_id,
                    local,
                    profile,
                    destination,
                    Some(remote),
                    1,
                    action_revision,
                    browser_binding,
                    fence,
                )
                .await;
        }
        let exchange_sha256 = self
            .persist_baseline_exchange(
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
                &remote.bytes,
            )
            .await?;
        let profile = self
            .profiles
            .update_remote_baseline(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                profile.state_version,
                PortableRemoteBaseline {
                    revision: remote.binding.revision,
                    etag: remote.etag.clone(),
                    content_sha256: local_sha256,
                    exchange_sha256,
                },
            )
            .await
            .map_err(map_store_error)?;
        let profile = self
            .profiles
            .mark_sync_succeeded(
                plugin_id.to_owned(),
                signer_fingerprint_sha256.to_owned(),
                profile_id.to_owned(),
                profile.state_version,
            )
            .await
            .map_err(map_store_error)?;
        Ok(aggregate_status(
            profile_id,
            self.account_state(plugin_id, signer_fingerprint_sha256, profile_id),
            PluginSshSyncOperationState::Succeeded,
            &local,
            Some((
                preview.host_count,
                preview.credential_count,
                preview.desktop_profile_count,
            )),
            PluginSshSyncDifferenceState::Equal,
            profile.scope_mode,
            profile.last_successful_sync_at_unix_ms,
            Some(remote.http_status),
            Some(remote.binding.revision),
            Some(remote.etag),
        ))
    }

    async fn logout(
        &self,
        namespace: &str,
        profile_id: &str,
        fence: &ActionFence,
    ) -> Result<PluginSshSyncStatus, BrokerError> {
        let service = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .get(namespace)
            .cloned()
            .ok_or(BrokerError::AuthorizationExpired)?;
        await_fenced(service.logout(), fence)
            .await?
            .map_err(map_oauth_error)?;
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.logout.01"));
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .remove(namespace);
        Ok(idle_status(
            profile_id.to_owned(),
            PluginSshSyncAccountState::Disconnected,
        ))
    }

    async fn token_for(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        required: bool,
        target_url: &Url,
        fence: &ActionFence,
    ) -> Result<Option<zeroize::Zeroizing<String>>, BrokerError> {
        if !required {
            return Ok(None);
        }
        let package_signer = if self.package_signer_sha256.is_empty() {
            signer_fingerprint_sha256
        } else {
            &self.package_signer_sha256
        };
        let service = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .get(&namespace(plugin_id, package_signer, profile_id))
            .cloned()
            .ok_or(BrokerError::AuthorizationExpired)?;
        if !service.allows_resource_url(target_url) {
            return Err(BrokerError::OperationRejected("broker.token_for.01"));
        }
        await_fenced(service.access_token(), fence)
            .await?
            .map(Some)
            .map_err(map_oauth_error)
    }

    /// Issues a secret-bearing lease only to the Core network driver. The
    /// origin must be an exact configured OAuth resource origin, and the
    /// returned fence is rechecked when the HTTP request is sent.
    pub(crate) async fn oauth_network_lease(
        &self,
        plugin_id: &str,
        signer: &str,
        profile_id: &str,
        requested_origin: &str,
        fence: ActionFence,
    ) -> Result<CredentialLease, PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let origin = normalized_resource_origin(requested_origin)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let url = Url::parse(&origin).map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let package_signer = if self.package_signer_sha256.is_empty() {
            signer
        } else {
            if signer != self.package_signer_sha256 {
                return Err(PluginApiErrorCode::PermissionDenied);
            }
            &self.package_signer_sha256
        };
        let service = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .get(&namespace(plugin_id, package_signer, profile_id))
            .cloned()
            .ok_or(PluginApiErrorCode::NotFound)?;
        if !service.allows_resource_url(&url) {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        let token = await_fenced(service.access_token(), &fence)
            .await
            .map_err(|_| PluginApiErrorCode::Revoked)?
            .map_err(|error| match error {
                NativeAuthError::VaultLocked => PluginApiErrorCode::VaultLocked,
                NativeAuthError::SessionMissing => PluginApiErrorCode::AccountNotConnected,
                NativeAuthError::RefreshExpired => PluginApiErrorCode::AuthorizationExpired,
                NativeAuthError::OperationInProgress => PluginApiErrorCode::Busy,
                NativeAuthError::AccessDenied => PluginApiErrorCode::PermissionDenied,
                NativeAuthError::Network => PluginApiErrorCode::NetworkUnavailable,
                NativeAuthError::Protocol | NativeAuthError::StateUnavailable => {
                    PluginApiErrorCode::Conflict
                }
                NativeAuthError::LocalCommit => PluginApiErrorCode::OutcomeUnknown,
            })?;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let mut value = Zeroizing::new(b"Bearer ".to_vec());
        value.extend_from_slice(token.as_bytes());
        Ok(CredentialLease {
            header_name: "Authorization".to_owned(),
            header_value: value,
            fence,
        })
    }

    async fn upload(
        &self,
        target: PluginSshSyncUploadTarget,
        token: Option<&str>,
        body: Vec<u8>,
        idempotency_key: &str,
        expected_next_revision: Option<u64>,
    ) -> Result<(u16, Option<String>, Option<u64>), BrokerError> {
        if Uuid::parse_str(idempotency_key).is_err() {
            return Err(BrokerError::OperationRejected("broker.upload.01"));
        }
        let url = sync_url(&target.url)?;
        let mut request = match target.method {
            PluginSshSyncHttpMethod::Post => self.http.post(url),
            PluginSshSyncHttpMethod::Put => self.http.put(url),
        }
        .header(CONTENT_TYPE, EXCHANGE_CONTENT_TYPE)
        .header("Idempotency-Key", idempotency_key)
        .body(body);
        if target.if_match.is_none()
            && let Some(revision) = expected_next_revision
        {
            if !(1..=MAX_EXCHANGE_REVISION).contains(&revision) {
                return Err(BrokerError::OperationRejected("broker.upload.02"));
            }
            request = request.header("X-NoriShell-Expected-Next-Revision", revision.to_string());
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(etag) = target.if_match {
            if !valid_etag(&etag) {
                return Err(BrokerError::OperationRejected("broker.upload.03"));
            }
            request = request.header(IF_MATCH, etag);
        }
        let response = request
            .send()
            .await
            .map_err(|_| BrokerError::NetworkUnavailable)?;
        let status = response.status();
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_etag(value))
            .map(str::to_owned);
        let remote_revision = response
            .headers()
            .get("x-norishell-revision")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0);
        let _ = read_bounded(response, MAX_ACK_BYTES).await?;
        if !status.is_success() {
            return Err(map_http_status(status));
        }
        Ok((status.as_u16(), etag, remote_revision))
    }

    async fn delete_remote(
        &self,
        target: PluginSshSyncDeleteTarget,
        token: Option<&str>,
        etag: &str,
        idempotency_key: &str,
    ) -> Result<u16, BrokerError> {
        if Uuid::parse_str(idempotency_key).is_err() || !valid_strong_etag(etag) {
            return Err(BrokerError::OperationRejected("broker.delete_remote.01"));
        }
        let url = sync_url(&target.url)?;
        for attempt in 0..2 {
            let mut request = self
                .http
                .delete(url.clone())
                .header(IF_MATCH, etag)
                .header("Idempotency-Key", idempotency_key);
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(_) if attempt == 0 => continue,
                Err(_) => return Err(BrokerError::NetworkUnavailable),
            };
            let status = response.status();
            let body = read_bounded(response, MAX_ACK_BYTES).await?;
            return validate_remote_delete_response(status, &body);
        }
        Err(BrokerError::NetworkUnavailable)
    }

    async fn download_optional(
        &self,
        source: PluginSshSyncDownloadSource,
        token: Option<&str>,
    ) -> Result<Option<HttpExchangeData>, BrokerError> {
        self.download_head(source, token)
            .await
            .map(|head| head.data)
    }

    async fn download_head(
        &self,
        source: PluginSshSyncDownloadSource,
        token: Option<&str>,
    ) -> Result<HttpExchangeHead, BrokerError> {
        let url = sync_url(&source.url)?;
        let mut request = self.http.get(url);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|_| BrokerError::NetworkUnavailable)?;
        let status = response.status();
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_etag(value))
            .map(str::to_owned);
        let remote_revision = response
            .headers()
            .get("x-norishell-revision")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0);
        let remote_updated_at_unix_ms = response
            .headers()
            .get("x-norishell-updated-at-unix-ms")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| (1..=253_402_300_799_000).contains(value));
        if status == StatusCode::NOT_FOUND {
            let next_revision =
                parse_next_revision(response.headers().get("x-norishell-next-revision"))?;
            let _ = read_bounded(response, MAX_ACK_BYTES).await;
            return Ok(HttpExchangeHead {
                data: None,
                next_revision,
            });
        }
        if !status.is_success() {
            let _ = read_bounded(response, MAX_ACK_BYTES).await;
            return Err(map_http_status(status));
        }
        let bytes = read_bounded(response, MAX_DOWNLOAD_BYTES).await?;
        Ok(HttpExchangeHead {
            data: Some((
                status.as_u16(),
                etag,
                remote_revision,
                remote_updated_at_unix_ms,
                bytes,
            )),
            next_revision: 1,
        })
    }

    fn account_state(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
    ) -> PluginSshSyncAccountState {
        let package_signer = if self.package_signer_sha256.is_empty() {
            signer_fingerprint_sha256
        } else {
            &self.package_signer_sha256
        };
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .oauth
            .get(&namespace(plugin_id, package_signer, profile_id))
            .map_or(PluginSshSyncAccountState::Disconnected, |oauth| {
                account_state(oauth.status().account_state)
            })
    }
}

fn reuse_or_restore_credential_service(
    oauth: &mut BTreeMap<String, PluginOAuthService>,
    key: &str,
    app_data_directory: &Path,
    vault: &crate::vault_service::VaultService,
    configuration: PluginOAuthConfiguration,
) -> Result<bool, NativeAuthError> {
    if oauth
        .get(key)
        .is_some_and(|service| service.matches_configuration(&configuration))
    {
        return Ok(false);
    }
    let service = PluginOAuthService::start(app_data_directory, vault.clone(), configuration)?;
    oauth.insert(key.to_owned(), service);
    Ok(true)
}

fn baseline_exchange_path(
    root: &Path,
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
    exchange_sha256: &str,
) -> Result<PathBuf, BrokerError> {
    if !valid_identifier(plugin_id, 160)
        || !valid_sha256(signer_fingerprint_sha256)
        || !valid_identifier(profile_id, 160)
        || !valid_sha256(exchange_sha256)
    {
        return Err(BrokerError::OperationRejected(
            "broker.baseline_exchange_path.01",
        ));
    }
    let plugin_hash = sha256_hex(plugin_id.as_bytes());
    let owner_hash =
        sha256_hex(format!("{plugin_id}\0{signer_fingerprint_sha256}\0{profile_id}").as_bytes());
    Ok(root
        .join("ssh-sync-baselines")
        .join(plugin_hash)
        .join(format!("{owner_hash}-{exchange_sha256}.exchange")))
}

fn persist_baseline_exchange_file(
    root: &Path,
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
    bytes: &[u8],
) -> Result<String, BrokerError> {
    if bytes.is_empty() || bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(BrokerError::RemoteDataInvalid);
    }
    let exchange_sha256 = sha256_hex(bytes);
    let path = baseline_exchange_path(
        root,
        plugin_id,
        signer_fingerprint_sha256,
        profile_id,
        &exchange_sha256,
    )?;
    let parent = path.parent().ok_or(BrokerError::Internal(
        "broker.persist_baseline_exchange_file.internal01",
    ))?;
    fs::create_dir_all(parent)
        .map_err(|_| BrokerError::Internal("broker.persist_baseline_exchange_file.internal02"))?;
    set_private_directory_permissions(parent)?;
    if path.exists() {
        let existing = read_baseline_file(&path)?;
        return if existing == bytes {
            Ok(exchange_sha256)
        } else {
            Err(BrokerError::RemoteDataInvalid)
        };
    }
    let temporary = parent.join(format!(".{}.staging", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let write_result = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(BrokerError::Internal(
            "broker.persist_baseline_exchange_file.internal03",
        ));
    }
    match fs::rename(&temporary, &path) {
        Ok(()) => {}
        Err(_) if path.exists() => {
            let _ = fs::remove_file(&temporary);
            if read_baseline_file(&path)? != bytes {
                return Err(BrokerError::RemoteDataInvalid);
            }
        }
        Err(_) => {
            let _ = fs::remove_file(&temporary);
            return Err(BrokerError::Internal(
                "broker.persist_baseline_exchange_file.internal04",
            ));
        }
    }
    sync_directory(parent)?;
    Ok(exchange_sha256)
}

fn remove_plugin_baseline_files(root: &Path, plugin_id: &str) -> Result<(), BrokerError> {
    if !valid_identifier(plugin_id, 160) {
        return Err(BrokerError::OperationRejected(
            "broker.remove_plugin_baseline_files.01",
        ));
    }
    let directory = root
        .join("ssh-sync-baselines")
        .join(sha256_hex(plugin_id.as_bytes()));
    match fs::remove_dir_all(&directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(BrokerError::Internal(
            "broker.remove_plugin_baseline_files.internal01",
        )),
    }
}

fn remove_profile_baseline_files(
    root: &Path,
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
) -> Result<(), BrokerError> {
    if !valid_identifier(plugin_id, 160)
        || !valid_sha256(signer_fingerprint_sha256)
        || !valid_identifier(profile_id, 160)
    {
        return Err(BrokerError::OperationRejected(
            "broker.remove_profile_baseline_files.01",
        ));
    }
    let directory = root
        .join("ssh-sync-baselines")
        .join(sha256_hex(plugin_id.as_bytes()));
    let prefix = format!(
        "{}-",
        sha256_hex(format!("{plugin_id}\0{signer_fingerprint_sha256}\0{profile_id}").as_bytes())
    );
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(BrokerError::Internal(
                "broker.remove_profile_baseline_files.internal01",
            ));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|_| {
            BrokerError::Internal("broker.remove_profile_baseline_files.internal02")
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".exchange") {
            continue;
        }
        let file_type = entry.file_type().map_err(|_| {
            BrokerError::Internal("broker.remove_profile_baseline_files.internal03")
        })?;
        if !(file_type.is_file() || file_type.is_symlink()) {
            return Err(BrokerError::Internal(
                "broker.remove_profile_baseline_files.internal04",
            ));
        }
        fs::remove_file(entry.path()).map_err(|_| {
            BrokerError::Internal("broker.remove_profile_baseline_files.internal05")
        })?;
    }
    sync_directory(&directory)
}

fn load_baseline_exchange_file(
    root: &Path,
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
    exchange_sha256: &str,
) -> Result<Vec<u8>, BrokerError> {
    let path = baseline_exchange_path(
        root,
        plugin_id,
        signer_fingerprint_sha256,
        profile_id,
        exchange_sha256,
    )?;
    let bytes = read_baseline_file(&path)?;
    if sha256_hex(&bytes) != exchange_sha256 {
        return Err(BrokerError::LocalDataInvalid(
            "load_baseline_exchange",
            "exchange_sha256",
        ));
    }
    Ok(bytes)
}

fn read_baseline_file(path: &Path) -> Result<Vec<u8>, BrokerError> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| BrokerError::Internal("broker.read_baseline_file.internal01"))?;
    let metadata = file
        .metadata()
        .map_err(|_| BrokerError::Internal("broker.read_baseline_file.internal02"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_DOWNLOAD_BYTES as u64 {
        return Err(BrokerError::LocalDataInvalid(
            "read_baseline_file",
            "file_shape",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_DOWNLOAD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BrokerError::Internal("broker.read_baseline_file.internal03"))?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(BrokerError::LocalDataInvalid("read_baseline_file", "size"));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), BrokerError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| BrokerError::Internal("broker.set_private_directory_permissions.internal01"))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), BrokerError> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), BrokerError> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| BrokerError::Internal("broker.sync_directory.internal01"))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), BrokerError> {
    Ok(())
}

async fn read_bounded(
    mut response: reqwest::Response,
    maximum: usize,
) -> Result<Vec<u8>, BrokerError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(BrokerError::RemoteDataInvalid);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| BrokerError::NetworkUnavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(BrokerError::RemoteDataInvalid);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn await_fenced<F, T>(future: F, fence: &ActionFence) -> Result<T, BrokerError>
where
    F: Future<Output = T>,
{
    tokio::pin!(future);
    loop {
        if !fence() {
            return Err(BrokerError::OperationRejected("broker.await_fenced.01"));
        }
        tokio::select! {
            result = &mut future => return Ok(result),
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if !fence() {
                    return Err(BrokerError::OperationRejected("broker.await_fenced.02"));
                }
            }
        }
    }
}

fn credential_configuration(
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
    profile: PluginSshSyncCredentialProfile,
) -> Result<PluginOAuthConfiguration, BrokerError> {
    let mut resource_origins = profile
        .resource_origins
        .iter()
        .map(|value| normalized_resource_origin(value))
        .collect::<Result<Vec<_>, _>>()?;
    resource_origins.sort();
    resource_origins.dedup();
    if resource_origins.is_empty() || resource_origins.len() > 16 {
        return Err(BrokerError::OperationRejected(
            "broker.credential_configuration.01",
        ));
    }
    let login_url = sync_url(&profile.login_url)?;
    Ok(PluginOAuthConfiguration {
        plugin_id: plugin_id.to_owned(),
        signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
        profile_id: profile_id.to_owned(),
        authorization_url: login_url.clone(),
        login_url: Some(login_url),
        registration_url: Some(sync_url(&profile.registration_url)?),
        email_verification_url: Some(sync_url(&profile.email_verification_url)?),
        mfa_url: Some(sync_url(&profile.mfa_url)?),
        token_url: sync_url(&profile.token_url)?,
        revoke_url: sync_url(&profile.revoke_url)?,
        client_id: profile.client_id,
        scopes: profile.scopes,
        resource_origins,
    })
}

fn credential_field<'a>(
    fields: &'a [PluginUiFieldValue],
    field_id: &PluginUiFieldId,
) -> Result<&'a str, BrokerError> {
    let mut matches = fields
        .iter()
        .filter(|field| field.field_id == *field_id)
        .map(|field| field.value.as_str());
    let value = matches
        .next()
        .ok_or(BrokerError::OperationRejected("broker.credential_field.01"))?;
    if matches.next().is_some() {
        return Err(BrokerError::OperationRejected("broker.credential_field.02"));
    }
    Ok(value)
}

fn credential_text(
    fields: &[PluginUiFieldValue],
    field_id: &PluginUiFieldId,
    maximum: usize,
) -> Result<Zeroizing<String>, BrokerError> {
    let value = credential_field(fields, field_id)?.trim();
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(BrokerError::OperationRejected("broker.credential_text.01"));
    }
    Ok(Zeroizing::new(value.to_owned()))
}

fn credential_password(
    fields: &[PluginUiFieldValue],
    field_id: &PluginUiFieldId,
) -> Result<Zeroizing<String>, BrokerError> {
    let value = credential_field(fields, field_id)?;
    if !(8..=1_024).contains(&value.len()) || value.contains('\0') {
        return Err(BrokerError::OperationRejected(
            "broker.credential_password.01",
        ));
    }
    Ok(Zeroizing::new(value.to_owned()))
}

fn credential_code(
    fields: &[PluginUiFieldValue],
    field_id: &PluginUiFieldId,
) -> Result<Zeroizing<String>, BrokerError> {
    let value = credential_field(fields, field_id)?.trim();
    if !(4..=16).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(BrokerError::OperationRejected("broker.credential_code.01"));
    }
    Ok(Zeroizing::new(value.to_owned()))
}

fn exchange_binding(
    plugin_id: &str,
    signer_fingerprint_sha256: &str,
    profile_id: &str,
    revision: u64,
    base_revision: Option<u64>,
    base_etag: Option<String>,
) -> Result<PluginExchangeBinding, BrokerError> {
    let binding = PluginExchangeBinding {
        plugin_id: plugin_id.to_owned(),
        signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
        profile_id: profile_id.to_owned(),
        revision,
        base_revision,
        base_etag,
    };
    binding
        .validate()
        .map_err(|_| BrokerError::OperationRejected("broker.exchange_binding.01"))?;
    Ok(binding)
}

fn sync_url(value: &str) -> Result<Url, BrokerError> {
    let url =
        Url::parse(value).map_err(|_| BrokerError::OperationRejected("broker.sync_url.01"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(BrokerError::OperationRejected("broker.sync_url.02"));
    }
    Ok(url)
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_etag(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1_024 && !value.chars().any(char::is_control)
}

fn map_http_status(status: StatusCode) -> BrokerError {
    BrokerError::HttpFailure(status.as_u16())
}

fn validate_remote_delete_response(status: StatusCode, body: &[u8]) -> Result<u16, BrokerError> {
    match status {
        StatusCode::NO_CONTENT if body.is_empty() => Ok(status.as_u16()),
        StatusCode::NOT_FOUND => Ok(status.as_u16()),
        status if status.is_success() => Err(BrokerError::RemoteDataInvalid),
        status => Err(map_http_status(status)),
    }
}

fn validate_remote_still_absent<T>(remote: Option<&T>) -> Result<(), BrokerError> {
    if remote.is_some() {
        Err(BrokerError::StateConflict)
    } else {
        Ok(())
    }
}

fn parse_next_revision(header: Option<&reqwest::header::HeaderValue>) -> Result<u64, BrokerError> {
    let Some(header) = header else {
        return Ok(1);
    };
    let value = header
        .to_str()
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BrokerError::RemoteDataInvalid);
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|revision| (1..=MAX_EXCHANGE_REVISION).contains(revision))
        .ok_or(BrokerError::RemoteDataInvalid)
}

fn is_legacy_exchange_format(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.get("format").and_then(serde_json::Value::as_str),
                Some("norishell-ssh-sync-exchange-v1" | "norishell-ssh-vault-exchange-v2")
            )
        })
}

fn ensure_same_remote(
    expected: &DownloadedExchange,
    actual: &DownloadedExchange,
) -> Result<(), BrokerError> {
    if expected.remote_origin != actual.remote_origin
        || expected.etag != actual.etag
        || expected.binding.revision != actual.binding.revision
        || sha256_hex(&expected.bytes) != sha256_hex(&actual.bytes)
    {
        Err(BrokerError::StateConflict)
    } else {
        Ok(())
    }
}

fn empty_portable_bundle(revision: u64) -> PortableBundleV1 {
    PortableBundleV1 {
        schema: BundleSchema::V6,
        revision,
        selected_categories: None,
        objects: PortableObjects::default(),
        preferences: None,
        secrets: Vec::new(),
        skipped_machine_bound: Vec::new(),
        tombstones: Vec::new(),
        update_times: Vec::new(),
        preference_update_times: Default::default(),
    }
}

fn valid_strong_etag(value: &str) -> bool {
    valid_etag(value)
        && !value.starts_with("W/")
        && value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
}

fn new_secret_ref_id() -> Result<SecretRefId, BrokerError> {
    SecretRefId::parse(Uuid::new_v4().to_string())
        .map_err(|_| BrokerError::Internal("broker.new_secret_ref_id.internal01"))
}

fn portable_content_sha256(
    bundle: &PortableBundleV1,
    key: &SyncKey,
) -> Result<String, BrokerError> {
    let mut normalized = bundle.clone();
    normalized.revision = 1;
    normalized.skipped_machine_bound.clear();
    canonical_bundle_bytes(&normalized)
        .map(|bytes| {
            key.baseline_digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        })
        .map_err(|_| BrokerError::Internal("broker.portable_content_sha256.internal01"))
}

fn business_only_bundle(mut bundle: PortableBundleV1) -> Result<PortableBundleV1, BrokerError> {
    // Authenticate legacy bundles before calling this projection. V6 keeps the
    // SSH and desktop object graph, deletion history, and per-item clocks.
    bundle.schema = BundleSchema::V6;
    bundle.preferences = None;
    bundle.preference_update_times.clear();
    bundle
        .validate_current_business_exchange()
        .map_err(|_| BrokerError::RemoteDataInvalid)?;
    Ok(bundle)
}

fn profile_with_business_baseline(
    profile: &PortableProfileState,
    base: Option<&PortableBundleV1>,
    key: &SyncKey,
) -> Result<PortableProfileState, BrokerError> {
    let mut logical = profile.clone();
    if let (Some(baseline), Some(base)) = (&mut logical.remote_baseline, base) {
        baseline.content_sha256 = portable_content_sha256(base, key)?;
    }
    Ok(logical)
}

fn remote_only_historical_tombstones(
    local: &PortableBundleV1,
    remote: &PortableBundleV1,
    key: &SyncKey,
) -> Result<bool, BrokerError> {
    let local_tombstones = local
        .tombstones
        .iter()
        .map(|value| (value.kind, value.id))
        .collect::<BTreeSet<_>>();
    let remote_tombstones = remote
        .tombstones
        .iter()
        .map(|value| (value.kind, value.id))
        .collect::<BTreeSet<_>>();
    if !local_tombstones.is_subset(&remote_tombstones) {
        return Ok(false);
    }
    let added = remote_tombstones
        .difference(&local_tombstones)
        .copied()
        .collect::<BTreeSet<_>>();
    if added.is_empty() {
        return Ok(false);
    }
    let local_objects = difference::object_keys(local);
    let remote_objects = difference::object_keys(remote);
    if added
        .iter()
        .any(|item| local_objects.contains(item) || remote_objects.contains(item))
    {
        return Ok(false);
    }

    // Accept only authenticated deletion history. Every other byte of the
    // canonical portable content, including live objects and their times,
    // must still match the local snapshot before advancing the baseline.
    let mut without_history = remote.clone();
    without_history
        .tombstones
        .retain(|value| !added.contains(&(value.kind, value.id)));
    without_history
        .update_times
        .retain(|value| !added.contains(&(value.kind, value.id)));
    Ok(portable_content_sha256(&without_history, key)? == portable_content_sha256(local, key)?)
}

fn verify_remote_baseline_invariants(
    profile: &PortableProfileState,
    revision: u64,
    etag: &str,
    content_sha256: &str,
    exchange_bytes: &[u8],
) -> Result<(), BrokerError> {
    let Some(baseline) = &profile.remote_baseline else {
        return Ok(());
    };
    if baseline.revision == revision
        && baseline.etag == etag
        && (baseline.content_sha256 != content_sha256
            || baseline.exchange_sha256 != sha256_hex(exchange_bytes))
    {
        return Err(BrokerError::RemoteDataInvalid);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn difference_state(
    profile: &PortableProfileState,
    local_sha256: &str,
    remote_revision: u64,
    remote_etag: &str,
    remote_sha256: &str,
    local_host_count: u32,
    local_credential_count: u32,
    local_desktop_profile_count: u32,
    remote_counts: (u32, u32, u32),
) -> PluginSshSyncDifferenceState {
    if local_sha256 == remote_sha256 {
        return PluginSshSyncDifferenceState::Equal;
    }
    if let Some(baseline) = &profile.remote_baseline {
        let local_unchanged = baseline.content_sha256 == local_sha256;
        let remote_unchanged = baseline.revision == remote_revision
            && baseline.etag == remote_etag
            && baseline.content_sha256 == remote_sha256;
        return match (local_unchanged, remote_unchanged) {
            (true, true) => PluginSshSyncDifferenceState::Equal,
            (false, false) => PluginSshSyncDifferenceState::Conflict,
            _ => PluginSshSyncDifferenceState::Different,
        };
    }
    if local_host_count == 0
        && local_credential_count == 0
        && local_desktop_profile_count == 0
        && remote_counts != (0, 0, 0)
    {
        PluginSshSyncDifferenceState::RemoteOnly
    } else if remote_counts == (0, 0, 0)
        && (local_host_count != 0
            || local_credential_count != 0
            || local_desktop_profile_count != 0)
    {
        PluginSshSyncDifferenceState::LocalOnly
    } else {
        PluginSshSyncDifferenceState::Different
    }
}

#[allow(clippy::too_many_arguments)]
fn aggregate_status(
    profile_id: &str,
    account_state: PluginSshSyncAccountState,
    operation_state: PluginSshSyncOperationState,
    local: &PortableSnapshot,
    remote_counts: Option<(u32, u32, u32)>,
    difference_state: PluginSshSyncDifferenceState,
    scope_mode: PluginSshSyncScopeMode,
    last_sync_at_unix_ms: Option<i64>,
    http_status: Option<u16>,
    remote_revision: Option<u64>,
    etag: Option<String>,
) -> PluginSshSyncStatus {
    PluginSshSyncStatus {
        profile_id: profile_id.to_owned(),
        account_state,
        operation_state,
        last_sync_at_unix_ms,
        desktop_profile_count: local.desktop_profile_count,
        local_desktop_profile_count: local.desktop_profile_count,
        remote_desktop_profile_count: remote_counts.map(|value| value.2),
        host_count: local.host_count,
        credential_count: local.credential_count,
        conflict_count: u32::from(difference_state == PluginSshSyncDifferenceState::Conflict),
        diagnostic_code: None,
        stable_error_code: None,
        http_status,
        remote_revision,
        etag,
        preview_id: None,
        exchange_sha256: None,
        local_host_count: local.host_count,
        local_credential_count: local.credential_count,
        remote_host_count: remote_counts.map(|value| value.0),
        remote_credential_count: remote_counts.map(|value| value.1),
        difference_state: Some(difference_state),
        scope_mode: Some(scope_mode),
    }
}

fn restore_conflict_status(
    mut status: PluginSshSyncStatus,
    conflict_count: u32,
) -> PluginSshSyncStatus {
    status.operation_state = PluginSshSyncOperationState::Failed;
    status.stable_error_code = Some(PluginSshSyncStableErrorCode::RestoreConflict);
    status.conflict_count = conflict_count;
    status
}

#[derive(Debug, PartialEq, Eq)]
enum StagedRestoreBlocker {
    RestoreConflict(u32),
    DeletionApproval,
}

fn staged_restore_blocker(
    conflict_count: u32,
    automatic: bool,
    deletion_needs_prompt: bool,
) -> Option<StagedRestoreBlocker> {
    if conflict_count > 0 {
        Some(StagedRestoreBlocker::RestoreConflict(conflict_count))
    } else if automatic && deletion_needs_prompt {
        Some(StagedRestoreBlocker::DeletionApproval)
    } else {
        None
    }
}

fn remote_reset_status(
    profile_id: &str,
    account_state: PluginSshSyncAccountState,
    local_counts: (u32, u32, u32),
    profile: &PortableProfileState,
    http_status: Option<u16>,
) -> PluginSshSyncStatus {
    let difference_state = if local_counts == (0, 0, 0) {
        PluginSshSyncDifferenceState::Equal
    } else {
        PluginSshSyncDifferenceState::LocalOnly
    };
    PluginSshSyncStatus {
        profile_id: profile_id.to_owned(),
        account_state,
        operation_state: PluginSshSyncOperationState::Succeeded,
        last_sync_at_unix_ms: profile.last_successful_sync_at_unix_ms,
        desktop_profile_count: local_counts.2,
        local_desktop_profile_count: local_counts.2,
        remote_desktop_profile_count: Some(0),
        host_count: local_counts.0,
        credential_count: local_counts.1,
        conflict_count: 0,
        diagnostic_code: None,
        stable_error_code: None,
        http_status,
        remote_revision: None,
        etag: None,
        preview_id: None,
        exchange_sha256: None,
        local_host_count: local_counts.0,
        local_credential_count: local_counts.1,
        remote_host_count: Some(0),
        remote_credential_count: Some(0),
        difference_state: Some(difference_state),
        scope_mode: Some(profile.scope_mode),
    }
}

fn namespace(plugin_id: &str, signer_fingerprint_sha256: &str, profile_id: &str) -> String {
    format!("{plugin_id}\0{signer_fingerprint_sha256}\0{profile_id}")
}

fn select_local_data_owner(plugin_id: &str, owners: &[String]) -> Result<String, BrokerError> {
    match owners {
        [] => stable_plugin_data_owner(plugin_id)
            .map_err(|_| BrokerError::OperationRejected("broker.select_local_data_owner.01")),
        [owner] if valid_sha256(owner) => Ok(owner.clone()),
        _ => Err(BrokerError::OwnerConflict),
    }
}

impl SshSyncExchangeBroker {
    fn secure_context(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
        remote_origin: Option<String>,
        fence: &ActionFence,
    ) -> SecureActionContext {
        SecureActionContext {
            background: self.background,
            plugin_id: plugin_id.to_owned(),
            data_owner_sha256: if self.data_owner_sha256.is_empty() {
                signer_fingerprint_sha256.to_owned()
            } else {
                self.data_owner_sha256.clone()
            },
            profile_id: profile_id.to_owned(),
            remote_origin,
            fence: fence.clone(),
        }
    }
}

fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

fn normalized_resource_origin(value: &str) -> Result<String, BrokerError> {
    let url = sync_url(value)?;
    if !matches!(url.path(), "" | "/") || url.query().is_some() {
        return Err(BrokerError::OperationRejected(
            "broker.normalized_resource_origin.01",
        ));
    }
    Ok(origin(&url))
}

fn request_profile_id(request: &PluginSshSyncRequest) -> &str {
    match request {
        PluginSshSyncRequest::Status { profile_id, .. }
        | PluginSshSyncRequest::Authorize { profile_id, .. }
        | PluginSshSyncRequest::Login { profile_id, .. }
        | PluginSshSyncRequest::Register { profile_id, .. }
        | PluginSshSyncRequest::VerifyEmail { profile_id, .. }
        | PluginSshSyncRequest::CompleteMfa { profile_id, .. }
        | PluginSshSyncRequest::Logout { profile_id, .. }
        | PluginSshSyncRequest::Refresh { profile_id, .. }
        | PluginSshSyncRequest::Sync { profile_id, .. }
        | PluginSshSyncRequest::ConfigureScope { profile_id }
        | PluginSshSyncRequest::ResetRemote { profile_id, .. } => profile_id,
    }
}

fn request_restore_profile(
    request: &PluginSshSyncRequest,
) -> Option<&PluginSshSyncCredentialProfile> {
    match request {
        PluginSshSyncRequest::Status { auth, .. } | PluginSshSyncRequest::Logout { auth, .. } => {
            auth.as_ref()
        }
        PluginSshSyncRequest::Refresh { auth, .. }
        | PluginSshSyncRequest::Sync { auth, .. }
        | PluginSshSyncRequest::ResetRemote { auth, .. } => Some(auth),
        _ => None,
    }
}

fn account_state(state: NativeAccountState) -> PluginSshSyncAccountState {
    match state {
        NativeAccountState::Disconnected | NativeAccountState::Unavailable => {
            PluginSshSyncAccountState::Disconnected
        }
        NativeAccountState::Authorizing => PluginSshSyncAccountState::Authorizing,
        NativeAccountState::NeedsMfa => PluginSshSyncAccountState::NeedsMfa,
        NativeAccountState::NeedsEmailVerification => {
            PluginSshSyncAccountState::NeedsEmailVerification
        }
        NativeAccountState::Connected => PluginSshSyncAccountState::Connected,
        NativeAccountState::Expired => PluginSshSyncAccountState::Expired,
    }
}

fn idle_status(
    profile_id: String,
    account_state: PluginSshSyncAccountState,
) -> PluginSshSyncStatus {
    PluginSshSyncStatus {
        profile_id,
        account_state,
        operation_state: PluginSshSyncOperationState::Idle,
        last_sync_at_unix_ms: None,
        desktop_profile_count: 0,
        local_desktop_profile_count: 0,
        remote_desktop_profile_count: None,
        host_count: 0,
        credential_count: 0,
        conflict_count: 0,
        diagnostic_code: None,
        stable_error_code: None,
        http_status: None,
        remote_revision: None,
        etag: None,
        preview_id: None,
        exchange_sha256: None,
        local_host_count: 0,
        local_credential_count: 0,
        remote_host_count: None,
        remote_credential_count: None,
        difference_state: Some(PluginSshSyncDifferenceState::Unavailable),
        scope_mode: None,
    }
}

fn vault_access_error(state: norishell_core_api::VaultState) -> Option<BrokerError> {
    match state {
        norishell_core_api::VaultState::Missing => Some(BrokerError::VaultMissing),
        norishell_core_api::VaultState::Locked | norishell_core_api::VaultState::RequiresReload => {
            Some(BrokerError::VaultLocked)
        }
        norishell_core_api::VaultState::Unlocked => None,
    }
}

fn map_sync_key_recovery_error(error: SyncKeyRecoveryError) -> BrokerError {
    match error {
        SyncKeyRecoveryError::VaultUnavailable => BrokerError::VaultLocked,
        SyncKeyRecoveryError::RemoteKeyAuthenticationFailed => {
            BrokerError::RecoveryRemoteKeyAuthenticationFailed
        }
        SyncKeyRecoveryError::RemoteKeyEnvelopeInvalid => BrokerError::RemoteDataInvalid,
    }
}

fn failed_status(profile_id: String, error: BrokerError) -> PluginSshSyncStatus {
    let stable_error_code = match error {
        BrokerError::VaultMissing => PluginSshSyncStableErrorCode::VaultMissing,
        BrokerError::VaultLocked => PluginSshSyncStableErrorCode::VaultLocked,
        BrokerError::InteractionRequired => PluginSshSyncStableErrorCode::InteractionRequired,
        BrokerError::Cancelled => return cancelled_status(profile_id, None),
        BrokerError::AuthorizationDenied => PluginSshSyncStableErrorCode::AuthorizationDenied,
        BrokerError::AuthorizationExpired => PluginSshSyncStableErrorCode::AuthorizationExpired,
        BrokerError::AccountNotConnected => PluginSshSyncStableErrorCode::AccountNotConnected,
        BrokerError::NetworkUnavailable => PluginSshSyncStableErrorCode::NetworkUnavailable,
        BrokerError::StateConflict => PluginSshSyncStableErrorCode::StateConflict,
        BrokerError::OwnerConflict => PluginSshSyncStableErrorCode::OwnerConflict,
        BrokerError::KeyBindingConflict => PluginSshSyncStableErrorCode::KeyBindingConflict,
        BrokerError::RevisionExhausted => PluginSshSyncStableErrorCode::RevisionExhausted,
        BrokerError::RestoreConflict => PluginSshSyncStableErrorCode::RestoreConflict,
        BrokerError::MergeInvalid(_) => PluginSshSyncStableErrorCode::MergeInvalid,
        BrokerError::HttpFailure(status) => match status {
            401 => PluginSshSyncStableErrorCode::AuthorizationExpired,
            403 => PluginSshSyncStableErrorCode::AccessDenied,
            409 | 412 => PluginSshSyncStableErrorCode::StateConflict,
            413 | 429 => PluginSshSyncStableErrorCode::QuotaExceeded,
            500..=599 => PluginSshSyncStableErrorCode::ServiceUnavailable,
            _ => PluginSshSyncStableErrorCode::RemoteRequestRejected,
        },
        BrokerError::RemoteDataInvalid => PluginSshSyncStableErrorCode::RemoteDataInvalid,
        BrokerError::RemoteFormatUnsupported => {
            PluginSshSyncStableErrorCode::RemoteFormatUnsupported
        }
        BrokerError::RecoveryRemoteKeyAuthenticationFailed => {
            PluginSshSyncStableErrorCode::RecoveryRemoteKeyAuthenticationFailed
        }
        BrokerError::RecoveryActionExpired => PluginSshSyncStableErrorCode::RecoveryActionExpired,
        BrokerError::OperationRejected(_) => PluginSshSyncStableErrorCode::OperationRejected,
        BrokerError::LocalDataInvalid(_, _) => PluginSshSyncStableErrorCode::LocalDataInvalid,
        BrokerError::LocalStateChanged | BrokerError::RetryLocalSnapshot => {
            PluginSshSyncStableErrorCode::LocalStateChanged
        }
        BrokerError::LocalKeyUnavailable => PluginSshSyncStableErrorCode::LocalKeyUnavailable,
        BrokerError::OperationBusy => PluginSshSyncStableErrorCode::OperationBusy,
        BrokerError::Internal(_) | BrokerError::Persistence(..) => {
            PluginSshSyncStableErrorCode::Internal
        }
    };
    PluginSshSyncStatus {
        profile_id,
        account_state: PluginSshSyncAccountState::Disconnected,
        operation_state: PluginSshSyncOperationState::Failed,
        last_sync_at_unix_ms: None,
        desktop_profile_count: 0,
        local_desktop_profile_count: 0,
        remote_desktop_profile_count: None,
        host_count: 0,
        credential_count: 0,
        conflict_count: 0,
        diagnostic_code: match error {
            BrokerError::OperationRejected(code)
            | BrokerError::Internal(code)
            | BrokerError::MergeInvalid(code) => Some(code.to_owned()),
            BrokerError::LocalDataInvalid(stage, reason) => Some(format!("{stage}: {reason}")),
            BrokerError::Persistence(stage, kind, code) => Some(match code {
                Some(code) => format!("{stage}: {kind} ({code})"),
                None => format!("{stage}: {kind}"),
            }),
            _ => None,
        },
        stable_error_code: Some(stable_error_code),
        http_status: match error {
            BrokerError::HttpFailure(status) => Some(status),
            _ => None,
        },
        remote_revision: None,
        etag: None,
        preview_id: None,
        exchange_sha256: None,
        local_host_count: 0,
        local_credential_count: 0,
        remote_host_count: None,
        remote_credential_count: None,
        difference_state: Some(PluginSshSyncDifferenceState::Unavailable),
        scope_mode: None,
    }
}

fn apply_verified_stale_counts(
    status: &mut PluginSshSyncStatus,
    snapshot: &PluginSshSyncBrowserSnapshot,
) {
    if snapshot.state != norishell_core_api::PluginSshSyncBrowserState::Stale
        || snapshot.verified_at_unix_ms.is_none()
        || snapshot.profile_id != status.profile_id
    {
        return;
    }
    status.remote_host_count = Some(snapshot.host_count);
    status.remote_credential_count = Some(snapshot.credential_count);
    status.remote_desktop_profile_count = Some(snapshot.desktop_profile_count);
}

fn cancelled_status(
    profile_id: String,
    previous: Option<PluginSshSyncStatus>,
) -> PluginSshSyncStatus {
    let mut status = previous
        .unwrap_or_else(|| idle_status(profile_id, PluginSshSyncAccountState::Disconnected));
    status.operation_state = PluginSshSyncOperationState::Idle;
    status.stable_error_code = None;
    status.diagnostic_code = None;
    status
}

// Authentication errors from login or a resource request do not revoke a saved session.
// The OAuth runtime marks a refresh token expired only when its own refresh is rejected.
fn account_state_after_result(
    saved: PluginSshSyncAccountState,
    _error: Option<PluginSshSyncStableErrorCode>,
) -> PluginSshSyncAccountState {
    saved
}

fn map_oauth_error(error: NativeAuthError) -> BrokerError {
    match error {
        NativeAuthError::AccessDenied => BrokerError::AuthorizationDenied,
        NativeAuthError::RefreshExpired => BrokerError::AuthorizationExpired,
        NativeAuthError::SessionMissing => BrokerError::AccountNotConnected,
        NativeAuthError::Network => BrokerError::NetworkUnavailable,
        NativeAuthError::VaultLocked => BrokerError::VaultLocked,
        NativeAuthError::OperationInProgress => BrokerError::OperationBusy,
        NativeAuthError::StateUnavailable => {
            BrokerError::Internal("broker.map_oauth_error.internal01")
        }
        NativeAuthError::Protocol => BrokerError::RemoteDataInvalid,
        NativeAuthError::LocalCommit => BrokerError::Internal("broker.map_oauth_error.internal02"),
    }
}

fn map_selection_error(error: SecureSelectionError) -> BrokerError {
    match error {
        SecureSelectionError::InteractionRequired => BrokerError::InteractionRequired,
        SecureSelectionError::Cancelled => BrokerError::Cancelled,
        SecureSelectionError::Unavailable => {
            BrokerError::Internal("broker.map_selection_error.internal01")
        }
    }
}

fn map_store_error(error: PortableStoreError) -> BrokerError {
    match error {
        PortableStoreError::InvalidSelection => {
            BrokerError::OperationRejected("local.selection.invalid")
        }
        PortableStoreError::Rejected(code) => BrokerError::OperationRejected(code),
        PortableStoreError::InvalidBundle(stage, reason) => {
            BrokerError::LocalDataInvalid(stage, reason)
        }
        PortableStoreError::Stale => BrokerError::LocalStateChanged,
        PortableStoreError::Internal(code) => BrokerError::Internal(code),
        PortableStoreError::Persistence(stage, kind, code) => {
            BrokerError::Persistence(stage, kind, code)
        }
    }
}

fn authenticated_upload_completion_proof(
    attempt: &PortableUploadAttempt,
    authenticated_remote_revision: u64,
    remote_body_sha256: &str,
    remote_etag: Option<&str>,
) -> Option<PortableUploadCompletionProof> {
    if remote_body_sha256 == attempt.body_sha256
        && authenticated_remote_revision == attempt.target_revision
    {
        Some(PortableUploadCompletionProof::ExactBodyObserved {
            observed_revision: authenticated_remote_revision,
        })
    } else if authenticated_remote_revision > attempt.target_revision {
        Some(PortableUploadCompletionProof::SupersededByNewerRemote {
            authenticated_remote_revision,
        })
    } else if authenticated_remote_revision == attempt.target_revision
        && remote_body_sha256 != attempt.body_sha256
        && let Some(etag) = remote_etag.filter(|value| valid_strong_etag(value))
    {
        Some(
            PortableUploadCompletionProof::ConflictingBodyObservedAtTargetRevision {
                authenticated_remote_revision,
                authenticated_remote_body_sha256: remote_body_sha256.to_owned(),
                authenticated_remote_etag: etag.to_owned(),
            },
        )
    } else {
        None
    }
}

fn durable_upload_fence(
    url: &str,
    method: PluginSshSyncHttpMethod,
    use_oauth: bool,
    action_revision: SshSyncActionRevision,
) -> Result<DurableUploadFence, BrokerError> {
    Ok(DurableUploadFence {
        canonical_url: sync_url(url)?.as_str().to_owned(),
        method,
        use_oauth,
        action_revision,
    })
}

fn upload_attempt_matches_fence(
    attempt: &PortableUploadAttempt,
    expected: &DurableUploadFence,
) -> bool {
    attempt.canonical_url == expected.canonical_url
        && attempt.method == expected.method
        && attempt.use_oauth == expected.use_oauth
        && attempt.authorization_revision == expected.action_revision.authorization
        && attempt.configuration_revision == expected.action_revision.configuration
}

fn bounded_count(value: usize) -> Result<u32, BrokerError> {
    u32::try_from(value).map_err(|_| BrokerError::RemoteDataInvalid)
}

fn browser_binding_current(
    vault: &crate::vault_service::VaultService,
    state: &Arc<Mutex<BrokerState>>,
    expected: &SshSyncBrowserCacheBinding,
) -> bool {
    if !vault.is_unlocked() {
        return false;
    }
    let state = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let namespace = namespace(
        &expected.plugin_id,
        &expected.signer_fingerprint_sha256,
        &expected.profile_id,
    );
    let same_epoch_and_source = state
        .account_epochs
        .get(&namespace)
        .copied()
        .unwrap_or_default()
        == expected.account_epoch
        && state.browser_sources.get(&namespace).cloned() == expected.source_url;
    let oauth = state.oauth.get(&namespace).cloned();
    drop(state);
    same_epoch_and_source
        && oauth.is_some_and(|service| {
            service.configuration_digest() == expected.account_configuration_sha256
                && service.session_id() == expected.oauth_session_id
        })
}

fn inspect_cached_browser_exchange(
    bytes: &[u8],
    plugin_id: &str,
    profile_id: &str,
) -> Option<(PluginExchangeBinding, Vec<u8>)> {
    let (binding, _) = inspect_plugin_exchange_data_owner(bytes, plugin_id, profile_id).ok()?;
    let envelope = plugin_exchange_vault_key_envelope(bytes, &binding).ok()?;
    Some((binding, envelope))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_timed_bundle() -> PortableBundleV1 {
        let mut bundle = empty_portable_bundle(1);
        bundle.schema = BundleSchema::V5;
        bundle.preferences = Some(norishell_ssh_profile_sync::PortablePreferencesV1 {
            product: "NoriShell".to_owned(),
            version: 1,
            groups: serde_json::from_value(serde_json::json!({
                "application": {"themePreference":null,"locale":null,"uiZoom":null,"terminalStartupBehavior":null,"newTerminalBehavior":null,"singlePaneTabCloseBehavior":null},
                "appearance": {"terminalThemeMode":null,"terminalFontFamily":null,"terminalFontSize":null,"terminalFontWeight":null,"terminalBoldFontWeight":null,"terminalLineHeight":null,"terminalLetterSpacing":null,"terminalCursorStyle":null,"terminalCursorBlink":null,"customTerminalPalette":null,"customTerminalPaletteName":null},
                "interaction": {"interaction":null,"pasteWarning":null},
                "highlights": {"enabled":null,"rules":null},
                "shortcuts": {"version":null,"bindings":null},
                "files": {"browser":null,"rememberLastDirectory":null},
                "desktop": {"windowCloseBehavior":null,"trayShowStatus":null,"trayRecentLimit":null,"trayShowHostNames":null,"notificationBackgroundOnly":null,"notificationFailureOnly":null,"notifyTransferCompleted":null,"notifyTransferFailed":null,"notifyDisconnected":null},
                "commandNotifications": {"notificationsEnabled":null,"notificationThresholdSeconds":null}
            }))
            .expect("preference groups"),
        });
        bundle
    }

    #[test]
    fn refresh_fast_forward_accepts_only_remote_historical_deletion() {
        use norishell_ssh_profile_sync::{
            PortableItemUpdateTime, PortableObjectId, PortableObjectKind, PortableTombstone,
        };

        let key = SyncKey::from_bytes([17; 32]);
        let local = empty_timed_bundle();
        let mut remote = local.clone();
        remote.revision = 2;
        let deleted_id = PortableObjectId::new();
        remote.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::Host,
            id: deleted_id,
        });
        remote.update_times.push(PortableItemUpdateTime {
            kind: PortableObjectKind::Host,
            id: deleted_id,
            update_time_unix_ms: 100,
        });
        assert!(remote_only_historical_tombstones(&local, &remote, &key).unwrap());

        let mut changed = remote.clone();
        changed
            .preference_update_times
            .insert("application".to_owned(), 101);
        assert!(!remote_only_historical_tombstones(&local, &changed, &key).unwrap());

        let mut local_deleted = local.clone();
        local_deleted.tombstones.push(PortableTombstone {
            kind: PortableObjectKind::Host,
            id: PortableObjectId::new(),
        });
        assert!(!remote_only_historical_tombstones(&local_deleted, &remote, &key).unwrap());
    }

    #[test]
    fn cached_exchange_uses_authenticated_data_owner_not_current_package_signer() {
        let plugin_id = "org.example.sync";
        let data_owner = stable_plugin_data_owner(plugin_id).unwrap();
        let package_signer = "a".repeat(64);
        assert_ne!(data_owner, package_signer);
        let binding = PluginExchangeBinding {
            plugin_id: plugin_id.to_owned(),
            signer_fingerprint_sha256: data_owner,
            profile_id: "primary".to_owned(),
            revision: 1,
            base_revision: None,
            base_etag: None,
        };
        let key = SyncKey::from_bytes([7; 32]);
        let envelope = b"opaque-vault-key-envelope";
        let bytes =
            create_plugin_exchange_with_key(&empty_portable_bundle(1), &key, envelope, &binding)
                .unwrap();
        assert!(
            inspect_plugin_exchange_owner(&bytes, plugin_id, &package_signer, "primary").is_err()
        );
        let (restored, restored_envelope) =
            inspect_cached_browser_exchange(&bytes, plugin_id, "primary").unwrap();
        assert_eq!(restored, binding);
        assert_eq!(restored_envelope, envelope);
        assert!(inspect_cached_browser_exchange(&bytes, "other.plugin", "primary").is_none());
    }

    #[test]
    fn local_data_owner_keeps_one_legacy_namespace_and_rejects_ambiguity() {
        let plugin_id = "org.example.sync";
        let legacy = "a".repeat(64);
        assert_eq!(
            select_local_data_owner(plugin_id, &[]).unwrap(),
            stable_plugin_data_owner(plugin_id).unwrap()
        );
        assert_eq!(
            select_local_data_owner(plugin_id, std::slice::from_ref(&legacy)).unwrap(),
            legacy
        );
        assert!(matches!(
            select_local_data_owner(plugin_id, &["a".repeat(64), "b".repeat(64)]),
            Err(BrokerError::OwnerConflict)
        ));
        assert!(matches!(
            select_local_data_owner(plugin_id, &["invalid".to_owned()]),
            Err(BrokerError::OwnerConflict)
        ));
    }

    #[tokio::test]
    async fn stale_fence_never_polls_network_future() {
        let polled = std::sync::atomic::AtomicBool::new(false);
        let future = async {
            polled.store(true, std::sync::atomic::Ordering::SeqCst);
        };
        let fence: ActionFence = Arc::new(|| false);
        assert!(matches!(
            await_fenced(future, &fence).await,
            Err(BrokerError::OperationRejected(_))
        ));
        assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn legacy_bundle_projection_preserves_business_graph_without_preferences() {
        let mut legacy = empty_portable_bundle(7);
        legacy.schema = BundleSchema::V5;
        legacy.preferences = Some(norishell_ssh_profile_sync::PortablePreferencesV1 {
            product: "NoriShell".to_owned(),
            version: 1,
            groups: BTreeMap::new(),
        });
        legacy
            .preference_update_times
            .insert("application".to_owned(), 123);
        let host_id = norishell_ssh_profile_sync::PortableObjectId::new();
        legacy
            .tombstones
            .push(norishell_ssh_profile_sync::PortableTombstone {
                kind: norishell_ssh_profile_sync::PortableObjectKind::Host,
                id: host_id,
            });
        legacy
            .update_times
            .push(norishell_ssh_profile_sync::PortableItemUpdateTime {
                kind: norishell_ssh_profile_sync::PortableObjectKind::Host,
                id: host_id,
                update_time_unix_ms: 456,
            });
        let projected = business_only_bundle(legacy.clone()).unwrap();
        assert_eq!(projected.schema, BundleSchema::V6);
        assert!(projected.preferences.is_none());
        assert!(projected.preference_update_times.is_empty());
        assert_eq!(projected.tombstones, legacy.tombstones);
        assert_eq!(projected.update_times, legacy.update_times);
        assert_eq!(projected.revision, 7);
    }

    #[test]
    fn legacy_format_is_distinct_from_malformed_or_current_data() {
        assert!(is_legacy_exchange_format(
            br#"{"format":"norishell-ssh-sync-exchange-v1"}"#
        ));
        assert!(is_legacy_exchange_format(
            br#"{"format":"norishell-ssh-vault-exchange-v2"}"#
        ));
        assert!(!is_legacy_exchange_format(
            br#"{"format":"norishell-ssh-vault-exchange-v3"}"#
        ));
        assert!(!is_legacy_exchange_format(b"not-json"));
        assert_eq!(
            failed_status("primary".into(), BrokerError::RemoteFormatUnsupported).stable_error_code,
            Some(PluginSshSyncStableErrorCode::RemoteFormatUnsupported)
        );
    }

    #[test]
    fn state_conflict_without_a_review_plan_is_a_failure() {
        let status = failed_status("primary".into(), BrokerError::StateConflict);
        assert_eq!(status.operation_state, PluginSshSyncOperationState::Failed);
        assert_eq!(
            status.stable_error_code,
            Some(PluginSshSyncStableErrorCode::StateConflict)
        );
        assert_eq!(status.conflict_count, 0);
        assert_eq!(
            status.difference_state,
            Some(PluginSshSyncDifferenceState::Unavailable)
        );
    }

    #[test]
    fn missing_login_has_an_actionable_code_and_does_not_impersonate_network_failure() {
        let status = failed_status(
            "primary".into(),
            map_oauth_error(NativeAuthError::SessionMissing),
        );
        assert_eq!(status.operation_state, PluginSshSyncOperationState::Failed);
        assert_eq!(
            status.stable_error_code,
            Some(PluginSshSyncStableErrorCode::AccountNotConnected)
        );
        assert_eq!(status.diagnostic_code, None);
        assert_eq!(
            failed_status("primary".into(), map_oauth_error(NativeAuthError::Network))
                .stable_error_code,
            Some(PluginSshSyncStableErrorCode::NetworkUnavailable)
        );
    }

    #[test]
    fn offline_failure_keeps_verified_remote_counts_without_claiming_freshness() {
        let mut status = failed_status("primary".into(), BrokerError::NetworkUnavailable);
        let mut snapshot = empty_snapshot(
            norishell_core_api::PluginSshSyncBrowserState::Stale,
            "primary".into(),
            WireSequence::new(1),
        );
        snapshot.host_count = 3;
        snapshot.credential_count = 2;
        snapshot.desktop_profile_count = 1;
        snapshot.verified_at_unix_ms = Some(42);
        apply_verified_stale_counts(&mut status, &snapshot);
        assert_eq!(status.remote_host_count, Some(3));
        assert_eq!(status.remote_credential_count, Some(2));
        assert_eq!(status.remote_desktop_profile_count, Some(1));
        assert_eq!(status.last_sync_at_unix_ms, None);
        assert_eq!(
            status.difference_state,
            Some(PluginSshSyncDifferenceState::Unavailable)
        );
        assert_eq!(
            status.stable_error_code,
            Some(PluginSshSyncStableErrorCode::NetworkUnavailable)
        );

        let mut unrelated = failed_status("other".into(), BrokerError::NetworkUnavailable);
        apply_verified_stale_counts(&mut unrelated, &snapshot);
        assert_eq!(unrelated.remote_host_count, None);
        snapshot.verified_at_unix_ms = None;
        let mut unverified = failed_status("primary".into(), BrokerError::NetworkUnavailable);
        apply_verified_stale_counts(&mut unverified, &snapshot);
        assert_eq!(unverified.remote_host_count, None);
    }

    #[test]
    fn unrecoverable_sync_conflicts_keep_their_specific_failure_code() {
        for (error, expected) in [
            (
                BrokerError::OwnerConflict,
                PluginSshSyncStableErrorCode::OwnerConflict,
            ),
            (
                BrokerError::KeyBindingConflict,
                PluginSshSyncStableErrorCode::KeyBindingConflict,
            ),
            (
                BrokerError::RevisionExhausted,
                PluginSshSyncStableErrorCode::RevisionExhausted,
            ),
            (
                BrokerError::RestoreConflict,
                PluginSshSyncStableErrorCode::RestoreConflict,
            ),
            (
                BrokerError::MergeInvalid("broker.sync.merge_initial"),
                PluginSshSyncStableErrorCode::MergeInvalid,
            ),
        ] {
            let status = failed_status("primary".into(), error);
            assert_eq!(status.operation_state, PluginSshSyncOperationState::Failed);
            assert_eq!(status.stable_error_code, Some(expected));
            assert_eq!(status.conflict_count, 0);
        }
    }

    #[test]
    fn restore_conflict_retains_actual_count_without_offering_review() {
        let status = restore_conflict_status(
            idle_status("primary".into(), PluginSshSyncAccountState::Connected),
            3,
        );
        assert_eq!(status.operation_state, PluginSshSyncOperationState::Failed);
        assert_eq!(
            status.stable_error_code,
            Some(PluginSshSyncStableErrorCode::RestoreConflict)
        );
        assert_eq!(status.conflict_count, 3);
    }

    #[test]
    fn restore_conflict_precedes_automatic_deletion_approval() {
        assert_eq!(
            staged_restore_blocker(3, true, true),
            Some(StagedRestoreBlocker::RestoreConflict(3))
        );
        assert_eq!(
            staged_restore_blocker(0, true, true),
            Some(StagedRestoreBlocker::DeletionApproval)
        );
        assert_eq!(staged_restore_blocker(0, false, true), None);
    }

    #[test]
    fn cancelling_protected_review_preserves_known_counts_without_an_error() {
        let mut previous = idle_status("primary".into(), PluginSshSyncAccountState::Expired);
        previous.local_host_count = 5;
        previous.remote_host_count = Some(3);
        previous.stable_error_code = Some(PluginSshSyncStableErrorCode::AuthorizationExpired);
        let cancelled = cancelled_status("primary".into(), Some(previous));
        assert_eq!(cancelled.local_host_count, 5);
        assert_eq!(cancelled.remote_host_count, Some(3));
        assert_eq!(cancelled.account_state, PluginSshSyncAccountState::Expired);
        assert_eq!(cancelled.operation_state, PluginSshSyncOperationState::Idle);
        assert!(cancelled.stable_error_code.is_none());
        assert!(
            failed_status("primary".into(), BrokerError::Cancelled)
                .stable_error_code
                .is_none()
        );
    }

    #[test]
    fn rejected_operation_preserves_the_actual_oauth_session() {
        assert_eq!(
            account_state_after_result(
                PluginSshSyncAccountState::Connected,
                Some(PluginSshSyncStableErrorCode::AuthorizationDenied)
            ),
            PluginSshSyncAccountState::Connected
        );
        assert_eq!(
            account_state_after_result(
                PluginSshSyncAccountState::Connected,
                Some(PluginSshSyncStableErrorCode::AuthorizationExpired)
            ),
            PluginSshSyncAccountState::Connected
        );
        assert_eq!(
            account_state_after_result(
                PluginSshSyncAccountState::Connected,
                Some(PluginSshSyncStableErrorCode::NetworkUnavailable)
            ),
            PluginSshSyncAccountState::Connected
        );
    }

    #[test]
    fn absent_remote_revision_accepts_reset_generation_and_rejects_invalid_headers() {
        use reqwest::header::HeaderValue;

        assert_eq!(parse_next_revision(None).expect("new account"), 1);
        for (value, expected) in [
            ("2", 2),
            ("43", 43),
            ("9007199254740991", MAX_EXCHANGE_REVISION),
        ] {
            assert_eq!(
                parse_next_revision(Some(&HeaderValue::from_str(value).expect("header")))
                    .expect("reset generation"),
                expected
            );
        }
        for value in [
            "",
            "0",
            "-1",
            "+2",
            "1.0",
            " 2",
            "2 ",
            "9007199254740992",
            "18446744073709551616",
        ] {
            assert!(matches!(
                parse_next_revision(Some(&HeaderValue::from_str(value).expect("header"))),
                Err(BrokerError::RemoteDataInvalid)
            ));
        }
    }

    #[test]
    fn request_namespace_and_urls_are_provider_neutral_and_strict() {
        assert_ne!(
            namespace("org.example.one", &"a".repeat(64), "primary"),
            namespace("org.example.two", &"a".repeat(64), "primary")
        );
        assert_ne!(
            namespace("org.example.one", &"a".repeat(64), "primary"),
            namespace("org.example.one", &"b".repeat(64), "primary")
        );
        assert!(sync_url("https://sync.example.test/v1/exchange").is_ok());
        assert!(sync_url("http://127.0.0.1:8080/v1/exchange").is_ok());
        assert!(sync_url("ftp://sync.example.test/v1/exchange").is_err());
        assert!(sync_url("https://user@sync.example.test/v1/exchange").is_err());
        assert!(sync_url("http://user@127.0.0.1:8080/v1/exchange").is_err());
        assert!(sync_url("http://127.0.0.1:8080/v1/exchange?token=secret").is_err());
        assert!(sync_url("http://127.0.0.1:8080/v1/exchange#fragment").is_err());
        assert_eq!(
            normalized_resource_origin("https://sync.example.test").expect("origin"),
            "https://sync.example.test"
        );
        assert!(normalized_resource_origin("https://sync.example.test/path").is_err());
        assert!(normalized_resource_origin("https://sync.example.test?next=x").is_err());
        assert_eq!(
            normalized_resource_origin("http://127.0.0.1:8080").expect("HTTP origin"),
            "http://127.0.0.1:8080"
        );
        assert!(normalized_resource_origin("http://127.0.0.1:8080/path").is_err());

        let configuration = credential_configuration(
            "org.example.self-hosted",
            &"c".repeat(64),
            "customer-server",
            PluginSshSyncCredentialProfile {
                login_url: "https://login.customer.example/native/login".to_owned(),
                registration_url: "https://login.customer.example/native/register".to_owned(),
                email_verification_url: "https://login.customer.example/native/verify-email"
                    .to_owned(),
                mfa_url: "https://login.customer.example/native/mfa".to_owned(),
                token_url: "https://login.customer.example/token".to_owned(),
                revoke_url: "https://login.customer.example/revoke".to_owned(),
                client_id: "desktop-client".to_owned(),
                scopes: vec!["ssh-sync:read".to_owned()],
                resource_origins: vec![
                    "https://backup.customer.example".to_owned(),
                    "https://restore.customer.example:8443".to_owned(),
                ],
            },
        )
        .expect("third-party provider profile");
        assert_eq!(
            configuration.resource_origins,
            vec![
                "https://backup.customer.example",
                "https://restore.customer.example:8443",
            ]
        );
    }

    #[test]
    fn account_service_reuses_same_configuration_and_rebinds_changed_provider() {
        let directory = tempfile::tempdir().expect("temporary account state");
        let vault = crate::vault_service::VaultService::start(directory.path());
        let plugin_id = "org.example.sync";
        let signer = "a".repeat(64);
        let profile_id = "primary";
        let auth = PluginSshSyncCredentialProfile {
            login_url: "https://auth.example.test/login".to_owned(),
            registration_url: "https://auth.example.test/register".to_owned(),
            email_verification_url: "https://auth.example.test/verify".to_owned(),
            mfa_url: "https://auth.example.test/mfa".to_owned(),
            token_url: "https://auth.example.test/token".to_owned(),
            revoke_url: "https://auth.example.test/revoke".to_owned(),
            client_id: "desktop".to_owned(),
            scopes: vec!["ssh-sync".to_owned()],
            resource_origins: vec!["https://sync.example.test".to_owned()],
        };
        let configuration = credential_configuration(plugin_id, &signer, profile_id, auth.clone())
            .expect("account configuration");
        let key = namespace(plugin_id, &signer, profile_id);
        let mut oauth = BTreeMap::new();
        assert!(
            reuse_or_restore_credential_service(
                &mut oauth,
                &key,
                directory.path(),
                &vault,
                configuration.clone(),
            )
            .expect("initial restore")
        );
        let initial = oauth.get(&key).expect("account service").clone();
        assert!(
            !reuse_or_restore_credential_service(
                &mut oauth,
                &key,
                directory.path(),
                &vault,
                configuration,
            )
            .expect("reuse active service")
        );
        assert!(
            oauth
                .get(&key)
                .expect("same service")
                .shares_runtime_with(&initial)
        );

        let mut changed_auth = auth;
        changed_auth.resource_origins = vec!["https://other.example.test".to_owned()];
        let changed = credential_configuration(plugin_id, &signer, profile_id, changed_auth)
            .expect("updated configuration");
        assert!(
            reuse_or_restore_credential_service(
                &mut oauth,
                &key,
                directory.path(),
                &vault,
                changed,
            )
            .expect("rebind changed provider")
        );
        assert!(
            !oauth
                .get(&key)
                .expect("new service")
                .shares_runtime_with(&initial)
        );
    }

    #[test]
    fn refresh_rejection_is_an_expired_session_not_bad_credentials() {
        assert!(matches!(
            map_oauth_error(NativeAuthError::RefreshExpired),
            BrokerError::AuthorizationExpired
        ));
        assert!(matches!(
            map_oauth_error(NativeAuthError::AccessDenied),
            BrokerError::AuthorizationDenied
        ));
    }

    #[test]
    fn browser_binding_rejects_locked_vault_and_changed_account_epoch() {
        let directory = tempfile::tempdir().expect("temporary broker state");
        let vault = crate::vault_service::VaultService::start(directory.path());
        vault
            .create_for_tests(b"test password")
            .expect("unlocked test Vault");
        let plugin_id = "org.example.browser";
        let signer = "a".repeat(64);
        let profile_id = "primary";
        let configuration = credential_configuration(
            plugin_id,
            &signer,
            profile_id,
            PluginSshSyncCredentialProfile {
                login_url: "https://auth.example.test/login".to_owned(),
                registration_url: "https://auth.example.test/register".to_owned(),
                email_verification_url: "https://auth.example.test/verify".to_owned(),
                mfa_url: "https://auth.example.test/mfa".to_owned(),
                token_url: "https://auth.example.test/token".to_owned(),
                revoke_url: "https://auth.example.test/revoke".to_owned(),
                client_id: "desktop".to_owned(),
                scopes: vec!["ssh-sync".to_owned()],
                resource_origins: vec!["https://sync.example.test".to_owned()],
            },
        )
        .expect("browser account configuration");
        let oauth = PluginOAuthService::start(directory.path(), vault.clone(), configuration)
            .expect("browser account service");
        let configuration_digest = oauth.configuration_digest();
        let key = namespace(plugin_id, &signer, profile_id);
        let mut broker_state = BrokerState::default();
        broker_state.oauth.insert(key.clone(), oauth);
        broker_state.account_epochs.insert(key.clone(), 3);
        broker_state
            .browser_sources
            .insert(key.clone(), "https://example.test/exchange".to_owned());
        let broker_state = Arc::new(Mutex::new(broker_state));
        let binding = SshSyncBrowserCacheBinding {
            plugin_id: plugin_id.to_owned(),
            signer_fingerprint_sha256: signer,
            profile_id: profile_id.to_owned(),
            package_sha256: "b".repeat(64),
            instance_generation: WireSequence::new(5),
            authorization_epoch: WireSequence::new(7),
            account_epoch: 3,
            account_configuration_sha256: configuration_digest,
            oauth_session_id: None,
            source_url: Some("https://example.test/exchange".to_owned()),
        };
        assert!(browser_binding_current(&vault, &broker_state, &binding));
        let mut other_session = binding.clone();
        other_session.oauth_session_id = Some("another-login".to_owned());
        assert!(!browser_binding_current(
            &vault,
            &broker_state,
            &other_session
        ));

        let locked_directory = tempfile::tempdir().expect("locked Vault directory");
        let locked_vault = crate::vault_service::VaultService::start(locked_directory.path());
        assert!(!browser_binding_current(
            &locked_vault,
            &broker_state,
            &binding
        ));

        broker_state
            .lock()
            .expect("broker state")
            .account_epochs
            .insert(key, 4);
        assert!(!browser_binding_current(&vault, &broker_state, &binding));
    }

    #[test]
    fn matching_current_content_is_equal_even_when_both_changed_from_baseline() {
        let profile = PortableProfileState {
            scope_mode: PluginSshSyncScopeMode::AllEligible,
            key_binding: None,
            remote_baseline: Some(PortableRemoteBaseline {
                revision: 1,
                etag: "\"old\"".into(),
                content_sha256: "old-content".into(),
                exchange_sha256: "old-wire".into(),
            }),
            state_version: WireSequence::new(1),
            last_successful_sync_at_unix_ms: None,
        };
        assert_eq!(
            difference_state(
                &profile,
                "new-content",
                2,
                "\"new\"",
                "new-content",
                1,
                2,
                0,
                (1, 2, 0)
            ),
            PluginSshSyncDifferenceState::Equal
        );
    }

    #[test]
    fn status_is_profile_scoped() {
        let status = idle_status(
            "primary".to_owned(),
            PluginSshSyncAccountState::Disconnected,
        );
        assert_eq!(status.profile_id, "primary");
        assert_eq!(status.operation_state, PluginSshSyncOperationState::Idle);
        assert!(status.preview_id.is_none());
    }

    #[test]
    fn remote_reset_status_keeps_local_counts_and_reports_an_empty_cloud() {
        let profile = PortableProfileState {
            scope_mode: PluginSshSyncScopeMode::Custom,
            key_binding: None,
            remote_baseline: None,
            state_version: WireSequence::new(5),
            last_successful_sync_at_unix_ms: Some(42),
        };
        let status = remote_reset_status(
            "primary",
            PluginSshSyncAccountState::Connected,
            (3, 4, 0),
            &profile,
            Some(204),
        );
        assert_eq!(status.local_host_count, 3);
        assert_eq!(status.local_credential_count, 4);
        assert_eq!(status.remote_host_count, Some(0));
        assert_eq!(status.remote_credential_count, Some(0));
        assert_eq!(
            status.difference_state,
            Some(PluginSshSyncDifferenceState::LocalOnly)
        );
        assert_eq!(status.last_sync_at_unix_ms, Some(42));
        assert_eq!(status.http_status, Some(204));
        assert_eq!(status.scope_mode, Some(PluginSshSyncScopeMode::Custom));
    }

    #[test]
    fn approved_empty_reset_rejects_remote_created_while_prompt_was_open() {
        assert!(validate_remote_still_absent::<()>(None).is_ok());
        assert!(matches!(
            validate_remote_still_absent(Some(&())),
            Err(BrokerError::StateConflict)
        ));
    }

    #[test]
    fn remote_delete_accepts_only_an_empty_no_content_response() {
        assert!(matches!(
            validate_remote_delete_response(StatusCode::NO_CONTENT, b""),
            Ok(204)
        ));
        for (status, expected) in [
            (
                StatusCode::BAD_REQUEST,
                PluginSshSyncStableErrorCode::RemoteRequestRejected,
            ),
            (
                StatusCode::UNAUTHORIZED,
                PluginSshSyncStableErrorCode::AuthorizationExpired,
            ),
            (
                StatusCode::FORBIDDEN,
                PluginSshSyncStableErrorCode::AccessDenied,
            ),
            (
                StatusCode::CONFLICT,
                PluginSshSyncStableErrorCode::StateConflict,
            ),
            (
                StatusCode::PRECONDITION_FAILED,
                PluginSshSyncStableErrorCode::StateConflict,
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                PluginSshSyncStableErrorCode::QuotaExceeded,
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                PluginSshSyncStableErrorCode::ServiceUnavailable,
            ),
        ] {
            let error = validate_remote_delete_response(status, b"").expect_err("HTTP failure");
            assert!(matches!(&error, BrokerError::HttpFailure(code) if *code == status.as_u16()));
            let failure = failed_status("primary".into(), error);
            assert_eq!(failure.stable_error_code, Some(expected));
            assert_eq!(failure.http_status, Some(status.as_u16()));
        }
        assert!(matches!(
            validate_remote_delete_response(StatusCode::OK, b""),
            Err(BrokerError::RemoteDataInvalid)
        ));
        assert!(matches!(
            validate_remote_delete_response(StatusCode::NO_CONTENT, b"unexpected"),
            Err(BrokerError::RemoteDataInvalid)
        ));
        assert!(matches!(
            validate_remote_delete_response(StatusCode::NOT_FOUND, b""),
            Ok(404)
        ));
    }

    #[test]
    fn protocol_seven_requires_strong_etags_and_uses_the_durable_baseline() {
        assert!(valid_strong_etag("\"revision-2\""));
        assert!(!valid_strong_etag("W/\"revision-2\""));
        assert!(!valid_strong_etag("revision-2"));

        let profile = PortableProfileState {
            scope_mode: PluginSshSyncScopeMode::AllEligible,
            key_binding: None,
            remote_baseline: Some(PortableRemoteBaseline {
                revision: 2,
                etag: "\"revision-2\"".to_owned(),
                content_sha256: "local-baseline".to_owned(),
                exchange_sha256: "a".repeat(64),
            }),
            state_version: WireSequence::new(1),
            last_successful_sync_at_unix_ms: None,
        };
        assert_eq!(
            difference_state(
                &profile,
                "local-baseline",
                2,
                "\"revision-2\"",
                "local-baseline",
                2,
                1,
                0,
                (2, 1, 0),
            ),
            PluginSshSyncDifferenceState::Equal
        );
        assert_eq!(
            difference_state(
                &profile,
                "locally-changed",
                2,
                "\"revision-2\"",
                "local-baseline",
                2,
                1,
                0,
                (2, 1, 0),
            ),
            PluginSshSyncDifferenceState::Different
        );
        assert_eq!(
            difference_state(
                &profile,
                "locally-changed",
                3,
                "\"revision-3\"",
                "remotely-changed",
                2,
                1,
                0,
                (3, 1, 0),
            ),
            PluginSshSyncDifferenceState::Conflict
        );

        assert!(matches!(
            verify_remote_baseline_invariants(
                &profile,
                2,
                "\"revision-2\"",
                "different-content",
                b"different-exchange",
            ),
            Err(BrokerError::RemoteDataInvalid)
        ));
    }

    #[test]
    fn authenticated_upload_reconciliation_retires_conflicting_target_body() {
        let attempt = PortableUploadAttempt {
            canonical_url: "https://sync.example.test/v1/exchange".to_owned(),
            method: PluginSshSyncHttpMethod::Put,
            use_oauth: true,
            authorization_revision: WireSequence::new(2),
            configuration_revision: WireSequence::new(3),
            base_revision: 4,
            base_etag: Some("\"revision-4\"".to_owned()),
            target_revision: 5,
            keyed_content_sha256: "a".repeat(64),
            body_sha256: "b".repeat(64),
            idempotency_key: Uuid::now_v7().to_string(),
            state: PortableUploadAttemptState::Sent,
            state_version: WireSequence::new(1),
        };

        assert_eq!(
            authenticated_upload_completion_proof(&attempt, 5, &"c".repeat(64), None),
            None
        );
        assert_eq!(
            authenticated_upload_completion_proof(&attempt, 5, &"c".repeat(64), Some("W/\"5\"")),
            None
        );
        assert!(matches!(
            authenticated_upload_completion_proof(&attempt, 5, &"c".repeat(64), Some("\"5\"")),
            Some(
                PortableUploadCompletionProof::ConflictingBodyObservedAtTargetRevision {
                    authenticated_remote_revision: 5,
                    ..
                }
            )
        ));
        assert!(matches!(
            authenticated_upload_completion_proof(&attempt, 5, &"b".repeat(64), Some("\"5\"")),
            Some(PortableUploadCompletionProof::ExactBodyObserved {
                observed_revision: 5
            })
        ));
        assert!(matches!(
            authenticated_upload_completion_proof(&attempt, 6, &"c".repeat(64), Some("\"6\"")),
            Some(PortableUploadCompletionProof::SupersededByNewerRemote {
                authenticated_remote_revision: 6
            })
        ));
        let exact_fence = DurableUploadFence {
            canonical_url: attempt.canonical_url.clone(),
            method: attempt.method,
            use_oauth: attempt.use_oauth,
            action_revision: SshSyncActionRevision {
                authorization: attempt.authorization_revision,
                configuration: attempt.configuration_revision,
            },
        };
        assert!(upload_attempt_matches_fence(&attempt, &exact_fence));
        for drifted in [
            DurableUploadFence {
                canonical_url: "https://other.example.test/v1/exchange".to_owned(),
                ..exact_fence.clone()
            },
            DurableUploadFence {
                action_revision: SshSyncActionRevision {
                    authorization: WireSequence::new(4),
                    configuration: exact_fence.action_revision.configuration,
                },
                ..exact_fence.clone()
            },
            DurableUploadFence {
                action_revision: SshSyncActionRevision {
                    authorization: exact_fence.action_revision.authorization,
                    configuration: WireSequence::new(5),
                },
                ..exact_fence.clone()
            },
        ] {
            assert!(!upload_attempt_matches_fence(&attempt, &drifted));
        }
    }

    #[test]
    fn encrypted_baseline_files_are_content_addressed_and_hash_checked() {
        let directory = tempfile::tempdir().expect("tempdir");
        let plugin_id = "org.example.sync";
        let signer = "a".repeat(64);
        let profile_id = "primary";
        let bytes = b"opaque encrypted exchange";
        let digest =
            persist_baseline_exchange_file(directory.path(), plugin_id, &signer, profile_id, bytes)
                .expect("persist encrypted baseline");
        assert_eq!(
            load_baseline_exchange_file(directory.path(), plugin_id, &signer, profile_id, &digest,)
                .expect("load encrypted baseline"),
            bytes
        );
        assert!(
            load_baseline_exchange_file(
                directory.path(),
                plugin_id,
                &signer,
                profile_id,
                &"b".repeat(64),
            )
            .is_err()
        );
        let other_digest = persist_baseline_exchange_file(
            directory.path(),
            "org.example.other",
            &signer,
            profile_id,
            b"other encrypted exchange",
        )
        .expect("persist other plugin baseline");
        remove_plugin_baseline_files(directory.path(), plugin_id)
            .expect("remove only target plugin baselines");
        assert!(
            load_baseline_exchange_file(directory.path(), plugin_id, &signer, profile_id, &digest,)
                .is_err()
        );
        assert!(
            load_baseline_exchange_file(
                directory.path(),
                "org.example.other",
                &signer,
                profile_id,
                &other_digest,
            )
            .is_ok()
        );
    }

    #[test]
    fn remote_reset_removes_only_the_exact_profile_baselines() {
        let directory = tempfile::tempdir().expect("tempdir");
        let plugin_id = "org.example.sync";
        let signer = "a".repeat(64);
        let first_digest = persist_baseline_exchange_file(
            directory.path(),
            plugin_id,
            &signer,
            "primary",
            b"first encrypted exchange",
        )
        .expect("persist first profile baseline");
        let second_digest = persist_baseline_exchange_file(
            directory.path(),
            plugin_id,
            &signer,
            "secondary",
            b"second encrypted exchange",
        )
        .expect("persist second profile baseline");

        remove_profile_baseline_files(directory.path(), plugin_id, &signer, "primary")
            .expect("remove exact profile baseline");
        assert!(
            load_baseline_exchange_file(
                directory.path(),
                plugin_id,
                &signer,
                "primary",
                &first_digest,
            )
            .is_err()
        );
        assert_eq!(
            load_baseline_exchange_file(
                directory.path(),
                plugin_id,
                &signer,
                "secondary",
                &second_digest,
            )
            .expect("other profile remains"),
            b"second encrypted exchange"
        );
    }

    #[tokio::test]
    async fn lifecycle_epoch_blocks_new_work_and_preserves_the_serialization_lock() {
        let plugin_id = "org.example.sync";
        let namespace = namespace(plugin_id, &"a".repeat(64), "primary");
        let mut operations = BrokerOperations::default();
        let (operation, epoch) = operations
            .register(plugin_id, &namespace)
            .expect("initial operation");
        let held = operation.clone().lock_owned().await;

        let stop_locks = operations.begin_stop(plugin_id);
        assert_eq!(stop_locks.len(), 1);
        assert!(operations.register(plugin_id, &namespace).is_none());
        assert!(!operations.epoch_current(plugin_id, epoch));

        let waiter = tokio::spawn(stop_locks[0].clone().lock_owned());
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "stop must wait for the old holder");
        drop(held);
        let stop_guard = waiter.await.expect("stop waiter");
        operations.finish_stop(plugin_id);
        let (same_operation, next_epoch) = operations
            .register(plugin_id, &namespace)
            .expect("new generation operation");
        assert!(Arc::ptr_eq(&operation, &same_operation));
        assert_ne!(epoch, next_epoch);
        drop(stop_guard);
    }
}
