mod desktop;
use desktop::mapped_desktop_id;

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use norishell_app_persistence::{
    AppPersistenceError, CredentialRecord, CredentialRecordDetails, HostConnectionSnapshot,
    SshSyncChangeFence, SshSyncHttpMethod, SshSyncHttpUploadAttemptInput,
    SshSyncHttpUploadAttemptState, SshSyncHttpUploadCompletionFence,
    SshSyncHttpUploadCompletionProof, SshSyncLocalObjectId, SshSyncLoginAutomation,
    SshSyncLoginAutomationStep, SshSyncObjectKind, SshSyncObjectMappingInput,
    SshSyncOwnedCreateBatch, SshSyncOwnedCredentialDelete, SshSyncOwnedCredentialUpdate,
    SshSyncOwnedDesktopProfileDelete, SshSyncOwnedDesktopProfileUpdate,
    SshSyncOwnedHostBaseVersions, SshSyncOwnedHostDelete, SshSyncOwnedHostUpdate,
    SshSyncOwnedIdentityDelete, SshSyncOwnedIdentityUpdate, SshSyncOwnedMetadataDelta,
    SshSyncOwnedSecretDelete, SshSyncOwnedSecretReplacement, SshSyncProfileKeyBinding,
    SshSyncProfileRemoteBaseline, SshSyncProfileScope, SshSyncProfileScopeMode,
    SshSyncProfileStateCreate, SshSyncProfileStateKey, SshSyncProfileStateRecord,
    SshSyncRestoreCredentialInput, SshSyncRestoreCredentialMaterial,
    SshSyncRestoreDesktopProfileInput, SshSyncRestoreHostInput, SshSyncRestoreIdentityInput,
    SshSyncRestorePlan, SshSyncRestoreSagaInput, SshSyncScopeMembershipInput,
    SshSyncScopeMembershipState,
};
use norishell_core_api::{
    AlgorithmCategory, AuthenticationMethodKind, AuthenticationPlanMode, CredentialRefId,
    DesktopProfile, DesktopProtocol, HeartbeatPolicy, HostCatalogSort, HostConfiguredCreateRequest,
    HostCreateLoginAutomationStep, HostId, IdentityId, LoginAutomationStepInput, MonitoringPolicy,
    OperationId, PluginId, PluginSshSyncHttpMethod, ProxyDnsMode, RequestId, RouteIngress,
    SecretRefId, ShellHeartbeatLineEnding, SshSyncSecureCredential, SshSyncSecureDecision,
    SshSyncSecureDecisionRequest, SshSyncSecureDecisionResponse, SshSyncSecureDesktopProfile,
    SshSyncSecureHost, SshSyncSecureOAuthSummary, SshSyncSecurePrompt,
    SshSyncSecurePromptGetRequest, SshSyncSecurePromptKind, WireSequence,
};
use norishell_secret_vault::{MAX_SECRET_BATCH_ENTRIES, MAX_SECRET_BATCH_VALUE_BYTES, SecretKind};
use norishell_ssh_profile_sync::{
    BundleSchema, HeartbeatMode, LoginAutomationStep as PortableLoginAutomationStep,
    MachineBoundKind, MachineBoundSkipReason, MonitoringResource, PortableAlgorithmCategory,
    PortableAlgorithmCompatibilityException, PortableAlgorithmPolicy, PortableAuthenticationPlan,
    PortableBundleV1, PortableCredential, PortableCredentialMaterial, PortableDesktopProfile,
    PortableDesktopProtocol, PortableHeartbeatPolicy, PortableHost, PortableIdentity,
    PortableLoginAutomation, PortableMonitoringPolicy, PortableObjectId, PortableObjectKind,
    PortableObjects, PortableRoute, PortableSecret, PortableSecretKind,
    PortableShellHeartbeatLineEnding, PortableTombstone, PublicKeyAlgorithm,
    RouteIngress as PortableIngress, SecretBytes, SkippedMachineBoundObject, SyncKey,
    canonical_bundle_bytes,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;
use tauri::{AppHandle, Manager as _, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    host_service::HostService,
    ssh_sync_exchange::{
        ActionFence, ApplyResult, DurableUploadFence, PendingRestoreHandle,
        PortableProfileKeyBinding, PortableProfileState, PortableRemoteBaseline, PortableSnapshot,
        PortableSshProfileStore, PortableStoreError, PortableUploadAttempt,
        PortableUploadAttemptState, PortableUploadCompletionProof, RestorePreview,
        SecureActionContext, SecureApplyApproval, SecureBackupSelection, SecureSelectionError,
        SecureSshSyncUi, SecureSyncDirection, SshSyncActionRevision, SyncDirectionReview,
    },
    vault_service::{VaultSecretInsert, VaultService},
};

type LocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
const PROMPT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const SELECTION_LEASE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const EXPIRY_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const MAX_SELECTION_LEASES: usize = 32;
const MAX_PENDING_RESTORE_PLAINTEXT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone)]
struct BackupSelection {
    desktop_profile_ids: BTreeSet<String>,
    host_ids: BTreeSet<String>,
    credential_ids: BTreeSet<String>,
}

#[derive(Default)]
struct LocalSyncFacts {
    desktop_profile_ids: BTreeSet<String>,
    host_ids: BTreeSet<String>,
    identity_ids: BTreeSet<String>,
    credential_ids: BTreeSet<String>,
    secret_ids: BTreeSet<String>,
}

type PortableIdMap = BTreeMap<(SshSyncObjectKind, String), PortableObjectId>;
type LocalIdMap = BTreeMap<(SshSyncObjectKind, PortableObjectId), SshSyncLocalObjectId>;

struct SecureDecisionPayload {
    desktop_profile_ids: Vec<String>,
    decision: SshSyncSecureDecision,
    host_ids: Vec<HostId>,
    credential_ids: Vec<CredentialRefId>,
    vault_password: Option<Zeroizing<String>>,
    vault_password_confirmation: Option<Zeroizing<String>>,
}

#[derive(Default)]
struct SecurePromptContent {
    desktop_profiles: Vec<SshSyncSecureDesktopProfile>,
    desktop_profile_count: u32,
    remote_desktop_profile_count: u32,
    oauth: Option<SshSyncSecureOAuthSummary>,
    hosts: Vec<SshSyncSecureHost>,
    host_count: u32,
    credential_count: u32,
    conflict_count: u32,
    update_count: u32,
    delete_count: u32,
    remote_host_count: u32,
    remote_credential_count: u32,
    local_compared_at_unix_ms: Option<i64>,
    remote_updated_at_unix_ms: Option<i64>,
    differences: Vec<norishell_core_api::SshSyncSecureDifference>,
    difference_total_count: u32,
    difference_omitted_count: u32,
}

struct PendingPrompt {
    prompt: SshSyncSecurePrompt,
    sender: oneshot::Sender<Option<SecureDecisionPayload>>,
}

struct PendingRestore {
    handle: PendingRestoreHandle,
    bundle: PortableBundleV1,
    owner: SyncOwner,
    bundle_sha256: String,
    plaintext_bytes: usize,
    owned_delta: Option<SshSyncOwnedMetadataDelta>,
    vault_inserts: Vec<VaultSecretInsert>,
    scope_memberships: Option<Vec<SshSyncScopeMembershipInput>>,
    reconcile_noop: bool,
    change_fence: Option<SshSyncChangeFence>,
    expires_at: Instant,
}

struct RestorePlanBuild {
    plan: SshSyncRestorePlan,
    vault_inserts: Vec<VaultSecretInsert>,
    mapping_inputs: Vec<SshSyncObjectMappingInput>,
}

struct ReconcilePlanBuild {
    delta: SshSyncOwnedMetadataDelta,
    vault_inserts: Vec<VaultSecretInsert>,
    create_count: u32,
    update_count: u32,
    delete_count: u32,
}

#[derive(Clone, PartialEq, Eq)]
struct SyncOwner {
    plugin_id: String,
    signer_fingerprint_sha256: String,
    profile_id: String,
}

struct BackupSelectionLease {
    owner: SyncOwner,
    selection: BackupSelection,
    fence: crate::ssh_sync_exchange::ActionFence,
    expires_at: Instant,
}

struct ApplyApprovalLease {
    owner: SyncOwner,
    restore_handle: Option<PendingRestoreHandle>,
    fence: crate::ssh_sync_exchange::ActionFence,
    expires_at: Instant,
}

#[derive(Default)]
struct LocalState {
    pending_prompts: BTreeMap<String, PendingPrompt>,
    backup_selections: BTreeMap<String, BackupSelectionLease>,
    apply_approvals: BTreeMap<String, ApplyApprovalLease>,
    pending_restores: BTreeMap<String, PendingRestore>,
    pending_restore_plaintext_bytes: usize,
}

impl LocalState {
    fn prune_expired(&mut self) {
        let now = Instant::now();
        self.backup_selections
            .retain(|_, lease| lease.expires_at > now && (lease.fence)());
        self.apply_approvals
            .retain(|_, lease| lease.expires_at > now && (lease.fence)());
        self.pending_restores
            .retain(|_, pending| pending.expires_at > now);
        self.pending_restore_plaintext_bytes = self
            .pending_restores
            .values()
            .map(|pending| pending.plaintext_bytes)
            .sum();
    }

    fn clear_plugin(&mut self, plugin_id: &str) -> Vec<String> {
        let prompt_ids = self
            .pending_prompts
            .iter()
            .filter(|(_, pending)| pending.prompt.plugin_id.as_str() == plugin_id)
            .map(|(prompt_id, _)| prompt_id.clone())
            .collect::<Vec<_>>();
        self.pending_prompts
            .retain(|_, pending| pending.prompt.plugin_id.as_str() != plugin_id);
        self.backup_selections
            .retain(|_, lease| lease.owner.plugin_id != plugin_id);
        self.apply_approvals
            .retain(|_, lease| lease.owner.plugin_id != plugin_id);
        self.pending_restores
            .retain(|_, pending| pending.owner.plugin_id != plugin_id);
        self.pending_restore_plaintext_bytes = self
            .pending_restores
            .values()
            .map(|pending| pending.plaintext_bytes)
            .sum();
        prompt_ids
    }
}

fn sync_owner(context: &SecureActionContext) -> SyncOwner {
    SyncOwner {
        plugin_id: context.plugin_id.clone(),
        signer_fingerprint_sha256: context.signer_fingerprint_sha256.clone(),
        profile_id: context.profile_id.clone(),
    }
}

#[derive(Clone)]
pub(crate) struct NoriShellSshSyncLocalAdapter {
    // The store/restore engine is independent of the Secure Surface. Keeping
    // the handle optional lets its real Vault + SQLite integration tests run
    // without a native window; production construction always supplies it.
    app: Option<AppHandle>,
    hosts: HostService,
    vault: VaultService,
    state: Arc<Mutex<LocalState>>,
    restore_commit: Arc<AsyncMutex<()>>,
}

