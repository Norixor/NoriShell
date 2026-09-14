//! Plugin identity/scope admission and result delivery for public operations.

use norishell_core_api::PluginRemoteOperationRequest;
use norishell_plugin_platform::operations::{
    ApprovalRequirement, OperationClass, RemoteOperation, plan,
};

use super::*;
use crate::plugin_contribution::PluginUiState;
use crate::plugin_operations::RemoteInvocation;

impl PluginService {
    pub(super) fn broker_action_current(&self, request: &PluginUiActionRequest) -> bool {
        let Some(definition) = plugin_extension_registry::find(&request.target_id) else {
            return false;
        };
        let Ok(installed) = self.has_capability(
            RequestId::new(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            definition.required_capability,
        ) else {
            return false;
        };
        if installed.package_sha256 != request.expected_package_sha256
            || installed.state_version != request.expected_state_version
        {
            return false;
        }
        self.broker_action_runtime_current(request)
    }

    /// Repository transactions use only this in-memory part after capability admission.
    /// Permission changes stop the instance before committing their new grant revision.
    pub(super) fn broker_action_runtime_current(&self, request: &PluginUiActionRequest) -> bool {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !runtime
            .mutation_reservations
            .contains_key(request.plugin_id.as_str())
            && runtime
                .target_contexts
                .get(request.context_handle.as_str())
                .is_some_and(|target| {
                    target.context.target_id == request.target_id
                        && target.context.target_revision == request.expected_target_revision
                })
            && runtime
                .active_instances
                .get(request.plugin_id.as_str())
                .is_some_and(|instance| {
                    !instance.operation_authority_revoked
                        && instance.signer_fingerprint_sha256 == request.signer_fingerprint_sha256
                        && instance.package_sha256 == request.expected_package_sha256
                        && instance.instance_generation == request.instance_generation
                        && instance.state_version == request.expected_state_version
                        && instance.contribution_revision == request.expected_contribution_revision
                        && instance.contribution_action_in_flight
                })
    }

    async fn session_operation_binding(
        &self,
        request: &PluginUiActionRequest,
        terminal_handle: &str,
        capability: PluginCapability,
    ) -> CoreResult<SessionOperationBinding> {
        let installed = self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            capability,
        )?;
        self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::TerminalMetadata,
        )?;
        let (target, instance_key) = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .target_contexts
            .get(request.context_handle.as_str())
            .map(|target| (target.context.clone(), target.instance_key.clone()))
            .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
        let metadata = self
            .terminal_metadata_for_action(request, &target, &installed)
            .await?
            .filter(|metadata| {
                metadata.kind == norishell_core_api::PluginTerminalKind::Ssh
                    && metadata.terminal_handle == terminal_handle
            })
            .ok_or_else(|| plugin_permission_error(request.meta.request_id.clone()))?;
        let parts = instance_key.split('|').collect::<Vec<_>>();
        if parts.len() != 5 || parts[2] != "ssh" {
            return Err(plugin_permission_error(request.meta.request_id.clone()));
        }
        let session_id = norishell_core_api::SshSessionId::parse(parts[3])
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        let lease = self
            .sessions
            .plugin_channel_lease(
                request.meta.request_id.clone(),
                session_id,
                metadata.generation,
            )
            .await?;
        let epoch = self.capability_grant_epoch_for_record(
            request.meta.request_id.clone(),
            &installed,
            capability,
        )?;
        let authority = SessionOperationAuthority {
            hosts: self.hosts.clone(),
            runtime: Arc::downgrade(&self.runtime),
            plugin_id: request.plugin_id.clone(),
            signer: request.signer_fingerprint_sha256.clone(),
            package: request.expected_package_sha256.clone(),
            state_version: request.expected_state_version,
            generation: request.instance_generation,
            context_handle: request.context_handle.clone(),
            target_id: request.target_id.clone(),
            target_revision: request.expected_target_revision,
            capability,
            epoch,
            lease: lease.clone(),
            require_context: true,
        };
        let mut handoff = authority.clone();
        handoff.require_context = false;
        let handoff_fence: crate::plugin_operations::OperationFence =
            Arc::new(move || handoff.current());
        let lifetime_fence: crate::plugin_operations::OperationFence =
            Arc::new(move || authority.current());
        if !self.broker_action_current(request) || !lifetime_fence() {
            return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
        }
        let service = self.clone();
        let action_request = request.clone();
        let lifetime = lifetime_fence.clone();
        let fence = Arc::new(move || lifetime() && service.broker_action_current(&action_request));
        let locale = self
            .active_instance(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                request.instance_generation,
            )?
            .locale;
        Ok(SessionOperationBinding {
            installed,
            instance_generation: request.instance_generation,
            lease,
            epoch,
            handoff_fence,
            fence,
            locale,
            label: metadata.label,
        })
    }

    async fn remote_operation_result(
        &self,
        request: &PluginUiActionRequest,
        supplied: &PluginRemoteOperationRequest,
        explicit_user_action: bool,
    ) -> CoreResult<serde_json::Value> {
        let operation: RemoteOperation = serde_json::from_str(&supplied.operation_json)
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        let plan = plan(&operation)
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        let capability = match (plan.class, plan.approval) {
            (OperationClass::ReadOnly, ApprovalRequirement::None) => {
                PluginCapability::RemoteInspect
            }
            (OperationClass::Mutation, ApprovalRequirement::CoreProtectedPerExecution) => {
                PluginCapability::RemoteExecRequest
            }
            _ => return Err(plugin_validation_error(request.meta.request_id.clone())),
        };
        let binding = self
            .session_operation_binding(request, &supplied.terminal_handle, capability)
            .await?;
        if !explicit_user_action && plan.class == OperationClass::Mutation {
            return Err(plugin_permission_error(request.meta.request_id.clone()));
        }
        let mut invocation = binding.remote_invocation(supplied.reason.clone());
        invocation.remembered_policy = self.session_operation_policy(
            &binding.installed,
            request.action_id.as_str(),
            norishell_core_api::PluginApprovalOperation::RemoteExecute,
            &binding.lease,
            &operation,
        );
        let operations = self
            .operations
            .as_ref()
            .ok_or_else(|| plugin_runtime_error(request.meta.request_id.clone(), None))?;
        let result = operations.invoke(invocation, plan).await;
        if !(binding.fence)() {
            return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
        }
        let mut result = serde_json::to_value(result)
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        result["kind"] = serde_json::json!("operation");
        Ok(result)
    }

    async fn resource_operation_result(
        &self,
        request: &PluginUiActionRequest,
        supplied: &norishell_core_api::PluginResourceOperationRequest,
        explicit_user_action: bool,
    ) -> CoreResult<(
        serde_json::Value,
        Option<norishell_core_api::PluginHostNavigationEvent>,
        Option<norishell_core_api::PluginApprovedTerminalChannelLaunch>,
    )> {
        use norishell_core_api::{
            PluginRemoteApprovalContent, PluginResourceOperation as Operation,
            PluginResourceOperationResult as ResourceResult,
        };
        if !explicit_user_action && !matches!(supplied.operation, Operation::LogsRead { .. }) {
            return Err(plugin_permission_error(request.meta.request_id.clone()));
        }
        let capability = match &supplied.operation {
            Operation::SftpOpen { .. } | Operation::LogsRead { .. } => PluginCapability::SftpRead,
            _ => PluginCapability::RemoteExecRequest,
        };
        let binding = self
            .session_operation_binding(request, &supplied.terminal_handle, capability)
            .await?;
        let mut remote = binding.remote_invocation(supplied.reason.clone());
        let operations = self
            .operations
            .as_ref()
            .ok_or_else(|| plugin_runtime_error(request.meta.request_id.clone(), None))?;
        let resource_error = |code: &str| {
            (
                serde_json::json!({"kind":"resource","error":code}),
                None,
                None,
            )
        };
        if let Operation::DockerTerminalOpen {
            container_id,
            shell,
        } = &supplied.operation
        {
            let Some(command) = super::terminal::startup_command(container_id, *shell) else {
                return Ok(resource_error("invalidContainer"));
            };
            remote.remembered_policy = self.session_operation_policy(
                &binding.installed,
                request.action_id.as_str(),
                norishell_core_api::PluginApprovalOperation::RemoteExecute,
                &binding.lease,
                &supplied.operation,
            );
            let approved = match operations
                .approve_resource(
                    &remote,
                    PluginRemoteApprovalContent::Execute {
                        command: command.clone(),
                        stdin: None,
                        reason: supplied.reason.clone(),
                    },
                )
                .await
            {
                Ok(approved) => approved,
                Err(code) => return Ok(resource_error(code)),
            };
            let handoff = binding.handoff_fence.clone();
            let handoff_fence: crate::plugin_operations::OperationFence = Arc::new(move || {
                handoff()
                    && approved
                        .as_ref()
                        .is_none_or(|policy| policy.admit_dispatch())
            });
            if !(binding.fence)() {
                return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
            }
            let launch = self.prepare_terminal_channel_launch(
                request.plugin_id.clone(),
                binding.lease,
                command,
                container_id,
                handoff_fence,
            )?;
            return Ok((
                serde_json::json!({"kind":"resource","value":ResourceResult::TerminalLaunchRequested}),
                None,
                Some(launch),
            ));
        }
        let Some(resources) = self.resources.as_ref() else {
            return Ok(resource_error("unavailable"));
        };
        let mut invocation = crate::plugin_resources::PluginResourceInvocation {
            owner: crate::plugin_resources::PluginResourceOwner {
                plugin_id: request.plugin_id.clone(),
                signer_fingerprint_sha256: request.signer_fingerprint_sha256.clone(),
                package_sha256: request.expected_package_sha256.clone(),
                instance_generation: request.instance_generation,
                capability_grant_epoch: binding.epoch,
            },
            lease: binding.lease.clone(),
            reason: supplied.reason.clone(),
            action_fence: binding.fence.clone(),
            lifetime_fence: binding.handoff_fence.clone(),
        };
        let review = match &supplied.operation {
            Operation::ForwardStart { rule } => {
                let rendered = serde_json::to_string(rule)
                    .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
                if rendered.len() > 4096 || rendered.chars().any(|ch| matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) { return Ok(resource_error("invalidForwardRule")); }
                Some(PluginRemoteApprovalContent::ForwardStart {
                    rule: rule.clone(),
                    reason: supplied.reason.clone(),
                })
            }
            Operation::ForwardStop { forward_handle } => {
                let (rule, actual_bind) =
                    match resources.forward_review(&invocation, forward_handle) {
                        Ok(value) => value,
                        Err(error) => return Ok(resource_error(error.stable_code)),
                    };
                Some(PluginRemoteApprovalContent::ForwardStop {
                    rule,
                    actual_bind,
                    forward_handle: forward_handle.clone(),
                    reason: supplied.reason.clone(),
                })
            }
            _ => None,
        };
        if let Some(content) = review {
            let (kind, scope) = match &content {
                PluginRemoteApprovalContent::ForwardStart { rule, .. } => (
                    norishell_core_api::PluginApprovalOperation::ForwardStart,
                    serde_json::json!({"rule": rule}),
                ),
                PluginRemoteApprovalContent::ForwardStop {
                    rule, actual_bind, ..
                } => (
                    norishell_core_api::PluginApprovalOperation::ForwardStop,
                    serde_json::json!({"rule": rule, "actualBind": actual_bind}),
                ),
                _ => return Ok(resource_error("invalidOperation")),
            };
            remote.remembered_policy = self.session_operation_policy(
                &binding.installed,
                request.action_id.as_str(),
                kind,
                &binding.lease,
                &scope,
            );
            let approved = match operations.approve_resource(&remote, content).await {
                Ok(approved) => approved,
                Err(code) => return Ok(resource_error(code)),
            };
            if let Some(approved) = approved {
                let action = invocation.action_fence.clone();
                let action_policy = approved.clone();
                invocation.action_fence =
                    Arc::new(move || action() && action_policy.admit_dispatch());
                let lifetime = invocation.lifetime_fence.clone();
                invocation.lifetime_fence = Arc::new(move || lifetime() && approved.current());
            }
        }
        let result = match resources
            .invoke(invocation.clone(), supplied.operation.clone())
            .await
        {
            Ok(value) => value,
            Err(error) => return Ok(resource_error(error.stable_code)),
        };
        if !(binding.fence)() {
            return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
        }
        let navigation = if let ResourceResult::NavigationRequested {
            request: navigation,
        } = &result
        {
            let summary = match resources.sftp_navigation_summary(&invocation).await {
                Ok(value) => value,
                Err(error) => return Ok(resource_error(error.stable_code)),
            };
            Some(norishell_core_api::PluginHostNavigationEvent {
                operation_id: norishell_core_api::OperationId::new(),
                plugin_id: request.plugin_id.clone(),
                sftp_session: summary,
                request: navigation.clone(),
            })
        } else {
            None
        };
        Ok((
            serde_json::json!({"kind":"resource","value":result}),
            navigation,
            None,
        ))
    }

    pub(super) async fn user_terminal_input(
        &self,
        request: &PluginUiActionRequest,
        terminal_handle: &str,
        payload: &str,
        append_enter: bool,
        focus: norishell_core_api::TerminalInputFocusSnapshot,
    ) -> CoreResult<norishell_core_api::PluginApiValue> {
        let binding = self
            .session_operation_binding(
                request,
                terminal_handle,
                PluginCapability::TerminalRequestInput,
            )
            .await?;
        if !matches!(&focus.target, Some(TerminalInputFocusTarget::Ssh(target))
            if target.session_id == binding.lease.session_id && target.expected_generation == binding.lease.generation)
        {
            return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
        }
        let mut pending = self
            .prepare_input_proposal(
                request.meta.request_id.clone(),
                request.plugin_id.clone(),
                request.signer_fingerprint_sha256.clone(),
                request.instance_generation,
                RuntimeTerminalInputRequest {
                    payload: payload.to_owned(),
                    append_enter,
                },
                focus,
            )
            .await?;
        pending.authority = Some(binding.handoff_fence);
        pending.remembered_policy = self.session_operation_policy(
            &binding.installed,
            request.action_id.as_str(),
            norishell_core_api::PluginApprovalOperation::TerminalInput,
            &binding.lease,
            &(payload, append_enter),
        );
        pending.proposal.remember_policy = if pending.remembered_policy.is_some() {
            norishell_core_api::PluginRememberPolicy::ExactOperation
        } else if pending.proposal.host_label.is_none() {
            norishell_core_api::PluginRememberPolicy::UnstableTarget
        } else {
            norishell_core_api::PluginRememberPolicy::StorageUnavailable
        };
        if let Some(prepared) = &pending.remembered_policy
            && let Some(approved) = prepared
                .find()
                .map_err(|_| plugin_conflict_error(request.meta.request_id.clone(), None))?
        {
            self.send_pending_input(request.meta.request_id.clone(), pending, Some(approved))
                .await?;
            return Ok(norishell_core_api::PluginApiValue::InputSent {});
        }
        let approval_id = pending.proposal.approval_id.clone();
        let state_version = pending.proposal.state_version;
        self.enqueue_input_proposal(request.meta.request_id.clone(), pending)?;
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| plugin_runtime_error(request.meta.request_id.clone(), None))?;
        self.open_input_proposal(
            PluginTerminalInputOpenRequest {
                meta: request.meta.clone(),
                approval_id: approval_id.clone(),
                expected_state_version: state_version,
            },
            &app,
        )?;
        Ok(norishell_core_api::PluginApiValue::InputApprovalRequested {
            approval_id: approval_id.to_string(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn invoke_operation_broker(
        &self,
        request: &PluginUiActionRequest,
        installed: &PluginInstalledRecord,
        parsed: &ParsedPluginUiOutputs,
        fields: &[PluginUiFieldValue],
        storage: &Option<PluginStorageSnapshot>,
        initial_state: &str,
        settings: &Option<serde_json::Value>,
        explicit_user_action: bool,
        input_focus: Option<norishell_core_api::TerminalInputFocusSnapshot>,
    ) -> CoreResult<(ParsedPluginUiOutputs, String)> {
        let mut current = parsed.clone();
        let mut state = current.ui_state.as_ref().map_or_else(
            || initial_state.to_owned(),
            |value| value.value_json.clone(),
        );
        let mut chain = super::broker_chain::BrokerChain::new();
        let mut callback_id = None;

        loop {
            let (result, navigation, terminal_launch) = if let Some(call) = &current.api_call {
                if !self.broker_action_current(request) {
                    return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
                }
                chain
                    .accept_call(&call.call_id)
                    .map_err(|error| broker_chain_error(request.meta.request_id.clone(), error))?;
                // The 120-second wall-clock budget includes protected approval waits. A timeout
                // cannot prove that an admitted API operation had no effect, so it is surfaced as
                // OutcomeUnknown rather than a plugin crash or a safe retry.
                let reply = tokio::time::timeout(
                    chain.remaining().map_err(|error| {
                        broker_chain_error(request.meta.request_id.clone(), error)
                    })?,
                    self.invoke_api_call(
                        request,
                        installed,
                        call,
                        explicit_user_action,
                        input_focus.clone(),
                    ),
                )
                .await
                .map_err(|_| broker_chain_outcome_unknown(request.meta.request_id.clone()))??;
                (
                    serde_json::json!({"kind":"api", "reply": reply}),
                    None,
                    None,
                )
            } else if let Some(operation) = &current.remote_operation {
                if callback_id.is_some() {
                    return Err(plugin_validation_error(request.meta.request_id.clone()));
                }
                (
                    self.remote_operation_result(request, operation, explicit_user_action)
                        .await?,
                    None,
                    None,
                )
            } else if let Some(operation) = &current.resource_operation {
                if callback_id.is_some() {
                    return Err(plugin_validation_error(request.meta.request_id.clone()));
                }
                self.resource_operation_result(request, operation, explicit_user_action)
                    .await?
            } else {
                return Err(plugin_validation_error(request.meta.request_id.clone()));
            };
            if !self.broker_action_current(request) {
                return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
            }
            let instance = self.active_instance(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                request.instance_generation,
            )?;
            let target = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .target_contexts
                .get(request.context_handle.as_str())
                .map(|target| target.context.clone())
                .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
            let terminal_metadata = self
                .terminal_metadata_for_action(request, &target, installed)
                .await?;
            let id = Uuid::new_v4().to_string();
            let callback_request = PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: instance.protocol_minor,
                request_id: id.clone(),
                kind: PluginHostMessageKind::BrokerResult,
                payload_json: plugin_host_payload_with_settings(
                    &instance.locale,
                    serde_json::json!({
                        "targetId": request.target_id.as_str(), "contextHandle": request.context_handle.as_str(),
                        "targetRevision": request.expected_target_revision.get().to_string(), "actionId": request.action_id.as_str(),
                        "fields": fields, "terminalMetadata": terminal_metadata, "nowUnixMs": unix_time_ms(),
                        "result": result, "storage": storage, "state": {"valueJson": state},
                    }),
                    settings.as_ref(),
                ),
            };
            let bytes = serde_json::to_vec(&callback_request)
                .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?
                .len();
            chain
                .account_callback(bytes)
                .map_err(|error| broker_chain_error(request.meta.request_id.clone(), error))?;
            let outputs = tokio::time::timeout(
                chain
                    .remaining()
                    .map_err(|error| broker_chain_error(request.meta.request_id.clone(), error))?,
                self.execute_instance(request.meta.request_id.clone(), instance, callback_request),
            )
            .await
            .map_err(|_| broker_chain_outcome_unknown(request.meta.request_id.clone()))??;
            let output_bytes = outputs
                .iter()
                .try_fold(0usize, |total, output| {
                    total.checked_add(output.payload_json.len())
                })
                .ok_or_else(|| {
                    broker_chain_error(
                        request.meta.request_id.clone(),
                        super::broker_chain::ChainError::Limit,
                    )
                })?;
            chain
                .account_callback(output_bytes)
                .map_err(|error| broker_chain_error(request.meta.request_id.clone(), error))?;
            let mut callback = parse_plugin_ui_outputs(&id, outputs).map_err(|_| {
                plugin_runtime_error(
                    request.meta.request_id.clone(),
                    Some(PluginHostProcessError::Rejected),
                )
            })?;
            if let Some(updated) = &callback.ui_state {
                state = updated.value_json.clone();
            }
            if callback.api_call.is_some() {
                // Intermediate replies are pure continuations. They cannot publish templates,
                // storage writes, navigation, launches, or legacy broker side effects.
                if !broker_chain_intermediate_is_pure(&callback) {
                    return Err(plugin_validation_error(request.meta.request_id.clone()));
                }
                if !self.broker_action_current(request) {
                    return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
                }
                current = callback;
                callback_id = Some(id);
                continue;
            }
            if !broker_chain_final_is_admissible(&callback, storage.is_some()) {
                return Err(plugin_runtime_error(
                    request.meta.request_id.clone(),
                    Some(PluginHostProcessError::Rejected),
                ));
            }
            if callback.ui_state.is_none() {
                callback.ui_state = current
                    .ui_state
                    .clone()
                    .or(Some(PluginUiState { value_json: state }));
            }
            callback.resource_navigation = navigation;
            callback.terminal_launch = terminal_launch;
            return Ok((callback, id));
        }
    }
}

fn broker_chain_error(
    request_id: RequestId,
    error: super::broker_chain::ChainError,
) -> Box<CoreApiError> {
    let (code, category, retry, message) = match error {
        super::broker_chain::ChainError::Limit => (
            "plugin.broker_chain_limit",
            ErrorCategory::Unavailable,
            RetryStrategy::Never,
            "plugins.remoteApproval.resultTooLarge",
        ),
        super::broker_chain::ChainError::TimedOut => (
            "plugin.broker_chain_timed_out",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::Reconcile,
            "plugins.remoteApproval.cleanupIncomplete",
        ),
    };
    plugin_error(request_id, code, category, retry, message, None)
}

fn broker_chain_outcome_unknown(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.broker_chain_outcome_unknown",
        ErrorCategory::NeedsReconciliation,
        RetryStrategy::Reconcile,
        "plugins.remoteApproval.cleanupIncomplete",
        None,
    )
}

