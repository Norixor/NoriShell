//! Main-window terminal IPC adapter between the session actor and reconstructible workspace projections.
use super::{protocol_driver::ProtocolDriverFactory, protocols::ProtocolBinding, *};
use crate::plugin_terminal_session_service as actor;
use norishell_core_api as wire;
use std::sync::atomic::Ordering;
use tauri::ipc::Channel;

#[derive(Clone)]
pub(super) struct ProtocolSessionContext {
    operation_id: Uuid,
    idempotency_key: String,
    initial_rows: u16,
    initial_cols: u16,
    pub(super) profile: wire::PluginTerminalProfile,
    tab_id: String,
    pane_id: String,
    label: String,
    provider: wire::PluginProtocolProvider,
    configuration: wire::PluginSettingsValues,
    binding: ProtocolBinding,
}

impl PluginService {
    pub(crate) fn bind_protocol_terminals(&self, service: actor::PluginTerminalSessionService) {
        *self
            .protocol_terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(service);
    }
    pub(super) fn protocol_actor(&self) -> CoreResult<actor::PluginTerminalSessionService> {
        self.protocol_terminal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| plugin_runtime_error(RequestId::new(), None))
    }
    fn protocol_context(&self, session_id: Uuid) -> CoreResult<ProtocolSessionContext> {
        self.protocol_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&session_id)
            .cloned()
            .ok_or_else(|| plugin_not_found_error(RequestId::new()))
    }
    pub(super) fn protocol_projection(
        &self,
        summary: &actor::PluginTerminalSessionSummary,
    ) -> CoreResult<wire::PluginTerminalSummary> {
        let context = self.protocol_context(summary.session_id)?;
        Ok(project_summary(summary, &context))
    }
    fn protocol_factory(
        &self,
        context: &ProtocolSessionContext,
    ) -> Arc<dyn actor::PluginTerminalDriverFactory> {
        Arc::new(ProtocolDriverFactory {
            service: self.clone(),
            binding: context.binding.clone(),
            configuration: context.configuration.clone(),
        })
    }
    pub(super) async fn claim_protocol_launch(
        &self,
        request: wire::PluginProtocolLaunchRequest,
    ) -> CoreResult<wire::PluginTerminalOpenResponse> {
        let _gate = self.protocol_launch_gate.lock().await;
        let record = self
            .protocol_launches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&request.launch_id)
            .cloned()
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        let service = self.protocol_actor()?;
        if let Some(session_id) = record.session_id {
            let session = service
                .get(session_id)
                .await
                .map_err(|error| terminal_error(request.meta.request_id.clone(), error))?;
            return Ok(wire::PluginTerminalOpenResponse {
                session: self.protocol_projection(&session)?,
            });
        }
        if record.summary.revision != request.expected_revision
            || record.summary.expires_at_unix_ms <= unix_time_ms()
            || !(record.binding.authority)()
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let context = ProtocolSessionContext {
            operation_id: uuid(&request.launch_id, &request.meta.request_id)?,
            idempotency_key: format!("protocol-launch-{}", request.launch_id),
            initial_rows: 24,
            initial_cols: 80,
            profile: wire::PluginTerminalProfile {
                plugin_id: record.summary.plugin_id.clone(),
                provider_id: record.summary.provider_id.clone(),
                schema_hash: record.schema_hash,
                configuration: record
                    .configuration
                    .iter()
                    .map(|(key, value)| {
                        serde_json::to_value(value).map(|value| (key.clone(), value))
                    })
                    .collect::<Result<_, _>>()
                    .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?,
            },
            tab_id: record.summary.tab_id.clone(),
            pane_id: record.summary.pane_id.clone(),
            label: record.summary.label,
            provider: record.provider,
            configuration: record.configuration,
            binding: record.binding,
        };
        let operation = uuid(&request.launch_id, &request.meta.request_id)?;
        let result = self
            .open_protocol_context(
                operation,
                format!("protocol-launch-{operation}"),
                context,
                24,
                80,
                request.meta.request_id.clone(),
            )
            .await?;
        if let Some(record) = self
            .protocol_launches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&request.launch_id)
        {
            record.summary.claimed = true;
            record.summary.revision = WireSequence::new(2);
            record.session_id = Some(uuid(&result.session.session_id, &request.meta.request_id)?);
        }
        Ok(result)
    }
    async fn open_protocol_context(
        &self,
        operation: Uuid,
        idempotency_key: String,
        context: ProtocolSessionContext,
        rows: u16,
        cols: u16,
        request_id: RequestId,
    ) -> CoreResult<wire::PluginTerminalOpenResponse> {
        let _permit = self
            .api_creation_permit(request_id.clone())
            .map_err(|_| plugin_conflict_error(request_id.clone(), None))?;
        if !(context.binding.authority)() {
            return Err(plugin_conflict_error(request_id, None));
        }
        let service = self.protocol_actor()?;
        let claim = actor::PluginTerminalFrontendLaunchClaim::new(operation)
            .ok_or_else(|| plugin_validation_error(request_id.clone()))?;
        let factory = self.protocol_factory(&context);
        let opened = service
            .open_after_frontend_launch(
                claim,
                actor::PluginTerminalOpenRequest {
                    operation_id: operation,
                    idempotency_key,
                    connection_handle: actor::PluginTerminalConnectionHandle::new(uuid(
                        &context.binding.connection_handle,
                        &request_id,
                    )?)
                    .ok_or_else(|| plugin_validation_error(request_id.clone()))?,
                    provider: actor::PluginTerminalProviderMetadata {
                        plugin_id: context.profile.plugin_id.to_string(),
                        provider_id: context.profile.provider_id.clone(),
                        display_label: context.label.clone(),
                    },
                    attach_attempt_id: operation,
                    view_id: context.pane_id.clone(),
                    rows,
                    cols,
                },
                factory,
            )
            .await
            .map_err(|error| terminal_error(request_id, error))?;
        let summary = project_summary(&opened.response.session, &context);
        self.protocol_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(opened.response.session.session_id)
            .or_insert(context);
        self.protocol_snapshot_revision
            .fetch_add(1, Ordering::AcqRel);
        spawn_focus_coordinator(
            self.sessions.clone(),
            service,
            opened.response.session.session_id,
            opened.events,
            self.protocol_snapshot_revision.clone(),
        );
        Ok(wire::PluginTerminalOpenResponse { session: summary })
    }

    async fn open_protocol_profile(
        &self,
        request: wire::PluginTerminalOpenRequest,
    ) -> CoreResult<wire::PluginTerminalOpenResponse> {
        let _gate = self.protocol_launch_gate.lock().await;
        let operation = uuid(&request.operation_id, &request.meta.request_id)?;
        let existing: Vec<_> = {
            self.protocol_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter(|(_, context)| {
                    context.operation_id == operation
                        || (context.tab_id == request.tab_id && context.pane_id == request.pane_id)
                })
                .map(|(id, context)| (*id, context.clone()))
                .collect()
        };
        for (id, context) in existing {
            if context.operation_id == operation {
                if context.idempotency_key != request.idempotency_key
                    || context.initial_rows != request.rows
                    || context.initial_cols != request.cols
                    || context.profile.plugin_id != request.plugin_id
                    || context.profile.provider_id != request.provider_id
                    || context.profile.schema_hash != request.schema_hash
                    || context.profile.configuration != request.configuration
                    || context.label != request.label
                    || context.tab_id != request.tab_id
                    || context.pane_id != request.pane_id
                {
                    return Err(plugin_conflict_error(request.meta.request_id, None));
                }
                let session = self
                    .protocol_actor()?
                    .get(id)
                    .await
                    .map_err(|error| terminal_error(request.meta.request_id.clone(), error))?;
                return Ok(wire::PluginTerminalOpenResponse {
                    session: project_summary(&session, &context),
                });
            }
            if let Ok(session) = self.protocol_actor()?.get(id).await
                && (session.cleanup_blocked
                    || !matches!(
                        session.state,
                        actor::PluginTerminalSessionState::Closed
                            | actor::PluginTerminalSessionState::Failed
                    ))
            {
                return Err(plugin_conflict_error(request.meta.request_id, None));
            }
        }
        let installed = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&request.plugin_id)
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let instance = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get(request.plugin_id.as_str())
            .cloned()
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &installed.signer_fingerprint_sha256,
            PluginCapability::TerminalProvider,
        )?;
        let catalog = self
            .installer
            .read_protocol_catalog(
                &request.plugin_id,
                &installed.active_version,
                &installed.package_sha256,
            )
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        if catalog.catalog_sha256 != request.schema_hash {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let provider = catalog
            .catalog
            .providers
            .into_iter()
            .find(|provider| provider.id == request.provider_id)
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        let configuration: wire::PluginSettingsValues =
            serde_json::from_value(serde_json::json!(request.configuration))
                .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        norishell_plugin_platform::validate_settings_values(
            &provider.configuration,
            &configuration,
        )
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        if request.label.is_empty()
            || request.label.len() > 256
            || request.label.chars().any(char::is_control)
            || Uuid::parse_str(&request.tab_id).is_err()
            || Uuid::parse_str(&request.pane_id).is_err()
        {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let owner = crate::plugin_api::ResourceOwner {
            plugin_id: installed.plugin_id.clone(),
            signer: installed.signer_fingerprint_sha256.clone(),
            package: installed.package_sha256.clone(),
            generation: instance.instance_generation,
        };
        let epoch = self.capability_grant_epoch_for_record(
            request.meta.request_id.clone(),
            &installed,
            PluginCapability::TerminalProvider,
        )?;
        let binding = ProtocolBinding {
            owner: owner.clone(),
            consumer: crate::plugin_api::ResourceConsumer {
                connection: Uuid::new_v4(),
                generation: WireSequence::new(1),
                stream: Uuid::new_v4(),
            },
            connection_handle: Uuid::new_v4().to_string(),
            provider_id: provider.id.clone(),
            resources: provider.resources.clone(),
            authority: self.api_resource_fence(
                owner.clone(),
                Some((PluginCapability::TerminalProvider, epoch)),
            ),
            transaction_authority: self.api_runtime_fence(owner),
        };
        let context = ProtocolSessionContext {
            operation_id: operation,
            idempotency_key: request.idempotency_key.clone(),
            initial_rows: request.rows,
            initial_cols: request.cols,
            profile: wire::PluginTerminalProfile {
                plugin_id: request.plugin_id,
                provider_id: request.provider_id,
                schema_hash: request.schema_hash,
                configuration: request.configuration,
            },
            tab_id: request.tab_id,
            pane_id: request.pane_id,
            label: request.label,
            provider,
            configuration,
            binding,
        };
        self.open_protocol_context(
            operation,
            request.idempotency_key,
            context,
            request.rows,
            request.cols,
            request.meta.request_id,
        )
        .await
    }

    pub(super) async fn stop_protocol_sessions(
        &self,
        plugin_id: &PluginId,
        generation: Option<WireSequence>,
    ) -> bool {
        self.protocol_launches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, record| {
                record.summary.plugin_id != *plugin_id
                    || generation
                        .is_some_and(|generation| record.binding.owner.generation != generation)
            });
        let Ok(service) = self.protocol_actor() else {
            return true;
        };
        let mut clean = true;
        for session in service.snapshot().await {
            if session.provider.plugin_id != plugin_id.as_str() {
                continue;
            }
            if generation.is_some_and(|generation| {
                self.protocol_context(session.session_id)
                    .is_ok_and(|context| context.binding.owner.generation != generation)
            }) {
                continue;
            }
            let result = if session.cleanup_blocked {
                service
                    .retry_cleanup(actor::PluginTerminalCleanupRetryRequest {
                        session_id: session.session_id,
                        expected_generation: session.generation,
                        expected_stream_id: session.stream_id,
                        expected_state_revision: session.state_revision,
                    })
                    .await
            } else if matches!(
                session.state,
                actor::PluginTerminalSessionState::Closed
                    | actor::PluginTerminalSessionState::Failed
            ) {
                continue;
            } else {
                service
                    .disconnect(actor::PluginTerminalDisconnectRequest {
                        operation_id: Uuid::new_v4(),
                        idempotency_key: Uuid::new_v4().to_string(),
                        session_id: session.session_id,
                        expected_generation: session.generation,
                        expected_stream_id: session.stream_id,
                        expected_state_revision: session.state_revision,
                    })
                    .await
            };
            clean &= result.is_ok();
        }
        clean
    }
}