impl NoriShellSshSyncLocalAdapter {
    pub(crate) fn new(app: AppHandle, hosts: HostService, vault: VaultService) -> Self {
        let adapter = Self {
            app: Some(app),
            hosts,
            vault,
            state: Arc::new(Mutex::new(LocalState::default())),
            restore_commit: Arc::new(AsyncMutex::new(())),
        };
        let weak_state = Arc::downgrade(&adapter.state);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(EXPIRY_SWEEP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(state) = weak_state.upgrade() else {
                    break;
                };
                state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .prune_expired();
            }
        });
        let reconcile_adapter = adapter.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(EXPIRY_SWEEP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if reconcile_adapter.vault.is_unlocked() {
                    let _restore_commit = reconcile_adapter.restore_commit.lock().await;
                    let _ = reconcile_adapter.reconcile_durable_restores();
                    let _ = reconcile_adapter.reconcile_plugin_deletes(None);
                }
            }
        });
        adapter
    }

    #[cfg(test)]
    fn new_for_store_test(hosts: HostService, vault: VaultService) -> Self {
        Self {
            app: None,
            hosts,
            vault,
            state: Arc::new(Mutex::new(LocalState::default())),
            restore_commit: Arc::new(AsyncMutex::new(())),
        }
    }

    fn clear_plugin_state(&self, plugin_id: &str) {
        let prompt_ids = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.clear_plugin(plugin_id)
        };
        if let Some(app) = &self.app {
            for prompt_id in prompt_ids {
                if let Some(window) = app.get_webview_window(&secure_window_label(&prompt_id)) {
                    let _ = window.close();
                }
            }
        }
    }

    async fn prompt(
        &self,
        context: SecureActionContext,
        kind: SshSyncSecurePromptKind,
        content: SecurePromptContent,
    ) -> Result<SecureDecisionPayload, SecureSelectionError> {
        if context.background {
            return Err(SecureSelectionError::InteractionRequired);
        }
        let prompt_id = Uuid::new_v4().to_string();
        let prompt = SshSyncSecurePrompt {
            prompt_id: prompt_id.clone(),
            plugin_id: norishell_core_api::PluginId::parse(context.plugin_id.clone())
                .map_err(|_| SecureSelectionError::Unavailable)?,
            profile_id: context.profile_id,
            remote_origin: context.remote_origin,
            kind,
            oauth: content.oauth,
            hosts: content.hosts,
            desktop_profiles: content.desktop_profiles,
            desktop_profile_count: content.desktop_profile_count,
            remote_desktop_profile_count: content.remote_desktop_profile_count,
            host_count: content.host_count,
            credential_count: content.credential_count,
            conflict_count: content.conflict_count,
            update_count: content.update_count,
            delete_count: content.delete_count,
            remote_host_count: content.remote_host_count,
            remote_credential_count: content.remote_credential_count,
            local_compared_at_unix_ms: content.local_compared_at_unix_ms,
            remote_updated_at_unix_ms: content.remote_updated_at_unix_ms,
            differences: content.differences,
            difference_total_count: content.difference_total_count,
            difference_omitted_count: content.difference_omitted_count,
        };
        let (sender, receiver) = oneshot::channel();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_prompts
            .insert(prompt_id.clone(), PendingPrompt { prompt, sender });
        let label = secure_window_label(&prompt_id);
        let app = self.app.as_ref().ok_or(SecureSelectionError::Unavailable)?;
        if let Err(error) =
            crate::secure_window_frame::apply_secure_window_frame(WebviewWindowBuilder::new(
                app,
                label,
                WebviewUrl::App(format!("secure-ssh-sync.html?promptId={prompt_id}").into()),
            ))
            .title("NoriShell")
            .resizable(true)
            .center()
            .build()
        {
            eprintln!("protected SSH sync window could not be created: {error}");
            self.remove_prompt(&prompt_id);
            return Err(SecureSelectionError::Unavailable);
        }
        let mut receiver = receiver;
        let deadline = tokio::time::Instant::now() + PROMPT_TIMEOUT;
        let decision = loop {
            tokio::select! {
                result = &mut receiver => break result.ok().flatten(),
                _ = tokio::time::sleep_until(deadline) => break None,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if !(context.fence)() {
                        eprintln!("protected SSH sync prompt was cancelled because its action fence expired");
                        break None;
                    }
                }
            }
        };
        self.remove_prompt(&prompt_id);
        decision.ok_or(SecureSelectionError::Cancelled)
    }

    fn remove_prompt(&self, prompt_id: &str) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_prompts
            .remove(prompt_id);
        if let Some(app) = &self.app
            && let Some(window) = app.get_webview_window(&secure_window_label(prompt_id))
        {
            let _ = window.close();
        }
    }

    fn issue_apply_approval(
        &self,
        owner: SyncOwner,
        fence: ActionFence,
        restore_handle: Option<PendingRestoreHandle>,
    ) -> Result<SecureApplyApproval, SecureSelectionError> {
        let token = Uuid::new_v4().to_string();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.prune_expired();
        if state.apply_approvals.len() >= MAX_SELECTION_LEASES {
            return Err(SecureSelectionError::Unavailable);
        }
        state.apply_approvals.insert(
            token.clone(),
            ApplyApprovalLease {
                owner: owner.clone(),
                restore_handle,
                fence,
                expires_at: Instant::now() + SELECTION_LEASE_TIMEOUT,
            },
        );
        Ok(SecureApplyApproval::new(
            token,
            owner.plugin_id,
            owner.signer_fingerprint_sha256,
            owner.profile_id,
        ))
    }

    fn selection_hosts(&self) -> Result<Vec<SshSyncSecureHost>, SecureSelectionError> {
        self.hosts
            .list_ssh_sync_snapshots()
            .map_err(|_| SecureSelectionError::Unavailable)
            .map(|snapshots| snapshots.iter().map(secure_host).collect())
    }

    fn profile_key(
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
    ) -> Result<SshSyncProfileStateKey, PortableStoreError> {
        Ok(SshSyncProfileStateKey {
            plugin_id: PluginId::parse(plugin_id).map_err(|_| PortableStoreError::Rejected)?,
            signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
            profile_id: profile_id.to_owned(),
        })
    }

    fn ensure_profile_state(
        &self,
        plugin_id: &str,
        signer_fingerprint_sha256: &str,
        profile_id: &str,
    ) -> Result<SshSyncProfileStateRecord, PortableStoreError> {
        let key = Self::profile_key(plugin_id, signer_fingerprint_sha256, profile_id)?;
        self.hosts
            .with_ssh_sync_repository(|repository| {
                if let Some(existing) = repository.get_ssh_sync_profile_state(&key)? {
                    return Ok(existing);
                }
                repository.ensure_ssh_sync_profile_state(&SshSyncProfileStateCreate {
                    key,
                    scope: SshSyncProfileScope {
                        mode: SshSyncProfileScopeMode::AllEligible,
                        custom_host_ids: Vec::new(),
                        custom_desktop_profile_ids: Vec::new(),
                        custom_credential_ref_ids: Vec::new(),
                    },
                })
            })
            .map_err(map_store_error)
    }

    fn portable_profile_state(record: SshSyncProfileStateRecord) -> PortableProfileState {
        PortableProfileState {
            scope_mode: match record.scope.mode {
                SshSyncProfileScopeMode::AllEligible => {
                    norishell_core_api::PluginSshSyncScopeMode::AllEligible
                }
                SshSyncProfileScopeMode::Custom => {
                    norishell_core_api::PluginSshSyncScopeMode::Custom
                }
            },
            key_binding: record.key_binding.and_then(|binding| {
                binding
                    .password_wrapped_sync_key_envelope
                    .map(|password_wrapped_envelope| PortableProfileKeyBinding {
                        secret_ref_id: binding.sync_key_secret_ref_id,
                        password_wrapped_envelope,
                    })
            }),
            remote_baseline: record
                .remote_baseline
                .map(|baseline| PortableRemoteBaseline {
                    revision: baseline.remote_revision,
                    etag: baseline.remote_etag,
                    content_sha256: baseline.baseline_content_sha256,
                    exchange_sha256: baseline.baseline_exchange_sha256,
                }),
            state_version: record.state_version,
            last_successful_sync_at_unix_ms: record.last_successful_sync_at_unix_ms,
        }
    }

    fn portable_upload_attempt(
        record: norishell_app_persistence::SshSyncHttpUploadAttemptRecord,
    ) -> PortableUploadAttempt {
        PortableUploadAttempt {
            canonical_url: record.input.canonical_url,
            method: match record.input.http_method {
                SshSyncHttpMethod::Put => PluginSshSyncHttpMethod::Put,
                SshSyncHttpMethod::Post => PluginSshSyncHttpMethod::Post,
            },
            use_oauth: record.input.use_oauth,
            authorization_revision: record.input.authorization_revision,
            configuration_revision: record.input.configuration_revision,
            base_revision: record.input.base_revision,
            base_etag: record.input.base_etag,
            target_revision: record.input.target_revision,
            keyed_content_sha256: record.input.keyed_content_sha256,
            body_sha256: record.input.body_sha256,
            idempotency_key: record.input.idempotency_key,
            state: match record.state {
                SshSyncHttpUploadAttemptState::Prepared => PortableUploadAttemptState::Prepared,
                SshSyncHttpUploadAttemptState::Sent => PortableUploadAttemptState::Sent,
                SshSyncHttpUploadAttemptState::Verifying => PortableUploadAttemptState::Verifying,
            },
            state_version: record.state_version,
        }
    }

    fn selection_for_scope(
        &self,
        scope: &SshSyncProfileScope,
    ) -> Result<BackupSelection, PortableStoreError> {
        let snapshots = self
            .hosts
            .list_ssh_sync_snapshots()
            .map_err(map_store_error)?;
        let eligible_hosts = snapshots
            .iter()
            .map(|snapshot| snapshot.host.host_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let mut eligible_credentials = snapshots
            .iter()
            .flat_map(|snapshot| snapshot.credentials.iter())
            .filter(|credential| !machine_bound(credential))
            .map(|credential| credential.credential_ref_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        for snapshot in &snapshots {
            if let Some(credential_ref_id) = proxy_credential_ref(snapshot) {
                eligible_credentials.insert(credential_ref_id.as_str().to_owned());
            }
        }
        let desktops = self.desktop_profiles()?;
        let eligible_desktops = desktops
            .iter()
            .map(|p| p.id.clone())
            .collect::<BTreeSet<_>>();
        for profile in &desktops {
            if let Some(id) = &profile.credential_ref_id {
                self.desktop_password(id)?;
                eligible_credentials.insert(id.as_str().to_owned());
            }
        }
        let selection = match scope.mode {
            SshSyncProfileScopeMode::AllEligible => BackupSelection {
                desktop_profile_ids: eligible_desktops,
                host_ids: eligible_hosts,
                credential_ids: eligible_credentials,
            },
            SshSyncProfileScopeMode::Custom => BackupSelection {
                desktop_profile_ids: scope
                    .custom_desktop_profile_ids
                    .iter()
                    .filter(|id| eligible_desktops.contains(*id))
                    .cloned()
                    .collect(),
                host_ids: scope
                    .custom_host_ids
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .filter(|value| eligible_hosts.contains(value))
                    .collect(),
                credential_ids: scope
                    .custom_credential_ref_ids
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .filter(|value| eligible_credentials.contains(value))
                    .collect(),
            },
        };
        self.add_route_dependencies(selection, &snapshots)
    }

    fn add_route_dependencies(
        &self,
        mut selection: BackupSelection,
        snapshots: &[HostConnectionSnapshot],
    ) -> Result<BackupSelection, PortableStoreError> {
        // Add desktop and gateway dependencies as configuration only; passwords still require explicit selection.
        for profile in self
            .desktop_profiles()?
            .iter()
            .filter(|p| selection.desktop_profile_ids.contains(&p.id))
        {
            for host in [&profile.host_id, &profile.gateway_host_id]
                .into_iter()
                .flatten()
            {
                selection.host_ids.insert(host.as_str().to_owned());
            }
        }
        loop {
            let before = selection.host_ids.len();
            let dependencies = snapshots
                .iter()
                .filter(|p| selection.host_ids.contains(p.host.host_id.as_str()))
                .flat_map(|p| {
                    p.config
                        .route_plan
                        .jump_host_ids
                        .iter()
                        .map(|id| id.as_str().to_owned())
                })
                .collect::<Vec<_>>();
            selection.host_ids.extend(dependencies);
            if selection.host_ids.len() == before {
                break;
            }
        }
        for snapshot in snapshots
            .iter()
            .filter(|snapshot| selection.host_ids.contains(snapshot.host.host_id.as_str()))
        {
            if let Some(credential_ref_id) = proxy_credential_ref(snapshot) {
                let credential = self
                    .hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.get_ready_credential_record(credential_ref_id)
                    })
                    .map_err(map_store_error)?;
                if !matches!(credential.details, CredentialRecordDetails::Password { .. }) {
                    return Err(PortableStoreError::Rejected);
                }
                selection
                    .credential_ids
                    .insert(credential_ref_id.as_str().to_owned());
            }
        }
        Ok(selection)
    }

    fn sync_referenced_local_facts(
        &self,
        snapshots: &[HostConnectionSnapshot],
    ) -> Result<LocalSyncFacts, PortableStoreError> {
        let mut facts = LocalSyncFacts::default();
        for snapshot in snapshots {
            facts
                .host_ids
                .insert(snapshot.host.host_id.as_str().to_owned());
            if let Some(identity) = &snapshot.identity {
                facts
                    .identity_ids
                    .insert(identity.identity_id.as_str().to_owned());
            }
            for credential in &snapshot.credentials {
                add_credential_to_local_facts(&mut facts, credential);
            }
            if let Some(credential_ref_id) = proxy_credential_ref(snapshot) {
                let credential = self
                    .hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.get_ready_credential_record(credential_ref_id)
                    })
                    .map_err(map_store_error)?;
                if !matches!(credential.details, CredentialRecordDetails::Password { .. }) {
                    return Err(PortableStoreError::Rejected);
                }
                add_credential_to_local_facts(&mut facts, &credential);
            }
            for step in &snapshot.login_automation.steps {
                if let LoginAutomationStepInput::SendSecret { secret_ref_id, .. } = step {
                    facts.secret_ids.insert(secret_ref_id.as_str().to_owned());
                }
            }
        }
        // Determine existence from global entities; deleting a desktop profile does not delete its independent password.
        self.hosts
            .with_ssh_sync_repository(|repository| {
                for identity in repository.list_identities()? {
                    facts
                        .identity_ids
                        .insert(identity.identity_id.as_str().to_owned());
                    for summary in repository.list_credential_refs(&identity.identity_id)? {
                        facts
                            .credential_ids
                            .insert(summary.credential_ref_id.as_str().to_owned());
                        match repository.get_credential_import_record(&summary.credential_ref_id) {
                            Ok(credential) => {
                                add_credential_to_local_facts(&mut facts, &credential)
                            }
                            Err(AppPersistenceError::NotFound) => {}
                            Err(error) => return Err(error),
                        }
                    }
                }
                Ok(())
            })
            .map_err(map_store_error)?;
        facts.desktop_profile_ids = self.desktop_profiles()?.into_iter().map(|p| p.id).collect();
        Ok(facts)
    }

    fn portable_ids_for_selection(
        &self,
        owner: &SshSyncProfileStateKey,
        selection: &BackupSelection,
    ) -> Result<PortableIdMap, PortableStoreError> {
        let snapshots = self
            .hosts
            .list_ssh_sync_snapshots()
            .map_err(map_store_error)?;
        let existing = self
            .hosts
            .with_ssh_sync_repository(|repository| repository.list_ssh_sync_object_mappings(owner))
            .map_err(map_store_error)?;
        let mut known = existing
            .into_iter()
            .map(|mapping| {
                let local_id = local_mapping_id(&mapping.local_object_id).to_owned();
                let portable_id = Uuid::parse_str(&mapping.portable_object_id)
                    .map_err(|_| PortableStoreError::Rejected)
                    .and_then(|value| {
                        PortableObjectId::from_uuid(value).map_err(|_| PortableStoreError::Rejected)
                    })?;
                Ok(((mapping.object_kind, local_id), portable_id))
            })
            .collect::<Result<PortableIdMap, PortableStoreError>>()?;
        let mut inputs = Vec::new();
        let mut input_keys = BTreeSet::new();
        let mut ensure = |kind: SshSyncObjectKind, local_object_id: SshSyncLocalObjectId| {
            let key = (kind, local_mapping_id(&local_object_id).to_owned());
            let portable_id = known.entry(key).or_insert_with(PortableObjectId::new);
            if input_keys.insert((kind, local_mapping_id(&local_object_id).to_owned())) {
                inputs.push(SshSyncObjectMappingInput {
                    portable_object_id: portable_id.as_uuid().to_string(),
                    local_object_id,
                });
            }
        };
        for snapshot in snapshots
            .iter()
            .filter(|snapshot| selection.host_ids.contains(snapshot.host.host_id.as_str()))
        {
            ensure(
                SshSyncObjectKind::Host,
                SshSyncLocalObjectId::Host(snapshot.host.host_id.clone()),
            );
            if let Some(identity) = &snapshot.identity {
                ensure(
                    SshSyncObjectKind::Identity,
                    SshSyncLocalObjectId::Identity(identity.identity_id.clone()),
                );
            }
            for credential in snapshot.credentials.iter().filter(|credential| {
                !machine_bound(credential)
                    && selection
                        .credential_ids
                        .contains(credential.credential_ref_id.as_str())
            }) {
                ensure(
                    SshSyncObjectKind::Credential,
                    SshSyncLocalObjectId::Credential(credential.credential_ref_id.clone()),
                );
                for secret_ref_id in credential_secret_refs(credential) {
                    ensure(
                        SshSyncObjectKind::Secret,
                        SshSyncLocalObjectId::Secret(secret_ref_id),
                    );
                }
            }
            if let Some(credential_ref_id) = proxy_credential_ref(snapshot) {
                let credential = self
                    .hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.get_ready_credential_record(credential_ref_id)
                    })
                    .map_err(map_store_error)?;
                ensure(
                    SshSyncObjectKind::Identity,
                    SshSyncLocalObjectId::Identity(credential.identity_id.clone()),
                );
                ensure(
                    SshSyncObjectKind::Credential,
                    SshSyncLocalObjectId::Credential(credential.credential_ref_id.clone()),
                );
                for secret_ref_id in credential_secret_refs(&credential) {
                    ensure(
                        SshSyncObjectKind::Secret,
                        SshSyncLocalObjectId::Secret(secret_ref_id),
                    );
                }
            }
            for step in &snapshot.login_automation.steps {
                match step {
                    LoginAutomationStepInput::SendSecret { secret_ref_id, .. } => ensure(
                        SshSyncObjectKind::Secret,
                        SshSyncLocalObjectId::Secret(
                            SecretRefId::parse(secret_ref_id.as_str())
                                .map_err(|_| PortableStoreError::Rejected)?,
                        ),
                    ),
                    LoginAutomationStepInput::PreserveExistingSecret { .. } => {
                        return Err(PortableStoreError::Rejected);
                    }
                    LoginAutomationStepInput::Expect { .. }
                    | LoginAutomationStepInput::SendText { .. } => {}
                }
            }
        }
        for profile in self
            .desktop_profiles()?
            .iter()
            .filter(|p| selection.desktop_profile_ids.contains(&p.id))
        {
            ensure(
                SshSyncObjectKind::DesktopProfile,
                SshSyncLocalObjectId::DesktopProfile(profile.id.clone()),
            );
            if let Some(id) = &profile.credential_ref_id
                && selection.credential_ids.contains(id.as_str())
            {
                let credential = self.desktop_password(id)?;
                ensure(
                    SshSyncObjectKind::Identity,
                    SshSyncLocalObjectId::Identity(credential.identity_id.clone()),
                );
                ensure(
                    SshSyncObjectKind::Credential,
                    SshSyncLocalObjectId::Credential(id.clone()),
                );
                for secret in credential_secret_refs(&credential) {
                    ensure(
                        SshSyncObjectKind::Secret,
                        SshSyncLocalObjectId::Secret(secret),
                    );
                }
            }
        }
        self.hosts
            .with_ssh_sync_repository(|repository| {
                repository.resolve_or_create_ssh_sync_object_mappings(owner, &inputs)
            })
            .map_err(map_store_error)?;
        Ok(known)
    }

    fn local_ids_for_owner(
        &self,
        owner: &SshSyncProfileStateKey,
    ) -> Result<LocalIdMap, PortableStoreError> {
        self.hosts
            .with_ssh_sync_repository(|repository| repository.list_ssh_sync_object_mappings(owner))
            .map_err(map_store_error)?
            .into_iter()
            .map(|mapping| {
                let portable_id = PortableObjectId::from_uuid(
                    Uuid::parse_str(&mapping.portable_object_id)
                        .map_err(|_| PortableStoreError::Rejected)?,
                )
                .map_err(|_| PortableStoreError::Rejected)?;
                Ok(((mapping.object_kind, portable_id), mapping.local_object_id))
            })
            .collect()
    }

    fn apply_scope_membership_exclusions(
        &self,
        owner: &SshSyncProfileStateKey,
        mut selection: BackupSelection,
    ) -> Result<BackupSelection, PortableStoreError> {
        let mappings = self.local_ids_for_owner(owner)?;
        let memberships = self
            .hosts
            .with_ssh_sync_repository(|repository| {
                repository.list_ssh_sync_scope_memberships(owner)
            })
            .map_err(map_store_error)?;
        for record in memberships {
            if record.membership.state != SshSyncScopeMembershipState::Excluded {
                continue;
            }
            let portable_id = PortableObjectId::from_uuid(
                Uuid::parse_str(&record.membership.portable_object_id)
                    .map_err(|_| PortableStoreError::Rejected)?,
            )
            .map_err(|_| PortableStoreError::Rejected)?;
            match mappings.get(&(record.membership.object_kind, portable_id)) {
                Some(SshSyncLocalObjectId::DesktopProfile(value)) => {
                    selection.desktop_profile_ids.remove(value);
                }
                Some(SshSyncLocalObjectId::Host(value)) => {
                    selection.host_ids.remove(value.as_str());
                }
                Some(SshSyncLocalObjectId::Credential(value)) => {
                    selection.credential_ids.remove(value.as_str());
                }
                _ => {}
            }
        }
        Ok(selection)
    }

    fn scope_memberships_for_bundle(
        &self,
        owner: &SshSyncProfileStateKey,
        bundle: &PortableBundleV1,
    ) -> Result<Vec<SshSyncScopeMembershipInput>, PortableStoreError> {
        let mut memberships = BTreeMap::new();
        let mut include = |kind: SshSyncObjectKind, id: PortableObjectId| {
            memberships.insert(
                (kind, id),
                SshSyncScopeMembershipInput {
                    object_kind: kind,
                    portable_object_id: id.as_uuid().to_string(),
                    state: SshSyncScopeMembershipState::Included,
                },
            );
        };
        for value in &bundle.objects.desktop_profiles {
            include(SshSyncObjectKind::DesktopProfile, value.id);
        }
        for value in &bundle.objects.hosts {
            include(SshSyncObjectKind::Host, value.id);
        }
        for value in &bundle.objects.identities {
            include(SshSyncObjectKind::Identity, value.id);
        }
        for value in &bundle.objects.credentials {
            include(SshSyncObjectKind::Credential, value.id);
        }
        for value in &bundle.secrets {
            include(SshSyncObjectKind::Secret, value.id);
        }
        let tombstones = bundle
            .tombstones
            .iter()
            .map(|value| (value.kind, value.id))
            .collect::<BTreeSet<_>>();
        for ((kind, portable_id), _) in self.local_ids_for_owner(owner)? {
            let portable_kind = portable_object_kind(kind);
            if !memberships.contains_key(&(kind, portable_id))
                && !tombstones.contains(&(portable_kind, portable_id))
            {
                memberships.insert(
                    (kind, portable_id),
                    SshSyncScopeMembershipInput {
                        object_kind: kind,
                        portable_object_id: portable_id.as_uuid().to_string(),
                        state: SshSyncScopeMembershipState::Excluded,
                    },
                );
            }
        }
        Ok(memberships.into_values().collect())
    }

    pub(crate) fn prompt_snapshot(
        &self,
        request: SshSyncSecurePromptGetRequest,
        window: &WebviewWindow,
    ) -> Result<SshSyncSecurePrompt, String> {
        require_secure_window(window, &request.prompt_id)?;
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_prompts
            .get(&request.prompt_id)
            .map(|pending| pending.prompt.clone())
            .ok_or_else(|| Uuid::new_v4().to_string())
    }

    pub(crate) fn decide_prompt(
        &self,
        mut request: SshSyncSecureDecisionRequest,
        window: &WebviewWindow,
    ) -> Result<SshSyncSecureDecisionResponse, String> {
        let result = (|| {
            require_secure_window(window, &request.prompt_id)?;
            let pending = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pending_prompts
                .remove(&request.prompt_id)
                .ok_or_else(|| Uuid::new_v4().to_string())?;
            let decision = if request.decision == SshSyncSecureDecision::Cancel {
                None
            } else {
                validate_decision(&pending.prompt, &request)?;
                Some(SecureDecisionPayload {
                    decision: request.decision,
                    desktop_profile_ids: std::mem::take(&mut request.selected_desktop_profile_ids),
                    host_ids: std::mem::take(&mut request.selected_host_ids),
                    credential_ids: std::mem::take(&mut request.selected_credential_ref_ids),
                    vault_password: request.vault_password.take().map(Zeroizing::new),
                    vault_password_confirmation: request
                        .vault_password_confirmation
                        .take()
                        .map(Zeroizing::new),
                })
            };
            pending
                .sender
                .send(decision)
                .map_err(|_| Uuid::new_v4().to_string())?;
            Ok(SshSyncSecureDecisionResponse { accepted: true })
        })();
        if let Some(value) = &mut request.vault_password {
            value.zeroize();
        }
        if let Some(value) = &mut request.vault_password_confirmation {
            value.zeroize();
        }
        result
    }

    fn build_snapshot_with_ids(
        &self,
        selection: BackupSelection,
        revision: u64,
        portable_ids: Option<&PortableIdMap>,
    ) -> Result<PortableSnapshot, PortableStoreError> {
        let all = self
            .hosts
            .list_ssh_sync_snapshots()
            .map_err(map_store_error)?;
        let host_tags = self
            .hosts
            .with_ssh_sync_repository(|repository| {
                repository.list_host_catalog(HostCatalogSort::Label)
            })
            .map_err(map_store_error)?
            .into_iter()
            .map(|entry| {
                (
                    entry.host.host_id.as_str().to_owned(),
                    entry
                        .tags
                        .into_iter()
                        .map(|tag| tag.label)
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let selected = all
            .into_iter()
            .filter(|snapshot| selection.host_ids.contains(snapshot.host.host_id.as_str()))
            .collect::<Vec<_>>();
        if selected.len() != selection.host_ids.len() {
            return Err(PortableStoreError::InvalidSelection);
        }
        let selected_host_ids = selected
            .iter()
            .map(|snapshot| snapshot.host.host_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        if selected.iter().any(|snapshot| {
            snapshot
                .config
                .route_plan
                .jump_host_ids
                .iter()
                .any(|host_id| !selected_host_ids.contains(host_id.as_str()))
        }) {
            return Err(PortableStoreError::InvalidSelection);
        }

        let mut objects = PortableObjects::default();
        let mut secrets = Vec::new();
        let mut skipped_machine_bound = Vec::new();
        let mut identity_credentials = BTreeMap::<String, Vec<PortableObjectId>>::new();
        let mut portable_credential_ids = BTreeMap::<String, PortableObjectId>::new();
        let mut credential_facts = BTreeMap::<String, CredentialRecord>::new();
        let mut identity_facts = BTreeMap::new();

        for snapshot in &selected {
            if let Some(identity) = &snapshot.identity {
                identity_facts
                    .entry(identity.identity_id.as_str().to_owned())
                    .or_insert_with(|| identity.clone());
            }
            for credential in &snapshot.credentials {
                if machine_bound(credential) {
                    skipped_machine_bound.push(SkippedMachineBoundObject {
                        id: portable_id("machine-bound", credential.credential_ref_id.as_str()),
                        kind: machine_bound_kind(credential),
                        reason: MachineBoundSkipReason::MachineBound,
                    });
                    continue;
                }
                if !selection
                    .credential_ids
                    .contains(credential.credential_ref_id.as_str())
                {
                    continue;
                }
                credential_facts
                    .entry(credential.credential_ref_id.as_str().to_owned())
                    .or_insert_with(|| credential.clone());
            }
            if let Some(credential_ref_id) = proxy_credential_ref(snapshot) {
                let credential = self
                    .hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.get_ready_credential_record(credential_ref_id)
                    })
                    .map_err(map_store_error)?;
                if !matches!(credential.details, CredentialRecordDetails::Password { .. }) {
                    return Err(PortableStoreError::Rejected);
                }
                let identity = self
                    .hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.get_identity(&credential.identity_id)
                    })
                    .map_err(map_store_error)?;
                identity_facts
                    .entry(identity.identity_id.as_str().to_owned())
                    .or_insert(identity);
                credential_facts
                    .entry(credential.credential_ref_id.as_str().to_owned())
                    .or_insert(credential);
            }
        }
        let desktops = self
            .desktop_profiles()?
            .into_iter()
            .filter(|p| selection.desktop_profile_ids.contains(&p.id))
            .collect::<Vec<_>>();
        if desktops.len() != selection.desktop_profile_ids.len() {
            return Err(PortableStoreError::InvalidSelection);
        }
        for profile in &desktops {
            if let Some(id) = &profile.credential_ref_id
                && selection.credential_ids.contains(id.as_str())
            {
                let credential = self.desktop_password(id)?;
                let identity = self
                    .hosts
                    .with_ssh_sync_repository(|r| r.get_identity(&credential.identity_id))
                    .map_err(map_store_error)?;
                identity_facts.insert(identity.identity_id.as_str().to_owned(), identity);
                credential_facts.insert(id.as_str().to_owned(), credential);
            }
        }
        if credential_facts.keys().cloned().collect::<BTreeSet<_>>() != selection.credential_ids {
            return Err(PortableStoreError::InvalidSelection);
        }

        let mut proxy_route_auth = BTreeMap::new();
        for (credential_key, credential) in &credential_facts {
            let portable = self.portable_credential(credential, &mut secrets, portable_ids)?;
            if matches!(
                portable.material,
                PortableCredentialMaterial::Password { .. }
            ) {
                let username = identity_facts
                    .get(credential.identity_id.as_str())
                    .and_then(|identity| identity.username.clone());
                proxy_route_auth.insert(credential_key.clone(), (username, portable.id));
            }
            portable_credential_ids.insert(credential_key.clone(), portable.id);
            identity_credentials
                .entry(credential.identity_id.as_str().to_owned())
                .or_default()
                .push(portable.id);
            objects.credentials.push(portable);
        }

        for (identity_key, identity) in identity_facts {
            objects.identities.push(PortableIdentity {
                id: resolved_portable_id(
                    portable_ids,
                    SshSyncObjectKind::Identity,
                    &identity_key,
                    "identity",
                ),
                label: identity.label,
                username: identity.username,
                credential_ids: identity_credentials
                    .remove(&identity_key)
                    .unwrap_or_default(),
            });
        }

        for snapshot in selected {
            let host_key = snapshot.host.host_id.as_str();
            let host_id =
                resolved_portable_id(portable_ids, SshSyncObjectKind::Host, host_key, "host");
            let portable_host_key = host_id.as_uuid().to_string();
            let route_id = portable_id("route", &portable_host_key);
            let auth_id = portable_id("auth", &portable_host_key);
            let algorithm_id = portable_id("algorithm", &portable_host_key);
            let heartbeat_id = portable_id("heartbeat", &portable_host_key);
            let monitoring_id = portable_id("monitoring", &portable_host_key);
            let login_automation_id = if snapshot.login_automation.enabled {
                let automation_id = portable_id("login-automation", &portable_host_key);
                let mut steps = Vec::with_capacity(snapshot.login_automation.steps.len());
                for step in &snapshot.login_automation.steps {
                    steps.push(match step {
                        LoginAutomationStepInput::Expect {
                            literal_text,
                            timeout_seconds,
                        } => PortableLoginAutomationStep::Expect {
                            pattern: literal_text.clone(),
                            timeout_seconds: u32::from(*timeout_seconds),
                        },
                        LoginAutomationStepInput::SendText {
                            text,
                            append_enter,
                            timeout_seconds,
                        } => PortableLoginAutomationStep::SendText {
                            text: text.clone(),
                            append_enter: *append_enter,
                            timeout_seconds: u32::from(*timeout_seconds),
                        },
                        LoginAutomationStepInput::SendSecret {
                            secret_ref_id,
                            secret_label,
                            append_enter,
                            timeout_seconds,
                        } => {
                            let local_secret_ref_id = SecretRefId::parse(secret_ref_id.as_str())
                                .map_err(|_| PortableStoreError::Rejected)?;
                            let secret_id = resolved_portable_id(
                                portable_ids,
                                SshSyncObjectKind::Secret,
                                local_secret_ref_id.as_str(),
                                "secret-login-automation",
                            );
                            secrets.push(self.portable_secret(
                                secret_id,
                                PortableSecretKind::LoginAutomation,
                                &local_secret_ref_id,
                                SecretKind::LoginAutomation,
                            )?);
                            PortableLoginAutomationStep::SendSecret {
                                secret_id,
                                secret_label: secret_label.clone(),
                                append_enter: *append_enter,
                                timeout_seconds: u32::from(*timeout_seconds),
                            }
                        }
                        LoginAutomationStepInput::PreserveExistingSecret { .. } => {
                            return Err(PortableStoreError::Rejected);
                        }
                    });
                }
                objects.login_automations.push(PortableLoginAutomation {
                    id: automation_id,
                    steps,
                });
                Some(automation_id)
            } else {
                None
            };
            objects.routes.push(portable_route(
                &snapshot,
                route_id,
                portable_ids,
                &proxy_route_auth,
            )?);
            objects
                .authentication_plans
                .push(PortableAuthenticationPlan {
                    id: auth_id,
                    credential_ids: snapshot
                        .config
                        .authentication_plan
                        .credential_ref_ids
                        .iter()
                        .filter_map(|id| portable_credential_ids.get(id.as_str()).copied())
                        .collect(),
                });
            objects
                .algorithm_policies
                .push(portable_algorithm_policy(&snapshot, algorithm_id));
            objects
                .heartbeat_policies
                .push(portable_heartbeat_policy(&snapshot, heartbeat_id));
            objects
                .monitoring_policies
                .push(portable_monitoring_policy(&snapshot, monitoring_id));
            objects.hosts.push(PortableHost {
                id: host_id,
                label: snapshot.host.label,
                address: snapshot.host.address,
                port: snapshot.host.port,
                username: snapshot.host.username,
                favorite: snapshot.host.favorite,
                tags: host_tags.get(host_key).cloned().unwrap_or_default(),
                identity_id: snapshot.identity.as_ref().map(|identity| {
                    resolved_portable_id(
                        portable_ids,
                        SshSyncObjectKind::Identity,
                        identity.identity_id.as_str(),
                        "identity",
                    )
                }),
                route_id,
                authentication_plan_id: auth_id,
                algorithm_policy_id: algorithm_id,
                heartbeat_policy_id: heartbeat_id,
                monitoring_policy_id: monitoring_id,
                login_automation_id,
            });
        }
        for profile in desktops {
            let map_host =
                |id: &Option<HostId>| -> Result<Option<PortableObjectId>, PortableStoreError> {
                    id.as_ref()
                        .map(|id| {
                            if !selection.host_ids.contains(id.as_str()) {
                                return Err(PortableStoreError::InvalidSelection);
                            }
                            Ok(resolved_portable_id(
                                portable_ids,
                                SshSyncObjectKind::Host,
                                id.as_str(),
                                "host",
                            ))
                        })
                        .transpose()
                };
            objects.desktop_profiles.push(PortableDesktopProfile {
                id: resolved_portable_id(
                    portable_ids,
                    SshSyncObjectKind::DesktopProfile,
                    &profile.id,
                    "desktop-profile",
                ),
                label: profile.label,
                protocol: match profile.protocol {
                    DesktopProtocol::Rdp => PortableDesktopProtocol::Rdp,
                    DesktopProtocol::Vnc => PortableDesktopProtocol::Vnc,
                },
                address: profile.address,
                port: profile.port,
                username: profile.username,
                domain: profile.domain,
                host_id: map_host(&profile.host_id)?,
                gateway_host_id: map_host(&profile.gateway_host_id)?,
                credential_id: profile
                    .credential_ref_id
                    .as_ref()
                    .and_then(|id| portable_credential_ids.get(id.as_str()).copied()),
                width: profile.width,
                height: profile.height,
                clipboard_enabled: profile.clipboard_enabled,
                audio_playback_enabled: profile.audio_playback_enabled,
            });
        }
        secrets.sort_by_key(|secret| secret.id);
        secrets.dedup_by_key(|secret| secret.id);
        let bundle = PortableBundleV1 {
            schema: BundleSchema::V3,
            revision,
            objects,
            secrets,
            skipped_machine_bound: if portable_ids.is_some() {
                Vec::new()
            } else {
                skipped_machine_bound
            },
            tombstones: Vec::new(),
        };
        bundle
            .validate()
            .map_err(|_| PortableStoreError::Rejected)?;
        let host_count = u32::try_from(bundle.objects.hosts.len()).unwrap_or(u32::MAX);
        let credential_count = u32::try_from(bundle.objects.credentials.len()).unwrap_or(u32::MAX);
        let desktop_profile_count = bounded_len(bundle.objects.desktop_profiles.len())?;
        Ok(PortableSnapshot {
            desktop_profile_count,
            bundle,
            host_count,
            credential_count,
        })
    }

    fn portable_credential(
        &self,
        credential: &CredentialRecord,
        secrets: &mut Vec<PortableSecret>,
        portable_ids: Option<&PortableIdMap>,
    ) -> Result<PortableCredential, PortableStoreError> {
        let id = resolved_portable_id(
            portable_ids,
            SshSyncObjectKind::Credential,
            credential.credential_ref_id.as_str(),
            "credential",
        );
        let identity_id = resolved_portable_id(
            portable_ids,
            SshSyncObjectKind::Identity,
            credential.identity_id.as_str(),
            "identity",
        );
        let material = match &credential.details {
            CredentialRecordDetails::Password { secret_ref_id } => {
                let secret_id = resolved_portable_id(
                    portable_ids,
                    SshSyncObjectKind::Secret,
                    secret_ref_id.as_str(),
                    "secret-password",
                );
                secrets.push(self.portable_secret(
                    secret_id,
                    PortableSecretKind::Password,
                    secret_ref_id,
                    SecretKind::Password,
                )?);
                PortableCredentialMaterial::Password {
                    password_secret_id: secret_id,
                }
            }
            CredentialRecordDetails::PrivateKey {
                secret_ref_id,
                passphrase_secret_ref_id,
                public_key_algorithm: key_algorithm,
                public_key_fingerprint,
            } => {
                let private_key_secret_id = resolved_portable_id(
                    portable_ids,
                    SshSyncObjectKind::Secret,
                    secret_ref_id.as_str(),
                    "secret-private-key",
                );
                secrets.push(self.portable_secret(
                    private_key_secret_id,
                    PortableSecretKind::PrivateKey,
                    secret_ref_id,
                    SecretKind::PrivateKey,
                )?);
                let passphrase_secret_id = passphrase_secret_ref_id
                    .as_ref()
                    .map(|secret_ref_id| {
                        let id = resolved_portable_id(
                            portable_ids,
                            SshSyncObjectKind::Secret,
                            secret_ref_id.as_str(),
                            "secret-passphrase",
                        );
                        self.portable_secret(
                            id,
                            PortableSecretKind::Passphrase,
                            secret_ref_id,
                            SecretKind::Passphrase,
                        )
                        .map(|secret| {
                            secrets.push(secret);
                            id
                        })
                    })
                    .transpose()?;
                PortableCredentialMaterial::PrivateKey {
                    algorithm: key_algorithm
                        .as_deref()
                        .and_then(parse_public_key_algorithm)
                        .ok_or(PortableStoreError::Rejected)?,
                    public_key_fingerprint: public_key_fingerprint
                        .clone()
                        .ok_or(PortableStoreError::Rejected)?,
                    private_key_secret_id,
                    passphrase_secret_id,
                }
            }
            CredentialRecordDetails::KeyboardInteractive { max_rounds } => {
                PortableCredentialMaterial::KeyboardInteractive {
                    max_rounds: *max_rounds,
                }
            }
            CredentialRecordDetails::SshAgent { .. }
            | CredentialRecordDetails::Certificate { .. }
            | CredentialRecordDetails::HardwareKey { .. } => {
                return Err(PortableStoreError::InvalidSelection);
            }
        };
        Ok(PortableCredential {
            id,
            identity_id,
            label: credential.label.clone(),
            material,
        })
    }

    fn portable_secret(
        &self,
        id: PortableObjectId,
        kind: PortableSecretKind,
        secret_ref_id: &norishell_core_api::SecretRefId,
        vault_kind: SecretKind,
    ) -> Result<PortableSecret, PortableStoreError> {
        let value = self
            .vault
            .read_secret(secret_ref_id, vault_kind)
            .map_err(|_| PortableStoreError::Rejected)?;
        Ok(PortableSecret {
            id,
            kind,
            selected_by_user: true,
            payload: SecretBytes::new(value.expose().to_vec())
                .map_err(|_| PortableStoreError::Rejected)?,
        })
    }

    fn build_restore_plan(
        &self,
        bundle: &PortableBundleV1,
        owner: &SyncOwner,
        bundle_sha256: &str,
    ) -> Result<RestorePlanBuild, PortableStoreError> {
        bundle
            .validate()
            .map_err(|_| PortableStoreError::Rejected)?;
        preflight_restore(bundle)?;
        let plugin_id =
            PluginId::parse(owner.plugin_id.clone()).map_err(|_| PortableStoreError::Rejected)?;
        let profile_state = self.ensure_profile_state(
            &owner.plugin_id,
            &owner.signer_fingerprint_sha256,
            &owner.profile_id,
        )?;
        let local_ids = self.local_ids_for_owner(&profile_state.key)?;
        let restore_namespace = format!(
            "{}\0{}\0{}",
            owner.plugin_id, owner.signer_fingerprint_sha256, owner.profile_id
        );
        let attempt_id =
            deterministic_named_operation_id(&restore_namespace, "attempt", bundle_sha256)?;
        let plan_sha256 = sha256_text(&format!(
            "norishell:ssh-sync-restore-plan:v1:{restore_namespace}:{bundle_sha256}"
        ));

        let identity_ids = bundle
            .objects
            .identities
            .iter()
            .map(|identity| {
                mapped_identity_id(&local_ids, identity.id)
                    .map_or_else(
                        || deterministic_identity_id(&restore_namespace, identity.id),
                        Ok,
                    )
                    .map(|id| (identity.id, id))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let identities = bundle
            .objects
            .identities
            .iter()
            .map(|identity| {
                Ok(SshSyncRestoreIdentityInput {
                    identity_id: identity_ids
                        .get(&identity.id)
                        .cloned()
                        .ok_or(PortableStoreError::Rejected)?,
                    label: identity.label.clone(),
                    username: identity.username.clone(),
                })
            })
            .collect::<Result<Vec<_>, PortableStoreError>>()?;

        let credential_priorities = bundle
            .objects
            .identities
            .iter()
            .flat_map(|identity| {
                identity
                    .credential_ids
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(priority, credential_id)| (credential_id, priority))
            })
            .map(|(credential_id, priority)| {
                u32::try_from(priority)
                    .map(|priority| (credential_id, priority))
                    .map_err(|_| PortableStoreError::Rejected)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let secret_map = bundle
            .secrets
            .iter()
            .map(|secret| (secret.id, secret))
            .collect::<BTreeMap<_, _>>();
        let mut credential_ids = BTreeMap::new();
        let mut credentials = Vec::with_capacity(bundle.objects.credentials.len());
        let mut vault_inserts = BTreeMap::<String, VaultSecretInsert>::new();
        for credential in &bundle.objects.credentials {
            let identity_id = identity_ids
                .get(&credential.identity_id)
                .cloned()
                .ok_or(PortableStoreError::Rejected)?;
            let credential_ref_id = mapped_credential_id(&local_ids, credential.id).map_or_else(
                || deterministic_credential_ref_id(&restore_namespace, credential.id),
                Ok,
            )?;
            let material = match credential.material {
                PortableCredentialMaterial::Password { password_secret_id } => {
                    let secret_ref_id = stable_secret_insert(
                        &restore_namespace,
                        &local_ids,
                        password_secret_id,
                        PortableSecretKind::Password,
                        SecretKind::Password,
                        &secret_map,
                        &mut vault_inserts,
                    )?;
                    SshSyncRestoreCredentialMaterial::Password { secret_ref_id }
                }
                PortableCredentialMaterial::PrivateKey {
                    algorithm,
                    ref public_key_fingerprint,
                    private_key_secret_id,
                    passphrase_secret_id,
                } => {
                    let secret_ref_id = stable_secret_insert(
                        &restore_namespace,
                        &local_ids,
                        private_key_secret_id,
                        PortableSecretKind::PrivateKey,
                        SecretKind::PrivateKey,
                        &secret_map,
                        &mut vault_inserts,
                    )?;
                    let passphrase_secret_ref_id = passphrase_secret_id
                        .map(|secret_id| {
                            stable_secret_insert(
                                &restore_namespace,
                                &local_ids,
                                secret_id,
                                PortableSecretKind::Passphrase,
                                SecretKind::Passphrase,
                                &secret_map,
                                &mut vault_inserts,
                            )
                        })
                        .transpose()?;
                    SshSyncRestoreCredentialMaterial::PrivateKey {
                        secret_ref_id,
                        passphrase_secret_ref_id,
                        public_key_algorithm: public_key_algorithm_wire(algorithm).to_owned(),
                        public_key_fingerprint: public_key_fingerprint.clone(),
                    }
                }
                PortableCredentialMaterial::KeyboardInteractive { max_rounds } => {
                    SshSyncRestoreCredentialMaterial::KeyboardInteractive { max_rounds }
                }
                PortableCredentialMaterial::Certificate { .. } => {
                    return Err(PortableStoreError::Rejected);
                }
            };
            credentials.push(SshSyncRestoreCredentialInput {
                credential_ref_id: credential_ref_id.clone(),
                identity_id,
                operation_id: deterministic_operation_id(
                    &restore_namespace,
                    "credential",
                    credential.id,
                )?,
                idempotency_key: restore_idempotency_key(
                    &restore_namespace,
                    "credential",
                    credential.id,
                ),
                priority: *credential_priorities
                    .get(&credential.id)
                    .ok_or(PortableStoreError::Rejected)?,
                label: credential.label.clone(),
                material,
            });
            credential_ids.insert(credential.id, credential_ref_id);
        }
        let proxy_credential_ids = credential_ids.clone();

        let routes = bundle
            .objects
            .routes
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let auth = bundle
            .objects
            .authentication_plans
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let algorithms = bundle
            .objects
            .algorithm_policies
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let heartbeats = bundle
            .objects
            .heartbeat_policies
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let monitoring = bundle
            .objects
            .monitoring_policies
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let automations = bundle
            .objects
            .login_automations
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let host_ids = bundle
            .objects
            .hosts
            .iter()
            .map(|host| {
                mapped_host_id(&local_ids, host.id)
                    .map_or_else(|| deterministic_host_id(&restore_namespace, host.id), Ok)
                    .map(|id| (host.id, id))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let mut hosts = Vec::with_capacity(bundle.objects.hosts.len());
        let mut resolved = BTreeSet::new();
        let mut remaining = bundle.objects.hosts.iter().collect::<Vec<_>>();
        while !remaining.is_empty() {
            let before = remaining.len();
            let mut deferred = Vec::new();
            for host in remaining {
                let route = routes
                    .get(&host.route_id)
                    .ok_or(PortableStoreError::Rejected)?;
                if route
                    .jump_hops
                    .iter()
                    .any(|hop| !resolved.contains(&hop.host_id))
                {
                    deferred.push(host);
                    continue;
                }
                let auth_plan = auth
                    .get(&host.authentication_plan_id)
                    .ok_or(PortableStoreError::Rejected)?;
                let algorithm = algorithms
                    .get(&host.algorithm_policy_id)
                    .ok_or(PortableStoreError::Rejected)?;
                let heartbeat = heartbeats
                    .get(&host.heartbeat_policy_id)
                    .ok_or(PortableStoreError::Rejected)?;
                let monitor = monitoring
                    .get(&host.monitoring_policy_id)
                    .ok_or(PortableStoreError::Rejected)?;
                let host_id = host_ids
                    .get(&host.id)
                    .cloned()
                    .ok_or(PortableStoreError::Rejected)?;
                let login_automation = restore_login_automation(
                    host.login_automation_id,
                    &automations,
                    &restore_namespace,
                    &local_ids,
                    &secret_map,
                    &mut vault_inserts,
                )?;
                hosts.push(SshSyncRestoreHostInput {
                    host_id,
                    tag_labels: host.tags.iter().cloned().collect(),
                    login_automation,
                    request: HostConfiguredCreateRequest {
                        meta: norishell_core_api::RequestMeta {
                            request_id: RequestId::new(),
                        },
                        operation_id: deterministic_operation_id(
                            &restore_namespace,
                            "host",
                            host.id,
                        )?,
                        idempotency_key: restore_idempotency_key(
                            &restore_namespace,
                            "host",
                            host.id,
                        ),
                        label: host.label.clone(),
                        address: host.address.clone(),
                        port: host.port,
                        username: host.username.clone(),
                        identity_id: host
                            .identity_id
                            .and_then(|id| identity_ids.get(&id).cloned()),
                        favorite: host.favorite,
                        group_id: None,
                        tag_ids: Vec::new(),
                        ingress: restore_route_ingress(&route.ingress, &proxy_credential_ids)?,
                        jump_host_ids: route
                            .jump_hops
                            .iter()
                            .map(|hop| {
                                host_ids
                                    .get(&hop.host_id)
                                    .cloned()
                                    .ok_or(PortableStoreError::Rejected)
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                        authentication_mode: AuthenticationPlanMode::Identity,
                        credential_ref_ids: auth_plan
                            .credential_ids
                            .iter()
                            .map(|id| {
                                credential_ids
                                    .get(id)
                                    .cloned()
                                    .ok_or(PortableStoreError::Rejected)
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                        algorithm_policy_id: algorithm.policy_id.clone(),
                        compatibility_exceptions: algorithm
                            .compatibility_exceptions
                            .iter()
                            .map(restore_algorithm_exception)
                            .collect(),
                        heartbeat_policy: restore_heartbeat(heartbeat),
                        monitoring_policy: restore_monitoring(monitor),
                        login_automation_enabled: false,
                        login_automation_confirmed: false,
                        login_automation_steps: Vec::<HostCreateLoginAutomationStep>::new(),
                        staged_password_id: None,
                    },
                });
                resolved.insert(host.id);
            }
            if deferred.len() == before {
                return Err(PortableStoreError::Rejected);
            }
            remaining = deferred;
        }
        let desktop_profiles = bundle
            .objects
            .desktop_profiles
            .iter()
            .map(|target| {
                let id = mapped_desktop_id(&local_ids, target.id).unwrap_or_else(|| {
                    deterministic_named_v7(
                        &restore_namespace,
                        "desktop-profile",
                        &target.id.as_uuid().to_string(),
                    )
                    .to_string()
                });
                let host = |value: Option<PortableObjectId>| {
                    value
                        .map(|id| {
                            host_ids
                                .get(&id)
                                .cloned()
                                .ok_or(PortableStoreError::Rejected)
                        })
                        .transpose()
                };
                Ok(SshSyncRestoreDesktopProfileInput {
                    portable_object_id: target.id.as_uuid().to_string(),
                    profile: DesktopProfile {
                        id,
                        label: target.label.clone(),
                        protocol: match target.protocol {
                            PortableDesktopProtocol::Rdp => DesktopProtocol::Rdp,
                            PortableDesktopProtocol::Vnc => DesktopProtocol::Vnc,
                        },
                        address: target.address.clone(),
                        port: target.port,
                        username: target.username.clone(),
                        domain: target.domain.clone(),
                        host_id: host(target.host_id)?,
                        gateway_host_id: host(target.gateway_host_id)?,
                        credential_ref_id: target
                            .credential_id
                            .map(|id| {
                                credential_ids
                                    .get(&id)
                                    .cloned()
                                    .ok_or(PortableStoreError::Rejected)
                            })
                            .transpose()?,
                        width: target.width,
                        height: target.height,
                        clipboard_enabled: target.clipboard_enabled,
                        audio_playback_enabled: target.audio_playback_enabled,
                        revision: WireSequence::new(0),
                    },
                })
            })
            .collect::<Result<Vec<_>, PortableStoreError>>()?;
        let mut mapping_inputs = Vec::new();
        mapping_inputs.extend(
            desktop_profiles
                .iter()
                .map(|input| SshSyncObjectMappingInput {
                    portable_object_id: input.portable_object_id.clone(),
                    local_object_id: SshSyncLocalObjectId::DesktopProfile(input.profile.id.clone()),
                }),
        );
        mapping_inputs.extend(identity_ids.iter().map(|(portable_id, local_id)| {
            SshSyncObjectMappingInput {
                portable_object_id: portable_id.as_uuid().to_string(),
                local_object_id: SshSyncLocalObjectId::Identity(local_id.clone()),
            }
        }));
        mapping_inputs.extend(credential_ids.iter().map(|(portable_id, local_id)| {
            SshSyncObjectMappingInput {
                portable_object_id: portable_id.as_uuid().to_string(),
                local_object_id: SshSyncLocalObjectId::Credential(local_id.clone()),
            }
        }));
        mapping_inputs.extend(host_ids.iter().map(|(portable_id, local_id)| {
            SshSyncObjectMappingInput {
                portable_object_id: portable_id.as_uuid().to_string(),
                local_object_id: SshSyncLocalObjectId::Host(local_id.clone()),
            }
        }));
        for secret in &bundle.secrets {
            let local_id = mapped_secret_id(&local_ids, secret.id).map_or_else(
                || deterministic_secret_ref_id(&restore_namespace, secret.id),
                Ok,
            )?;
            mapping_inputs.push(SshSyncObjectMappingInput {
                portable_object_id: secret.id.as_uuid().to_string(),
                local_object_id: SshSyncLocalObjectId::Secret(local_id),
            });
        }
        Ok(RestorePlanBuild {
            plan: SshSyncRestorePlan {
                attempt_id,
                plugin_id,
                profile_id: owner.profile_id.clone(),
                bundle_sha256: bundle_sha256.to_owned(),
                plan_sha256,
                identities,
                credentials,
                hosts,
                desktop_profiles,
            },
            vault_inserts: vault_inserts.into_values().collect(),
            mapping_inputs,
        })
    }

    fn build_reconcile_plan(
        &self,
        bundle: &PortableBundleV1,
        owner: &SyncOwner,
        bundle_fingerprint: &str,
    ) -> Result<ReconcilePlanBuild, PortableStoreError> {
        let mut restore = self.build_restore_plan(bundle, owner, bundle_fingerprint)?;
        let profile = self.ensure_profile_state(
            &owner.plugin_id,
            &owner.signer_fingerprint_sha256,
            &owner.profile_id,
        )?;
        let mappings = self.local_ids_for_owner(&profile.key)?;
        let current_selection = self.apply_scope_membership_exclusions(
            &profile.key,
            self.selection_for_scope(&profile.scope)?,
        )?;
        let portable_ids = self.portable_ids_for_selection(&profile.key, &current_selection)?;
        let current =
            self.build_snapshot_with_ids(current_selection, bundle.revision, Some(&portable_ids))?;
        let snapshots = self
            .hosts
            .list_ssh_sync_snapshots()
            .map_err(map_store_error)?;
        let hosts_by_id = snapshots
            .iter()
            .map(|snapshot| (snapshot.host.host_id.as_str(), snapshot))
            .collect::<BTreeMap<_, _>>();
        let mut identities_by_id = snapshots
            .iter()
            .filter_map(|snapshot| snapshot.identity.as_ref())
            .map(|identity| (identity.identity_id.as_str().to_owned(), identity.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut credentials_by_id = snapshots
            .iter()
            .flat_map(|snapshot| snapshot.credentials.iter())
            .map(|credential| {
                (
                    credential.credential_ref_id.as_str().to_owned(),
                    credential.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for snapshot in &snapshots {
            if let Some(credential_ref_id) = proxy_credential_ref(snapshot) {
                let credential = self
                    .hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.get_ready_credential_record(credential_ref_id)
                    })
                    .map_err(map_store_error)?;
                let identity = self
                    .hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.get_identity(&credential.identity_id)
                    })
                    .map_err(map_store_error)?;
                identities_by_id.insert(identity.identity_id.as_str().to_owned(), identity);
                credentials_by_id
                    .insert(credential.credential_ref_id.as_str().to_owned(), credential);
            }
        }

        self.hosts
            .with_ssh_sync_repository(|repository| {
                for identity in repository.list_identities()? {
                    for credential in repository.list_credential_refs(&identity.identity_id)? {
                        match repository.get_credential_import_record(&credential.credential_ref_id)
                        {
                            Ok(record) => {
                                credentials_by_id
                                    .insert(record.credential_ref_id.as_str().to_owned(), record);
                            }
                            Err(AppPersistenceError::NotFound) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    identities_by_id.insert(identity.identity_id.as_str().to_owned(), identity);
                }
                Ok(())
            })
            .map_err(map_store_error)?;
        let desktops_by_id = self
            .desktop_profiles()?
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<_, _>>();
        let restore_namespace = format!(
            "{}\0{}\0{}",
            owner.plugin_id, owner.signer_fingerprint_sha256, owner.profile_id
        );
        let mut secret_replacements = Vec::new();
        for secret in &bundle.secrets {
            let Some(old_ref) = mapped_secret_id(&mappings, secret.id) else {
                continue;
            };
            let kind = portable_secret_vault_kind(secret.kind)?;
            let existing = self
                .vault
                .read_secret(&old_ref, kind)
                .map_err(|_| PortableStoreError::Stale)?;
            let target_value = secret.payload.copy_for_vault();
            if bool::from(existing.expose().ct_eq(target_value.as_slice())) {
                restore
                    .vault_inserts
                    .retain(|insert| insert.secret_ref_id != old_ref);
                continue;
            }
            let replacement_ref = SecretRefId::parse(
                deterministic_named_v7(
                    &restore_namespace,
                    "secret-replacement",
                    &format!("{}:{bundle_fingerprint}", secret.id.as_uuid()),
                )
                .to_string(),
            )
            .map_err(|_| PortableStoreError::Internal)?;
            let replacement = restore
                .vault_inserts
                .iter_mut()
                .find(|insert| insert.secret_ref_id == old_ref)
                .ok_or(PortableStoreError::Rejected)?;
            replacement.secret_ref_id = replacement_ref.clone();
            replace_restore_secret_ref(&mut restore.plan, &old_ref, &replacement_ref);
            restore.mapping_inputs.retain(|mapping| {
                !(mapping.portable_object_id == secret.id.as_uuid().to_string()
                    && matches!(mapping.local_object_id, SshSyncLocalObjectId::Secret(_)))
            });
            secret_replacements.push(SshSyncOwnedSecretReplacement {
                portable_object_id: secret.id.as_uuid().to_string(),
                expected_secret_ref_id: old_ref,
                new_secret_ref_id: replacement_ref,
            });
        }

        let current_identities = current
            .bundle
            .objects
            .identities
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let current_credentials = current
            .bundle
            .objects
            .credentials
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();
        let current_hosts = current
            .bundle
            .objects
            .hosts
            .iter()
            .map(|value| (value.id, value))
            .collect::<BTreeMap<_, _>>();

        let mut identity_updates = Vec::new();
        let mut existing_identity_ids = BTreeSet::new();
        for target in &bundle.objects.identities {
            let Some(local_id) = mapped_identity_id(&mappings, target.id) else {
                continue;
            };
            let Some(record) = identities_by_id.get(local_id.as_str()) else {
                continue;
            };
            existing_identity_ids.insert(local_id.as_str().to_owned());
            if current_identities.get(&target.id).copied() != Some(target) {
                identity_updates.push(SshSyncOwnedIdentityUpdate {
                    portable_object_id: target.id.as_uuid().to_string(),
                    identity_id: local_id,
                    expected_state_version: record.state_version,
                    label: target.label.clone(),
                    username: target.username.clone(),
                });
            }
        }
        restore
            .plan
            .identities
            .retain(|input| !existing_identity_ids.contains(input.identity_id.as_str()));

        let mut credential_updates = Vec::new();
        let mut existing_credential_ids = BTreeSet::new();
        for target in &bundle.objects.credentials {
            let Some(local_id) = mapped_credential_id(&mappings, target.id) else {
                continue;
            };
            let Some(record) = credentials_by_id.get(local_id.as_str()) else {
                continue;
            };
            existing_credential_ids.insert(local_id.as_str().to_owned());
            let desired = restore
                .plan
                .credentials
                .iter()
                .find(|input| input.credential_ref_id == local_id)
                .ok_or(PortableStoreError::Rejected)?;
            if current_credentials.get(&target.id).copied() != Some(target)
                || secret_replacements.iter().any(|replacement| {
                    owned_material_contains_secret(
                        &desired.material,
                        &replacement.new_secret_ref_id,
                    )
                })
            {
                credential_updates.push(SshSyncOwnedCredentialUpdate {
                    portable_object_id: target.id.as_uuid().to_string(),
                    credential_ref_id: local_id,
                    expected_state_version: record.state_version,
                    identity_id: desired.identity_id.clone(),
                    priority: desired.priority,
                    label: desired.label.clone(),
                    material: desired.material.clone(),
                });
            }
        }
        restore
            .plan
            .credentials
            .retain(|input| !existing_credential_ids.contains(input.credential_ref_id.as_str()));

        let mut host_updates = Vec::new();
        let mut existing_host_ids = BTreeSet::new();
        for target in &bundle.objects.hosts {
            let Some(local_id) = mapped_host_id(&mappings, target.id) else {
                continue;
            };
            let Some(snapshot) = hosts_by_id.get(local_id.as_str()).copied() else {
                continue;
            };
            existing_host_ids.insert(local_id.as_str().to_owned());
            if current_hosts.get(&target.id).copied() != Some(target)
                || !portable_host_configuration_equal(&current.bundle, bundle, target.id)
                || restore.plan.hosts.iter().any(|input| {
                    input.host_id == local_id
                        && secret_replacements.iter().any(|replacement| {
                            login_automation_contains_secret(
                                &input.login_automation,
                                &replacement.new_secret_ref_id,
                            )
                        })
                })
            {
                let desired = restore
                    .plan
                    .hosts
                    .iter()
                    .find(|input| input.host_id == local_id)
                    .ok_or(PortableStoreError::Rejected)?;
                host_updates.push(SshSyncOwnedHostUpdate {
                    portable_object_id: target.id.as_uuid().to_string(),
                    host_id: local_id,
                    expected: host_base_versions(snapshot),
                    desired: desired.request.clone(),
                    tag_labels: desired.tag_labels.clone(),
                    login_automation: desired.login_automation.clone(),
                });
            }
        }
        restore
            .plan
            .hosts
            .retain(|input| !existing_host_ids.contains(input.host_id.as_str()));

        let mut desktop_profile_updates = Vec::new();
        let mut existing_desktop_ids = BTreeSet::new();
        for desired in &restore.plan.desktop_profiles {
            let Some(record) = desktops_by_id.get(&desired.profile.id) else {
                continue;
            };
            existing_desktop_ids.insert(record.id.clone());
            let mut compared = record.clone();
            compared.revision = WireSequence::new(0);
            if compared != desired.profile {
                desktop_profile_updates.push(SshSyncOwnedDesktopProfileUpdate {
                    portable_object_id: desired.portable_object_id.clone(),
                    profile: desired.profile.clone(),
                    expected_revision: record.revision,
                });
            }
        }
        restore
            .plan
            .desktop_profiles
            .retain(|p| !existing_desktop_ids.contains(&p.profile.id));
        let mut desktop_profile_deletes = Vec::new();
        let mut identity_deletes = Vec::new();
        let mut credential_deletes = Vec::new();
        let mut host_deletes = Vec::new();
        let mut secret_deletes = Vec::new();
        for tombstone in &bundle.tombstones {
            match tombstone.kind {
                PortableObjectKind::DesktopProfile => {
                    if let Some(id) = mapped_desktop_id(&mappings, tombstone.id)
                        && let Some(record) = desktops_by_id.get(&id)
                    {
                        desktop_profile_deletes.push(SshSyncOwnedDesktopProfileDelete {
                            portable_object_id: tombstone.id.as_uuid().to_string(),
                            profile_id: id,
                            expected_revision: record.revision,
                        });
                    }
                }
                PortableObjectKind::Identity => {
                    if let Some(local_id) = mapped_identity_id(&mappings, tombstone.id)
                        && let Some(record) = identities_by_id.get(local_id.as_str())
                    {
                        identity_deletes.push(SshSyncOwnedIdentityDelete {
                            portable_object_id: tombstone.id.as_uuid().to_string(),
                            identity_id: local_id,
                            expected_state_version: record.state_version,
                        });
                    }
                }
                PortableObjectKind::Credential => {
                    if let Some(local_id) = mapped_credential_id(&mappings, tombstone.id)
                        && let Some(record) = credentials_by_id.get(local_id.as_str())
                    {
                        credential_deletes.push(SshSyncOwnedCredentialDelete {
                            portable_object_id: tombstone.id.as_uuid().to_string(),
                            credential_ref_id: local_id,
                            expected_state_version: record.state_version,
                        });
                    }
                }
                PortableObjectKind::Host => {
                    if let Some(local_id) = mapped_host_id(&mappings, tombstone.id)
                        && let Some(snapshot) = hosts_by_id.get(local_id.as_str()).copied()
                    {
                        host_deletes.push(SshSyncOwnedHostDelete {
                            portable_object_id: tombstone.id.as_uuid().to_string(),
                            host_id: local_id,
                            expected: host_base_versions(snapshot),
                        });
                    }
                }
                PortableObjectKind::Secret => {
                    if let Some(local_id) = mapped_secret_id(&mappings, tombstone.id) {
                        secret_deletes.push(SshSyncOwnedSecretDelete {
                            portable_object_id: tombstone.id.as_uuid().to_string(),
                            secret_ref_id: local_id,
                        });
                    }
                }
                _ => {}
            }
        }
        restore.mapping_inputs.retain(|mapping| {
            let kind = match mapping.local_object_id {
                SshSyncLocalObjectId::DesktopProfile(_) => SshSyncObjectKind::DesktopProfile,
                SshSyncLocalObjectId::Host(_) => SshSyncObjectKind::Host,
                SshSyncLocalObjectId::Identity(_) => SshSyncObjectKind::Identity,
                SshSyncLocalObjectId::Credential(_) => SshSyncObjectKind::Credential,
                SshSyncLocalObjectId::Secret(_) => SshSyncObjectKind::Secret,
            };
            PortableObjectId::from_uuid(
                Uuid::parse_str(&mapping.portable_object_id).unwrap_or_else(|_| Uuid::nil()),
            )
            .ok()
            .is_none_or(|portable_id| !mappings.contains_key(&(kind, portable_id)))
        });
        let create_count = bounded_len(
            restore.plan.identities.len()
                + restore.plan.credentials.len()
                + restore.plan.hosts.len()
                + restore.plan.desktop_profiles.len(),
        )?;
        let update_count = bounded_len(
            identity_updates.len()
                + credential_updates.len()
                + host_updates.len()
                + desktop_profile_updates.len()
                + secret_replacements.len(),
        )?;
        let delete_count = bounded_len(
            identity_deletes.len()
                + credential_deletes.len()
                + host_deletes.len()
                + desktop_profile_deletes.len()
                + secret_deletes.len(),
        )?;
        let delta = SshSyncOwnedMetadataDelta {
            owner: profile.key,
            attempt_id: restore.plan.attempt_id.clone(),
            delta_sha256: sha256_text(&format!(
                "norishell:ssh-sync-owned-delta:v1:{bundle_fingerprint}"
            )),
            creates: Some(SshSyncOwnedCreateBatch {
                plan: restore.plan,
                mappings: restore.mapping_inputs,
            }),
            identity_updates,
            identity_deletes,
            credential_updates,
            credential_deletes,
            host_updates,
            host_deletes,
            desktop_profile_updates,
            desktop_profile_deletes,
            secret_replacements,
            secret_deletes,
        };
        Ok(ReconcilePlanBuild {
            delta,
            vault_inserts: restore.vault_inserts,
            create_count,
            update_count,
            delete_count,
        })
    }

    pub(crate) fn reconcile_durable_restores(&self) -> Result<(), PortableStoreError> {
        self.reconcile_restore_sagas_for_plugin(None)
    }

    pub(crate) async fn reconcile_after_vault_unlock(&self) -> Result<(), PortableStoreError> {
        let _restore_commit = self.restore_commit.lock().await;
        self.reconcile_durable_restores()?;
        self.reconcile_plugin_deletes(None)
    }

    fn reconcile_restore_sagas_for_plugin(
        &self,
        plugin_id: Option<&str>,
    ) -> Result<(), PortableStoreError> {
        let sagas = self
            .hosts
            .with_ssh_sync_repository(|repository| repository.list_pending_ssh_sync_restore_sagas())
            .map_err(map_store_error)?;
        for saga in sagas {
            if plugin_id.is_some_and(|plugin_id| saga.plugin_id.as_str() != plugin_id) {
                continue;
            }
            for chunk in saga.secret_ref_ids.chunks(MAX_SECRET_BATCH_ENTRIES) {
                self.vault
                    .delete_secrets(chunk)
                    .map_err(|_| PortableStoreError::Internal)?;
            }
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.abort_ssh_sync_restore_saga(&saga.attempt_id)
                })
                .map_err(map_store_error)?;
        }
        Ok(())
    }

    fn reconcile_restore_sagas(&self) -> Result<(), PortableStoreError> {
        self.reconcile_restore_sagas_for_plugin(None)
    }

    fn apply_restore_plan(
        &self,
        bundle: &PortableBundleV1,
        owner: &SyncOwner,
        bundle_sha256: &str,
    ) -> Result<ApplyResult, PortableStoreError> {
        self.reconcile_restore_sagas()?;
        let RestorePlanBuild {
            plan,
            vault_inserts,
            mapping_inputs,
        } = self.build_restore_plan(bundle, owner, bundle_sha256)?;
        let preview = self
            .hosts
            .with_ssh_sync_repository(|repository| repository.preview_ssh_sync_restore_plan(&plan))
            .map_err(map_store_error)?;
        if preview.conflict_count > 0 {
            return Err(PortableStoreError::Stale);
        }
        let owner_key = Self::profile_key(
            &owner.plugin_id,
            &owner.signer_fingerprint_sha256,
            &owner.profile_id,
        )?;
        self.hosts
            .with_ssh_sync_repository(|repository| {
                repository.resolve_or_create_ssh_sync_object_mappings(&owner_key, &mapping_inputs)
            })
            .map_err(map_store_error)?;
        validate_restore_vault_batch(&vault_inserts, &preview.new_secret_ref_ids)?;
        let new_secret_refs = preview
            .new_secret_ref_ids
            .iter()
            .map(SecretRefId::as_str)
            .collect::<BTreeSet<_>>();
        let inserts = vault_inserts
            .into_iter()
            .filter(|insert| new_secret_refs.contains(insert.secret_ref_id.as_str()))
            .collect::<Vec<_>>();
        if inserts.len() != preview.new_secret_ref_ids.len() {
            return Err(PortableStoreError::Rejected);
        }
        self.hosts
            .with_ssh_sync_repository(|repository| {
                repository.begin_ssh_sync_restore_saga(&SshSyncRestoreSagaInput {
                    attempt_id: plan.attempt_id.clone(),
                    plugin_id: plan.plugin_id.clone(),
                    profile_id: plan.profile_id.clone(),
                    bundle_sha256: plan.bundle_sha256.clone(),
                    plan_sha256: plan.plan_sha256.clone(),
                    secret_ref_ids: preview.new_secret_ref_ids.clone(),
                })
            })
            .map_err(map_store_error)?;
        if !inserts.is_empty()
            && let Err(error) = self.vault.insert_secrets(&inserts)
        {
            if error.secret_insert_definitely_absent() {
                self.hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.abort_ssh_sync_restore_saga(&plan.attempt_id)
                    })
                    .map_err(map_store_error)?;
            }
            return Err(PortableStoreError::Internal);
        }
        let committed = self
            .hosts
            .with_ssh_sync_repository(|repository| repository.commit_ssh_sync_restore_plan(&plan));
        match committed {
            Ok(_) => Ok(ApplyResult {
                desktop_profile_count: bounded_len(bundle.objects.desktop_profiles.len())?,
                host_count: u32::try_from(bundle.objects.hosts.len()).unwrap_or(u32::MAX),
                credential_count: u32::try_from(bundle.objects.credentials.len())
                    .unwrap_or(u32::MAX),
                conflict_count: 0,
            }),
            Err(AppPersistenceError::RestoreCommitUnknown) => Err(PortableStoreError::Internal),
            Err(error) => {
                self.reconcile_restore_sagas()?;
                Err(map_store_error(error))
            }
        }
    }

    fn apply_owned_reconcile(
        &self,
        bundle: &PortableBundleV1,
        delta: SshSyncOwnedMetadataDelta,
        vault_inserts: Vec<VaultSecretInsert>,
        scope_memberships: Option<&[SshSyncScopeMembershipInput]>,
        change_fence: &SshSyncChangeFence,
        action_fence: &ActionFence,
    ) -> Result<ApplyResult, PortableStoreError> {
        self.reconcile_restore_sagas()?;
        let create = delta.creates.as_ref().ok_or(PortableStoreError::Internal)?;
        let secret_ref_ids = vault_inserts
            .iter()
            .map(|insert| insert.secret_ref_id.clone())
            .collect::<Vec<_>>();
        validate_restore_vault_batch(&vault_inserts, &secret_ref_ids)?;
        let begun = self
            .hosts
            .with_ssh_sync_repository(|repository| {
                repository.begin_ssh_sync_restore_saga_with_fence(
                    &SshSyncRestoreSagaInput {
                        attempt_id: create.plan.attempt_id.clone(),
                        plugin_id: create.plan.plugin_id.clone(),
                        profile_id: create.plan.profile_id.clone(),
                        bundle_sha256: create.plan.bundle_sha256.clone(),
                        plan_sha256: create.plan.plan_sha256.clone(),
                        secret_ref_ids: secret_ref_ids.clone(),
                    },
                    change_fence,
                )
            })
            .map_err(map_store_error)?;
        if !action_fence() {
            self.reconcile_restore_sagas()?;
            return Err(PortableStoreError::Stale);
        }
        if !vault_inserts.is_empty()
            && let Err(error) = self.vault.insert_secrets(&vault_inserts)
        {
            if error.secret_insert_definitely_absent() {
                self.hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.abort_ssh_sync_restore_saga(&create.plan.attempt_id)
                    })
                    .map_err(map_store_error)?;
            }
            return Err(PortableStoreError::Internal);
        }
        let owner = delta.owner.clone();
        let committed = self.hosts.with_ssh_sync_repository(|repository| {
            if !action_fence() {
                return Err(AppPersistenceError::Conflict);
            }
            repository.apply_ssh_sync_owned_metadata_delta_with_scope_memberships_and_fence(
                &delta,
                scope_memberships,
                &begun.change_fence,
            )
        });
        match committed {
            Ok(_) => {
                let _ = self.reconcile_vault_gc(&owner);
                Ok(ApplyResult {
                    desktop_profile_count: bounded_len(bundle.objects.desktop_profiles.len())?,
                    host_count: bounded_len(bundle.objects.hosts.len())?,
                    credential_count: bounded_len(bundle.objects.credentials.len())?,
                    conflict_count: 0,
                })
            }
            Err(error) => {
                self.reconcile_restore_sagas()?;
                Err(map_store_error(error))
            }
        }
    }

    fn reconcile_vault_gc(&self, owner: &SshSyncProfileStateKey) -> Result<(), PortableStoreError> {
        let pending = self
            .hosts
            .with_ssh_sync_repository(|repository| repository.list_ssh_sync_vault_gc(owner))
            .map_err(map_store_error)?;
        if pending.is_empty() {
            return Ok(());
        }
        let refs = pending
            .into_iter()
            .map(|record| record.secret_ref_id)
            .collect::<Vec<_>>();
        for chunk in refs.chunks(MAX_SECRET_BATCH_ENTRIES) {
            self.vault
                .delete_secrets(chunk)
                .map_err(|_| PortableStoreError::Internal)?;
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.ack_ssh_sync_vault_gc(owner, chunk)
                })
                .map_err(map_store_error)?;
        }
        Ok(())
    }

    fn reconcile_plugin_deletes(
        &self,
        plugin_filter: Option<&PluginId>,
    ) -> Result<(), PortableStoreError> {
        let tasks = self
            .hosts
            .with_ssh_sync_repository(|repository| {
                repository.list_pending_ssh_sync_plugin_deletes()
            })
            .map_err(map_store_error)?;
        for task in tasks
            .into_iter()
            .filter(|task| plugin_filter.is_none_or(|plugin_id| task.owner.plugin_id == *plugin_id))
        {
            for chunk in task.secret_ref_ids.chunks(MAX_SECRET_BATCH_ENTRIES) {
                self.vault
                    .delete_secrets(chunk)
                    .map_err(|_| PortableStoreError::Internal)?;
                self.hosts
                    .with_ssh_sync_repository(|repository| {
                        repository.ack_ssh_sync_plugin_delete_refs(
                            &task.owner,
                            &task.operation_id,
                            chunk,
                        )
                    })
                    .map_err(map_store_error)?;
            }
        }
        Ok(())
    }

    fn stage_restore_bundle(
        &self,
        bundle: PortableBundleV1,
        owner: SyncOwner,
        key: &SyncKey,
    ) -> Result<RestorePreview, PortableStoreError> {
        bundle
            .validate()
            .map_err(|_| PortableStoreError::Rejected)?;
        let change_fence = self
            .hosts
            .with_ssh_sync_repository(|repository| repository.ssh_sync_change_fence())
            .map_err(map_store_error)?;
        let handle = PendingRestoreHandle::new(Uuid::new_v4().to_string());
        let encoded = canonical_bundle_bytes(&bundle).map_err(|_| PortableStoreError::Internal)?;
        let bundle_sha256 = key
            .baseline_digest(&encoded)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        // Each explicit sync is a new operation; the same cloud revision can restore later local changes again.
        // Persistent replay remains bound to the same plan for this specific operation.
        let operation_fingerprint = sha256_text(&format!(
            "norishell:ssh-sync-apply:v1:{}:{bundle_sha256}",
            handle.token()
        ));
        let reconcile = self.build_reconcile_plan(&bundle, &owner, &operation_fingerprint)?;
        let scope_memberships = self.scope_memberships_for_bundle(
            &Self::profile_key(
                &owner.plugin_id,
                &owner.signer_fingerprint_sha256,
                &owner.profile_id,
            )?,
            &bundle,
        )?;
        let create_batch = reconcile
            .delta
            .creates
            .as_ref()
            .ok_or(PortableStoreError::Internal)?;
        let restore_facts = self
            .hosts
            .with_ssh_sync_repository(|repository| {
                repository.preview_ssh_sync_restore_plan(&create_batch.plan)
            })
            .map_err(map_store_error)?;
        let mut new_secret_refs = restore_facts.new_secret_ref_ids.clone();
        new_secret_refs.extend(
            reconcile
                .delta
                .secret_replacements
                .iter()
                .map(|value| value.new_secret_ref_id.clone()),
        );
        validate_restore_vault_batch(&reconcile.vault_inserts, &new_secret_refs)?;
        let plaintext_bytes = canonical_bundle_bytes(&bundle)
            .map_err(|_| PortableStoreError::Internal)?
            .len();
        let reconcile_noop = reconcile.create_count == 0
            && reconcile.update_count == 0
            && reconcile.delete_count == 0;
        let preview = RestorePreview {
            handle: handle.clone(),
            desktop_profile_count: bounded_len(bundle.objects.desktop_profiles.len())?,
            host_count: u32::try_from(bundle.objects.hosts.len()).unwrap_or(u32::MAX),
            credential_count: u32::try_from(bundle.objects.credentials.len()).unwrap_or(u32::MAX),
            conflict_count: restore_facts.conflict_count,
            delete_count: reconcile.delete_count,
        };
        let current_fence = self
            .hosts
            .with_ssh_sync_repository(|repository| repository.ssh_sync_change_fence())
            .map_err(map_store_error)?;
        if current_fence != change_fence {
            return Err(PortableStoreError::Stale);
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.prune_expired();
        if state.pending_restores.len() >= MAX_SELECTION_LEASES
            || state
                .pending_restore_plaintext_bytes
                .checked_add(plaintext_bytes)
                .is_none_or(|total| total > MAX_PENDING_RESTORE_PLAINTEXT_BYTES)
        {
            return Err(PortableStoreError::Stale);
        }
        state.pending_restore_plaintext_bytes += plaintext_bytes;
        state.pending_restores.insert(
            handle.token().to_owned(),
            PendingRestore {
                handle,
                bundle,
                owner,
                bundle_sha256,
                plaintext_bytes,
                owned_delta: (!reconcile_noop).then_some(reconcile.delta),
                vault_inserts: reconcile.vault_inserts,
                scope_memberships: Some(scope_memberships),
                reconcile_noop,
                change_fence: Some(change_fence),
                expires_at: Instant::now() + SELECTION_LEASE_TIMEOUT,
            },
        );
        Ok(preview)
    }
}

impl SecureSshSyncUi for NoriShellSshSyncLocalAdapter {
    fn create_vault(
        &self,
        context: SecureActionContext,
    ) -> LocalFuture<'_, Result<(), SecureSelectionError>> {
        Box::pin(async move {
            let fence = context.fence.clone();
            let decision = self
                .prompt(
                    context,
                    SshSyncSecurePromptKind::CreateLocalVault,
                    SecurePromptContent::default(),
                )
                .await?;
            let password = decision
                .vault_password
                .ok_or(SecureSelectionError::Cancelled)?;
            let confirmation = decision
                .vault_password_confirmation
                .ok_or(SecureSelectionError::Cancelled)?;
            let vault = self.vault.clone();
            tokio::task::spawn_blocking(move || {
                if !fence() {
                    return Err(SecureSelectionError::Cancelled);
                }
                vault
                    .create_for_protected_operation(password.as_bytes(), confirmation.as_bytes())
                    .map(|_| ())
                    .map_err(|_| SecureSelectionError::Unavailable)
            })
            .await
            .map_err(|_| SecureSelectionError::Unavailable)?
        })
    }

    fn select_backup(
        &self,
        context: SecureActionContext,
    ) -> LocalFuture<'_, Result<SecureBackupSelection, SecureSelectionError>> {
        Box::pin(async move {
            let owner = sync_owner(&context);
            let fence = context.fence.clone();
            let hosts = self.selection_hosts()?;
            let desktop_profiles = self
                .selection_desktops()
                .map_err(|_| SecureSelectionError::Unavailable)?;
            let decision = self
                .prompt(
                    context,
                    SshSyncSecurePromptKind::SelectBackup,
                    SecurePromptContent {
                        desktop_profiles,
                        hosts,
                        ..SecurePromptContent::default()
                    },
                )
                .await?;
            let selection = BackupSelection {
                desktop_profile_ids: decision.desktop_profile_ids.into_iter().collect(),
                host_ids: decision
                    .host_ids
                    .into_iter()
                    .map(|value| value.as_str().to_owned())
                    .collect(),
                credential_ids: decision
                    .credential_ids
                    .into_iter()
                    .map(|value| value.as_str().to_owned())
                    .collect(),
            };
            let snapshots = self
                .hosts
                .list_ssh_sync_snapshots()
                .map_err(|_| SecureSelectionError::Unavailable)?;
            let selection = self
                .add_route_dependencies(selection, &snapshots)
                .map_err(|_| SecureSelectionError::Unavailable)?;
            let token = Uuid::new_v4().to_string();
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.prune_expired();
            if state.backup_selections.len() >= MAX_SELECTION_LEASES {
                return Err(SecureSelectionError::Unavailable);
            }
            state.backup_selections.insert(
                token.clone(),
                BackupSelectionLease {
                    owner: owner.clone(),
                    selection,
                    fence,
                    expires_at: Instant::now() + SELECTION_LEASE_TIMEOUT,
                },
            );
            Ok(SecureBackupSelection::new(
                token,
                owner.plugin_id,
                owner.signer_fingerprint_sha256,
                owner.profile_id,
            ))
        })
    }

    fn vault_password(
        &self,
        context: SecureActionContext,
    ) -> LocalFuture<'_, Result<Zeroizing<Vec<u8>>, SecureSelectionError>> {
        Box::pin(async move {
            let kind = if context.remote_origin.is_some() {
                SshSyncSecurePromptKind::RecoverSynchronizedKey
            } else {
                SshSyncSecurePromptKind::UnlockSynchronizedVault
            };
            let decision = self
                .prompt(context, kind, SecurePromptContent::default())
                .await?;
            let password = decision
                .vault_password
                .ok_or(SecureSelectionError::Cancelled)?;
            if !(12..=65_536).contains(&password.len()) {
                return Err(SecureSelectionError::Unavailable);
            }
            Ok(Zeroizing::new(password.as_bytes().to_vec()))
        })
    }

    fn choose_sync_direction(
        &self,
        context: SecureActionContext,
        review: SyncDirectionReview,
    ) -> LocalFuture<'_, Result<SecureSyncDirection, SecureSelectionError>> {
        Box::pin(async move {
            let owner = sync_owner(&context);
            let fence = context.fence.clone();
            let decision = self
                .prompt(
                    context,
                    SshSyncSecurePromptKind::ChooseSyncDirection,
                    SecurePromptContent {
                        desktop_profile_count: review.local_desktop_profile_count,
                        remote_desktop_profile_count: review.remote_desktop_profile_count,
                        host_count: review.local_host_count,
                        credential_count: review.local_credential_count,
                        conflict_count: review.conflict_count,
                        delete_count: review.delete_count,
                        remote_host_count: review.remote_host_count,
                        remote_credential_count: review.remote_credential_count,
                        local_compared_at_unix_ms: Some(review.local_compared_at_unix_ms),
                        remote_updated_at_unix_ms: review.remote_updated_at_unix_ms,
                        differences: review.differences,
                        difference_total_count: review.difference_total_count,
                        difference_omitted_count: review.difference_omitted_count,
                        ..SecurePromptContent::default()
                    },
                )
                .await?;
            match decision.decision {
                SshSyncSecureDecision::KeepLocal => Ok(SecureSyncDirection::LocalOverRemote),
                SshSyncSecureDecision::UseRemote => {
                    let approval =
                        self.issue_apply_approval(owner, fence, Some(review.restore_handle))?;
                    Ok(SecureSyncDirection::RemoteOverLocal(approval))
                }
                _ => Err(SecureSelectionError::Unavailable),
            }
        })
    }

    fn approve_remote_reset(
        &self,
        context: SecureActionContext,
    ) -> LocalFuture<'_, Result<(), SecureSelectionError>> {
        Box::pin(async move {
            self.prompt(
                context,
                SshSyncSecurePromptKind::ResetRemote,
                SecurePromptContent::default(),
            )
            .await?;
            Ok(())
        })
    }
}

