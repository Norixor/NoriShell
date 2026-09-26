//! Typed API admission. Plugin identities and scopes are always taken from Core state.

use super::api_invocation::ApiInvocation;
use super::*;
use norishell_core_api::{
    PluginApiCall, PluginApiErrorCode, PluginApiReply, PluginApiValue, PluginApprovalOperation,
};

pub(super) struct ApiAccessReview {
    pub(super) capability: PluginCapability,
    pub(super) operation: PluginApprovalOperation,
    /// Visible only while the protected approval is active. It may identify
    /// the chosen file/device because it is never copied to an approval row.
    pub(super) target_label: String,
    /// The non-secret management label persisted with the exact-operation
    /// approval. Selection-based surfaces use an opaque Core-generated id.
    pub(super) persisted_target_label: String,
    pub(super) details: String,
    pub(super) exact_scope: serde_json::Value,
    /// A Core-validated permission family may span several UI actions. Other
    /// calls remain bound to their individual action identity.
    pub(super) policy_identity: Option<&'static str>,
    pub(super) target_fence: Option<crate::plugin_api::ResourceFence>,
}

impl PluginService {
    /// The document-action entry point is a declarative wrapper. Its request identity and action
    /// fence are Core-validated before this method is reached.
    pub(super) async fn invoke_api_call(
        &self,
        request: &PluginUiActionRequest,
        installed: &PluginInstalledRecord,
        call: &PluginApiCall,
        explicit_user_action: bool,
        input_focus: Option<norishell_core_api::TerminalInputFocusSnapshot>,
    ) -> CoreResult<PluginApiReply> {
        self.invoke_api(
            installed,
            ApiInvocation::declarative(request, explicit_user_action, input_focus),
            call,
        )
        .await
    }