fn project_summary(
    summary: &actor::PluginTerminalSessionSummary,
    context: &ProtocolSessionContext,
) -> wire::PluginTerminalSummary {
    wire::PluginTerminalSummary {
        plugin_id: context.profile.plugin_id.clone(),
        provider_id: context.profile.provider_id.clone(),
        schema_hash: context.profile.schema_hash.clone(),
        configuration: context.profile.configuration.clone(),
        session_id: summary.session_id.to_string(),
        tab_id: context.tab_id.clone(),
        pane_id: context.pane_id.clone(),
        label: context.label.clone(),
        generation: WireSequence::new(summary.generation),
        state_revision: WireSequence::new(summary.state_revision),
        attachment_revision: WireSequence::new(summary.attachment_revision),
        event_seq: WireSequence::new(summary.event_sequence),
        stream_id: summary.stream_id.to_string(),
        attachment_count: summary.attachment_count,
        cleanup_blocked: summary.cleanup_blocked,
        state: match summary.state {
            actor::PluginTerminalSessionState::Connecting => {
                wire::PluginTerminalSessionState::Connecting
            }
            actor::PluginTerminalSessionState::Running => wire::PluginTerminalSessionState::Running,
            actor::PluginTerminalSessionState::Closing => {
                wire::PluginTerminalSessionState::Disconnecting
            }
            actor::PluginTerminalSessionState::Closed => wire::PluginTerminalSessionState::Closed,
            actor::PluginTerminalSessionState::Failed => wire::PluginTerminalSessionState::Failed,
        },
        failure_reason: summary
            .failure_reason
            .as_ref()
            .map(|reason| wire::PluginTerminalFailure {
                code: format!("{:?}", reason.code),
                message_key: "pluginTerminal.failureFallback".to_owned(),
            }),
    }
}
fn project_attachment(value: &actor::PluginTerminalAttachment) -> wire::PluginTerminalAttachment {
    wire::PluginTerminalAttachment {
        attachment_id: value.attachment_id.to_string(),
        session_id: value.session_id.to_string(),
        generation: WireSequence::new(value.generation),
        stream_id: value.stream_id.to_string(),
        view_id: value.view_id.clone(),
        state_revision: WireSequence::new(value.state_revision),
        attachment_revision: WireSequence::new(value.attachment_revision),
    }
}
pub(crate) fn project_lease(
    value: &actor::PluginTerminalInputLease,
) -> wire::PluginTerminalInputLease {
    wire::PluginTerminalInputLease {
        lease_id: value.lease_id.to_string(),
        session_id: value.session_id.to_string(),
        generation: WireSequence::new(value.generation),
        stream_id: value.stream_id.to_string(),
        attachment_id: value.attachment_id.to_string(),
        view_id: value.view_id.clone(),
        focus_epoch: WireSequence::new(value.focus_epoch),
        input_epoch: WireSequence::new(value.input_epoch),
        expires_at_unix_ms: value.expires_at_unix_ms,
    }
}
fn project_frame(
    session_id: Uuid,
    frame: &actor::PluginTerminalOutputFrame,
) -> wire::PluginTerminalFrame {
    wire::PluginTerminalFrame {
        session_id: session_id.to_string(),
        generation: WireSequence::new(frame.generation),
        stream_id: frame.stream_id.to_string(),
        output_seq: WireSequence::new(frame.sequence),
        bytes: frame.data.clone(),
    }
}
fn project_replay(
    session_id: Uuid,
    item: &actor::PluginTerminalReplayItem,
) -> wire::PluginTerminalOutputItem {
    match item {
        actor::PluginTerminalReplayItem::Frame(frame) => {
            wire::PluginTerminalOutputItem::Frame(project_frame(session_id, frame))
        }
        actor::PluginTerminalReplayItem::Gap {
            generation,
            stream_id,
            dropped_through_sequence,
        } => wire::PluginTerminalOutputItem::Gap(wire::PluginTerminalGap {
            session_id: session_id.to_string(),
            generation: WireSequence::new(*generation),
            stream_id: stream_id.to_string(),
            dropped_from_output_seq: WireSequence::new(1),
            resumes_at_output_seq: WireSequence::new(dropped_through_sequence.saturating_add(1)),
            reason: "ringBufferLimit".to_owned(),
        }),
    }
}
fn event_summary(
    event: &actor::PluginTerminalSessionEvent,
) -> &actor::PluginTerminalSessionSummary {
    match event {
        actor::PluginTerminalSessionEvent::StateChanged { session }
        | actor::PluginTerminalSessionEvent::AttachmentChanged { session, .. }
        | actor::PluginTerminalSessionEvent::AttachmentDetached { session, .. }
        | actor::PluginTerminalSessionEvent::InputLeaseRevoked { session, .. }
        | actor::PluginTerminalSessionEvent::Output { session, .. } => session,
    }
}
fn project_event(
    event: &actor::PluginTerminalSessionEvent,
    context: &ProtocolSessionContext,
) -> wire::PluginTerminalEvent {
    let raw = event_summary(event);
    let session = project_summary(raw, context);
    let payload = match event {
        actor::PluginTerminalSessionEvent::StateChanged { .. } => {
            wire::PluginTerminalEventPayload::StateChanged {
                session: session.clone(),
            }
        }
        actor::PluginTerminalSessionEvent::AttachmentChanged { attachment, .. } => {
            wire::PluginTerminalEventPayload::AttachmentAttached {
                attachment: project_attachment(attachment),
            }
        }
        actor::PluginTerminalSessionEvent::AttachmentDetached { attachment_id, .. } => {
            wire::PluginTerminalEventPayload::AttachmentDetached {
                attachment_id: attachment_id.to_string(),
            }
        }
        actor::PluginTerminalSessionEvent::InputLeaseRevoked { focus_epoch, .. } => {
            wire::PluginTerminalEventPayload::InputLeaseRevoked {
                focus_epoch: WireSequence::new(*focus_epoch),
            }
        }
        actor::PluginTerminalSessionEvent::Output { frame, .. } => {
            wire::PluginTerminalEventPayload::Output {
                frame: project_frame(raw.session_id, frame),
            }
        }
    };
    wire::PluginTerminalEvent {
        session_id: session.session_id.clone(),
        generation: session.generation,
        state_revision: session.state_revision,
        event_seq: session.event_seq,
        session,
        payload,
    }
}