impl PortableSshProfileStore for NoriShellSshSyncLocalAdapter {
    fn discard_staged_restore(&self, handle: &PendingRestoreHandle) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pending) = state.pending_restores.remove(handle.token()) {
            state.pending_restore_plaintext_bytes = state
                .pending_restore_plaintext_bytes
                .saturating_sub(pending.plaintext_bytes);
        }
        state
            .apply_approvals
            .retain(|_, value| value.restore_handle.as_ref() != Some(handle));
    }

    fn clear_plugin(&self, plugin_id: &str) {
        self.clear_plugin_state(plugin_id);
    }

    fn unlock_vault_for_operation(
        &self,
        password: Zeroizing<Vec<u8>>,
    ) -> LocalFuture<'_, Result<(), PortableStoreError>> {
        Box::pin(async move {
            self.vault
                .unlock_for_protected_operation(password.as_slice())
                .map_err(|_| PortableStoreError::Rejected)?;
            if self.reconcile_after_vault_unlock().await.is_err() {
                eprintln!(
                    "SSH sync cleanup remains pending after protected Vault unlock; background reconciliation will retry"
                );
            }
            Ok(())
        })
    }

    fn delete_plugin_data(
        &self,
        plugin_id: String,
    ) -> LocalFuture<'_, Result<(), PortableStoreError>> {
        Box::pin(async move {
            let _restore_commit = self.restore_commit.lock().await;
            self.reconcile_restore_sagas_for_plugin(Some(&plugin_id))?;
            let plugin_id = PluginId::parse(plugin_id).map_err(|_| PortableStoreError::Rejected)?;
            let existing_operation = self
                .hosts
                .with_ssh_sync_repository(|repository| {
                    repository.list_pending_ssh_sync_plugin_deletes()
                })
                .map_err(map_store_error)?
                .into_iter()
                .find(|task| task.owner.plugin_id == plugin_id)
                .map(|task| task.operation_id);
            let operation_id = existing_operation.unwrap_or_default();
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.begin_ssh_sync_plugin_delete(&plugin_id, &operation_id)
                })
                .map_err(map_store_error)?;
            self.reconcile_plugin_deletes(Some(&plugin_id))?;
            if self
                .hosts
                .with_ssh_sync_repository(|repository| {
                    repository.list_pending_ssh_sync_plugin_deletes()
                })
                .map_err(map_store_error)?
                .iter()
                .any(|task| task.owner.plugin_id == plugin_id)
            {
                return Err(PortableStoreError::Internal);
            }
            Ok(())
        })
    }

    fn profile_state(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
    ) -> LocalFuture<'_, Result<PortableProfileState, PortableStoreError>> {
        Box::pin(async move {
            if self.vault.is_unlocked() {
                self.reconcile_plugin_deletes(None)?;
            }
            let state =
                self.ensure_profile_state(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            if self.vault.is_unlocked() {
                let _ = self.reconcile_vault_gc(&state.key);
            }
            Ok(Self::portable_profile_state(state))
        })
    }

    fn local_counts(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
    ) -> LocalFuture<'_, Result<(u32, u32, u32), PortableStoreError>> {
        Box::pin(async move {
            let state =
                self.ensure_profile_state(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            let selection = self.apply_scope_membership_exclusions(
                &state.key,
                self.selection_for_scope(&state.scope)?,
            )?;
            Ok((
                u32::try_from(selection.host_ids.len())
                    .map_err(|_| PortableStoreError::Internal)?,
                u32::try_from(selection.credential_ids.len())
                    .map_err(|_| PortableStoreError::Internal)?,
                bounded_len(selection.desktop_profile_ids.len())?,
            ))
        })
    }

    fn snapshot_current(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        revision: u64,
    ) -> LocalFuture<'_, Result<PortableSnapshot, PortableStoreError>> {
        Box::pin(async move {
            let state =
                self.ensure_profile_state(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            let selection = self.apply_scope_membership_exclusions(
                &state.key,
                self.selection_for_scope(&state.scope)?,
            )?;
            let portable_ids = self.portable_ids_for_selection(&state.key, &selection)?;
            self.build_snapshot_with_ids(selection, revision, Some(&portable_ids))
        })
    }

    fn prepare_local_merge(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        base: PortableBundleV1,
        mut current: PortableBundleV1,
    ) -> LocalFuture<'_, Result<PortableBundleV1, PortableStoreError>> {
        Box::pin(async move {
            base.validate().map_err(|_| PortableStoreError::Rejected)?;
            current
                .validate()
                .map_err(|_| PortableStoreError::Rejected)?;
            let inherited_tombstones = base
                .tombstones
                .iter()
                .copied()
                .filter(|value| !bundle_contains_portable_object(&current, value.kind, value.id))
                .collect::<Vec<_>>();
            current.tombstones.extend(inherited_tombstones);
            let owner = Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            let mappings = self.local_ids_for_owner(&owner)?;
            let snapshots = self
                .hosts
                .list_ssh_sync_snapshots()
                .map_err(map_store_error)?;
            let facts = self.sync_referenced_local_facts(&snapshots)?;
            let mut deleted_hosts = BTreeSet::new();
            for ((kind, portable_id), local_id) in mappings {
                let present = match local_id {
                    SshSyncLocalObjectId::DesktopProfile(value) => {
                        facts.desktop_profile_ids.contains(&value)
                    }
                    SshSyncLocalObjectId::Host(value) => facts.host_ids.contains(value.as_str()),
                    SshSyncLocalObjectId::Identity(value) => {
                        facts.identity_ids.contains(value.as_str())
                    }
                    SshSyncLocalObjectId::Credential(value) => {
                        facts.credential_ids.contains(value.as_str())
                    }
                    SshSyncLocalObjectId::Secret(value) => {
                        facts.secret_ids.contains(value.as_str())
                    }
                };
                if present || !base_contains_mapped_object(&base, kind, portable_id) {
                    continue;
                }
                let tombstone_kind = match kind {
                    SshSyncObjectKind::DesktopProfile => PortableObjectKind::DesktopProfile,
                    SshSyncObjectKind::Host => PortableObjectKind::Host,
                    SshSyncObjectKind::Identity => PortableObjectKind::Identity,
                    SshSyncObjectKind::Credential => PortableObjectKind::Credential,
                    SshSyncObjectKind::Secret => PortableObjectKind::Secret,
                };
                if kind == SshSyncObjectKind::Host {
                    deleted_hosts.insert(portable_id);
                }
                current.tombstones.push(PortableTombstone {
                    kind: tombstone_kind,
                    id: portable_id,
                });
            }
            for host in base
                .objects
                .hosts
                .iter()
                .filter(|host| deleted_hosts.contains(&host.id))
            {
                current.tombstones.extend([
                    PortableTombstone {
                        kind: PortableObjectKind::Route,
                        id: host.route_id,
                    },
                    PortableTombstone {
                        kind: PortableObjectKind::AuthenticationPlan,
                        id: host.authentication_plan_id,
                    },
                    PortableTombstone {
                        kind: PortableObjectKind::AlgorithmPolicy,
                        id: host.algorithm_policy_id,
                    },
                    PortableTombstone {
                        kind: PortableObjectKind::HeartbeatPolicy,
                        id: host.heartbeat_policy_id,
                    },
                    PortableTombstone {
                        kind: PortableObjectKind::MonitoringPolicy,
                        id: host.monitoring_policy_id,
                    },
                ]);
                if let Some(id) = host.login_automation_id {
                    current.tombstones.push(PortableTombstone {
                        kind: PortableObjectKind::LoginAutomation,
                        id,
                    });
                }
            }
            current.schema = BundleSchema::V3;
            current
                .tombstones
                .sort_by_key(|value| (value.kind, value.id));
            current.tombstones.dedup();
            current
                .validate()
                .map_err(|_| PortableStoreError::Rejected)?;
            Ok(current)
        })
    }

    fn configure_scope(
        &self,
        signer_fingerprint_sha256: String,
        selection: SecureBackupSelection,
    ) -> LocalFuture<'_, Result<PortableSnapshot, PortableStoreError>> {
        Box::pin(async move {
            let (plugin_id, selection_signer, profile_id) = selection.owner();
            let owner = SyncOwner {
                plugin_id: plugin_id.to_owned(),
                signer_fingerprint_sha256: selection_signer.to_owned(),
                profile_id: profile_id.to_owned(),
            };
            if selection_signer != signer_fingerprint_sha256 {
                return Err(PortableStoreError::Stale);
            }
            let lease = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.prune_expired();
                state
                    .backup_selections
                    .remove(selection.token())
                    .ok_or(PortableStoreError::InvalidSelection)?
            };
            if lease.owner != owner || !(lease.fence)() {
                return Err(PortableStoreError::Stale);
            }
            let current = self.ensure_profile_state(
                &owner.plugin_id,
                &signer_fingerprint_sha256,
                &owner.profile_id,
            )?;
            let portable_ids = self.portable_ids_for_selection(&current.key, &lease.selection)?;
            let snapshot =
                self.build_snapshot_with_ids(lease.selection.clone(), 1, Some(&portable_ids))?;
            let all = self.selection_for_scope(&SshSyncProfileScope {
                mode: SshSyncProfileScopeMode::AllEligible,
                custom_host_ids: Vec::new(),
                custom_desktop_profile_ids: Vec::new(),
                custom_credential_ref_ids: Vec::new(),
            })?;
            let scope = if lease.selection.host_ids == all.host_ids
                && lease.selection.credential_ids == all.credential_ids
                && lease.selection.desktop_profile_ids == all.desktop_profile_ids
            {
                SshSyncProfileScope {
                    mode: SshSyncProfileScopeMode::AllEligible,
                    custom_host_ids: Vec::new(),
                    custom_desktop_profile_ids: Vec::new(),
                    custom_credential_ref_ids: Vec::new(),
                }
            } else {
                SshSyncProfileScope {
                    mode: SshSyncProfileScopeMode::Custom,
                    custom_desktop_profile_ids: lease
                        .selection
                        .desktop_profile_ids
                        .into_iter()
                        .collect(),
                    custom_host_ids: lease
                        .selection
                        .host_ids
                        .into_iter()
                        .map(HostId::parse)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| PortableStoreError::Rejected)?,
                    custom_credential_ref_ids: lease
                        .selection
                        .credential_ids
                        .into_iter()
                        .map(CredentialRefId::parse)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| PortableStoreError::Rejected)?,
                }
            };
            let updated = self
                .hosts
                .with_ssh_sync_repository(|repository| {
                    repository.replace_ssh_sync_profile_scope(
                        &current.key,
                        current.state_version,
                        &scope,
                    )
                })
                .map_err(map_store_error)?;
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.replace_ssh_sync_scope_memberships(
                        &current.key,
                        updated.state_version,
                        &[],
                    )
                })
                .map_err(map_store_error)?;
            Ok(snapshot)
        })
    }

    fn bind_sync_key(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: norishell_core_api::WireSequence,
        secret_ref_id: SecretRefId,
        key: SyncKey,
        password_wrapped_envelope: Vec<u8>,
    ) -> LocalFuture<'_, Result<PortableProfileState, PortableStoreError>> {
        Box::pin(async move {
            let profile_key =
                Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            let binding = SshSyncProfileKeyBinding {
                sync_key_secret_ref_id: secret_ref_id.clone(),
                password_wrapped_sync_key_envelope: Some(password_wrapped_envelope),
            };
            self.vault
                .insert_secrets(&[VaultSecretInsert {
                    secret_ref_id: secret_ref_id.clone(),
                    kind: SecretKind::SshSyncKey,
                    value: Zeroizing::new(key.copy_for_vault().to_vec()),
                }])
                .map_err(|_| PortableStoreError::Internal)?;
            let persisted = self.hosts.with_ssh_sync_repository(|repository| {
                repository.bind_ssh_sync_profile_key(&profile_key, expected_state_version, &binding)
            });
            match persisted {
                Ok(record) => Ok(Self::portable_profile_state(record)),
                Err(error) => {
                    let exact_replay = self
                        .hosts
                        .with_ssh_sync_repository(|repository| {
                            repository.get_ssh_sync_profile_state(&profile_key)
                        })
                        .ok()
                        .flatten()
                        .filter(|record| record.key_binding.as_ref() == Some(&binding));
                    if let Some(record) = exact_replay {
                        return Ok(Self::portable_profile_state(record));
                    }
                    let _ = self.vault.delete_secrets(&[secret_ref_id]);
                    Err(map_store_error(error))
                }
            }
        })
    }

    fn update_remote_baseline(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: norishell_core_api::WireSequence,
        baseline: PortableRemoteBaseline,
    ) -> LocalFuture<'_, Result<PortableProfileState, PortableStoreError>> {
        Box::pin(async move {
            let key = Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.replace_ssh_sync_profile_remote_baseline(
                        &key,
                        expected_state_version,
                        Some(&SshSyncProfileRemoteBaseline {
                            remote_revision: baseline.revision,
                            remote_etag: baseline.etag,
                            baseline_content_sha256: baseline.content_sha256,
                            baseline_exchange_sha256: baseline.exchange_sha256,
                        }),
                    )
                })
                .map(Self::portable_profile_state)
                .map_err(map_store_error)
        })
    }

    fn reset_remote_state(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: norishell_core_api::WireSequence,
        expected_upload_fence: DurableUploadFence,
    ) -> LocalFuture<'_, Result<PortableProfileState, PortableStoreError>> {
        Box::pin(async move {
            let key = SshSyncProfileStateKey {
                plugin_id: PluginId::parse(plugin_id).map_err(|_| PortableStoreError::Rejected)?,
                signer_fingerprint_sha256,
                profile_id,
            };
            let record = self
                .hosts
                .with_ssh_sync_repository(|repository| {
                    repository.reset_ssh_sync_profile_remote_state(
                        &key,
                        expected_state_version,
                        &SshSyncHttpUploadCompletionFence {
                            canonical_url: expected_upload_fence.canonical_url,
                            http_method: match expected_upload_fence.method {
                                PluginSshSyncHttpMethod::Put => SshSyncHttpMethod::Put,
                                PluginSshSyncHttpMethod::Post => SshSyncHttpMethod::Post,
                            },
                            use_oauth: expected_upload_fence.use_oauth,
                            authorization_revision: expected_upload_fence
                                .action_revision
                                .authorization,
                            configuration_revision: expected_upload_fence
                                .action_revision
                                .configuration,
                        },
                    )
                })
                .map_err(map_store_error)?;
            Ok(Self::portable_profile_state(record))
        })
    }

    fn mark_sync_succeeded(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: norishell_core_api::WireSequence,
    ) -> LocalFuture<'_, Result<PortableProfileState, PortableStoreError>> {
        Box::pin(async move {
            let key = Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.mark_ssh_sync_profile_sync_succeeded(&key, expected_state_version)
                })
                .map(Self::portable_profile_state)
                .map_err(map_store_error)
        })
    }

    fn pending_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
    ) -> LocalFuture<'_, Result<Option<PortableUploadAttempt>, PortableStoreError>> {
        Box::pin(async move {
            let owner = Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.get_ssh_sync_http_upload_attempt(&owner)
                })
                .map(|value| value.map(Self::portable_upload_attempt))
                .map_err(map_store_error)
        })
    }

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
    ) -> LocalFuture<'_, Result<PortableUploadAttempt, PortableStoreError>> {
        Box::pin(async move {
            let owner = Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.ensure_ssh_sync_http_upload_attempt(&SshSyncHttpUploadAttemptInput {
                        owner,
                        canonical_url,
                        http_method: match method {
                            PluginSshSyncHttpMethod::Put => SshSyncHttpMethod::Put,
                            PluginSshSyncHttpMethod::Post => SshSyncHttpMethod::Post,
                        },
                        use_oauth,
                        authorization_revision: action_revision.authorization,
                        configuration_revision: action_revision.configuration,
                        base_revision,
                        base_etag,
                        target_revision,
                        keyed_content_sha256,
                        body_sha256,
                        idempotency_key: proposed_idempotency_key,
                    })
                })
                .map(Self::portable_upload_attempt)
                .map_err(map_store_error)
        })
    }

    fn advance_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        expected_state_version: norishell_core_api::WireSequence,
        state: PortableUploadAttemptState,
    ) -> LocalFuture<'_, Result<PortableUploadAttempt, PortableStoreError>> {
        Box::pin(async move {
            let owner = Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            let state = match state {
                PortableUploadAttemptState::Prepared => SshSyncHttpUploadAttemptState::Prepared,
                PortableUploadAttemptState::Sent => SshSyncHttpUploadAttemptState::Sent,
                PortableUploadAttemptState::Verifying => SshSyncHttpUploadAttemptState::Verifying,
            };
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.advance_ssh_sync_http_upload_attempt(
                        &owner,
                        expected_state_version,
                        state,
                    )
                })
                .map(Self::portable_upload_attempt)
                .map_err(map_store_error)
        })
    }

    fn complete_upload_attempt(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        idempotency_key: String,
        body_sha256: String,
        expected_fence: DurableUploadFence,
        proof: PortableUploadCompletionProof,
    ) -> LocalFuture<'_, Result<(), PortableStoreError>> {
        Box::pin(async move {
            let owner = Self::profile_key(&plugin_id, &signer_fingerprint_sha256, &profile_id)?;
            let proof = match proof {
                PortableUploadCompletionProof::ServerAcknowledged {
                    acknowledged_revision,
                } => SshSyncHttpUploadCompletionProof::ServerAcknowledged {
                    acknowledged_revision,
                    acknowledged_body_sha256: body_sha256.clone(),
                },
                PortableUploadCompletionProof::ExactBodyObserved { observed_revision } => {
                    SshSyncHttpUploadCompletionProof::ExactBodyObserved {
                        observed_revision,
                        observed_body_sha256: body_sha256.clone(),
                    }
                }
                PortableUploadCompletionProof::SupersededByNewerRemote {
                    authenticated_remote_revision,
                } => SshSyncHttpUploadCompletionProof::SupersededByNewerRemote {
                    authenticated_remote_revision,
                },
            };
            self.hosts
                .with_ssh_sync_repository(|repository| {
                    repository.complete_ssh_sync_http_upload_attempt(
                        &owner,
                        &idempotency_key,
                        &SshSyncHttpUploadCompletionFence {
                            canonical_url: expected_fence.canonical_url,
                            http_method: match expected_fence.method {
                                PluginSshSyncHttpMethod::Put => SshSyncHttpMethod::Put,
                                PluginSshSyncHttpMethod::Post => SshSyncHttpMethod::Post,
                            },
                            use_oauth: expected_fence.use_oauth,
                            authorization_revision: expected_fence.action_revision.authorization,
                            configuration_revision: expected_fence.action_revision.configuration,
                        },
                        &proof,
                    )
                })
                .map(|_| ())
                .map_err(map_store_error)
        })
    }

    fn stage_restore(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        bundle: PortableBundleV1,
        key: SyncKey,
    ) -> LocalFuture<'_, Result<RestorePreview, PortableStoreError>> {
        Box::pin(async move {
            self.stage_restore_bundle(
                bundle,
                SyncOwner {
                    plugin_id,
                    signer_fingerprint_sha256: signer_fingerprint_sha256.to_owned(),
                    profile_id,
                },
                &key,
            )
        })
    }

    fn apply_staged_restore(
        &self,
        plugin_id: String,
        signer_fingerprint_sha256: String,
        profile_id: String,
        handle: PendingRestoreHandle,
        approval: Option<SecureApplyApproval>,
    ) -> LocalFuture<'_, Result<ApplyResult, PortableStoreError>> {
        Box::pin(async move {
            let _restore_commit = self.restore_commit.lock().await;
            let owner = SyncOwner {
                plugin_id,
                signer_fingerprint_sha256,
                profile_id,
            };
            let (pending, approved) = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.prune_expired();
                let pending = state
                    .pending_restores
                    .remove(handle.token())
                    .ok_or(PortableStoreError::Stale)?;
                state.pending_restore_plaintext_bytes = state
                    .pending_restore_plaintext_bytes
                    .saturating_sub(pending.plaintext_bytes);
                let approved = match approval.as_ref() {
                    Some(value) => state.apply_approvals.remove(value.token()),
                    None => None,
                };
                (pending, approved)
            };
            if pending.handle != handle
                || pending.owner != owner
                || pending.owned_delta.is_none()
                    && !pending.reconcile_noop
                    && portable_bundle_sha256(&pending.bundle)? != pending.bundle_sha256
            {
                return Err(PortableStoreError::Stale);
            }
            let approved = approved.ok_or(PortableStoreError::Stale)?;
            if approved.owner != owner
                || !(approved.fence)()
                || approved.restore_handle.as_ref() != Some(&handle)
            {
                return Err(PortableStoreError::Stale);
            }
            if pending.reconcile_noop {
                let change_fence = pending
                    .change_fence
                    .as_ref()
                    .ok_or(PortableStoreError::Stale)?;
                if let Some(memberships) = pending.scope_memberships.as_deref() {
                    let key = Self::profile_key(
                        &pending.owner.plugin_id,
                        &pending.owner.signer_fingerprint_sha256,
                        &pending.owner.profile_id,
                    )?;
                    let profile = self
                        .hosts
                        .with_ssh_sync_repository(|repository| {
                            repository.get_ssh_sync_profile_state(&key)
                        })
                        .map_err(map_store_error)?
                        .ok_or(PortableStoreError::Stale)?;
                    self.hosts
                        .with_ssh_sync_repository(|repository| {
                            if !(approved.fence)() {
                                return Err(AppPersistenceError::Conflict);
                            }
                            repository.replace_ssh_sync_scope_memberships_with_fence(
                                &key,
                                profile.state_version,
                                memberships,
                                change_fence,
                            )
                        })
                        .map_err(map_store_error)?;
                }
                Ok(ApplyResult {
                    desktop_profile_count: bounded_len(
                        pending.bundle.objects.desktop_profiles.len(),
                    )?,
                    host_count: bounded_len(pending.bundle.objects.hosts.len())?,
                    credential_count: bounded_len(pending.bundle.objects.credentials.len())?,
                    conflict_count: 0,
                })
            } else if let Some(delta) = pending.owned_delta {
                self.apply_owned_reconcile(
                    &pending.bundle,
                    delta,
                    pending.vault_inserts,
                    pending.scope_memberships.as_deref(),
                    pending
                        .change_fence
                        .as_ref()
                        .ok_or(PortableStoreError::Stale)?,
                    &approved.fence,
                )
            } else {
                self.apply_restore_plan(&pending.bundle, &pending.owner, &pending.bundle_sha256)
            }
        })
    }
}

