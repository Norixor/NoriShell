//! Owner-scoped plugin access to existing SFTP and Forward Core resources.
//!
//! This module is not a Tauri command surface. The caller must resolve the plugin's opaque Host
//! handle and construct the complete owner/action fence before invoking it.

#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use norishell_core_api::{
    ForwardSessionId, OperationId, PluginActualForwardBind, PluginForwardRule,
    PluginHostNavigationRequest, PluginId, PluginLogReadRequest, PluginLogReadResult,
    PluginOwnedForwardState, PluginOwnedForwardSummary, PluginResourceOperation,
    PluginResourceOperationResult, RequestId, RequestMeta, SftpDirectoryListRequest,
    SftpFileTailRequest, SftpRemoteEntryKind, SftpRemotePath, SftpSessionDisconnectRequest,
    SftpSessionId, SftpSessionState, WireSequence,
};
use uuid::Uuid;

use crate::{
    forward_session_service::{
        ActualForwardBind, ForwardFailureCode, ForwardSessionService, ForwardSessionState,
        PortForwardRule,
    },
    lifecycle::LifecycleState,
    sftp_session_service::SftpSessionService,
    ssh_session_service::SessionChannelLease,
};

const MAX_LOG_FILES: usize = 8;
const MAX_LOG_FILE_BYTES: usize = 16 * 1024;
const MAX_LOG_TOTAL_BYTES: usize = 64 * 1024;
const MAX_DIRECTORY_PAGES: usize = 4;
const DIRECTORY_PAGE_SIZE: u16 = 512;
const MAX_FORWARDS_PER_OWNER_HOST: usize = 8;
const MAX_PLUGIN_FORWARDS: usize = 32;
const MAX_REASON_BYTES: usize = 2048;

pub(crate) type PluginResourceFence = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PluginResourceOwner {
    pub(crate) plugin_id: PluginId,
    pub(crate) signer_fingerprint_sha256: String,
    pub(crate) package_sha256: String,
    pub(crate) instance_generation: WireSequence,
    pub(crate) capability_grant_epoch: WireSequence,
}

#[derive(Clone)]
pub(crate) struct PluginResourceInvocation {
    pub(crate) owner: PluginResourceOwner,
    pub(crate) lease: SessionChannelLease,
    pub(crate) reason: String,
    pub(crate) action_fence: PluginResourceFence,
    pub(crate) lifetime_fence: PluginResourceFence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PluginResourceError {
    pub(crate) stable_code: &'static str,
}

impl PluginResourceError {
    const fn new(stable_code: &'static str) -> Self {
        Self { stable_code }
    }
}

impl std::fmt::Display for PluginResourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.stable_code)
    }
}

impl std::error::Error for PluginResourceError {}

type ResourceResult<T> = Result<T, PluginResourceError>;

#[derive(Clone)]
pub(crate) struct PluginResourceService {
    sftp: SftpSessionService,
    forwards: ForwardSessionService,
    lifecycle: LifecycleState,
    gate: Arc<tokio::sync::Mutex<()>>,
    state: Arc<Mutex<ResourceState>>,
}

#[derive(Default)]
struct ResourceState {
    sftp: BTreeMap<ResourceScopeKey, OwnedSftpSession>,
    forwards: BTreeMap<String, OwnedForwardSession>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ResourceScopeKey {
    plugin_id: String,
    signer_fingerprint_sha256: String,
    package_sha256: String,
    instance_generation: u64,
    capability_grant_epoch: u64,
    ssh_session_id: String,
    ssh_generation: u64,
    parent_channel_id: String,
}

#[derive(Clone)]
struct OwnedSftpSession {
    session_id: SftpSessionId,
    generation: WireSequence,
}

#[derive(Clone)]
struct OwnedForwardSession {
    scope: ResourceScopeKey,
    session_id: ForwardSessionId,
    generation: WireSequence,
    rule: PluginForwardRule,
}

impl PluginResourceService {
    pub(crate) fn new(
        sftp: SftpSessionService,
        forwards: ForwardSessionService,
        lifecycle: LifecycleState,
    ) -> Self {
        Self {
            sftp,
            forwards,
            lifecycle,
            gate: Arc::new(tokio::sync::Mutex::new(())),
            state: Arc::new(Mutex::new(ResourceState::default())),
        }
    }