fn uuid(value: &str, request_id: &RequestId) -> CoreResult<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|value| !value.is_nil())
        .ok_or_else(|| plugin_validation_error(request_id.clone()))
}
fn terminal_error(
    request_id: RequestId,
    error: actor::PluginTerminalSessionError,
) -> Box<wire::CoreApiError> {
    use actor::PluginTerminalSessionError as E;
    let (code, category, retry) = match error {
        E::NotFound => (
            "plugin_terminal.not_found",
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::RefreshSnapshot,
        ),
        E::Validation => (
            "plugin_terminal.invalid_request",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        ),
        E::OutcomeUnknown => (
            "plugin_terminal.outcome_unknown",
            wire::ErrorCategory::Conflict,
            wire::RetryStrategy::RefreshSnapshot,
        ),
        E::CleanupIncomplete => (
            "plugin_terminal.cleanup_incomplete",
            wire::ErrorCategory::Conflict,
            wire::RetryStrategy::RefreshSnapshot,
        ),
        _ => (
            "plugin_terminal.stale_fence",
            wire::ErrorCategory::Conflict,
            wire::RetryStrategy::RefreshSnapshot,
        ),
    };
    crate::core_api_error::core_error(
        request_id,
        code,
        category,
        retry,
        "pluginTerminal.failureFallback",
    )
}