#[tauri::command]
pub(crate) fn ssh_sync_secure_prompt_get(
    request: SshSyncSecurePromptGetRequest,
    window: WebviewWindow,
    adapter: State<'_, NoriShellSshSyncLocalAdapter>,
) -> Result<SshSyncSecurePrompt, String> {
    adapter.prompt_snapshot(request, &window)
}

#[tauri::command]
pub(crate) fn ssh_sync_secure_prompt_decide(
    request: SshSyncSecureDecisionRequest,
    window: WebviewWindow,
    adapter: State<'_, NoriShellSshSyncLocalAdapter>,
) -> Result<SshSyncSecureDecisionResponse, String> {
    adapter.decide_prompt(request, &window)
}

fn secure_host(snapshot: &HostConnectionSnapshot) -> SshSyncSecureHost {
    SshSyncSecureHost {
        host_id: snapshot.host.host_id.clone(),
        label: snapshot.host.label.clone(),
        endpoint: format!("{}:{}", snapshot.host.address, snapshot.host.port),
        credentials: snapshot
            .credentials
            .iter()
            .map(|credential| SshSyncSecureCredential {
                credential_ref_id: credential.credential_ref_id.clone(),
                label: credential.label.clone(),
                method_label: authentication_method_label(credential.method).to_owned(),
                machine_bound: machine_bound(credential),
            })
            .collect(),
    }
}