fn broker_chain_intermediate_is_pure(value: &ParsedPluginUiOutputs) -> bool {
    value.panels.is_empty()
        && value.templates.is_empty()
        && value.clipboard_text.is_none()
        && value.navigation.is_empty()
        && value.pages.is_empty()
        && value.storage_write.is_none()
        && value.host_mutation.is_none()
        && value.host_session.is_none()
        && value.host_dom_operations.is_none()
        && value.terminal_input_suggestion.is_none()
        && value.isolated_surface.is_none()
        && value.ssh_sync_request.is_none()
        && value.remote_operation.is_none()
        && value.resource_operation.is_none()
        && value.resource_navigation.is_none()
        && value.terminal_launch.is_none()
}

fn broker_chain_final_is_admissible(
    value: &ParsedPluginUiOutputs,
    storage_available: bool,
) -> bool {
    value.panels.is_empty()
        && value.templates.len() == 1
        && value.navigation.is_empty()
        && value.pages.is_empty()
        && value.clipboard_text.is_none()
        && value.host_mutation.is_none()
        && value.host_session.is_none()
        && value.host_dom_operations.is_none()
        && value.terminal_input_suggestion.is_none()
        && value.isolated_surface.is_none()
        && value.ssh_sync_request.is_none()
        && value.remote_operation.is_none()
        && value.resource_operation.is_none()
        && value.api_call.is_none()
        && (value.storage_write.is_none() || storage_available)
}