    pub(crate) async fn invoke(
        &self,
        invocation: PluginResourceInvocation,
        operation: PluginResourceOperation,
    ) -> ResourceResult<PluginResourceOperationResult> {
        validate_invocation(&invocation)?;
        let _gate = self.gate.lock().await;
        require_action_current(&invocation)?;
        match operation {
            PluginResourceOperation::DockerTerminalOpen { .. } => {
                Err(PluginResourceError::new("unsupportedOperation"))
            }
            PluginResourceOperation::SftpOpen { path, edit } => {
                validate_remote_path(&path, false)?;
                self.ensure_sftp(&invocation).await?;
                Ok(PluginResourceOperationResult::NavigationRequested {
                    request: PluginHostNavigationRequest::Sftp { path, edit },
                })
            }
            PluginResourceOperation::LogsRead { files } => self.read_logs(&invocation, files).await,
            PluginResourceOperation::ForwardStart { rule } => {
                self.start_forward(&invocation, rule).await
            }
            PluginResourceOperation::ForwardList => self.list_forwards(&invocation),
            PluginResourceOperation::ForwardStop { forward_handle } => {
                self.stop_forward(&invocation, &forward_handle).await
            }
        }
    }

    /// Stops resources created by every generation/package of one plugin. User-created SFTP and
    /// Forward sessions are absent from this owner ledger and are never touched.
    pub(crate) async fn stop_plugin(&self, plugin_id: &PluginId) -> ResourceResult<()> {
        self.stop_plugin_matching(plugin_id, None).await
    }

    /// Stops resources owned by one exact plugin process generation. A delayed crash callback can
    /// therefore never tear down resources created after that plugin has restarted.
    pub(crate) async fn stop_plugin_generation(
        &self,
        plugin_id: &PluginId,
        instance_generation: WireSequence,
    ) -> ResourceResult<()> {
        if instance_generation.get() == 0 {
            return Err(PluginResourceError::new("invalidRequest"));
        }
        self.stop_plugin_matching(plugin_id, Some(instance_generation))
            .await
    }