fn authentication_method_label(method: AuthenticationMethodKind) -> &'static str {
    match method {
        AuthenticationMethodKind::Password => "Password",
        AuthenticationMethodKind::PrivateKey => "Private key",
        AuthenticationMethodKind::KeyboardInteractive => "Keyboard interactive",
        AuthenticationMethodKind::SshAgent => "SSH Agent",
        AuthenticationMethodKind::Certificate => "Certificate",
        AuthenticationMethodKind::HardwareKey => "Hardware key",
    }
}

fn machine_bound(credential: &CredentialRecord) -> bool {
    matches!(
        credential.details,
        CredentialRecordDetails::SshAgent { .. }
            | CredentialRecordDetails::Certificate { .. }
            | CredentialRecordDetails::HardwareKey { .. }
    )
}

fn credential_secret_refs(credential: &CredentialRecord) -> Vec<SecretRefId> {
    match &credential.details {
        CredentialRecordDetails::Password { secret_ref_id } => vec![secret_ref_id.clone()],
        CredentialRecordDetails::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            ..
        } => std::iter::once(secret_ref_id.clone())
            .chain(passphrase_secret_ref_id.iter().cloned())
            .collect(),
        _ => Vec::new(),
    }
}

fn add_credential_to_local_facts(facts: &mut LocalSyncFacts, credential: &CredentialRecord) {
    facts
        .identity_ids
        .insert(credential.identity_id.as_str().to_owned());
    facts
        .credential_ids
        .insert(credential.credential_ref_id.as_str().to_owned());
    facts.secret_ids.extend(
        credential_secret_refs(credential)
            .into_iter()
            .map(|secret_ref_id| secret_ref_id.as_str().to_owned()),
    );
}