struct SessionOperationBinding {
    installed: PluginInstalledRecord,
    instance_generation: WireSequence,
    lease: crate::ssh_session_service::SessionChannelLease,
    epoch: WireSequence,
    handoff_fence: crate::plugin_operations::OperationFence,
    fence: crate::plugin_operations::OperationFence,
    locale: norishell_core_api::PluginLocale,
    label: String,
}
impl SessionOperationBinding {
    fn remote_invocation(&self, reason: String) -> RemoteInvocation {
        let endpoint = &self.lease.endpoint;
        RemoteInvocation {
            plugin_id: self.installed.plugin_id.clone(),
            instance_generation: self.instance_generation,
            plugin_name: self.installed.name.clone(),
            locale: self.locale.clone(),
            lease: self.lease.clone(),
            host_label: self.label.clone(),
            endpoint: format!(
                "{}@{}:{}",
                endpoint.username.as_deref().unwrap_or(""),
                endpoint.address,
                endpoint.port
            ),
            reason,
            fence: self.fence.clone(),
            remembered_policy: None,
        }
    }
}

#[derive(Clone)]
struct SessionOperationAuthority {
    hosts: HostService,
    runtime: std::sync::Weak<Mutex<PluginRuntimeState>>,
    plugin_id: PluginId,
    signer: String,
    package: String,
    state_version: WireSequence,
    generation: WireSequence,
    context_handle: PluginTargetContextHandle,
    target_id: norishell_core_api::PluginExtensionTargetId,
    target_revision: WireSequence,
    require_context: bool,
    capability: PluginCapability,
    epoch: WireSequence,
    lease: crate::ssh_session_service::SessionChannelLease,
}
impl SessionOperationAuthority {
    fn current(&self) -> bool {
        if !self.lease.is_current() {
            return false;
        }
        let Some(runtime) = self.runtime.upgrade() else {
            return false;
        };
        let runtime = runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .mutation_reservations
            .contains_key(self.plugin_id.as_str())
            || (self.require_context
                && !runtime
                    .target_contexts
                    .get(self.context_handle.as_str())
                    .is_some_and(|context| {
                        context.context.target_id == self.target_id
                            && context.context.target_revision == self.target_revision
                    }))
            || !runtime
                .active_instances
                .get(self.plugin_id.as_str())
                .is_some_and(|instance| {
                    !instance.operation_authority_revoked
                        && instance.package_sha256 == self.package
                        && instance.signer_fingerprint_sha256 == self.signer
                        && instance.state_version == self.state_version
                        && instance.instance_generation == self.generation
                })
        {
            return false;
        }
        drop(runtime);
        self.hosts
            .with_plugin_repository(|repository| {
                let installed = repository.get_plugin_installation(&self.plugin_id)?;
                let grants = repository.list_plugin_capability_grants(
                    &self.plugin_id,
                    &self.signer,
                    u64::from(PLUGIN_PROTOCOL_MAJOR),
                )?;
                Ok(installed.state == PluginInstallState::Enabled
                    && installed.state_version == self.state_version
                    && installed.package_sha256 == self.package
                    && installed.signer_fingerprint_sha256 == self.signer
                    && [self.capability, PluginCapability::TerminalMetadata]
                        .iter()
                        .all(|required| {
                            installed.capabilities.contains(required)
                                && grants.iter().any(|grant| {
                                    grant.capability == *required
                                        && effective_plugin_grant(&installed, grant)
                                        && (*required != self.capability
                                            || grant.state_version == self.epoch)
                                })
                        }))
            })
            .unwrap_or(false)
    }
}