    async fn stop_plugin_matching(
        &self,
        plugin_id: &PluginId,
        instance_generation: Option<WireSequence>,
    ) -> ResourceResult<()> {
        let _gate = self.gate.lock().await;
        let plugin_id = plugin_id.as_str();
        let (sftp, forwards) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let sftp = state
                .sftp
                .iter()
                .filter(|(scope, _)| resource_scope_matches(scope, plugin_id, instance_generation))
                .map(|(scope, session)| (scope.clone(), session.clone()))
                .collect::<Vec<_>>();
            let forwards = state
                .forwards
                .iter()
                .filter(|(_, session)| {
                    resource_scope_matches(&session.scope, plugin_id, instance_generation)
                })
                .map(|(handle, session)| (handle.clone(), session.clone()))
                .collect::<Vec<_>>();
            (sftp, forwards)
        };
        let mut first_error = None;
        for (scope, session) in sftp {
            match self.disconnect_sftp(&session).await {
                Ok(()) => {
                    self.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .sftp
                        .remove(&scope);
                }
                Err(_) if first_error.is_none() => {
                    first_error = Some(PluginResourceError::new("cleanupIncomplete"));
                }
                Err(_) => {}
            }
        }
        for (handle, session) in forwards {
            match self
                .forwards
                .stop_plugin_session(&session.session_id, session.generation)
                .await
            {
                Ok(summary) if !summary.cleanup.uncertain => {
                    self.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .forwards
                        .remove(&handle);
                }
                Ok(_) | Err(_) if first_error.is_none() => {
                    first_error = Some(PluginResourceError::new("cleanupIncomplete"));
                }
                Ok(_) | Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(crate) fn active_plugin_ids(&self) -> Vec<PluginId> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut values = state
            .sftp
            .keys()
            .map(|scope| scope.plugin_id.as_str())
            .chain(
                state
                    .forwards
                    .values()
                    .map(|session| session.scope.plugin_id.as_str()),
            )
            .collect::<Vec<_>>();
        values.sort_unstable();
        values.dedup();
        values
            .into_iter()
            .filter_map(|value| PluginId::parse(value.to_owned()).ok())
            .collect()
    }

    /// Returns the exact shared SFTP child that a committed navigation event must attach. The
    /// caller must publish this trusted Core fact separately from the plugin guest result.
    pub(crate) async fn sftp_navigation_summary(
        &self,
        invocation: &PluginResourceInvocation,
    ) -> ResourceResult<norishell_core_api::SftpSessionSummary> {
        validate_invocation(invocation)?;
        require_action_current(invocation)?;
        let owned = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sftp
            .get(&scope_key(invocation))
            .cloned()
            .ok_or_else(|| PluginResourceError::new("sftpSessionNotFound"))?;
        self.sftp
            .wire_summaries()
            .await
            .map_err(|_| PluginResourceError::new("sftpSnapshotFailed"))?
            .into_iter()
            .find(|summary| {
                summary.session_id == owned.session_id && summary.generation == owned.generation
            })
            .ok_or_else(|| PluginResourceError::new("sftpSessionNotFound"))
    }

    async fn read_logs(
        &self,
        invocation: &PluginResourceInvocation,
        files: Vec<PluginLogReadRequest>,
    ) -> ResourceResult<PluginResourceOperationResult> {
        validate_log_requests(&files)?;
        let session = self.ensure_sftp(invocation).await?;
        let mut results = Vec::with_capacity(files.len());
        let mut total_bytes = 0usize;
        for file in files {
            require_action_current(invocation)?;
            let (directory, _) = split_remote_file_path(&file.path)?;
            let (directory_ref, entry_ref, observed_size) = self
                .find_remote_file(&session, &directory, file.path.as_bytes())
                .await?;
            let offset = file.offset.unwrap_or_else(|| {
                WireSequence::new(observed_size.saturating_sub(u64::from(file.max_bytes)))
            });
            let tail = self
                .sftp
                .tail_file_limited(
                    SftpFileTailRequest {
                        meta: request_meta(),
                        session_id: session.session_id.clone(),
                        expected_generation: session.generation,
                        directory_ref,
                        entry_ref,
                        offset,
                    },
                    usize::from(file.max_bytes),
                )
                .await
                .map_err(|_| PluginResourceError::new("sftpReadFailed"))?;
            total_bytes = total_bytes.saturating_add(tail.bytes.len());
            if total_bytes > MAX_LOG_TOTAL_BYTES {
                return Err(PluginResourceError::new("outputLimit"));
            }
            let invalid_utf8_replaced = std::str::from_utf8(&tail.bytes).is_err();
            let text = safe_log_text(&tail.bytes);
            results.push(PluginLogReadResult {
                path: file.path,
                start_offset: tail.start_offset,
                next_offset: tail.next_offset,
                total_size: tail.total_size,
                reset: tail.reset,
                text,
                invalid_utf8_replaced,
            });
        }
        require_action_current(invocation)?;
        Ok(PluginResourceOperationResult::LogsRead {
            files: results,
            total_bytes: u32::try_from(total_bytes).unwrap_or(u32::MAX),
        })
    }

    async fn ensure_sftp(
        &self,
        invocation: &PluginResourceInvocation,
    ) -> ResourceResult<OwnedSftpSession> {
        let scope = scope_key(invocation);
        let existing = {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .sftp
                .get(&scope)
                .cloned()
        };
        if let Some(existing) = existing.clone()
            && self.sftp_session_ready(&existing).await
        {
            return Ok(existing);
        }
        if let Some(stale) = existing {
            self.disconnect_sftp(&stale).await?;
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .sftp
                .remove(&scope);
        }
        require_action_current(invocation)?;
        let _creation = self
            .lifecycle
            .acquire_resource_creation(RequestId::new())
            .map_err(|_| PluginResourceError::new("applicationExiting"))?;
        let lease = scoped_resource_lease(invocation);
        let summary = self
            .sftp
            .open_on_ssh_session(SftpSessionId::new(), lease)
            .await
            .map_err(|_| PluginResourceError::new("sftpOpenFailed"))?;
        if summary.state != SftpSessionState::Ready {
            use norishell_core_api::SftpFailureCode;
            let code = match summary.failure.as_ref().map(|failure| failure.code) {
                Some(
                    SftpFailureCode::HostKeyRejected
                    | SftpFailureCode::VaultLocked
                    | SftpFailureCode::CredentialUnavailable,
                ) => "interactionRequired",
                Some(SftpFailureCode::HostKeyMismatch) => "hostKeyMismatch",
                Some(SftpFailureCode::AuthenticationRejected) => "authenticationFailed",
                _ if matches!(
                    summary.state,
                    SftpSessionState::VerifyingHostKey
                        | SftpSessionState::Authenticating
                        | SftpSessionState::NeedsAuthentication
                ) =>
                {
                    "interactionRequired"
                }
                _ => "sftpOpenFailed",
            };
            // `open_on_ssh_session` retains failed records so their cleanup state remains
            // observable. Adopt the record before cleanup; if cleanup is uncertain the owner
            // ledger must retain it for the plugin/application shutdown path.
            let failed = OwnedSftpSession {
                session_id: summary.session_id,
                generation: summary.generation,
            };
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .sftp
                .insert(scope.clone(), failed.clone());
            if self.disconnect_sftp(&failed).await.is_err() {
                return Err(PluginResourceError::new("cleanupIncomplete"));
            }
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .sftp
                .remove(&scope);
            return Err(PluginResourceError::new(code));
        }
        let session = OwnedSftpSession {
            session_id: summary.session_id,
            generation: summary.generation,
        };
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sftp
            .insert(scope.clone(), session.clone());
        if require_action_current(invocation).is_err() {
            if self.disconnect_sftp(&session).await.is_ok() {
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .sftp
                    .remove(&scope);
                return Err(PluginResourceError::new("authorizationChanged"));
            }
            return Err(PluginResourceError::new("cleanupIncomplete"));
        }
        Ok(session)
    }

    async fn sftp_session_ready(&self, session: &OwnedSftpSession) -> bool {
        self.sftp.wire_summaries().await.is_ok_and(|summaries| {
            summaries.iter().any(|summary| {
                summary.session_id == session.session_id
                    && summary.generation == session.generation
                    && summary.state == SftpSessionState::Ready
            })
        })
    }

    async fn disconnect_sftp(&self, session: &OwnedSftpSession) -> ResourceResult<()> {
        match self
            .sftp
            .disconnect(SftpSessionDisconnectRequest {
                meta: request_meta(),
                operation_id: OperationId::new(),
                idempotency_key: Uuid::now_v7().to_string(),
                session_id: session.session_id.clone(),
                expected_generation: session.generation,
            })
            .await
        {
            Ok(summary) if summary.state == SftpSessionState::Closed => Ok(()),
            Ok(_) | Err(_) => Err(PluginResourceError::new("cleanupIncomplete")),
        }
    }

    async fn find_remote_file(
        &self,
        session: &OwnedSftpSession,
        directory: &str,
        exact_path: &[u8],
    ) -> ResourceResult<(String, String, u64)> {
        let mut cursor = None;
        for _ in 0..MAX_DIRECTORY_PAGES {
            let operation_id = OperationId::new();
            let listing = self
                .sftp
                .list_directory(SftpDirectoryListRequest {
                    meta: request_meta(),
                    operation_id: operation_id.clone(),
                    idempotency_key: operation_id.to_string(),
                    session_id: session.session_id.clone(),
                    expected_generation: session.generation,
                    path: SftpRemotePath {
                        bytes: directory.as_bytes().to_vec(),
                    },
                    cursor,
                    page_size: DIRECTORY_PAGE_SIZE,
                })
                .await
                .map_err(|_| PluginResourceError::new("sftpListFailed"))?;
            if let Some(entry) = listing.entries.into_iter().find(|entry| {
                entry.kind == SftpRemoteEntryKind::File && entry.path.bytes == exact_path
            }) {
                return Ok((
                    listing.directory_ref,
                    entry.entry_ref,
                    entry.size.unwrap_or(0),
                ));
            }
            let Some(next) = listing.next_cursor else {
                return Err(PluginResourceError::new("logFileNotFound"));
            };
            cursor = Some(next);
        }
        Err(PluginResourceError::new("directoryPageLimit"))
    }

    async fn start_forward(
        &self,
        invocation: &PluginResourceInvocation,
        rule: PluginForwardRule,
    ) -> ResourceResult<PluginResourceOperationResult> {
        let scope = scope_key(invocation);
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.forwards.len() >= MAX_PLUGIN_FORWARDS
                || state
                    .forwards
                    .values()
                    .filter(|session| session.scope == scope)
                    .count()
                    >= MAX_FORWARDS_PER_OWNER_HOST
            {
                return Err(PluginResourceError::new("resourceLimit"));
            }
        }
        require_action_current(invocation)?;
        let _creation = self
            .lifecycle
            .acquire_resource_creation(RequestId::new())
            .map_err(|_| PluginResourceError::new("applicationExiting"))?;
        let session_id = ForwardSessionId::new();
        let runtime_rule = forward_rule(invocation.lease.session_id.as_str(), &rule)?;
        let lease = scoped_resource_lease(invocation);
        let summary = self
            .forwards
            .start_plugin_session(
                session_id.clone(),
                runtime_rule,
                lease.channels,
                lease.cancelled,
            )
            .map_err(|_| PluginResourceError::new("invalidForwardRule"))?;
        let handle = Uuid::now_v7().to_string();
        let generation = WireSequence::new(summary.generation.get());
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forwards
            .insert(
                handle.clone(),
                OwnedForwardSession {
                    scope,
                    session_id: session_id.clone(),
                    generation,
                    rule,
                },
            );
        if require_action_current(invocation).is_err() {
            let stopped = self
                .forwards
                .stop_plugin_session(&session_id, generation)
                .await;
            if stopped
                .as_ref()
                .is_ok_and(|summary| !summary.cleanup.uncertain)
            {
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .forwards
                    .remove(&handle);
                return Err(PluginResourceError::new("authorizationChanged"));
            }
            return Err(PluginResourceError::new("cleanupIncomplete"));
        }
        Ok(PluginResourceOperationResult::ForwardStarted {
            forward: map_forward(&handle, summary),
        })
    }