fn portable_secret_vault_kind(kind: PortableSecretKind) -> Result<SecretKind, PortableStoreError> {
    match kind {
        PortableSecretKind::Password => Ok(SecretKind::Password),
        PortableSecretKind::PrivateKey => Ok(SecretKind::PrivateKey),
        PortableSecretKind::Passphrase => Ok(SecretKind::Passphrase),
        PortableSecretKind::LoginAutomation => Ok(SecretKind::LoginAutomation),
        PortableSecretKind::Certificate => Err(PortableStoreError::Rejected),
    }
}

fn replace_restore_secret_ref(plan: &mut SshSyncRestorePlan, old: &SecretRefId, new: &SecretRefId) {
    for credential in &mut plan.credentials {
        match &mut credential.material {
            SshSyncRestoreCredentialMaterial::Password { secret_ref_id } => {
                if secret_ref_id == old {
                    *secret_ref_id = new.clone();
                }
            }
            SshSyncRestoreCredentialMaterial::PrivateKey {
                secret_ref_id,
                passphrase_secret_ref_id,
                ..
            } => {
                if secret_ref_id == old {
                    *secret_ref_id = new.clone();
                }
                if passphrase_secret_ref_id.as_ref() == Some(old) {
                    *passphrase_secret_ref_id = Some(new.clone());
                }
            }
            SshSyncRestoreCredentialMaterial::KeyboardInteractive { .. } => {}
        }
    }
    for host in &mut plan.hosts {
        for step in &mut host.login_automation.steps {
            if let SshSyncLoginAutomationStep::SendSecret { secret_ref_id, .. } = step
                && secret_ref_id == old
            {
                *secret_ref_id = new.clone();
            }
        }
    }
}

fn login_automation_contains_secret(
    automation: &SshSyncLoginAutomation,
    secret_ref_id: &SecretRefId,
) -> bool {
    automation.steps.iter().any(|step| {
        matches!(
            step,
            SshSyncLoginAutomationStep::SendSecret {
                secret_ref_id: value,
                ..
            } if value == secret_ref_id
        )
    })
}

fn owned_material_contains_secret(
    material: &SshSyncRestoreCredentialMaterial,
    secret_ref_id: &SecretRefId,
) -> bool {
    match material {
        SshSyncRestoreCredentialMaterial::Password {
            secret_ref_id: value,
        } => value == secret_ref_id,
        SshSyncRestoreCredentialMaterial::PrivateKey {
            secret_ref_id: value,
            passphrase_secret_ref_id,
            ..
        } => value == secret_ref_id || passphrase_secret_ref_id.as_ref() == Some(secret_ref_id),
        SshSyncRestoreCredentialMaterial::KeyboardInteractive { .. } => false,
    }
}

fn host_base_versions(snapshot: &HostConnectionSnapshot) -> SshSyncOwnedHostBaseVersions {
    SshSyncOwnedHostBaseVersions {
        host: snapshot.host.state_version,
        route: snapshot.config.route_plan.revision,
        authentication: snapshot.config.authentication_plan.revision,
        algorithm: snapshot.config.algorithm_policy.revision,
        heartbeat: snapshot.config.heartbeat_policy.revision,
        monitoring: snapshot.config.monitoring_policy.revision,
        login_automation: snapshot.login_automation.revision,
    }
}

fn portable_host_configuration_equal(
    current: &PortableBundleV1,
    target: &PortableBundleV1,
    host_id: PortableObjectId,
) -> bool {
    let Some(current_host) = current
        .objects
        .hosts
        .iter()
        .find(|value| value.id == host_id)
    else {
        return false;
    };
    let Some(target_host) = target
        .objects
        .hosts
        .iter()
        .find(|value| value.id == host_id)
    else {
        return false;
    };
    current_host == target_host
        && current
            .objects
            .routes
            .iter()
            .find(|value| value.id == current_host.route_id)
            == target
                .objects
                .routes
                .iter()
                .find(|value| value.id == target_host.route_id)
        && current
            .objects
            .authentication_plans
            .iter()
            .find(|value| value.id == current_host.authentication_plan_id)
            == target
                .objects
                .authentication_plans
                .iter()
                .find(|value| value.id == target_host.authentication_plan_id)
        && current
            .objects
            .algorithm_policies
            .iter()
            .find(|value| value.id == current_host.algorithm_policy_id)
            == target
                .objects
                .algorithm_policies
                .iter()
                .find(|value| value.id == target_host.algorithm_policy_id)
        && current
            .objects
            .heartbeat_policies
            .iter()
            .find(|value| value.id == current_host.heartbeat_policy_id)
            == target
                .objects
                .heartbeat_policies
                .iter()
                .find(|value| value.id == target_host.heartbeat_policy_id)
        && current
            .objects
            .monitoring_policies
            .iter()
            .find(|value| value.id == current_host.monitoring_policy_id)
            == target
                .objects
                .monitoring_policies
                .iter()
                .find(|value| value.id == target_host.monitoring_policy_id)
        && current_host.login_automation_id.and_then(|id| {
            current
                .objects
                .login_automations
                .iter()
                .find(|value| value.id == id)
        }) == target_host.login_automation_id.and_then(|id| {
            target
                .objects
                .login_automations
                .iter()
                .find(|value| value.id == id)
        })
}

fn bounded_len(value: usize) -> Result<u32, PortableStoreError> {
    u32::try_from(value).map_err(|_| PortableStoreError::Rejected)
}

fn local_mapping_id(value: &SshSyncLocalObjectId) -> &str {
    match value {
        SshSyncLocalObjectId::DesktopProfile(value) => value.as_str(),
        SshSyncLocalObjectId::Host(value) => value.as_str(),
        SshSyncLocalObjectId::Identity(value) => value.as_str(),
        SshSyncLocalObjectId::Credential(value) => value.as_str(),
        SshSyncLocalObjectId::Secret(value) => value.as_str(),
    }
}

fn mapped_host_id(mappings: &LocalIdMap, id: PortableObjectId) -> Option<HostId> {
    match mappings.get(&(SshSyncObjectKind::Host, id)) {
        Some(SshSyncLocalObjectId::Host(value)) => Some(value.clone()),
        _ => None,
    }
}

fn mapped_identity_id(mappings: &LocalIdMap, id: PortableObjectId) -> Option<IdentityId> {
    match mappings.get(&(SshSyncObjectKind::Identity, id)) {
        Some(SshSyncLocalObjectId::Identity(value)) => Some(value.clone()),
        _ => None,
    }
}

fn mapped_credential_id(mappings: &LocalIdMap, id: PortableObjectId) -> Option<CredentialRefId> {
    match mappings.get(&(SshSyncObjectKind::Credential, id)) {
        Some(SshSyncLocalObjectId::Credential(value)) => Some(value.clone()),
        _ => None,
    }
}

fn mapped_secret_id(mappings: &LocalIdMap, id: PortableObjectId) -> Option<SecretRefId> {
    match mappings.get(&(SshSyncObjectKind::Secret, id)) {
        Some(SshSyncLocalObjectId::Secret(value)) => Some(value.clone()),
        _ => None,
    }
}

fn base_contains_mapped_object(
    base: &PortableBundleV1,
    kind: SshSyncObjectKind,
    id: PortableObjectId,
) -> bool {
    match kind {
        SshSyncObjectKind::DesktopProfile => base
            .objects
            .desktop_profiles
            .iter()
            .any(|value| value.id == id),
        SshSyncObjectKind::Host => base.objects.hosts.iter().any(|value| value.id == id),
        SshSyncObjectKind::Identity => base.objects.identities.iter().any(|value| value.id == id),
        SshSyncObjectKind::Credential => {
            base.objects.credentials.iter().any(|value| value.id == id)
        }
        SshSyncObjectKind::Secret => base.secrets.iter().any(|value| value.id == id),
    }
}

const fn portable_object_kind(kind: SshSyncObjectKind) -> PortableObjectKind {
    match kind {
        SshSyncObjectKind::DesktopProfile => PortableObjectKind::DesktopProfile,
        SshSyncObjectKind::Host => PortableObjectKind::Host,
        SshSyncObjectKind::Identity => PortableObjectKind::Identity,
        SshSyncObjectKind::Credential => PortableObjectKind::Credential,
        SshSyncObjectKind::Secret => PortableObjectKind::Secret,
    }
}

fn bundle_contains_portable_object(
    bundle: &PortableBundleV1,
    kind: PortableObjectKind,
    id: PortableObjectId,
) -> bool {
    match kind {
        PortableObjectKind::DesktopProfile => bundle
            .objects
            .desktop_profiles
            .iter()
            .any(|value| value.id == id),
        PortableObjectKind::Host => bundle.objects.hosts.iter().any(|value| value.id == id),
        PortableObjectKind::Identity => {
            bundle.objects.identities.iter().any(|value| value.id == id)
        }
        PortableObjectKind::Credential => bundle
            .objects
            .credentials
            .iter()
            .any(|value| value.id == id),
        PortableObjectKind::Route => bundle.objects.routes.iter().any(|value| value.id == id),
        PortableObjectKind::AuthenticationPlan => bundle
            .objects
            .authentication_plans
            .iter()
            .any(|value| value.id == id),
        PortableObjectKind::AlgorithmPolicy => bundle
            .objects
            .algorithm_policies
            .iter()
            .any(|value| value.id == id),
        PortableObjectKind::HeartbeatPolicy => bundle
            .objects
            .heartbeat_policies
            .iter()
            .any(|value| value.id == id),
        PortableObjectKind::MonitoringPolicy => bundle
            .objects
            .monitoring_policies
            .iter()
            .any(|value| value.id == id),
        PortableObjectKind::LoginAutomation => bundle
            .objects
            .login_automations
            .iter()
            .any(|value| value.id == id),
        PortableObjectKind::Secret => bundle.secrets.iter().any(|value| value.id == id),
    }
}

fn machine_bound_kind(credential: &CredentialRecord) -> MachineBoundKind {
    match credential.details {
        CredentialRecordDetails::SshAgent { .. } | CredentialRecordDetails::Certificate { .. } => {
            MachineBoundKind::SshAgent
        }
        CredentialRecordDetails::HardwareKey { .. } => MachineBoundKind::HardwareKey,
        _ => MachineBoundKind::SshAgent,
    }
}

fn portable_id(domain: &str, value: &str) -> PortableObjectId {
    let digest = Sha256::digest(format!("norishell:ssh-sync:v1:{domain}:{value}"));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    PortableObjectId::from_uuid(Uuid::from_bytes(bytes)).expect("derived portable ID is non-nil")
}

fn resolved_portable_id(
    mappings: Option<&PortableIdMap>,
    kind: SshSyncObjectKind,
    local_id: &str,
    legacy_domain: &str,
) -> PortableObjectId {
    mappings
        .and_then(|values| values.get(&(kind, local_id.to_owned())).copied())
        .unwrap_or_else(|| portable_id(legacy_domain, local_id))
}

fn preflight_restore(bundle: &PortableBundleV1) -> Result<(), PortableStoreError> {
    let identities = bundle
        .objects
        .identities
        .iter()
        .map(|value| value.id)
        .collect::<BTreeSet<_>>();
    let credentials = bundle
        .objects
        .credentials
        .iter()
        .map(|value| value.id)
        .collect::<BTreeSet<_>>();
    let secrets = bundle
        .secrets
        .iter()
        .map(|value| (value.id, value.kind))
        .collect::<BTreeMap<_, _>>();
    let mut password_credentials = BTreeSet::new();
    for credential in &bundle.objects.credentials {
        if !identities.contains(&credential.identity_id) {
            return Err(PortableStoreError::Rejected);
        }
        match credential.material {
            PortableCredentialMaterial::Password { password_secret_id } => {
                if secrets.get(&password_secret_id) != Some(&PortableSecretKind::Password) {
                    return Err(PortableStoreError::Rejected);
                }
                password_credentials.insert(credential.id);
            }
            PortableCredentialMaterial::PrivateKey {
                private_key_secret_id,
                passphrase_secret_id,
                ..
            } => {
                if secrets.get(&private_key_secret_id) != Some(&PortableSecretKind::PrivateKey)
                    || passphrase_secret_id
                        .is_some_and(|id| secrets.get(&id) != Some(&PortableSecretKind::Passphrase))
                {
                    return Err(PortableStoreError::Rejected);
                }
            }
            PortableCredentialMaterial::KeyboardInteractive { .. } => {}
            PortableCredentialMaterial::Certificate { .. } => {
                return Err(PortableStoreError::Rejected);
            }
        }
    }
    for route in &bundle.objects.routes {
        let credential_id = match route.ingress {
            PortableIngress::Direct => None,
            PortableIngress::HttpConnect { credential_id, .. }
            | PortableIngress::Socks5 { credential_id, .. } => credential_id,
        };
        if credential_id.is_some_and(|id| !password_credentials.contains(&id)) {
            return Err(PortableStoreError::Rejected);
        }
    }
    let routes = bundle
        .objects
        .routes
        .iter()
        .map(|value| (value.id, value))
        .collect::<BTreeMap<_, _>>();
    let plans = bundle
        .objects
        .authentication_plans
        .iter()
        .map(|value| (value.id, value))
        .collect::<BTreeMap<_, _>>();
    let mut resolved = BTreeSet::new();
    let mut remaining = bundle.objects.hosts.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|host| {
            let Some(route) = routes.get(&host.route_id) else {
                return true;
            };
            if route
                .jump_hops
                .iter()
                .any(|hop| !resolved.contains(&hop.host_id))
            {
                return true;
            }
            let valid = host
                .identity_id
                .is_none_or(|identity_id| identities.contains(&identity_id))
                && plans.get(&host.authentication_plan_id).is_some_and(|plan| {
                    plan.credential_ids
                        .iter()
                        .all(|id| credentials.contains(id))
                });
            if valid {
                resolved.insert(host.id);
                false
            } else {
                true
            }
        });
        if remaining.len() == before {
            return Err(PortableStoreError::Rejected);
        }
    }
    Ok(())
}

fn deterministic_identity_id(
    restore_namespace: &str,
    id: PortableObjectId,
) -> Result<IdentityId, PortableStoreError> {
    IdentityId::parse(
        deterministic_named_v7(restore_namespace, "identity", &id.as_uuid().to_string())
            .to_string(),
    )
    .map_err(|_| PortableStoreError::Internal)
}

fn deterministic_credential_ref_id(
    restore_namespace: &str,
    id: PortableObjectId,
) -> Result<CredentialRefId, PortableStoreError> {
    CredentialRefId::parse(
        deterministic_named_v7(
            restore_namespace,
            "credential-ref",
            &id.as_uuid().to_string(),
        )
        .to_string(),
    )
    .map_err(|_| PortableStoreError::Internal)
}

fn deterministic_host_id(
    restore_namespace: &str,
    id: PortableObjectId,
) -> Result<HostId, PortableStoreError> {
    HostId::parse(
        deterministic_named_v7(restore_namespace, "host-id", &id.as_uuid().to_string()).to_string(),
    )
    .map_err(|_| PortableStoreError::Internal)
}

fn deterministic_secret_ref_id(
    restore_namespace: &str,
    id: PortableObjectId,
) -> Result<SecretRefId, PortableStoreError> {
    SecretRefId::parse(
        deterministic_named_v7(restore_namespace, "secret-ref", &id.as_uuid().to_string())
            .to_string(),
    )
    .map_err(|_| PortableStoreError::Internal)
}

fn deterministic_operation_id(
    restore_namespace: &str,
    domain: &str,
    id: PortableObjectId,
) -> Result<OperationId, PortableStoreError> {
    deterministic_named_operation_id(restore_namespace, domain, &id.as_uuid().to_string())
}

fn deterministic_named_operation_id(
    restore_namespace: &str,
    domain: &str,
    value: &str,
) -> Result<OperationId, PortableStoreError> {
    OperationId::parse(deterministic_named_v7(restore_namespace, domain, value).to_string())
        .map_err(|_| PortableStoreError::Internal)
}