fn spawn_focus_coordinator(
    ssh: SshSessionService,
    terminal: actor::PluginTerminalSessionService,
    session_id: Uuid,
    mut events: tokio::sync::broadcast::Receiver<actor::PluginTerminalSessionEvent>,
    revision: Arc<std::sync::atomic::AtomicU64>,
) {
    tokio::spawn(async move {
        loop {
            let value = events.recv().await;
            revision.fetch_add(1, Ordering::AcqRel);
            let ended = value.as_ref().is_ok_and(|value| {
                matches!(
                    event_summary(value).state,
                    actor::PluginTerminalSessionState::Closed
                        | actor::PluginTerminalSessionState::Failed
                ) && !event_summary(value).cleanup_blocked
            });
            if matches!(value, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                return;
            }
            let broker = ssh.focus_broker();
            broker
                .linearize(async {
                    let Ok(focus) = ssh
                        .terminal_focus_snapshot_unserialized(RequestId::new())
                        .await
                    else {
                        return;
                    };
                    let Some(wire::TerminalInputFocusTarget::Plugin(target)) = focus.target else {
                        return;
                    };
                    if target.session_id != session_id.to_string() {
                        return;
                    }
                    let invalidate = match &value {
                        Err(_) => true,
                        Ok(actor::PluginTerminalSessionEvent::InputLeaseRevoked {
                            session,
                            focus_epoch,
                        }) => {
                            *focus_epoch == focus.focus_epoch.get()
                                && session.generation == target.expected_generation.get()
                                && session.stream_id.to_string() == target.stream_id
                        }
                        Ok(actor::PluginTerminalSessionEvent::AttachmentDetached {
                            session,
                            attachment_id,
                        }) => {
                            session.generation == target.expected_generation.get()
                                && attachment_id.to_string() == target.attachment_id
                        }
                        Ok(actor::PluginTerminalSessionEvent::StateChanged { session }) => {
                            session.generation != target.expected_generation.get()
                                || session.stream_id.to_string() != target.stream_id
                                || session.state != actor::PluginTerminalSessionState::Running
                        }
                        _ => false,
                    };
                    if invalidate {
                        let _ = ssh
                            .clear_plugin_focus_exact_unserialized(target, focus.focus_epoch.get())
                            .await;
                        let _ = terminal
                            .revoke_input(session_id, focus.focus_epoch.get())
                            .await;
                    }
                })
                .await;
            if ended {
                return;
            }
        }
    });
}