    fn list_forwards(
        &self,
        invocation: &PluginResourceInvocation,
    ) -> ResourceResult<PluginResourceOperationResult> {
        let scope = scope_key(invocation);
        let owned = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forwards
            .iter()
            .filter(|(_, session)| session.scope == scope)
            .map(|(handle, session)| (handle.clone(), session.clone()))
            .collect::<BTreeMap<_, _>>();
        let forwards = owned
            .into_iter()
            .filter_map(|(handle, session)| {
                self.forwards
                    .plugin_session_snapshot(&session.session_id, session.generation)
                    .ok()
                    .map(|summary| map_forward(&handle, summary))
            })
            .collect();
        Ok(PluginResourceOperationResult::ForwardList { forwards })
    }

    async fn stop_forward(
        &self,
        invocation: &PluginResourceInvocation,
        handle: &str,
    ) -> ResourceResult<PluginResourceOperationResult> {
        validate_opaque_handle(handle)?;
        let owned = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forwards
            .get(handle)
            .cloned()
            .ok_or_else(|| PluginResourceError::new("forwardNotFound"))?;
        if owned.scope != scope_key(invocation) {
            return Err(PluginResourceError::new("forwardNotFound"));
        }
        require_action_current(invocation)?;
        let summary = self
            .forwards
            .stop_plugin_session(&owned.session_id, owned.generation)
            .await
            .map_err(|_| PluginResourceError::new("forwardStopFailed"))?;
        if !summary.cleanup.uncertain {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .forwards
                .remove(handle);
        }
        Ok(PluginResourceOperationResult::ForwardStopped {
            forward: map_forward(handle, summary),
        })
    }

