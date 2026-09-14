//! Exact user input proposals and final ownership checks.

use super::*;

impl PluginService {
    pub(super) async fn queue_runtime_input(
        &self,
        request_id: RequestId,
        plugin_id: PluginId,
        signer: String,
        instance_generation: WireSequence,
        output: norishell_core_api::PluginRuntimeOutput,
    ) -> CoreResult<()> {
        if output.kind != "terminal.requestInput" {
            return Err(plugin_validation_error(request_id));
        }
        let request: RuntimeTerminalInputRequest = serde_json::from_str(&output.payload_json)
            .map_err(|_| plugin_validation_error(request_id.clone()))?;
        let focus = self
            .sessions
            .terminal_focus_snapshot_internal(request_id.clone())
            .await?;
        let pending = self
            .prepare_input_proposal(
                request_id.clone(),
                plugin_id,
                signer,
                instance_generation,
                request,
                focus,
            )
            .await?;
        self.enqueue_input_proposal(request_id, pending)
    }

    pub(super) async fn prepare_input_proposal(
        &self,
        request_id: RequestId,
        plugin_id: PluginId,
        signer: String,
        instance_generation: WireSequence,
        request: RuntimeTerminalInputRequest,
        focus: norishell_core_api::TerminalInputFocusSnapshot,
    ) -> CoreResult<PendingPluginInput> {
        let mut bytes = request.payload.as_bytes().to_vec();
        if bytes.is_empty()
            || bytes.len() > PLUGIN_INPUT_MAX_BYTES
            || request
                .payload
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\t' | '\n'))
        {
            return Err(plugin_validation_error(request_id));
        }
        if request.append_enter {
            bytes.push(b'\r');
        }
        if bytes.len() > PLUGIN_INPUT_MAX_BYTES {
            return Err(plugin_validation_error(request_id));
        }
        let installed = self.has_capability(
            request_id.clone(),
            &plugin_id,
            &signer,
            PluginCapability::TerminalRequestInput,
        )?;
        self.ensure_instance(&plugin_id, &signer, instance_generation, request_id.clone())?;
        let (target, lease) = match (focus.target, focus.lease) {
            (Some(TerminalInputFocusTarget::Ssh(target)), Some(TerminalInputLease::Ssh(lease)))
                if target.session_id == lease.session_id
                    && target.expected_generation == lease.generation
                    && target.attachment_id == lease.attachment_id
                    && target.view_id == lease.view_id
                    && focus.focus_epoch == lease.focus_epoch =>
            {
                (target, lease)
            }
            _ => return Err(plugin_conflict_error(request_id, None)),
        };
        let now = unix_time_ms();
        let expires_at_unix_ms = now
            .saturating_add(PLUGIN_INPUT_APPROVAL_MILLIS)
            .min(lease.expires_at_unix_ms);
        if expires_at_unix_ms <= now {
            return Err(plugin_conflict_error(request_id, None));
        }
        let approval_id = PluginInputApprovalId::new();
        let session = self
            .sessions
            .snapshot(request_id.clone())
            .await?
            .sessions
            .into_iter()
            .find(|session| session.session_id == target.session_id)
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        let endpoint = session
            .endpoint
            .as_ref()
            .map(|endpoint| {
                format!(
                    "{}{}:{}",
                    endpoint
                        .username
                        .as_ref()
                        .map(|username| format!("{username}@"))
                        .unwrap_or_default(),
                    endpoint.address,
                    endpoint.port
                )
            })
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        let host_label = match session.target {
            SshSessionTarget::Host { host_id, .. } => self
                .hosts
                .with_plugin_repository(|repository| repository.get_host(&host_id))
                .ok()
                .map(|host| host.label),
            SshSessionTarget::QuickConnect { .. } => None,
        };
        let proposal = PluginTerminalInputProposal {
            approval_id: approval_id.clone(),
            plugin_id,
            plugin_name: installed.name,
            publisher: installed.publisher,
            signer_fingerprint_sha256: signer,
            package_sha256: installed.package_sha256,
            instance_generation,
            host_label,
            endpoint,
            session_id: target.session_id,
            generation: target.expected_generation,
            channel_id: target.channel_id,
            attachment_id: target.attachment_id,
            view_id: target.view_id,
            focus_epoch: focus.focus_epoch,
            input_epoch: lease.input_epoch,
            payload: request.payload,
            append_enter: request.append_enter,
            payload_sha256: hex::encode(Sha256::digest(&bytes)),
            expires_at_unix_ms,
            state_version: WireSequence::new(1),
            remember_policy: norishell_core_api::PluginRememberPolicy::Unavailable,
        };
        Ok(PendingPluginInput {
            proposal,
            lease_id: lease.lease_id,
            bytes,
            remembered_policy: None,
            authority: None,
        })
    }