async fn verify_input_focus(
    service: &PluginService,
    fence: &actor::PluginTerminalInputFence,
    request_id: &RequestId,
) -> CoreResult<()> {
    let focus = service
        .sessions
        .terminal_focus_snapshot_unserialized(request_id.clone())
        .await?;
    let matches = focus.focus_epoch.get() == fence.focus_epoch
        && matches!(focus.target,
        Some(wire::TerminalInputFocusTarget::Plugin(target)) if target.session_id == fence.session_id.to_string()
            && target.expected_generation.get() == fence.expected_generation && target.expected_state_revision.get() == fence.expected_state_revision
            && target.stream_id == fence.expected_stream_id.to_string() && target.attachment_id == fence.attachment_id.to_string() && target.view_id == fence.view_id);
    if matches {
        Ok(())
    } else {
        Err(plugin_conflict_error(request_id.clone(), None))
    }
}
// The command boundary receives the individual wire identity facts before parsing them.
#[allow(clippy::too_many_arguments)]
fn input_fence(
    request_id: &RequestId,
    session_id: &str,
    generation: WireSequence,
    state_revision: WireSequence,
    stream: &str,
    attachment: &str,
    view_id: &str,
    lease: &str,
    focus_epoch: WireSequence,
    input_epoch: WireSequence,
) -> CoreResult<actor::PluginTerminalInputFence> {
    Ok(actor::PluginTerminalInputFence {
        session_id: uuid(session_id, request_id)?,
        expected_generation: generation.get(),
        expected_state_revision: state_revision.get(),
        expected_stream_id: uuid(stream, request_id)?,
        attachment_id: uuid(attachment, request_id)?,
        view_id: view_id.to_owned(),
        lease_id: uuid(lease, request_id)?,
        focus_epoch: focus_epoch.get(),
        input_epoch: input_epoch.get(),
    })
}