fn deterministic_named_v7(restore_namespace: &str, domain: &str, value: &str) -> Uuid {
    let digest = Sha256::digest(format!(
        "norishell:ssh-sync-restore:v3:{restore_namespace}:{domain}:{value}"
    ));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn restore_idempotency_key(restore_namespace: &str, domain: &str, id: PortableObjectId) -> String {
    let namespace_hash = Sha256::digest(restore_namespace.as_bytes())
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("ssh-sync-{domain}-{namespace_hash}-{}", id.as_uuid())
}

fn portable_bundle_sha256(bundle: &PortableBundleV1) -> Result<String, PortableStoreError> {
    let encoded = canonical_bundle_bytes(bundle).map_err(|_| PortableStoreError::Internal)?;
    Ok(Sha256::digest(encoded.as_slice())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn sha256_text(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stable_secret_insert(
    restore_namespace: &str,
    local_ids: &LocalIdMap,
    secret_id: PortableObjectId,
    expected_portable_kind: PortableSecretKind,
    vault_kind: SecretKind,
    secrets: &BTreeMap<PortableObjectId, &PortableSecret>,
    inserts: &mut BTreeMap<String, VaultSecretInsert>,
) -> Result<SecretRefId, PortableStoreError> {
    let secret = secrets
        .get(&secret_id)
        .ok_or(PortableStoreError::Rejected)?;
    if secret.kind != expected_portable_kind || !secret.selected_by_user {
        return Err(PortableStoreError::Rejected);
    }
    let secret_ref_id = mapped_secret_id(local_ids, secret_id).map_or_else(
        || deterministic_secret_ref_id(restore_namespace, secret_id),
        Ok,
    )?;
    if !inserts.contains_key(secret_ref_id.as_str()) {
        inserts.insert(
            secret_ref_id.as_str().to_owned(),
            VaultSecretInsert {
                secret_ref_id: secret_ref_id.clone(),
                kind: vault_kind,
                value: secret.payload.copy_for_vault(),
            },
        );
    }
    Ok(secret_ref_id)
}

fn validate_restore_vault_batch(
    inserts: &[VaultSecretInsert],
    new_secret_ref_ids: &[SecretRefId],
) -> Result<(), PortableStoreError> {
    validate_restore_vault_batch_limits(new_secret_ref_ids.len(), 0)?;
    let expected = new_secret_ref_ids
        .iter()
        .map(SecretRefId::as_str)
        .collect::<BTreeSet<_>>();
    if expected.len() != new_secret_ref_ids.len() {
        return Err(PortableStoreError::Rejected);
    }
    let mut matched = 0_usize;
    let mut value_bytes = 0_usize;
    for insert in inserts {
        if expected.contains(insert.secret_ref_id.as_str()) {
            matched = matched.checked_add(1).ok_or(PortableStoreError::Rejected)?;
            value_bytes = value_bytes
                .checked_add(insert.value.len())
                .ok_or(PortableStoreError::Rejected)?;
        }
    }
    if matched != expected.len() {
        return Err(PortableStoreError::Rejected);
    }
    validate_restore_vault_batch_limits(matched, value_bytes)?;
    Ok(())
}

fn validate_restore_vault_batch_limits(
    entry_count: usize,
    value_bytes: usize,
) -> Result<(), PortableStoreError> {
    if entry_count > MAX_SECRET_BATCH_ENTRIES || value_bytes > MAX_SECRET_BATCH_VALUE_BYTES {
        Err(PortableStoreError::Rejected)
    } else {
        Ok(())
    }
}

fn portable_route(
    snapshot: &HostConnectionSnapshot,
    id: PortableObjectId,
    portable_ids: Option<&PortableIdMap>,
    proxy_auth: &BTreeMap<String, (Option<String>, PortableObjectId)>,
) -> Result<PortableRoute, PortableStoreError> {
    let ingress = match &snapshot.config.route_plan.ingress {
        RouteIngress::DirectTcp => PortableIngress::Direct,
        RouteIngress::HttpConnectProxy {
            endpoint,
            proxy_auth_credential_ref_id,
        } => {
            let (username, credential_id) =
                portable_proxy_auth(proxy_auth_credential_ref_id.as_ref(), proxy_auth)?;
            PortableIngress::HttpConnect {
                host: endpoint.address.clone(),
                port: endpoint.port,
                username,
                credential_id,
            }
        }
        RouteIngress::Socks5Proxy {
            endpoint,
            dns_mode,
            proxy_auth_credential_ref_id,
        } => {
            let (username, credential_id) =
                portable_proxy_auth(proxy_auth_credential_ref_id.as_ref(), proxy_auth)?;
            PortableIngress::Socks5 {
                host: endpoint.address.clone(),
                port: endpoint.port,
                username,
                credential_id,
                resolve_dns_remotely: *dns_mode == ProxyDnsMode::Proxy,
            }
        }
    };
    Ok(PortableRoute {
        id,
        ingress,
        jump_hops: snapshot
            .config
            .route_plan
            .jump_host_ids
            .iter()
            .map(|host_id| norishell_ssh_profile_sync::JumpHop {
                host_id: resolved_portable_id(
                    portable_ids,
                    SshSyncObjectKind::Host,
                    host_id.as_str(),
                    "host",
                ),
            })
            .collect(),
    })
}

fn portable_proxy_auth(
    credential_ref_id: Option<&CredentialRefId>,
    proxy_auth: &BTreeMap<String, (Option<String>, PortableObjectId)>,
) -> Result<(Option<String>, Option<PortableObjectId>), PortableStoreError> {
    credential_ref_id.map_or(Ok((None, None)), |credential_ref_id| {
        proxy_auth
            .get(credential_ref_id.as_str())
            .cloned()
            .map(|(username, secret_id)| (username, Some(secret_id)))
            .ok_or(PortableStoreError::Rejected)
    })
}

fn proxy_credential_ref(snapshot: &HostConnectionSnapshot) -> Option<&CredentialRefId> {
    match &snapshot.config.route_plan.ingress {
        RouteIngress::DirectTcp => None,
        RouteIngress::HttpConnectProxy {
            proxy_auth_credential_ref_id,
            ..
        }
        | RouteIngress::Socks5Proxy {
            proxy_auth_credential_ref_id,
            ..
        } => proxy_auth_credential_ref_id.as_ref(),
    }
}

fn portable_algorithm_policy(
    snapshot: &HostConnectionSnapshot,
    id: PortableObjectId,
) -> PortableAlgorithmPolicy {
    PortableAlgorithmPolicy {
        id,
        policy_id: snapshot.config.algorithm_policy.policy_id.clone(),
        compatibility_exceptions: snapshot
            .config
            .algorithm_policy
            .compatibility_exceptions
            .iter()
            .map(|value| PortableAlgorithmCompatibilityException {
                category: match value.category {
                    AlgorithmCategory::KeyExchange => PortableAlgorithmCategory::KeyExchange,
                    AlgorithmCategory::HostKey => PortableAlgorithmCategory::HostKey,
                    AlgorithmCategory::Cipher => PortableAlgorithmCategory::Cipher,
                    AlgorithmCategory::Mac => PortableAlgorithmCategory::Mac,
                },
                exception_id: value.exception_id.clone(),
                reason: value.reason.clone(),
            })
            .collect(),
    }
}

fn portable_heartbeat_policy(
    snapshot: &HostConnectionSnapshot,
    id: PortableObjectId,
) -> PortableHeartbeatPolicy {
    let mode = match &snapshot.config.heartbeat_policy.policy {
        HeartbeatPolicy::Disabled => HeartbeatMode::Disabled,
        HeartbeatPolicy::TransportKeepalive {
            interval_seconds,
            reply_timeout_seconds,
            failure_threshold,
        } => HeartbeatMode::TransportKeepalive {
            interval_seconds: *interval_seconds,
            reply_timeout_seconds: *reply_timeout_seconds,
            max_missed_replies: *failure_threshold,
        },
        HeartbeatPolicy::ShellHeartbeat {
            payload_text,
            line_ending,
            interval_seconds,
            user_idle_seconds,
        } => HeartbeatMode::ShellHeartbeat {
            interval_seconds: *interval_seconds,
            idle_seconds: *user_idle_seconds,
            payload: payload_text.clone(),
            line_ending: match line_ending {
                ShellHeartbeatLineEnding::None => PortableShellHeartbeatLineEnding::None,
                ShellHeartbeatLineEnding::Cr => PortableShellHeartbeatLineEnding::Cr,
                ShellHeartbeatLineEnding::Lf => PortableShellHeartbeatLineEnding::Lf,
                ShellHeartbeatLineEnding::Crlf => PortableShellHeartbeatLineEnding::Crlf,
            },
        },
    };
    PortableHeartbeatPolicy { id, mode }
}

fn portable_monitoring_policy(
    snapshot: &HostConnectionSnapshot,
    id: PortableObjectId,
) -> PortableMonitoringPolicy {
    let policy = &snapshot.config.monitoring_policy.policy;
    let mut resources = BTreeSet::from([MonitoringResource::Cpu, MonitoringResource::Memory]);
    if !policy.disk_mount_ids.is_empty() {
        resources.insert(MonitoringResource::RootDisk);
    }
    if !policy.network_interface_ids.is_empty() {
        resources.insert(MonitoringResource::AggregateNonLoopbackNetwork);
    }
    PortableMonitoringPolicy {
        id,
        enabled: policy.enabled,
        interval_seconds: policy.sample_interval_seconds,
        timeout_seconds: policy.sample_timeout_seconds,
        resources,
    }
}

fn parse_public_key_algorithm(value: &str) -> Option<PublicKeyAlgorithm> {
    match value {
        "ssh-ed25519" => Some(PublicKeyAlgorithm::Ed25519),
        "rsa-sha2-256" | "ssh-rsa" => Some(PublicKeyAlgorithm::RsaSha256),
        "rsa-sha2-512" => Some(PublicKeyAlgorithm::RsaSha512),
        "ecdsa-sha2-nistp256" => Some(PublicKeyAlgorithm::EcdsaP256),
        "ecdsa-sha2-nistp384" => Some(PublicKeyAlgorithm::EcdsaP384),
        "ecdsa-sha2-nistp521" => Some(PublicKeyAlgorithm::EcdsaP521),
        _ => None,
    }
}

const fn public_key_algorithm_wire(value: PublicKeyAlgorithm) -> &'static str {
    match value {
        PublicKeyAlgorithm::Ed25519 => "ssh-ed25519",
        PublicKeyAlgorithm::RsaSha256 => "rsa-sha2-256",
        PublicKeyAlgorithm::RsaSha512 => "rsa-sha2-512",
        PublicKeyAlgorithm::EcdsaP256 => "ecdsa-sha2-nistp256",
        PublicKeyAlgorithm::EcdsaP384 => "ecdsa-sha2-nistp384",
        PublicKeyAlgorithm::EcdsaP521 => "ecdsa-sha2-nistp521",
    }
}

fn restore_route_ingress(
    value: &PortableIngress,
    proxy_credentials: &BTreeMap<PortableObjectId, CredentialRefId>,
) -> Result<RouteIngress, PortableStoreError> {
    Ok(match value {
        PortableIngress::Direct => RouteIngress::DirectTcp,
        PortableIngress::HttpConnect {
            host,
            port,
            credential_id,
            ..
        } => RouteIngress::HttpConnectProxy {
            endpoint: norishell_core_api::ProxyEndpoint {
                address: host.clone(),
                normalized_address: host.clone(),
                port: *port,
            },
            proxy_auth_credential_ref_id: credential_id
                .map(|id| {
                    proxy_credentials
                        .get(&id)
                        .cloned()
                        .ok_or(PortableStoreError::Rejected)
                })
                .transpose()?,
        },
        PortableIngress::Socks5 {
            host,
            port,
            credential_id,
            resolve_dns_remotely,
            ..
        } => RouteIngress::Socks5Proxy {
            endpoint: norishell_core_api::ProxyEndpoint {
                address: host.clone(),
                normalized_address: host.clone(),
                port: *port,
            },
            dns_mode: if *resolve_dns_remotely {
                ProxyDnsMode::Proxy
            } else {
                ProxyDnsMode::Local
            },
            proxy_auth_credential_ref_id: credential_id
                .map(|id| {
                    proxy_credentials
                        .get(&id)
                        .cloned()
                        .ok_or(PortableStoreError::Rejected)
                })
                .transpose()?,
        },
    })
}

fn restore_login_automation(
    automation_id: Option<PortableObjectId>,
    automations: &BTreeMap<PortableObjectId, &PortableLoginAutomation>,
    restore_namespace: &str,
    local_ids: &LocalIdMap,
    secrets: &BTreeMap<PortableObjectId, &PortableSecret>,
    vault_inserts: &mut BTreeMap<String, VaultSecretInsert>,
) -> Result<SshSyncLoginAutomation, PortableStoreError> {
    let Some(automation_id) = automation_id else {
        return Ok(SshSyncLoginAutomation::disabled());
    };
    let automation = automations
        .get(&automation_id)
        .ok_or(PortableStoreError::Rejected)?;
    let mut steps = Vec::with_capacity(automation.steps.len());
    for step in &automation.steps {
        steps.push(match step {
            PortableLoginAutomationStep::Expect {
                pattern,
                timeout_seconds,
            } => SshSyncLoginAutomationStep::Expect {
                literal_text: pattern.clone(),
                timeout_seconds: u8::try_from(*timeout_seconds)
                    .map_err(|_| PortableStoreError::Rejected)?,
            },
            PortableLoginAutomationStep::SendText {
                text,
                append_enter,
                timeout_seconds,
            } => SshSyncLoginAutomationStep::SendText {
                text: text.clone(),
                append_enter: *append_enter,
                timeout_seconds: u8::try_from(*timeout_seconds)
                    .map_err(|_| PortableStoreError::Rejected)?,
            },
            PortableLoginAutomationStep::SendSecret {
                secret_id,
                secret_label,
                append_enter,
                timeout_seconds,
            } => SshSyncLoginAutomationStep::SendSecret {
                secret_ref_id: stable_secret_insert(
                    restore_namespace,
                    local_ids,
                    *secret_id,
                    PortableSecretKind::LoginAutomation,
                    SecretKind::LoginAutomation,
                    secrets,
                    vault_inserts,
                )?,
                secret_label: secret_label.clone(),
                append_enter: *append_enter,
                timeout_seconds: u8::try_from(*timeout_seconds)
                    .map_err(|_| PortableStoreError::Rejected)?,
            },
        });
    }
    Ok(SshSyncLoginAutomation {
        enabled: true,
        // A definition may synchronize, but another device must explicitly
        // reconfirm it before the Core executes any secret-bearing input.
        confirmed: false,
        steps,
    })
}

fn restore_algorithm_exception(
    value: &PortableAlgorithmCompatibilityException,
) -> norishell_core_api::AlgorithmCompatibilityException {
    norishell_core_api::AlgorithmCompatibilityException {
        category: match value.category {
            PortableAlgorithmCategory::KeyExchange => AlgorithmCategory::KeyExchange,
            PortableAlgorithmCategory::HostKey => AlgorithmCategory::HostKey,
            PortableAlgorithmCategory::Cipher => AlgorithmCategory::Cipher,
            PortableAlgorithmCategory::Mac => AlgorithmCategory::Mac,
        },
        exception_id: value.exception_id.clone(),
        reason: value.reason.clone(),
    }
}

fn restore_heartbeat(value: &PortableHeartbeatPolicy) -> HeartbeatPolicy {
    match &value.mode {
        HeartbeatMode::Disabled => HeartbeatPolicy::Disabled,
        HeartbeatMode::TransportKeepalive {
            interval_seconds,
            reply_timeout_seconds,
            max_missed_replies,
        } => HeartbeatPolicy::TransportKeepalive {
            interval_seconds: *interval_seconds,
            reply_timeout_seconds: *reply_timeout_seconds,
            failure_threshold: *max_missed_replies,
        },
        HeartbeatMode::ShellHeartbeat {
            interval_seconds,
            idle_seconds,
            payload,
            line_ending,
        } => HeartbeatPolicy::ShellHeartbeat {
            payload_text: payload.clone(),
            line_ending: match line_ending {
                PortableShellHeartbeatLineEnding::None => ShellHeartbeatLineEnding::None,
                PortableShellHeartbeatLineEnding::Cr => ShellHeartbeatLineEnding::Cr,
                PortableShellHeartbeatLineEnding::Lf => ShellHeartbeatLineEnding::Lf,
                PortableShellHeartbeatLineEnding::Crlf => ShellHeartbeatLineEnding::Crlf,
            },
            interval_seconds: *interval_seconds,
            user_idle_seconds: *idle_seconds,
        },
    }
}

fn restore_monitoring(value: &PortableMonitoringPolicy) -> MonitoringPolicy {
    MonitoringPolicy {
        enabled: value.enabled,
        sample_interval_seconds: value.interval_seconds,
        sample_timeout_seconds: value.timeout_seconds,
        disk_mount_ids: value
            .resources
            .contains(&MonitoringResource::RootDisk)
            .then_some(norishell_core_api::DiskResourceId::Root)
            .into_iter()
            .collect(),
        network_interface_ids: value
            .resources
            .contains(&MonitoringResource::AggregateNonLoopbackNetwork)
            .then_some(norishell_core_api::NetworkResourceId::AggregateNonLoopback)
            .into_iter()
            .collect(),
    }
}

fn validate_decision(
    prompt: &SshSyncSecurePrompt,
    request: &SshSyncSecureDecisionRequest,
) -> Result<(), String> {
    if request.prompt_id != prompt.prompt_id {
        return Err(Uuid::new_v4().to_string());
    }
    if (prompt.kind == SshSyncSecurePromptKind::AuthorizeProvider) != prompt.oauth.is_some() {
        return Err(Uuid::new_v4().to_string());
    }
    let decision_allowed = match prompt.kind {
        SshSyncSecurePromptKind::ResolveConflicts
        | SshSyncSecurePromptKind::ChooseSyncDirection => matches!(
            request.decision,
            SshSyncSecureDecision::KeepLocal | SshSyncSecureDecision::UseRemote
        ),
        _ => request.decision == SshSyncSecureDecision::Approve,
    };
    if !decision_allowed {
        return Err(Uuid::new_v4().to_string());
    }
    let allowed_hosts = prompt
        .hosts
        .iter()
        .map(|host| host.host_id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let allowed_desktops = prompt
        .desktop_profiles
        .iter()
        .map(|p| p.profile_id.as_str())
        .collect::<BTreeSet<_>>();
    if request.selected_desktop_profile_ids.len() > allowed_desktops.len()
        || request
            .selected_desktop_profile_ids
            .iter()
            .any(|id| !allowed_desktops.contains(id.as_str()))
        || request
            .selected_desktop_profile_ids
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != request.selected_desktop_profile_ids.len()
    {
        return Err(Uuid::new_v4().to_string());
    }
    let mut allowed_credentials = prompt
        .hosts
        .iter()
        .flat_map(|host| {
            host.credentials
                .iter()
                .filter(|credential| !credential.machine_bound)
                .map(|credential| credential.credential_ref_id.as_str().to_owned())
        })
        .collect::<BTreeSet<_>>();
    allowed_credentials.extend(
        prompt
            .desktop_profiles
            .iter()
            .flat_map(|p| p.credentials.iter())
            .filter(|c| !c.machine_bound)
            .map(|c| c.credential_ref_id.as_str().to_owned()),
    );
    if request.selected_host_ids.len() > allowed_hosts.len()
        || request.selected_credential_ref_ids.len() > allowed_credentials.len()
        || request
            .selected_host_ids
            .iter()
            .any(|value| !allowed_hosts.contains(value.as_str()))
        || request
            .selected_credential_ref_ids
            .iter()
            .any(|value| !allowed_credentials.contains(value.as_str()))
        || request
            .selected_host_ids
            .iter()
            .map(HostId::as_str)
            .collect::<BTreeSet<_>>()
            .len()
            != request.selected_host_ids.len()
        || request
            .selected_credential_ref_ids
            .iter()
            .map(CredentialRefId::as_str)
            .collect::<BTreeSet<_>>()
            .len()
            != request.selected_credential_ref_ids.len()
    {
        return Err(Uuid::new_v4().to_string());
    }
    if prompt.kind == SshSyncSecurePromptKind::CreateLocalVault {
        let matches = request
            .vault_password
            .as_ref()
            .zip(request.vault_password_confirmation.as_ref())
            .is_some_and(|(password, confirmation)| {
                crate::vault_service::confirmations_match(
                    password.as_bytes(),
                    confirmation.as_bytes(),
                )
            });
        if !matches {
            return Err(Uuid::new_v4().to_string());
        }
    } else if request.vault_password_confirmation.is_some() {
        return Err(Uuid::new_v4().to_string());
    }
    let needs_password = matches!(
        prompt.kind,
        SshSyncSecurePromptKind::CreateLocalVault
            | SshSyncSecurePromptKind::RecoverSynchronizedKey
            | SshSyncSecurePromptKind::CreateRecoveryPassword
            | SshSyncSecurePromptKind::RecoverExistingKey
            | SshSyncSecurePromptKind::UnlockSynchronizedVault
    );
    if needs_password
        != request
            .vault_password
            .as_ref()
            .is_some_and(|value| (12..=65_536).contains(&value.len()))
    {
        return Err(Uuid::new_v4().to_string());
    }
    Ok(())
}

fn secure_window_label(prompt_id: &str) -> String {
    format!("secure-ssh-sync-{prompt_id}")
}

fn require_secure_window(window: &WebviewWindow, prompt_id: &str) -> Result<(), String> {
    if Uuid::parse_str(prompt_id).is_err() || window.label() != secure_window_label(prompt_id) {
        return Err(Uuid::new_v4().to_string());
    }
    Ok(())
}

fn map_store_error(error: AppPersistenceError) -> PortableStoreError {
    match error {
        AppPersistenceError::Conflict | AppPersistenceError::IdempotencyConflict => {
            PortableStoreError::Stale
        }
        AppPersistenceError::InvalidInput(_)
        | AppPersistenceError::Endpoint(_)
        | AppPersistenceError::InvalidStoredData => PortableStoreError::Rejected,
        _ => PortableStoreError::Internal,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::Arc,
        time::{Duration, Instant},
    };

    use norishell_app_persistence::{
        CredentialImportState, CredentialRecord, CredentialRecordDetails,
    };
    use norishell_core_api::{
        AuthenticationMethodKind, CredentialKind, CredentialRefId, DesktopProfile, DesktopProtocol,
        HostId, IdentityId, OperationId, RequestId, RequestMeta, RouteIngress, SecretRefId,
        SshSyncSecureCredential, SshSyncSecureDecision, SshSyncSecureDecisionRequest,
        SshSyncSecureHost, SshSyncSecurePrompt, SshSyncSecurePromptKind, WireSequence,
    };
    use norishell_secret_vault::{
        MAX_SECRET_BATCH_ENTRIES, MAX_SECRET_BATCH_VALUE_BYTES, SecretKind,
    };
    use norishell_ssh_profile_sync::{
        BundleSchema, PluginExchangeBinding, PortableBundleV1, PortableObjectId, PortableObjects,
        RouteIngress as PortableIngress, SyncKey, create_plugin_exchange_with_key,
        open_plugin_exchange_with_key,
    };
    use uuid::Uuid;
    use zeroize::Zeroizing;

    use super::{
        BackupSelection, BackupSelectionLease, LocalState, LocalSyncFacts,
        NoriShellSshSyncLocalAdapter, PendingRestore, SELECTION_LEASE_TIMEOUT, SyncOwner,
        add_credential_to_local_facts, deterministic_operation_id, restore_route_ingress,
        validate_decision, validate_restore_vault_batch_limits,
    };
    use crate::{
        host_service::HostService,
        ssh_sync_exchange::{PendingRestoreHandle, PortableSshProfileStore},
        vault_service::{VaultSecretInsert, VaultService},
    };

    fn request_meta() -> RequestMeta {
        RequestMeta {
            request_id: RequestId::new(),
        }
    }

    #[tokio::test]
    async fn background_sync_status_distinguishes_missing_locked_and_ready_without_prompts() {
        use crate::ssh_sync_exchange::{
            SshSyncActionRevision, SshSyncExchangeBroker, SshSyncInvocationBinding,
        };
        use norishell_core_api::{PluginSshSyncRequest, PluginSshSyncStableErrorCode};
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        let hosts = HostService::start(directory.path()).expect("hosts");
        let adapter = Arc::new(NoriShellSshSyncLocalAdapter::new_for_store_test(
            hosts,
            vault.clone(),
        ));
        let broker = SshSyncExchangeBroker::new(
            directory.path(),
            vault.clone(),
            adapter.clone(),
            adapter.clone(),
        );
        let signer = "a".repeat(64);
        let invoke = |request| {
            broker.clone().invoke(
                "org.example.sync",
                &signer,
                request,
                &[],
                SshSyncActionRevision {
                    authorization: WireSequence::new(1),
                    configuration: WireSequence::new(1),
                },
                SshSyncInvocationBinding {
                    package_sha256: "b".repeat(64),
                    instance_generation: WireSequence::new(1),
                    authorization_epoch: WireSequence::new(1),
                },
                true,
                Arc::new(|| true),
            )
        };
        let status_request = || PluginSshSyncRequest::Status {
            profile_id: "primary".into(),
            auth: None,
        };
        assert_eq!(
            invoke(status_request()).await.stable_error_code,
            Some(PluginSshSyncStableErrorCode::VaultMissing)
        );
        assert_eq!(
            invoke(PluginSshSyncRequest::ConfigureScope {
                profile_id: "primary".into()
            })
            .await
            .stable_error_code,
            Some(PluginSshSyncStableErrorCode::InteractionRequired)
        );
        vault
            .create_for_protected_operation(b"shared-vault-password", b"shared-vault-password")
            .unwrap();
        assert_eq!(invoke(status_request()).await.stable_error_code, None);
        drop(vault);
        let locked = VaultService::start(directory.path());
        let locked_hosts = HostService::start(directory.path()).expect("hosts");
        let locked_adapter = Arc::new(NoriShellSshSyncLocalAdapter::new_for_store_test(
            locked_hosts,
            locked.clone(),
        ));
        let locked_broker = SshSyncExchangeBroker::new(
            directory.path(),
            locked,
            locked_adapter.clone(),
            locked_adapter.clone(),
        );
        let status = locked_broker
            .invoke(
                "org.example.sync",
                &"a".repeat(64),
                status_request(),
                &[],
                SshSyncActionRevision {
                    authorization: WireSequence::new(1),
                    configuration: WireSequence::new(1),
                },
                SshSyncInvocationBinding {
                    package_sha256: "b".repeat(64),
                    instance_generation: WireSequence::new(1),
                    authorization_epoch: WireSequence::new(1),
                },
                true,
                Arc::new(|| true),
            )
            .await;
        assert_eq!(
            status.stable_error_code,
            Some(PluginSshSyncStableErrorCode::VaultLocked)
        );
        assert!(adapter.state.lock().unwrap().pending_prompts.is_empty());
        assert!(
            locked_adapter
                .state
                .lock()
                .unwrap()
                .pending_prompts
                .is_empty()
        );
    }

    #[tokio::test]
    async fn background_actions_never_open_any_secure_prompt() {
        use crate::ssh_sync_exchange::{SecureActionContext, SecureSelectionError};
        let directory = tempfile::tempdir().expect("tempdir");
        let vault = VaultService::start(directory.path());
        let hosts = HostService::start(directory.path()).expect("hosts");
        let adapter = NoriShellSshSyncLocalAdapter::new_for_store_test(hosts, vault.clone());
        for kind in [
            SshSyncSecurePromptKind::CreateLocalVault,
            SshSyncSecurePromptKind::UnlockSynchronizedVault,
            SshSyncSecurePromptKind::RecoverSynchronizedKey,
            SshSyncSecurePromptKind::ChooseSyncDirection,
        ] {
            let context = SecureActionContext {
                background: true,
                plugin_id: "org.example.sync".into(),
                signer_fingerprint_sha256: "a".repeat(64),
                profile_id: "primary".into(),
                remote_origin: None,
                fence: Arc::new(|| true),
            };
            assert!(matches!(
                adapter
                    .prompt(context, kind, super::SecurePromptContent::default())
                    .await,
                Err(SecureSelectionError::InteractionRequired)
            ));
        }
        assert_eq!(
            vault.status().state,
            norishell_core_api::VaultState::Missing
        );
        assert!(adapter.state.lock().unwrap().pending_prompts.is_empty());
    }

    #[test]
    fn restore_operation_ids_are_stable_and_domain_separated_uuid_v7_values() {
        let portable = norishell_ssh_profile_sync::PortableObjectId::new();
        let credential = deterministic_operation_id("plugin\0primary", "credential", portable)
            .expect("credential");
        let replay =
            deterministic_operation_id("plugin\0primary", "credential", portable).expect("replay");
        let host = deterministic_operation_id("plugin\0primary", "host", portable).expect("host");
        let other_provider = deterministic_operation_id("other\0primary", "credential", portable)
            .expect("other provider");
        assert_eq!(credential, replay);
        assert_ne!(credential, host);
        assert_ne!(credential, other_provider);
    }

    #[test]
    fn proxy_route_restore_requires_and_preserves_its_password_credential() {
        let portable_credential_id = PortableObjectId::new();
        let credential_ref_id = CredentialRefId::new();
        let credentials = [(portable_credential_id, credential_ref_id.clone())]
            .into_iter()
            .collect();
        let restored = restore_route_ingress(
            &PortableIngress::HttpConnect {
                host: "proxy.example".to_owned(),
                port: 8443,
                username: Some("alice".to_owned()),
                credential_id: Some(portable_credential_id),
            },
            &credentials,
        )
        .expect("portable proxy credential must map back to its local credential");
        assert!(matches!(
            restored,
            RouteIngress::HttpConnectProxy {
                proxy_auth_credential_ref_id: Some(value),
                ..
            } if value == credential_ref_id
        ));

        assert!(
            restore_route_ingress(
                &PortableIngress::HttpConnect {
                    host: "proxy.example".to_owned(),
                    port: 8443,
                    username: Some("alice".to_owned()),
                    credential_id: Some(PortableObjectId::new()),
                },
                &credentials,
            )
            .is_err()
        );
    }

    #[test]
    fn proxy_credential_facts_remain_live_for_follow_up_merge_detection() {
        let identity_id = IdentityId::new();
        let credential_ref_id = CredentialRefId::new();
        let secret_ref_id = SecretRefId::new();
        let credential = CredentialRecord {
            credential_ref_id: credential_ref_id.clone(),
            identity_id: identity_id.clone(),
            method: AuthenticationMethodKind::Password,
            details: CredentialRecordDetails::Password {
                secret_ref_id: secret_ref_id.clone(),
            },
            priority: 0,
            label: "proxy".to_owned(),
            state_version: WireSequence::new(1),
            import_operation_id: None,
            import_idempotency_key: None,
            import_state: CredentialImportState::Ready,
        };
        let mut facts = LocalSyncFacts::default();

        add_credential_to_local_facts(&mut facts, &credential);

        assert!(facts.identity_ids.contains(identity_id.as_str()));
        assert!(facts.credential_ids.contains(credential_ref_id.as_str()));
        assert!(facts.secret_ids.contains(secret_ref_id.as_str()));
    }

    #[test]
    fn two_independent_stores_restore_password_metadata_with_the_same_vault_password() {
        const VAULT_PASSWORD: &[u8] = b"two-independent-vault-password";
        const WRONG_PASSWORD: &[u8] = b"different-vault-password";
        const SSH_PASSWORD: &[u8] = b"restored-ssh-password";
        const PLUGIN_ID: &str = "org.norixor.store-recovery-tests";
        const SIGNER: &str = "a3b1c5d7e9f1023456789abcdef0123456789abcdef0123456789abcdef01234";
        const PROFILE_ID: &str = "primary";

        for linked in [false, true] {
            let source_directory = tempfile::tempdir().expect("source directory");
            let source_hosts = HostService::start(source_directory.path()).expect("source hosts");
            let source_vault = VaultService::start(source_directory.path());
            source_vault
                .create(VAULT_PASSWORD)
                .expect("create source vault");
            let source_identity = source_hosts
                .with_ssh_sync_repository(|repository| {
                    repository.create_identity("Source deploy", Some("deploy"))
                })
                .expect("create source identity");
            let credential_operation = OperationId::new();
            let source_credential = source_hosts
                .with_ssh_sync_repository(|repository| {
                    repository.begin_credential_import(
                        &credential_operation,
                        "source-password-import",
                        &source_identity.identity_id,
                        CredentialKind::Password,
                        0,
                        "Source password",
                        false,
                    )
                })
                .expect("begin source password import");
            let source_secret_ref = match &source_credential.details {
                CredentialRecordDetails::Password { secret_ref_id } => secret_ref_id.clone(),
                other => panic!("expected password credential, got {other:?}"),
            };
            source_vault
                .insert_secrets(&[VaultSecretInsert {
                    secret_ref_id: source_secret_ref.clone(),
                    kind: SecretKind::Password,
                    value: Zeroizing::new(SSH_PASSWORD.to_vec()),
                }])
                .expect("write source password to vault");
            source_hosts
                .with_ssh_sync_repository(|repository| {
                    repository.mark_credential_import_ready(
                        &source_credential.credential_ref_id,
                        &credential_operation,
                        source_credential.state_version,
                        None,
                        None,
                    )
                })
                .expect("publish source password metadata");
            let source_host = source_hosts
                .with_ssh_sync_repository(|repository| {
                    repository.create_host(
                        "Source edge",
                        "source.example",
                        2222,
                        Some("deploy"),
                        Some(&source_identity.identity_id),
                        true,
                    )
                })
                .expect("create source host");
            let source_desktop = DesktopProfile {
                id: Uuid::new_v4().to_string(),
                label: "Desktop fixture".into(),
                protocol: DesktopProtocol::Rdp,
                address: "desktop.example".into(),
                port: 3389,
                username: "desktop-user".into(),
                domain: "TEST".into(),
                host_id: linked.then(|| source_host.host_id.clone()),
                gateway_host_id: linked.then(|| source_host.host_id.clone()),
                credential_ref_id: Some(source_credential.credential_ref_id.clone()),
                width: 1280,
                height: 720,
                clipboard_enabled: false,
                audio_playback_enabled: false,
                revision: WireSequence::new(0),
            };
            source_hosts
                .with_ssh_sync_repository(|r| r.save_desktop_profile(&source_desktop))
                .expect("desktop save");
            let source_adapter = NoriShellSshSyncLocalAdapter::new_for_store_test(
                source_hosts.clone(),
                source_vault.clone(),
            );
            let snapshot = source_adapter
                .build_snapshot_with_ids(
                    BackupSelection {
                        desktop_profile_ids: BTreeSet::from([source_desktop.id.clone()]),
                        host_ids: if linked {
                            BTreeSet::from([source_host.host_id.as_str().to_owned()])
                        } else {
                            BTreeSet::new()
                        },
                        credential_ids: BTreeSet::from([source_credential
                            .credential_ref_id
                            .as_str()
                            .to_owned()]),
                    },
                    1,
                    None,
                )
                .expect("serialize source portable bundle");
            assert_eq!(snapshot.host_count, u32::from(linked));
            assert_eq!(snapshot.desktop_profile_count, 1);
            assert_eq!(snapshot.credential_count, 1);
            assert_eq!(snapshot.bundle.secrets.len(), 1);

            let scoped = source_adapter
                .selection_for_scope(&norishell_app_persistence::SshSyncProfileScope {
                    mode: norishell_app_persistence::SshSyncProfileScopeMode::Custom,
                    custom_host_ids: Vec::new(),
                    custom_desktop_profile_ids: vec![source_desktop.id.clone()],
                    custom_credential_ref_ids: Vec::new(),
                })
                .expect("expand desktop gateway dependencies without selecting passwords");
            assert_eq!(
                scoped.host_ids.contains(source_host.host_id.as_str()),
                linked
            );
            assert_eq!(scoped.desktop_profile_ids.len(), 1);
            assert!(scoped.credential_ids.is_empty());

            let metadata_only = source_adapter
                .build_snapshot_with_ids(
                    BackupSelection {
                        desktop_profile_ids: BTreeSet::from([source_desktop.id.clone()]),
                        host_ids: if linked {
                            BTreeSet::from([source_host.host_id.as_str().to_owned()])
                        } else {
                            BTreeSet::new()
                        },
                        credential_ids: BTreeSet::new(),
                    },
                    1,
                    None,
                )
                .expect("desktop metadata without password");
            assert!(
                metadata_only.bundle.objects.desktop_profiles[0]
                    .credential_id
                    .is_none()
            );
            assert!(metadata_only.bundle.objects.credentials.is_empty());
            assert!(metadata_only.bundle.secrets.is_empty());

            let source_key_material = source_vault
                .export_sync_key_material()
                .expect("export password-wrapped source sync key");
            let binding = PluginExchangeBinding {
                plugin_id: PLUGIN_ID.to_owned(),
                signer_fingerprint_sha256: SIGNER.to_owned(),
                profile_id: PROFILE_ID.to_owned(),
                revision: 1,
                base_revision: None,
                base_etag: None,
            };
            let exchange = create_plugin_exchange_with_key(
                &snapshot.bundle,
                &SyncKey::from_bytes(*source_key_material.key_bytes()),
                source_key_material.envelope(),
                &binding,
            )
            .expect("encrypt source bundle");

            let wrong_directory = tempfile::tempdir().expect("wrong-password target directory");
            let wrong_hosts =
                HostService::start(wrong_directory.path()).expect("wrong-password hosts");
            let wrong_vault = VaultService::start(wrong_directory.path());
            wrong_vault
                .create(WRONG_PASSWORD)
                .expect("create wrong-password target vault");
            assert!(
                wrong_vault
                    .open_synchronized_key_material(source_key_material.envelope(), WRONG_PASSWORD)
                    .is_err()
            );
            assert!(
                wrong_hosts
                    .list_ssh_sync_snapshots()
                    .expect("read untouched wrong-password target metadata")
                    .is_empty()
            );
            assert_eq!(wrong_vault.status().entry_count, Some(0));

            let target_directory = tempfile::tempdir().expect("target directory");
            let target_hosts = HostService::start(target_directory.path()).expect("target hosts");
            let target_vault = VaultService::start(target_directory.path());
            target_vault
                .create(VAULT_PASSWORD)
                .expect("create independent target vault");
            let target_key_material = target_vault
                .open_synchronized_key_material(source_key_material.envelope(), VAULT_PASSWORD)
                .expect("same vault password recovers source sync key");
            let target_sync_key = SyncKey::from_bytes(*target_key_material.key_bytes());
            let restored_bundle =
                open_plugin_exchange_with_key(&exchange, &target_sync_key, &binding)
                    .expect("decrypt source bundle on target");
            let target_adapter = NoriShellSshSyncLocalAdapter::new_for_store_test(
                target_hosts.clone(),
                target_vault.clone(),
            );
            let owner = SyncOwner {
                plugin_id: PLUGIN_ID.to_owned(),
                signer_fingerprint_sha256: SIGNER.to_owned(),
                profile_id: PROFILE_ID.to_owned(),
            };
            let target_before_restore =
                tauri::async_runtime::block_on(target_adapter.snapshot_current(
                    PLUGIN_ID.to_owned(),
                    SIGNER.to_owned(),
                    PROFILE_ID.to_owned(),
                    1,
                ))
                .expect("create the current protocol-7 target profile state");
            assert_eq!(target_before_restore.host_count, 0);
            assert_eq!(target_before_restore.credential_count, 0);
            let staged = tauri::async_runtime::block_on(target_adapter.stage_restore(
                PLUGIN_ID.to_owned(),
                SIGNER.to_owned(),
                PROFILE_ID.to_owned(),
                restored_bundle,
                target_sync_key,
            ))
            .expect("stage target restore through the current sync path");
            let approval = target_adapter
                .issue_apply_approval(owner, Arc::new(|| true), Some(staged.handle.clone()))
                .expect("issue explicit target restore approval");
            let applied = tauri::async_runtime::block_on(target_adapter.apply_staged_restore(
                PLUGIN_ID.to_owned(),
                SIGNER.to_owned(),
                PROFILE_ID.to_owned(),
                staged.handle,
                Some(approval),
            ))
            .expect("apply staged target restore with its approval fence");
            assert_eq!(applied.host_count, u32::from(linked));
            assert_eq!(applied.desktop_profile_count, 1);
            assert_eq!(applied.credential_count, 1);

            let target_snapshots = target_hosts
                .list_ssh_sync_snapshots()
                .expect("read restored target metadata");
            assert_eq!(target_snapshots.len(), usize::from(linked));
            let target_desktops = target_adapter.desktop_profiles().expect("restored desktop");
            assert_eq!(target_desktops.len(), 1);
            let desktop = &target_desktops[0];
            assert_ne!(desktop.id, source_desktop.id);
            assert_eq!(
                (
                    &desktop.address,
                    desktop.port,
                    &desktop.username,
                    &desktop.domain
                ),
                (
                    &source_desktop.address,
                    3389,
                    &source_desktop.username,
                    &source_desktop.domain
                )
            );
            if linked {
                assert_eq!(
                    desktop.host_id.as_ref(),
                    Some(&target_snapshots[0].host.host_id)
                );
                assert_eq!(desktop.gateway_host_id, desktop.host_id);
                assert_eq!(
                    desktop.credential_ref_id.as_ref(),
                    Some(&target_snapshots[0].credentials[0].credential_ref_id)
                );
            }
            let target_credential = target_adapter
                .desktop_password(desktop.credential_ref_id.as_ref().unwrap())
                .expect("desktop password restored");
            let target_secret_ref = match &target_credential.details {
                CredentialRecordDetails::Password { secret_ref_id } => secret_ref_id.clone(),
                other => panic!("expected restored password credential, got {other:?}"),
            };
            assert_ne!(target_secret_ref, source_secret_ref);
            assert_eq!(
                target_vault
                    .read_secret(&target_secret_ref, SecretKind::Password)
                    .expect("read restored target secret")
                    .expose(),
                SSH_PASSWORD,
            );
        }
    }

    #[test]
    fn plugin_clear_and_expiry_release_pending_restore_plaintext_budget() {
        fn bundle(revision: u64) -> PortableBundleV1 {
            PortableBundleV1 {
                schema: BundleSchema::V1,
                revision,
                objects: PortableObjects::default(),
                secrets: Vec::new(),
                skipped_machine_bound: Vec::new(),
                tombstones: Vec::new(),
            }
        }

        let mut state = LocalState::default();
        for (token, plugin_id, bytes, expires_at) in [
            (
                "restore-a",
                "org.example.a",
                50,
                Instant::now() + SELECTION_LEASE_TIMEOUT,
            ),
            (
                "restore-b",
                "org.example.b",
                70,
                Instant::now() + SELECTION_LEASE_TIMEOUT,
            ),
        ] {
            state.pending_restores.insert(
                token.to_owned(),
                PendingRestore {
                    handle: PendingRestoreHandle::new(token),
                    bundle: bundle(bytes as u64),
                    owner: SyncOwner {
                        plugin_id: plugin_id.to_owned(),
                        signer_fingerprint_sha256: "a".repeat(64),
                        profile_id: "primary".to_owned(),
                    },
                    bundle_sha256: format!("{bytes:064x}"),
                    plaintext_bytes: bytes,
                    owned_delta: None,
                    vault_inserts: Vec::new(),
                    scope_memberships: None,
                    reconcile_noop: false,
                    change_fence: None,
                    expires_at,
                },
            );
            state.pending_restore_plaintext_bytes += bytes;
        }

        assert!(state.clear_plugin("org.example.a").is_empty());
        assert_eq!(state.pending_restore_plaintext_bytes, 70);
        assert!(!state.pending_restores.contains_key("restore-a"));
        state
            .pending_restores
            .get_mut("restore-b")
            .expect("remaining restore")
            .expires_at = Instant::now() - Duration::from_secs(1);
        state.prune_expired();
        assert_eq!(state.pending_restore_plaintext_bytes, 0);
        assert!(state.pending_restores.is_empty());
    }

    #[test]
    fn restore_vault_batch_limits_match_the_vault_contract_exactly() {
        assert!(
            validate_restore_vault_batch_limits(
                MAX_SECRET_BATCH_ENTRIES,
                MAX_SECRET_BATCH_VALUE_BYTES,
            )
            .is_ok()
        );
        assert!(
            validate_restore_vault_batch_limits(
                MAX_SECRET_BATCH_ENTRIES + 1,
                MAX_SECRET_BATCH_VALUE_BYTES,
            )
            .is_err()
        );
        assert!(
            validate_restore_vault_batch_limits(
                MAX_SECRET_BATCH_ENTRIES,
                MAX_SECRET_BATCH_VALUE_BYTES + 1,
            )
            .is_err()
        );
    }

    #[test]
    fn backup_selection_tokens_do_not_overwrite_other_plugin_profiles() {
        let fence: crate::ssh_sync_exchange::ActionFence = Arc::new(|| true);
        let mut state = LocalState::default();
        for (token, plugin_id, host_id) in [
            ("token-a", "org.example.a", "host-a"),
            ("token-b", "org.example.b", "host-b"),
        ] {
            state.backup_selections.insert(
                token.to_owned(),
                BackupSelectionLease {
                    owner: SyncOwner {
                        plugin_id: plugin_id.to_owned(),
                        signer_fingerprint_sha256: "a".repeat(64),
                        profile_id: "primary".to_owned(),
                    },
                    selection: BackupSelection {
                        desktop_profile_ids: BTreeSet::new(),
                        host_ids: BTreeSet::from([host_id.to_owned()]),
                        credential_ids: BTreeSet::new(),
                    },
                    fence: fence.clone(),
                    expires_at: Instant::now() + SELECTION_LEASE_TIMEOUT,
                },
            );
        }
        let selected = state
            .backup_selections
            .remove("token-a")
            .expect("first plugin selection");
        assert_eq!(selected.owner.plugin_id, "org.example.a");
        assert!(selected.selection.host_ids.contains("host-a"));
        assert!(state.backup_selections.contains_key("token-b"));
    }

    #[test]
    fn secure_decision_rejects_machine_bound_credentials_and_missing_recovery_password() {
        let host_id = HostId::new();
        let portable_credential = CredentialRefId::new();
        let machine_bound = CredentialRefId::new();
        let prompt = SshSyncSecurePrompt {
            desktop_profiles: Vec::new(),
            desktop_profile_count: 0,
            remote_desktop_profile_count: 0,
            prompt_id: uuid::Uuid::new_v4().to_string(),
            plugin_id: norishell_core_api::PluginId::parse("org.example.sync").expect("plugin"),
            profile_id: "primary".to_owned(),
            remote_origin: Some("https://sync.example.test".to_owned()),
            kind: SshSyncSecurePromptKind::SelectBackup,
            oauth: None,
            hosts: vec![SshSyncSecureHost {
                host_id: host_id.clone(),
                label: "Production".into(),
                endpoint: "example.test:22".into(),
                credentials: vec![
                    SshSyncSecureCredential {
                        credential_ref_id: portable_credential.clone(),
                        label: "Password".into(),
                        method_label: "Password".into(),
                        machine_bound: false,
                    },
                    SshSyncSecureCredential {
                        credential_ref_id: machine_bound.clone(),
                        label: "Agent".into(),
                        method_label: "SSH Agent".into(),
                        machine_bound: true,
                    },
                ],
            }],
            host_count: 1,
            credential_count: 2,
            conflict_count: 0,
            update_count: 0,
            delete_count: 0,
            remote_host_count: 0,
            remote_credential_count: 0,
            local_compared_at_unix_ms: None,
            remote_updated_at_unix_ms: None,
            differences: Vec::new(),
            difference_total_count: 0,
            difference_omitted_count: 0,
        };
        let valid = SshSyncSecureDecisionRequest {
            selected_desktop_profile_ids: Vec::new(),
            meta: request_meta(),
            prompt_id: prompt.prompt_id.clone(),
            decision: SshSyncSecureDecision::Approve,
            selected_host_ids: vec![host_id],
            selected_credential_ref_ids: vec![portable_credential],
            vault_password: None,
            vault_password_confirmation: None,
        };
        assert!(validate_decision(&prompt, &valid).is_ok());
        assert!(
            validate_decision(
                &prompt,
                &SshSyncSecureDecisionRequest {
                    selected_desktop_profile_ids: Vec::new(),
                    selected_credential_ref_ids: vec![machine_bound],
                    ..valid.clone()
                }
            )
            .is_err()
        );

        let recovery = SshSyncSecurePrompt {
            desktop_profiles: Vec::new(),
            desktop_profile_count: 0,
            remote_desktop_profile_count: 0,
            kind: SshSyncSecurePromptKind::CreateRecoveryPassword,
            oauth: None,
            hosts: Vec::new(),
            ..prompt
        };
        assert!(
            validate_decision(
                &recovery,
                &SshSyncSecureDecisionRequest {
                    selected_desktop_profile_ids: Vec::new(),
                    prompt_id: recovery.prompt_id.clone(),
                    selected_host_ids: Vec::new(),
                    selected_credential_ref_ids: Vec::new(),
                    ..valid
                }
            )
            .is_err()
        );

        let unlock = SshSyncSecurePrompt {
            desktop_profiles: Vec::new(),
            desktop_profile_count: 0,
            remote_desktop_profile_count: 0,
            kind: SshSyncSecurePromptKind::UnlockSynchronizedVault,
            ..recovery.clone()
        };
        assert!(
            validate_decision(
                &unlock,
                &SshSyncSecureDecisionRequest {
                    selected_desktop_profile_ids: Vec::new(),
                    prompt_id: unlock.prompt_id.clone(),
                    decision: SshSyncSecureDecision::Approve,
                    selected_host_ids: Vec::new(),
                    selected_credential_ref_ids: Vec::new(),
                    vault_password: Some("one-shared-vault-password".to_owned()),
                    vault_password_confirmation: None,
                    meta: request_meta(),
                }
            )
            .is_ok()
        );

        let create = SshSyncSecurePrompt {
            desktop_profiles: Vec::new(),
            desktop_profile_count: 0,
            remote_desktop_profile_count: 0,
            kind: SshSyncSecurePromptKind::CreateLocalVault,
            ..recovery.clone()
        };
        let mut create_request = SshSyncSecureDecisionRequest {
            selected_desktop_profile_ids: Vec::new(),
            prompt_id: create.prompt_id.clone(),
            decision: SshSyncSecureDecision::Approve,
            selected_host_ids: Vec::new(),
            selected_credential_ref_ids: Vec::new(),
            vault_password: Some("one-shared-vault-password".to_owned()),
            vault_password_confirmation: None,
            meta: request_meta(),
        };
        assert!(validate_decision(&create, &create_request).is_err());
        create_request.vault_password_confirmation = Some("another-vault-password".to_owned());
        assert!(validate_decision(&create, &create_request).is_err());
        create_request.vault_password_confirmation = create_request.vault_password.clone();
        assert!(validate_decision(&create, &create_request).is_ok());
        assert!(validate_decision(&unlock, &create_request).is_err());

        let conflict = SshSyncSecurePrompt {
            desktop_profiles: Vec::new(),
            desktop_profile_count: 0,
            remote_desktop_profile_count: 0,
            kind: SshSyncSecurePromptKind::ResolveConflicts,
            conflict_count: 2,
            ..recovery
        };
        let keep_local = SshSyncSecureDecisionRequest {
            selected_desktop_profile_ids: Vec::new(),
            meta: request_meta(),
            prompt_id: conflict.prompt_id.clone(),
            decision: SshSyncSecureDecision::KeepLocal,
            selected_host_ids: Vec::new(),
            selected_credential_ref_ids: Vec::new(),
            vault_password: None,
            vault_password_confirmation: None,
        };
        assert!(validate_decision(&conflict, &keep_local).is_ok());
        assert!(
            validate_decision(
                &conflict,
                &SshSyncSecureDecisionRequest {
                    selected_desktop_profile_ids: Vec::new(),
                    decision: SshSyncSecureDecision::Approve,
                    ..keep_local
                }
            )
            .is_err()
        );

        let reset = SshSyncSecurePrompt {
            desktop_profiles: Vec::new(),
            desktop_profile_count: 0,
            remote_desktop_profile_count: 0,
            kind: SshSyncSecurePromptKind::ResetRemote,
            conflict_count: 0,
            ..conflict
        };
        assert!(
            validate_decision(
                &reset,
                &SshSyncSecureDecisionRequest {
                    selected_desktop_profile_ids: Vec::new(),
                    meta: request_meta(),
                    prompt_id: reset.prompt_id.clone(),
                    decision: SshSyncSecureDecision::Approve,
                    selected_host_ids: Vec::new(),
                    selected_credential_ref_ids: Vec::new(),
                    vault_password: None,
                    vault_password_confirmation: None,
                }
            )
            .is_ok()
        );
    }
}