    /// Isolated surfaces enter the same admission path only after a Core-owned bridge has
    /// atomically claimed its pending call. Guest supplied call IDs never become authority.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn invoke_isolated_api(
        &self,
        request_id: RequestId,
        installed: &PluginInstalledRecord,
        owner: crate::plugin_api::ResourceOwner,
        operation_id: String,
        authority: crate::plugin_api::ResourceFence,
        transaction_authority: crate::plugin_api::ResourceFence,
        call: &PluginApiCall,
        explicit_user_action: bool,
    ) -> CoreResult<PluginApiReply> {
        self.invoke_api(
            installed,
            ApiInvocation::isolated(
                request_id,
                owner,
                operation_id,
                authority,
                transaction_authority,
                explicit_user_action,
            ),
            call,
        )
        .await
    }

    pub(super) async fn invoke_api(
        &self,
        installed: &PluginInstalledRecord,
        invocation: ApiInvocation<'_>,
        call: &PluginApiCall,
    ) -> CoreResult<PluginApiReply> {
        if let Err(code) = crate::plugin_api::validate_call(call) {
            return Ok(api_reply(call, Err(code)));
        }
        // Workflows may use their own scoped resources, but cannot turn a background
        // continuation into UI, terminal input, capability escalation or nested tasks.
        if matches!(invocation, ApiInvocation::Task { .. })
            && !matches!(
                &call.operation,
                norishell_core_api::PluginApiOperation::Describe { .. }
                    | norishell_core_api::PluginApiOperation::DataCatalog { .. }
                    | norishell_core_api::PluginApiOperation::DataRead { .. }
                    | norishell_core_api::PluginApiOperation::DataSnapshot { .. }
                    | norishell_core_api::PluginApiOperation::DataInspect { .. }
                    | norishell_core_api::PluginApiOperation::DataCompose { .. }
                    | norishell_core_api::PluginApiOperation::DataApply { .. }
                    | norishell_core_api::PluginApiOperation::DataExport { .. }
                    | norishell_core_api::PluginApiOperation::DataCheckpoint { .. }
                    | norishell_core_api::PluginApiOperation::DataRelease { .. }
                    | norishell_core_api::PluginApiOperation::Permissions { .. }
                    | norishell_core_api::PluginApiOperation::PermissionRevoke { .. }
                    | norishell_core_api::PluginApiOperation::PermissionsForget { .. }
                    | norishell_core_api::PluginApiOperation::ResourcesList { .. }
                    | norishell_core_api::PluginApiOperation::ResourceClose { .. }
                    | norishell_core_api::PluginApiOperation::ResourceEvents { .. }
                    | norishell_core_api::PluginApiOperation::TimerStart { .. }
                    | norishell_core_api::PluginApiOperation::SubscriptionStart { .. }
                    | norishell_core_api::PluginApiOperation::NetworkStart { .. }
                    | norishell_core_api::PluginApiOperation::NetworkSend { .. }
                    | norishell_core_api::PluginApiOperation::RemoteExecStart { .. }
                    | norishell_core_api::PluginApiOperation::RemoteExecSend { .. }
                    | norishell_core_api::PluginApiOperation::ProcessStart { .. }
                    | norishell_core_api::PluginApiOperation::ProcessSend { .. }
                    | norishell_core_api::PluginApiOperation::SftpOpen { .. }
                    | norishell_core_api::PluginApiOperation::Sftp { .. }
                    | norishell_core_api::PluginApiOperation::File { .. }
                    | norishell_core_api::PluginApiOperation::Storage { .. }
                    | norishell_core_api::PluginApiOperation::SerialDevices { .. }
                    | norishell_core_api::PluginApiOperation::SerialOpen { .. }
                    | norishell_core_api::PluginApiOperation::SerialSend { .. }
            )
        {
            return Ok(api_reply(call, Err(PluginApiErrorCode::PermissionDenied)));
        }
        // A provider can only operate its own protocol transport. Guest output cannot expand
        // this into page actions, existing terminal input, credentials or filesystem access.
        if matches!(invocation, ApiInvocation::Provider { .. })
            && !matches!(
                &call.operation,
                norishell_core_api::PluginApiOperation::NetworkStart { .. }
                    | norishell_core_api::PluginApiOperation::SerialDevices { .. }
                    | norishell_core_api::PluginApiOperation::SerialOpen { .. }
                    | norishell_core_api::PluginApiOperation::SerialSend { .. }
                    | norishell_core_api::PluginApiOperation::NetworkSend { .. }
                    | norishell_core_api::PluginApiOperation::ResourceClose { .. }
                    | norishell_core_api::PluginApiOperation::ResourcesList { .. }
                    | norishell_core_api::PluginApiOperation::Describe { .. }
            )
        {
            return Ok(api_reply(call, Err(PluginApiErrorCode::PermissionDenied)));
        }
        let owner = invocation.owner(installed);
        if !invocation.matches_installation(installed)
            || !api_owner_matches_installation(&owner, installed)
        {
            return Ok(api_reply(call, Err(PluginApiErrorCode::Revoked)));
        }
        let current = self.api_invocation_fence(&owner, &invocation);
        if !current() {
            return Ok(api_reply(call, Err(PluginApiErrorCode::Revoked)));
        }
        let resource_fence = self.api_resource_fence(owner.clone(), None);
        let resource_fence = if invocation.resource_consumer().is_some() {
            let consumer_authority = invocation.transaction_authority(self);
            Arc::new(move || resource_fence() && consumer_authority())
                as crate::plugin_api::ResourceFence
        } else {
            resource_fence
        };
        let _creation_permit = if matches!(
            call.operation,
            norishell_core_api::PluginApiOperation::TimerStart { .. }
        ) {
            Some(self.api_creation_permit(invocation.request_id().clone())?)
        } else {
            None
        };
        if !current() {
            return Ok(api_reply(call, Err(PluginApiErrorCode::Revoked)));
        }

        let reply = match &call.operation {
            norishell_core_api::PluginApiOperation::TaskStart { .. }
            | norishell_core_api::PluginApiOperation::TaskGet { .. }
            | norishell_core_api::PluginApiOperation::TaskList { .. }
            | norishell_core_api::PluginApiOperation::TaskCancel { .. }
            | norishell_core_api::PluginApiOperation::TaskResume { .. } => api_reply(
                call,
                self.invoke_task_api(&invocation, installed, call).await,
            ),
            norishell_core_api::PluginApiOperation::AppRegister { .. }
            | norishell_core_api::PluginApiOperation::AppNotify { .. }
            | norishell_core_api::PluginApiOperation::AppNavigate { .. } => api_reply(
                call,
                self.invoke_app_integration(&invocation, &owner, &call.operation),
            ),
            norishell_core_api::PluginApiOperation::SerialDevices {} => {
                api_reply(call, self.list_api_serial(&invocation, &owner).await)
            }
            norishell_core_api::PluginApiOperation::SerialOpen {
                candidate_id,
                settings,
            } => api_reply(
                call,
                self.open_api_serial(&invocation, installed, &owner, candidate_id, *settings)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::SerialSend { request } => api_reply(
                call,
                self.send_api_serial(&invocation, installed, &owner, request)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::ProtocolOpen { request } => api_reply(
                call,
                self.open_protocol_launch(&invocation, &owner, &call.call_id, request),
            ),
            norishell_core_api::PluginApiOperation::Permissions {} => {
                api_reply(call, self.api_permissions(installed))
            }
            norishell_core_api::PluginApiOperation::PermissionRequest { capability } => api_reply(
                call,
                self.request_api_permission(&invocation, installed, *capability),
            ),
            norishell_core_api::PluginApiOperation::PermissionRevoke {
                permission_id,
                expected_policy_revision,
            } => api_reply(
                call,
                self.revoke_api_operation_permission(
                    &invocation,
                    installed,
                    permission_id,
                    *expected_policy_revision,
                ),
            ),
            norishell_core_api::PluginApiOperation::PermissionsForget {} => {
                api_reply(call, self.forget_api_permissions(&invocation, installed))
            }
            norishell_core_api::PluginApiOperation::TerminalRequestInput {
                terminal_handle: _,
                payload: _,
                append_enter: _,
            } if invocation.is_isolated() => {
                api_reply(call, Err(PluginApiErrorCode::InteractionRequired))
            }
            norishell_core_api::PluginApiOperation::TerminalRequestInput {
                terminal_handle,
                payload,
                append_enter,
            } => {
                let focus = invocation
                    .input_focus()
                    .filter(|_| invocation.explicit_user_action())
                    .ok_or_else(|| plugin_permission_error(invocation.request_id().clone()))?;
                if !current() {
                    return Ok(api_reply(call, Err(PluginApiErrorCode::Revoked)));
                }
                let value = self
                    .user_terminal_input(
                        match &invocation {
                            ApiInvocation::Declarative { request, .. } => request,
                            ApiInvocation::Isolated { .. }
                            | ApiInvocation::Provider { .. }
                            | ApiInvocation::Task { .. } => {
                                unreachable!("isolated input is rejected before dispatch")
                            }
                        },
                        terminal_handle,
                        payload,
                        *append_enter,
                        focus,
                    )
                    .await?;
                if !current() {
                    return Ok(api_reply(call, Err(PluginApiErrorCode::Revoked)));
                }
                PluginApiReply {
                    call_id: call.call_id.clone(),
                    outcome: norishell_core_api::PluginApiOutcome::Completed { value },
                }
            }
            norishell_core_api::PluginApiOperation::Credential { operation } => api_reply(
                call,
                self.invoke_api_credential(&invocation, installed, &owner, operation)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::Storage { operation } => api_reply(
                call,
                self.invoke_api_storage(&invocation, &owner, operation, &current),
            ),
            norishell_core_api::PluginApiOperation::DataRead { request } => api_reply(
                call,
                self.read_api_data(&invocation, &owner, request, &current),
            ),
            norishell_core_api::PluginApiOperation::DataSnapshot { .. }
            | norishell_core_api::PluginApiOperation::DataInspect { .. }
            | norishell_core_api::PluginApiOperation::DataCompose { .. }
            | norishell_core_api::PluginApiOperation::DataApply { .. }
            | norishell_core_api::PluginApiOperation::DataExport { .. }
            | norishell_core_api::PluginApiOperation::DataCheckpoint { .. }
            | norishell_core_api::PluginApiOperation::DataRelease { .. } => api_reply(
                call,
                self.invoke_api_data(&invocation, &owner, &call.operation, &current)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::FilePick {
                picker_kind,
                access,
            } => api_reply(
                call,
                self.pick_api_file(&invocation, installed, &owner, *picker_kind, *access)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::File { operation } => {
                let result = async {
                    self.has_capability(
                        invocation.request_id().clone(),
                        &owner.plugin_id,
                        &owner.signer,
                        PluginCapability::LocalFiles,
                    )
                    .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
                    if !current() {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    let _permit = self
                        .api_creation_permit(invocation.request_id().clone())
                        .map_err(|_| PluginApiErrorCode::Revoked)?;
                    if !current() {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    let files = invocation.resource_consumer().map_or_else(
                        || self.api.files.clone(),
                        |consumer| self.api.files.for_consumer(consumer),
                    );
                    let result = files
                        .invoke(&owner, operation, resource_fence.clone())
                        .await?;
                    if !current() {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    Ok(PluginApiValue::File { result })
                }
                .await;
                api_reply(call, result)
            }
            norishell_core_api::PluginApiOperation::NetworkStart {
                endpoint,
                request: network,
            } => api_reply(
                call,
                self.start_api_network(&invocation, installed, &owner, endpoint, network)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::NetworkSend { request: network } => {
                let result = async {
                    self.has_capability(
                        invocation.request_id().clone(),
                        &owner.plugin_id,
                        &owner.signer,
                        PluginCapability::NetworkDomain,
                    )
                    .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
                    if !current() {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    crate::plugin_api::network::NetworkDriver::send(
                        &invocation.resource_registry(&self.api.resources),
                        &owner,
                        network.clone(),
                        &resource_fence,
                    )
                    .await?;
                    if !current() {
                        return Err(PluginApiErrorCode::Revoked);
                    }
                    Ok(PluginApiValue::NetworkSent {
                        handle: network.handle.clone(),
                    })
                }
                .await;
                api_reply(call, result)
            }
            norishell_core_api::PluginApiOperation::SubscriptionStart { topics } => api_reply(
                call,
                self.start_api_subscription(&invocation, installed, &owner, topics)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::SftpOpen {
                host_handle,
                root_path,
                write,
            } => api_reply(
                call,
                self.open_api_sftp(
                    &invocation,
                    installed,
                    &owner,
                    host_handle,
                    root_path,
                    *write,
                )
                .await,
            ),
            norishell_core_api::PluginApiOperation::Sftp { operation } => api_reply(
                call,
                self.invoke_api_sftp(&invocation, installed, &owner, operation)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::RemoteExecStart { request } => api_reply(
                call,
                self.start_api_remote_exec(&invocation, installed, &owner, request)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::RemoteExecSend { request } => api_reply(
                call,
                self.send_api_remote_exec(&invocation, installed, &owner, request)
                    .await,
            ),
            norishell_core_api::PluginApiOperation::ProcessStart {
                program,
                arguments,
                timeout_ms,
            } => api_reply(
                call,
                self.start_api_process(
                    &invocation,
                    installed,
                    &owner,
                    program,
                    arguments,
                    *timeout_ms,
                )
                .await,
            ),
            norishell_core_api::PluginApiOperation::ProcessSend { request: process } => api_reply(
                call,
                self.send_api_process(&invocation, &owner, process).await,
            ),
            _ => {
                if !current() {
                    return Ok(api_reply(call, Err(PluginApiErrorCode::Revoked)));
                }
                let reply = crate::plugin_api::PluginApi {
                    resources: invocation.resource_registry(&self.api.resources),
                    ..self.api.clone()
                }
                .invoke(&owner, call, resource_fence)
                .await;
                if !current() {
                    return Ok(api_reply(call, Err(PluginApiErrorCode::Revoked)));
                }
                reply
            }
        };
        Ok(reply)
    }

    fn invoke_api_storage(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &crate::plugin_api::ResourceOwner,
        operation: &norishell_core_api::PluginStorageOperation,
        current: &crate::plugin_api::ResourceFence,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::StoragePlugin,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let _permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Revoked)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        // The final check must include task/provider cancellation while the repository is
        // locked. The transaction fence is atomic and never reenters that repository.
        let fence = self.api_transaction_fence(owner, invocation);
        crate::plugin_api::storage::PluginStorageDriver::new(self.hosts.clone())
            .invoke(owner, fence, operation)
            .map(|result| PluginApiValue::Storage { result })
    }

    pub(super) async fn authorize_api_access(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &crate::plugin_api::ResourceOwner,
        review: ApiAccessReview,
    ) -> Result<crate::plugin_api::ResourceFence, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &installed.plugin_id,
            &installed.signer_fingerprint_sha256,
            review.capability,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                installed,
                review.capability,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let lifetime = self.api_resource_fence(owner.clone(), Some((review.capability, epoch)));
        let policy_identity = review
            .policy_identity
            .unwrap_or_else(|| invocation.operation_identity());
        let prepared = self
            .operation_policies
            .prepare(
                installed,
                review.capability,
                current_plugin_permission_binding(&installed.package_sha256),
                review.operation,
                policy_identity,
                &review.persisted_target_label,
                &(policy_identity, review.exact_scope),
            )
            .ok();
        let invocation_fence = self.api_invocation_fence(owner, invocation);
        let approval_fence: crate::plugin_api::ResourceFence = {
            let lifetime = lifetime.clone();
            let target_fence = review.target_fence;
            Arc::new(move || {
                lifetime()
                    && invocation_fence()
                    && target_fence.as_ref().is_none_or(|fence| fence())
            })
        };
        if !approval_fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let locale = self
            .active_instance(
                invocation.request_id().clone(),
                &installed.plugin_id,
                &installed.signer_fingerprint_sha256,
                owner.generation,
            )
            .map_err(|_| PluginApiErrorCode::Revoked)?
            .locale;
        let approved = self
            .operations
            .as_ref()
            .ok_or(PluginApiErrorCode::Unavailable)?
            .approve_independent(
                crate::plugin_operations::prompts::IndependentApproval {
                    plugin_id: installed.plugin_id.clone(),
                    instance_generation: owner.generation,
                    plugin_name: installed.name.clone(),
                    locale,
                    host_label: review.target_label.clone(),
                    endpoint: review.target_label,
                    fence: approval_fence.clone(),
                    remembered_policy: prepared,
                    unavailable_policy:
                        norishell_core_api::PluginRememberPolicy::StorageUnavailable,
                },
                norishell_core_api::PluginRemoteApprovalContent::Access {
                    operation: review.operation,
                    details: review.details,
                    reason: invocation.operation_identity().to_owned(),
                },
                invocation.explicit_user_action(),
            )
            .await?;
        if !approval_fence()
            || !approved
                .as_ref()
                .is_none_or(|policy| policy.admit_dispatch())
        {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(Arc::new(move || {
            lifetime()
                && approved
                    .as_ref()
                    .is_none_or(|policy| policy.admit_dispatch())
        }))
    }

    pub(super) fn api_invocation_fence(
        &self,
        owner: &crate::plugin_api::ResourceOwner,
        invocation: &ApiInvocation<'_>,
    ) -> crate::plugin_api::ResourceFence {
        let runtime = self.api_runtime_fence(owner.clone());
        let action = invocation.action_authority(self);
        Arc::new(move || runtime() && action())
    }

    /// Never acquires HostRepository; safe for a final check while its transaction is locked.
    pub(super) fn api_transaction_fence(
        &self,
        owner: &crate::plugin_api::ResourceOwner,
        invocation: &ApiInvocation<'_>,
    ) -> crate::plugin_api::ResourceFence {
        let runtime = self.api_runtime_fence(owner.clone());
        let action = invocation.transaction_authority(self);
        Arc::new(move || runtime() && action())
    }

    pub(super) fn api_creation_permit(
        &self,
        request_id: RequestId,
    ) -> CoreResult<crate::lifecycle::ResourceCreationPermit> {
        if let Some(app) = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return app
                .try_state::<LifecycleState>()
                .ok_or_else(|| plugin_runtime_error(request_id.clone(), None))?
                .acquire_resource_creation(request_id);
        }
        #[cfg(test)]
        if let Some(lifecycle) = self
            .test_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return lifecycle.acquire_resource_creation(request_id);
        }
        Err(plugin_runtime_error(request_id, None))
    }

    pub(super) fn api_runtime_fence(
        &self,
        owner: crate::plugin_api::ResourceOwner,
    ) -> crate::plugin_api::ResourceFence {
        let token = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get(owner.plugin_id.as_str())
            .filter(|instance| {
                !instance.operation_authority_revoked
                    && instance.instance_generation == owner.generation
                    && instance.package_sha256 == owner.package
                    && instance.signer_fingerprint_sha256 == owner.signer
            })
            .map(|instance| Arc::downgrade(&instance.api_authority));
        Arc::new(move || {
            token
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .is_some_and(|token| token.load(std::sync::atomic::Ordering::Acquire))
        })
    }

    pub(super) fn api_resource_fence(
        &self,
        resource_owner: crate::plugin_api::ResourceOwner,
        capability: Option<(PluginCapability, WireSequence)>,
    ) -> crate::plugin_api::ResourceFence {
        let current = self.api_runtime_fence(resource_owner.clone());
        let hosts = self.hosts.clone();
        Arc::new(move || {
            current()
                && hosts
                    .with_plugin_repository(|repository| {
                        let installed =
                            repository.get_plugin_installation(&resource_owner.plugin_id)?;
                        if installed.state != PluginInstallState::Enabled
                            || installed.package_sha256 != resource_owner.package
                            || installed.signer_fingerprint_sha256 != resource_owner.signer
                        {
                            return Ok(false);
                        }
                        let Some((capability, epoch)) = capability else {
                            return Ok(true);
                        };
                        Ok(repository
                            .list_plugin_capability_grants(
                                &installed.plugin_id,
                                &installed.signer_fingerprint_sha256,
                                u64::from(PLUGIN_PROTOCOL_MAJOR),
                            )?
                            .iter()
                            .any(|grant| {
                                grant.capability == capability
                                    && grant.state_version == epoch
                                    && effective_plugin_grant(&installed, grant)
                            }))
                    })
                    .unwrap_or(false)
        })
    }
}

fn api_owner_matches_installation(
    owner: &crate::plugin_api::ResourceOwner,
    installed: &PluginInstalledRecord,
) -> bool {
    owner.plugin_id == installed.plugin_id
        && owner.signer == installed.signer_fingerprint_sha256
        && owner.package == installed.package_sha256
}

fn api_reply(
    call: &PluginApiCall,
    result: Result<PluginApiValue, PluginApiErrorCode>,
) -> PluginApiReply {
    PluginApiReply {
        call_id: call.call_id.clone(),
        outcome: match result {
            Ok(value) => norishell_core_api::PluginApiOutcome::Completed { value },
            Err(code) => norishell_core_api::PluginApiOutcome::Failed { code },
        },
    }
}