#[tauri::command]
pub(crate) fn plugin_protocol_launch_list<R: tauri::Runtime>(
    request: wire::PluginReadinessGetRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<wire::PluginProtocolLaunchSummary>> {
    require_main_plugin_management_window(&window, request.meta.request_id)?;
    Ok(service.pending_protocol_launches())
}
#[tauri::command]
pub(crate) async fn plugin_protocol_launch_claim<R: tauri::Runtime>(
    request: wire::PluginProtocolLaunchRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalOpenResponse> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    service.claim_protocol_launch(request).await
}
#[tauri::command]
pub(crate) async fn plugin_terminal_open<R: tauri::Runtime>(
    request: wire::PluginTerminalOpenRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalOpenResponse> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    service.open_protocol_profile(request).await
}
#[tauri::command]
pub(crate) async fn plugin_terminal_snapshot<R: tauri::Runtime>(
    request: wire::PluginReadinessGetRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalSnapshot> {
    require_main_plugin_management_window(&window, request.meta.request_id)?;
    let _gate = service.protocol_launch_gate.lock().await;
    let raw = service.protocol_actor()?.snapshot().await;
    let ids: BTreeSet<_> = raw.iter().map(|session| session.session_id).collect();
    service
        .protocol_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|id, _| ids.contains(id));
    let sessions = raw
        .iter()
        .map(|session| service.protocol_projection(session))
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(wire::PluginTerminalSnapshot {
        snapshot_revision: WireSequence::new(
            service.protocol_snapshot_revision.load(Ordering::Acquire),
        ),
        sessions,
    })
}
#[tauri::command]
pub(crate) async fn plugin_terminal_attach<R: tauri::Runtime>(
    request: wire::PluginTerminalAttachRequest,
    on_event: Channel<wire::PluginTerminalEvent>,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalAttachResponse> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let request_id = request.meta.request_id.clone();
    let actor = service.protocol_actor()?;
    let session_id = uuid(&request.session_id, &request_id)?;
    let context = service.protocol_context(session_id)?;
    let attached = service
        .sessions
        .focus_broker()
        .linearize(actor.attach(actor::PluginTerminalAttachRequest {
            operation_id: uuid(&request.operation_id, &request_id)?,
            idempotency_key: request.idempotency_key,
            session_id,
            expected_generation: request.expected_generation.get(),
            expected_stream_id: uuid(&request.stream_id, &request_id)?,
            expected_state_revision: request.expected_state_revision.get(),
            attach_attempt_id: uuid(&request.attach_attempt_id, &request_id)?,
            view_id: request.view_id.clone(),
            after_output_sequence: request.after_output_seq.map(WireSequence::get),
        }))
        .await
        .map_err(|error| terminal_error(request_id.clone(), error))?;
    let response = wire::PluginTerminalAttachResponse {
        session: project_summary(&attached.response.session, &context),
        attachment: project_attachment(&attached.response.attachment),
        replay: attached
            .response
            .replay
            .iter()
            .map(|item| project_replay(session_id, item))
            .collect(),
    };
    let mut events = attached.events;
    let attachment_id = attached.response.attachment.attachment_id;
    let ssh = service.sessions.clone();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(value) => {
                    if on_event.send(project_event(&value, &context)).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Force a fenced reattach/replay; no silent output loss after channel overrun.
                    let Ok(summary) = actor.get(session_id).await else {
                        return;
                    };
                    let session = project_summary(&summary, &context);
                    let gap = wire::PluginTerminalGap {
                        session_id: session_id.to_string(),
                        generation: session.generation,
                        stream_id: session.stream_id.clone(),
                        dropped_from_output_seq: WireSequence::new(0),
                        resumes_at_output_seq: WireSequence::new(0),
                        reason: "eventOverrun".to_owned(),
                    };
                    if on_event
                        .send(wire::PluginTerminalEvent {
                            session_id: session.session_id.clone(),
                            generation: session.generation,
                            state_revision: session.state_revision,
                            event_seq: session.event_seq,
                            payload: wire::PluginTerminalEventPayload::OutputGap { gap },
                            session,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
        if let Ok(session) = actor.get(session_id).await {
            let _ = ssh
                .focus_broker()
                .linearize(actor.detach(actor::PluginTerminalDetachRequest {
                    operation_id: Uuid::new_v4(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    session_id,
                    expected_generation: session.generation,
                    expected_stream_id: session.stream_id,
                    expected_state_revision: session.state_revision,
                    expected_attachment_revision: session.attachment_revision,
                    attachment_id,
                    view_id: request.view_id,
                    intent: actor::PluginTerminalDetachIntent::RendererUnavailable,
                    disconnect_if_last: false,
                }))
                .await;
        }
    });
    Ok(response)
}
#[tauri::command]
pub(crate) async fn plugin_terminal_attachment_heartbeat<R: tauri::Runtime>(
    request: wire::PluginTerminalAttachmentHeartbeatRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalAttachment> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let actor = service.protocol_actor()?;
    let id = uuid(&request.session_id, &request.meta.request_id)?;
    let session = actor
        .get(id)
        .await
        .map_err(|error| terminal_error(request.meta.request_id.clone(), error))?;
    actor
        .renew_attachment(actor::PluginTerminalAttachmentRenewRequest {
            session_id: id,
            expected_generation: request.expected_generation.get(),
            expected_stream_id: session.stream_id,
            expected_attachment_revision: request.expected_attachment_revision.get(),
            attachment_id: uuid(&request.attachment_id, &request.meta.request_id)?,
            view_id: request.view_id,
        })
        .await
        .map(|value| project_attachment(&value))
        .map_err(|error| terminal_error(request.meta.request_id, error))
}
#[tauri::command]
pub(crate) async fn plugin_terminal_detach<R: tauri::Runtime>(
    request: wire::PluginTerminalDetachRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalDetachResponse> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let actor = service.protocol_actor()?;
    let id = uuid(&request.session_id, &request.meta.request_id)?;
    let previous = actor
        .get(id)
        .await
        .map_err(|error| terminal_error(request.meta.request_id.clone(), error))?;
    let value = service
        .sessions
        .focus_broker()
        .linearize(actor.detach(actor::PluginTerminalDetachRequest {
            operation_id: uuid(&request.operation_id, &request.meta.request_id)?,
            idempotency_key: request.idempotency_key,
            session_id: id,
            expected_generation: request.expected_generation.get(),
            expected_stream_id: previous.stream_id,
            expected_state_revision: request.expected_state_revision.get(),
            expected_attachment_revision: request.expected_attachment_revision.get(),
            attachment_id: uuid(&request.attachment_id, &request.meta.request_id)?,
            view_id: request.view_id,
            intent: match request.intent {
                wire::PluginTerminalDetachIntent::UserClose => {
                    actor::PluginTerminalDetachIntent::UserClose
                }
                wire::PluginTerminalDetachIntent::RendererUnavailable => {
                    actor::PluginTerminalDetachIntent::RendererUnavailable
                }
            },
            disconnect_if_last: request.disconnect_if_last,
        }))
        .await
        .map_err(|error| terminal_error(request.meta.request_id, error))?;
    Ok(wire::PluginTerminalDetachResponse {
        remaining_attachment_count: value.attachment_count,
        session: service.protocol_projection(&value)?,
    })
}

#[tauri::command]
pub(crate) async fn plugin_terminal_input_lease_renew<R: tauri::Runtime>(
    request: wire::PluginTerminalLeaseRenewRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalInputLease> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let fence = input_fence(
        &request.meta.request_id,
        &request.session_id,
        request.expected_generation,
        request.expected_state_revision,
        &request.stream_id,
        &request.attachment_id,
        &request.view_id,
        &request.lease_id,
        request.focus_epoch,
        request.input_epoch,
    )?;
    let actor = service.protocol_actor()?;
    service
        .sessions
        .focus_broker()
        .linearize(async {
            verify_input_focus(&service, &fence, &request.meta.request_id).await?;
            let lease = actor
                .validate_fence(fence)
                .await
                .map_err(|error| terminal_error(request.meta.request_id.clone(), error))?;
            actor
                .renew_input(lease)
                .await
                .map(|value| project_lease(&value))
                .map_err(|error| terminal_error(request.meta.request_id, error))
        })
        .await
}
#[tauri::command]
pub(crate) async fn plugin_terminal_input<R: tauri::Runtime>(
    request: wire::PluginTerminalInputRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let fence = input_fence(
        &request.meta.request_id,
        &request.session_id,
        request.expected_generation,
        request.expected_state_revision,
        &request.stream_id,
        &request.attachment_id,
        &request.view_id,
        &request.lease_id,
        request.focus_epoch,
        request.input_epoch,
    )?;
    let actor = service.protocol_actor()?;
    service
        .sessions
        .focus_broker()
        .linearize(async {
            verify_input_focus(&service, &fence, &request.meta.request_id).await?;
            actor
                .input(actor::PluginTerminalInputRequest {
                    fence,
                    client_sequence: request.client_seq.get(),
                    data: request.bytes,
                })
                .await
                .map_err(|error| terminal_error(request.meta.request_id, error))
        })
        .await
}
#[tauri::command]
pub(crate) async fn plugin_terminal_resize<R: tauri::Runtime>(
    request: wire::PluginTerminalResizeRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let fence = input_fence(
        &request.meta.request_id,
        &request.session_id,
        request.expected_generation,
        request.expected_state_revision,
        &request.stream_id,
        &request.attachment_id,
        &request.view_id,
        &request.lease_id,
        request.focus_epoch,
        request.input_epoch,
    )?;
    let actor = service.protocol_actor()?;
    service
        .sessions
        .focus_broker()
        .linearize(async {
            verify_input_focus(&service, &fence, &request.meta.request_id).await?;
            actor
                .resize(actor::PluginTerminalResizeRequest {
                    fence,
                    resize_sequence: request.resize_seq.get(),
                    rows: request.rows,
                    cols: request.cols,
                })
                .await
                .map_err(|error| terminal_error(request.meta.request_id, error))
        })
        .await
}
#[tauri::command]
pub(crate) async fn plugin_terminal_disconnect<R: tauri::Runtime>(
    request: wire::PluginTerminalDisconnectRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalSummary> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let actor = service.protocol_actor()?;
    let id = uuid(&request.session_id, &request.meta.request_id)?;
    let previous = actor
        .get(id)
        .await
        .map_err(|error| terminal_error(request.meta.request_id.clone(), error))?;
    let value = service
        .sessions
        .focus_broker()
        .linearize(async {
            if previous.cleanup_blocked {
                actor
                    .retry_cleanup(actor::PluginTerminalCleanupRetryRequest {
                        session_id: id,
                        expected_generation: request.expected_generation.get(),
                        expected_stream_id: previous.stream_id,
                        expected_state_revision: request.expected_state_revision.get(),
                    })
                    .await
            } else {
                actor
                    .disconnect(actor::PluginTerminalDisconnectRequest {
                        operation_id: uuid(&request.operation_id, &request.meta.request_id)
                            .map_err(|_| actor::PluginTerminalSessionError::Validation)?,
                        idempotency_key: request.idempotency_key,
                        session_id: id,
                        expected_generation: request.expected_generation.get(),
                        expected_stream_id: previous.stream_id,
                        expected_state_revision: request.expected_state_revision.get(),
                    })
                    .await
            }
        })
        .await
        .map_err(|error| terminal_error(request.meta.request_id, error))?;
    service.protocol_projection(&value)
}
#[tauri::command]
pub(crate) async fn plugin_terminal_reconnect<R: tauri::Runtime>(
    request: wire::PluginTerminalReconnectRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<wire::PluginTerminalSummary> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    let _gate = service.protocol_launch_gate.lock().await;
    let _permit = service
        .api_creation_permit(request.meta.request_id.clone())
        .map_err(|_| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    let actor = service.protocol_actor()?;
    let id = uuid(&request.session_id, &request.meta.request_id)?;
    let previous = actor
        .get(id)
        .await
        .map_err(|error| terminal_error(request.meta.request_id.clone(), error))?;
    let context = service.protocol_context(id)?;
    if !context.provider.features.reconnect || !(context.binding.authority)() {
        return Err(plugin_permission_error(request.meta.request_id));
    }
    let operation = uuid(&request.operation_id, &request.meta.request_id)?;
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(
        format!("{}:{operation}", context.binding.connection_handle).as_bytes(),
    );
    let mut handle_bytes = [0u8; 16];
    handle_bytes.copy_from_slice(&digest[..16]);
    let connection_handle = uuid::Builder::from_random_bytes(handle_bytes).into_uuid();

    let claim = actor::PluginTerminalFrontendLaunchClaim::new(operation)
        .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
    let value =
        service
            .sessions
            .focus_broker()
            .linearize(actor.reconnect_after_frontend_launch(
                claim,
                actor::PluginTerminalReconnectRequest {
                    operation_id: operation,
                    idempotency_key: request.idempotency_key,
                    session_id: id,
                    expected_generation: request.expected_generation.get(),
                    expected_stream_id: previous.stream_id,
                    expected_state_revision: request.expected_state_revision.get(),
                    connection_handle:
                        actor::PluginTerminalConnectionHandle::new(connection_handle).ok_or_else(
                            || plugin_validation_error(request.meta.request_id.clone()),
                        )?,
                    provider: previous.provider,
                    rows: request.rows,
                    cols: request.cols,
                },
                service.protocol_factory(&context),
            ))
            .await
            .map_err(|error| terminal_error(request.meta.request_id, error))?;
    service
        .protocol_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, context.clone());
    spawn_focus_coordinator(
        service.sessions.clone(),
        actor.clone(),
        id,
        actor
            .subscribe(id)
            .await
            .map_err(|error| terminal_error(RequestId::new(), error))?,
        service.protocol_snapshot_revision.clone(),
    );
    Ok(project_summary(&value, &context))
}