    /// Returns the Core-retained rule and actual bind for the protected stop review. The plugin
    /// cannot substitute either value when asking to stop an opaque handle.
    pub(crate) fn forward_review(
        &self,
        invocation: &PluginResourceInvocation,
        handle: &str,
    ) -> ResourceResult<(PluginForwardRule, Option<PluginActualForwardBind>)> {
        validate_opaque_handle(handle)?;
        require_action_current(invocation)?;
        let owned = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forwards
            .get(handle)
            .cloned()
            .filter(|owned| owned.scope == scope_key(invocation))
            .ok_or_else(|| PluginResourceError::new("forwardNotFound"))?;
        let summary = self
            .forwards
            .plugin_session_snapshot(&owned.session_id, owned.generation)
            .map_err(|_| PluginResourceError::new("forwardSnapshotFailed"))?;
        Ok((owned.rule, map_actual_bind(summary.actual_bind)))
    }
}

fn validate_invocation(invocation: &PluginResourceInvocation) -> ResourceResult<()> {
    if invocation.reason.trim().is_empty()
        || invocation.reason.len() > MAX_REASON_BYTES
        || invocation.reason.bytes().any(|byte| byte == 0)
        || !valid_sha256(&invocation.owner.signer_fingerprint_sha256)
        || !valid_sha256(&invocation.owner.package_sha256)
        || invocation.owner.instance_generation.get() == 0
        || invocation.owner.capability_grant_epoch.get() == 0
    {
        return Err(PluginResourceError::new("invalidRequest"));
    }
    Ok(())
}

fn require_action_current(invocation: &PluginResourceInvocation) -> ResourceResult<()> {
    if invocation.lease.is_current() && (invocation.action_fence)() && (invocation.lifetime_fence)()
    {
        Ok(())
    } else {
        Err(PluginResourceError::new("authorizationChanged"))
    }
}

fn scoped_resource_lease(invocation: &PluginResourceInvocation) -> SessionChannelLease {
    let mut lease = invocation.lease.clone();
    let parent = invocation.lease.clone();
    let lifetime_fence = invocation.lifetime_fence.clone();
    let initially_cancelled = !parent.is_current() || !lifetime_fence();
    let (cancel, cancelled) = tokio::sync::watch::channel(initially_cancelled);
    if !initially_cancelled {
        tokio::spawn(async move {
            tokio::select! {
                () = parent.wait_cancelled() => {},
                () = async {
                    loop {
                        if !lifetime_fence() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                } => {},
            }
            cancel.send_replace(true);
        });
    }
    lease.cancelled = cancelled;
    lease
}

fn scope_key(invocation: &PluginResourceInvocation) -> ResourceScopeKey {
    scope_key_parts(
        &invocation.owner,
        invocation.lease.session_id.as_str(),
        invocation.lease.generation.get(),
        invocation.lease.parent_channel_id.as_str(),
    )
}

fn resource_scope_matches(
    scope: &ResourceScopeKey,
    plugin_id: &str,
    instance_generation: Option<WireSequence>,
) -> bool {
    scope.plugin_id == plugin_id
        && instance_generation
            .is_none_or(|generation| scope.instance_generation == generation.get())
}

fn scope_key_parts(
    owner: &PluginResourceOwner,
    ssh_session_id: &str,
    ssh_generation: u64,
    parent_channel_id: &str,
) -> ResourceScopeKey {
    ResourceScopeKey {
        plugin_id: owner.plugin_id.as_str().to_owned(),
        signer_fingerprint_sha256: owner.signer_fingerprint_sha256.clone(),
        package_sha256: owner.package_sha256.clone(),
        instance_generation: owner.instance_generation.get(),
        capability_grant_epoch: owner.capability_grant_epoch.get(),
        ssh_session_id: ssh_session_id.to_owned(),
        ssh_generation,
        parent_channel_id: parent_channel_id.to_owned(),
    }
}

fn validate_log_requests(files: &[PluginLogReadRequest]) -> ResourceResult<()> {
    if files.is_empty() || files.len() > MAX_LOG_FILES {
        return Err(PluginResourceError::new("invalidLogFiles"));
    }
    let mut total = 0usize;
    let mut paths = BTreeMap::new();
    for file in files {
        validate_remote_path(&file.path, true)?;
        if file.max_bytes == 0 || usize::from(file.max_bytes) > MAX_LOG_FILE_BYTES {
            return Err(PluginResourceError::new("invalidLogLimit"));
        }
        total = total.saturating_add(usize::from(file.max_bytes));
        if total > MAX_LOG_TOTAL_BYTES || paths.insert(file.path.as_str(), ()).is_some() {
            return Err(PluginResourceError::new("invalidLogFiles"));
        }
    }
    Ok(())
}

fn validate_remote_path(path: &str, require_file: bool) -> ResourceResult<()> {
    if !path.starts_with('/')
        || path.len() > 4096
        || path
            .bytes()
            .any(|byte| byte == 0 || byte < 0x20 || byte == 0x7f)
        || path
            .split('/')
            .skip(1)
            .any(|part| part == "." || part == "..")
        || path.contains("//")
        || (require_file && path.ends_with('/'))
    {
        return Err(PluginResourceError::new("invalidRemotePath"));
    }
    Ok(())
}

fn split_remote_file_path(path: &str) -> ResourceResult<(String, String)> {
    validate_remote_path(path, true)?;
    let split = path
        .rfind('/')
        .ok_or_else(|| PluginResourceError::new("invalidRemotePath"))?;
    let name = &path[split + 1..];
    if name.is_empty() {
        return Err(PluginResourceError::new("invalidRemotePath"));
    }
    let parent = if split == 0 { "/" } else { &path[..split] };
    Ok((parent.to_owned(), name.to_owned()))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_opaque_handle(value: &str) -> ResourceResult<()> {
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| PluginResourceError::new("forwardNotFound"))
}

fn request_meta() -> RequestMeta {
    RequestMeta {
        request_id: RequestId::new(),
    }
}

fn safe_log_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .collect()
}

fn forward_rule(host_ref: &str, rule: &PluginForwardRule) -> ResourceResult<PortForwardRule> {
    match rule {
        PluginForwardRule::Local {
            local_bind_address,
            local_listen_port,
            remote_target_host,
            remote_target_port,
        } => Ok(PortForwardRule::Local {
            host_ref: host_ref.to_owned(),
            local_bind_address: local_bind_address
                .parse::<IpAddr>()
                .map_err(|_| PluginResourceError::new("invalidForwardRule"))?,
            local_listen_port: *local_listen_port,
            remote_target_host: remote_target_host.clone(),
            remote_target_port: *remote_target_port,
        }),
        PluginForwardRule::Remote {
            remote_bind_address,
            remote_listen_port,
            local_target_host,
            local_target_port,
        } => Ok(PortForwardRule::Remote {
            host_ref: host_ref.to_owned(),
            remote_bind_address: remote_bind_address.clone(),
            remote_listen_port: *remote_listen_port,
            local_target_host: local_target_host.clone(),
            local_target_port: *local_target_port,
        }),
        PluginForwardRule::Dynamic {
            local_bind_address,
            local_listen_port,
        } => Ok(PortForwardRule::Dynamic {
            host_ref: host_ref.to_owned(),
            local_bind_address: local_bind_address
                .parse::<IpAddr>()
                .map_err(|_| PluginResourceError::new("invalidForwardRule"))?,
            local_listen_port: *local_listen_port,
        }),
    }
}

fn map_forward(
    handle: &str,
    summary: crate::forward_session_service::ForwardSessionSummary,
) -> PluginOwnedForwardSummary {
    let state = match summary.state {
        ForwardSessionState::Starting => PluginOwnedForwardState::Starting,
        ForwardSessionState::Running => PluginOwnedForwardState::Running,
        ForwardSessionState::Failed => PluginOwnedForwardState::Failed,
        ForwardSessionState::Stopping => PluginOwnedForwardState::Stopping,
        ForwardSessionState::Stopped => PluginOwnedForwardState::Stopped,
    };
    let actual_bind = map_actual_bind(summary.actual_bind);
    PluginOwnedForwardSummary {
        forward_handle: handle.to_owned(),
        generation: WireSequence::new(summary.generation.get()),
        state,
        actual_bind,
        child_count: u32::try_from(summary.child_count).unwrap_or(u32::MAX),
        listener_to_target_bytes: WireSequence::new(summary.listener_to_target_bytes),
        target_to_listener_bytes: WireSequence::new(summary.target_to_listener_bytes),
        stable_error: summary
            .failure
            .map(|failure| stable_forward_error(failure.code).to_owned()),
        cleanup_uncertain: summary.cleanup.uncertain,
    }
}

fn map_actual_bind(bind: Option<ActualForwardBind>) -> Option<PluginActualForwardBind> {
    bind.map(|bind| match bind {
        ActualForwardBind::Local { address } => PluginActualForwardBind::Local {
            address: address.ip().to_string(),
            port: address.port(),
        },
        ActualForwardBind::Remote { address, port } => {
            PluginActualForwardBind::Remote { address, port }
        }
    })
}

const fn stable_forward_error(code: ForwardFailureCode) -> &'static str {
    match code {
        ForwardFailureCode::InvalidRule => "invalidForwardRule",
        ForwardFailureCode::HostUnavailable => "parentSessionUnavailable",
        ForwardFailureCode::HostKeyReviewRequired
        | ForwardFailureCode::HostKeyMismatch
        | ForwardFailureCode::CredentialUnavailable
        | ForwardFailureCode::AuthenticationRejected => "unexpectedAuthenticationPath",
        ForwardFailureCode::TransportConnect | ForwardFailureCode::TransportLost => {
            "parentTransportLost"
        }
        ForwardFailureCode::Bind => "forwardBindFailed",
        ForwardFailureCode::RemoteRegistrationRejected => "remoteForwardRejected",
        ForwardFailureCode::RemoteRegistrationUncertain => "remoteForwardUncertain",
        ForwardFailureCode::Protocol => "forwardProtocolFailed",
        ForwardFailureCode::ResourceLimit => "forwardResourceLimit",
        ForwardFailureCode::SocksTruncated
        | ForwardFailureCode::SocksAuthenticationRejected
        | ForwardFailureCode::SocksCommandRejected
        | ForwardFailureCode::SocksAddressRejected => "socksRequestRejected",
        ForwardFailureCode::ConnectTimeout => "forwardConnectTimeout",
        ForwardFailureCode::TargetConnect => "forwardTargetUnavailable",
        ForwardFailureCode::Relay => "forwardRelayFailed",
        ForwardFailureCode::CleanupUncertain => "cleanupIncomplete",
    }
}