    pub(super) fn enqueue_input_proposal(
        &self,
        request_id: RequestId,
        pending: PendingPluginInput,
    ) -> CoreResult<()> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime
            .pending_inputs
            .retain(|_, pending| pending.proposal.expires_at_unix_ms > unix_time_ms());
        if runtime.pending_inputs.len() >= 64 {
            return Err(plugin_error(
                request_id,
                "plugin.input_queue_full",
                ErrorCategory::Unavailable,
                RetryStrategy::AfterMilliseconds(1_000),
                "errors.plugin.inputQueueFull",
                None,
            ));
        }
        runtime
            .pending_inputs
            .insert(pending.proposal.approval_id.as_str().to_owned(), pending);
        Ok(())
    }

    pub(super) async fn decide_terminal_input(
        &self,
        request: PluginTerminalInputDecisionRequest,
    ) -> CoreResult<PluginTerminalInputDecisionResponse> {
        let pending = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(pending) = runtime.pending_inputs.get(request.approval_id.as_str()) else {
                return Err(plugin_not_found_error(request.meta.request_id));
            };
            if pending.proposal.state_version != request.expected_state_version {
                return Err(plugin_conflict_error(
                    request.meta.request_id,
                    Some(pending.proposal.state_version),
                ));
            }
            runtime
                .pending_inputs
                .remove(request.approval_id.as_str())
                .expect("pending approval was checked")
        };
        let next_state = WireSequence::new(pending.proposal.state_version.get().saturating_add(1));
        if request.decision == PluginTerminalInputDecision::Reject {
            return Ok(PluginTerminalInputDecisionResponse {
                approval_id: pending.proposal.approval_id,
                decision: request.decision,
                state_version: next_state,
                consumed: true,
            });
        }
        self.validate_pending_input(&request.meta.request_id, &pending)?;
        let focus = self
            .sessions
            .terminal_focus_snapshot_internal(request.meta.request_id.clone())
            .await?;
        if !input_focus_matches(&pending, &focus) {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let approved = match request.policy {
            norishell_core_api::PluginApprovalPolicy::Once => None,
            norishell_core_api::PluginApprovalPolicy::Always => Some(
                pending
                    .remembered_policy
                    .as_ref()
                    .ok_or_else(|| plugin_permission_error(request.meta.request_id.clone()))?
                    .remember(request.expiry)
                    .map_err(|_| plugin_conflict_error(request.meta.request_id.clone(), None))?,
            ),
        };
        let approval_id = pending.proposal.approval_id.clone();
        self.send_pending_input(request.meta.request_id, pending, approved)
            .await?;
        Ok(PluginTerminalInputDecisionResponse {
            approval_id,
            decision: request.decision,
            state_version: next_state,
            consumed: true,
        })
    }

    fn validate_pending_input(
        &self,
        request_id: &RequestId,
        pending: &PendingPluginInput,
    ) -> CoreResult<()> {
        if !pending.authority.as_ref().is_none_or(|fence| fence())
            || pending.proposal.expires_at_unix_ms <= unix_time_ms()
            || hex::encode(Sha256::digest(&pending.bytes)) != pending.proposal.payload_sha256
        {
            return Err(plugin_conflict_error(request_id.clone(), None));
        }
        self.ensure_instance(
            &pending.proposal.plugin_id,
            &pending.proposal.signer_fingerprint_sha256,
            pending.proposal.instance_generation,
            request_id.clone(),
        )?;
        self.has_capability(
            request_id.clone(),
            &pending.proposal.plugin_id,
            &pending.proposal.signer_fingerprint_sha256,
            PluginCapability::TerminalRequestInput,
        )?;
        Ok(())
    }

    pub(super) async fn send_pending_input(
        &self,
        request_id: RequestId,
        pending: PendingPluginInput,
        approved: Option<crate::plugin_operation_policy::ApprovedOperationPolicy>,
    ) -> CoreResult<()> {
        self.validate_pending_input(&request_id, &pending)?;
        let authority = pending.authority;
        let approval_fence: crate::plugin_operations::OperationFence = Arc::new(move || {
            authority.as_ref().is_none_or(|fence| fence())
                && approved
                    .as_ref()
                    .is_none_or(|policy| policy.admit_dispatch())
        });
        self.sessions
            .send_approved_plugin_input(ApprovedPluginInput {
                request_id,
                session_id: pending.proposal.session_id,
                expected_generation: pending.proposal.generation,
                channel_id: pending.proposal.channel_id,
                attachment_id: pending.proposal.attachment_id,
                view_id: pending.proposal.view_id,
                focus_epoch: pending.proposal.focus_epoch,
                lease_id: pending.lease_id,
                input_epoch: pending.proposal.input_epoch,
                bytes: pending.bytes,
                approval_fence: Some(approval_fence),
            })
            .await
    }

    pub(super) fn terminal_input_snapshot(
        &self,
        request: PluginTerminalInputGetRequest,
    ) -> CoreResult<PluginTerminalInputProposal> {
        let now = unix_time_ms();
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime
            .pending_inputs
            .retain(|_, pending| pending.proposal.expires_at_unix_ms > now);
        runtime
            .pending_inputs
            .get(request.approval_id.as_str())
            .map(|pending| pending.proposal.clone())
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id))
    }
    pub(super) fn open_input_proposal(
        &self,
        request: PluginTerminalInputOpenRequest,
        app: &AppHandle,
    ) -> CoreResult<()> {
        let proposal = self.terminal_input_snapshot(PluginTerminalInputGetRequest {
            meta: RequestMeta {
                request_id: request.meta.request_id.clone(),
            },
            approval_id: request.approval_id.clone(),
        })?;
        if proposal.state_version != request.expected_state_version {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(proposal.state_version),
            ));
        }
        crate::secure_window_frame::apply_secure_window_frame(WebviewWindowBuilder::new(
            app,
            secure_terminal_input_window_label(&proposal.approval_id),
            WebviewUrl::App(
                format!(
                    "secure-plugin-terminal-input.html?approvalId={}",
                    proposal.approval_id.as_str()
                )
                .into(),
            ),
        ))
        .title("NoriShell")
        .resizable(true)
        .center()
        .build()
        .map_err(|_| plugin_runtime_error(request.meta.request_id, None))?;
        Ok(())
    }
}

fn input_focus_matches(
    pending: &PendingPluginInput,
    focus: &norishell_core_api::TerminalInputFocusSnapshot,
) -> bool {
    let proposal = &pending.proposal;
    matches!((&focus.target, &focus.lease),
        (Some(TerminalInputFocusTarget::Ssh(target)), Some(TerminalInputLease::Ssh(lease)))
        if focus.focus_epoch == proposal.focus_epoch
            && target.session_id == proposal.session_id
            && target.expected_generation == proposal.generation
            && target.channel_id == proposal.channel_id
            && target.attachment_id == proposal.attachment_id
            && target.view_id == proposal.view_id
            && lease.lease_id == pending.lease_id && lease.input_epoch == proposal.input_epoch
            && lease.expires_at_unix_ms > unix_time_ms())
}
