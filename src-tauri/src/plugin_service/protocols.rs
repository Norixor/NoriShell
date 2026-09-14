//! Dispatch protocol events only within the private connection scope; release the process lock after Wasm before awaiting broker I/O.
use super::{api_invocation::ApiInvocation, *};
use crate::plugin_api::{ResourceConsumer, ResourceFence, ResourceOwner, ResourceRegistry};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiOperation, PluginNetworkOperation, PluginProtocolEvent,
    PluginProtocolEventKind, PluginProtocolOutput, PluginProtocolResource,
    parse_plugin_protocol_response, validate_plugin_protocol_event,
};

/// A Core-minted connection binding. Guest JSON cannot construct identity, consumer, or authority.
#[derive(Clone)]
pub(crate) struct ProtocolBinding {
    pub owner: ResourceOwner,
    pub consumer: ResourceConsumer,
    pub connection_handle: String,
    pub provider_id: String,
    pub resources: Vec<PluginProtocolResource>,
    pub authority: ResourceFence,
    pub transaction_authority: ResourceFence,
}

impl PluginService {
    pub(super) async fn notify_protocol_closed(&self, binding: &ProtocolBinding) {
        let request_id = RequestId::new();
        let Ok(instance) = self.active_instance(
            request_id.clone(),
            &binding.owner.plugin_id,
            &binding.owner.signer,
            binding.owner.generation,
        ) else {
            return;
        };
        let message = PluginProtocolEvent {
            connection_handle: binding.connection_handle.clone(),
            event_id: Uuid::new_v4().to_string(),
            event: PluginProtocolEventKind::Close {
                reason: norishell_core_api::PluginProtocolCloseReason::User,
            },
        };
        let Ok(payload_json) = serde_json::to_string(&message) else {
            return;
        };
        // Close cannot obtain new authority: all returned broker requests/output are discarded.
        let _ = self
            .execute_instance(
                request_id,
                instance,
                PluginHostRequest {
                    protocol_major: PLUGIN_PROTOCOL_MAJOR,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    request_id: Uuid::new_v4().to_string(),
                    kind: PluginHostMessageKind::ProtocolEvent,
                    payload_json,
                },
            )
            .await;
    }

    pub(crate) fn protocol_resources(&self, binding: &ProtocolBinding) -> ResourceRegistry {
        self.api.resources.for_consumer(binding.consumer.clone())
    }

    /// An event may have bounded broker continuations. Every send waits for the actual owner's acknowledgement;
    /// callers must also run the resource pump so output can reach the session actor while input is waiting.
    pub(crate) async fn execute_protocol_event(
        &self,
        binding: &ProtocolBinding,
        event: PluginProtocolEvent,
        opening_action: bool,
        output: &tokio::sync::mpsc::Sender<PluginProtocolOutput>,
    ) -> Result<(), PluginApiErrorCode> {
        validate_plugin_protocol_event(&event).map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        if event.connection_handle != binding.connection_handle || !(binding.authority)() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let _queue_slot = self
            .protocol_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| PluginApiErrorCode::Busy)?;
        let request_id = RequestId::new();
        let installed = self
            .has_capability(
                request_id.clone(),
                &binding.owner.plugin_id,
                &binding.owner.signer,
                PluginCapability::TerminalProvider,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        if installed.package_sha256 != binding.owner.package {
            return Err(PluginApiErrorCode::Revoked);
        }
        let invocation = ApiInvocation::Provider {
            request_id: request_id.clone(),
            owner: binding.owner.clone(),
            operation_id: format!("protocol:{}", binding.provider_id),
            authority: binding.authority.clone(),
            transaction_authority: binding.transaction_authority.clone(),
            consumer: binding.consumer.clone(),
            explicit_user_action: opening_action,
        };
        let mut current = event;
        let mut bytes = 0usize;
        let mut call_ids = BTreeSet::new();
        for _ in 0..16 {
            if !(binding.authority)() {
                return Err(PluginApiErrorCode::Revoked);
            }
            let instance = self
                .active_instance(
                    request_id.clone(),
                    &binding.owner.plugin_id,
                    &binding.owner.signer,
                    binding.owner.generation,
                )
                .map_err(|_| PluginApiErrorCode::Revoked)?;
            let request = PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: Uuid::new_v4().to_string(),
                kind: PluginHostMessageKind::ProtocolEvent,
                payload_json: serde_json::to_string(&current)
                    .map_err(|_| PluginApiErrorCode::InvalidRequest)?,
            };
            let outputs = self
                .execute_instance(request_id.clone(), instance, request)
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)?;
            if !(binding.authority)() {
                return Err(PluginApiErrorCode::Revoked);
            }
            if outputs.len() != 1 || outputs[0].kind != "protocol.response" {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            bytes = bytes
                .checked_add(outputs[0].payload_json.len())
                .filter(|bytes| *bytes <= 512 * 1024)
                .ok_or(PluginApiErrorCode::QuotaExceeded)?;
            let response = parse_plugin_protocol_response(outputs[0].payload_json.as_bytes())
                .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
            if response.connection_handle != binding.connection_handle
                || response.event_id != current.event_id
                || response.complete == response.call.is_some()
            {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            for value in response.outputs {
                if !(binding.authority)() {
                    return Err(PluginApiErrorCode::Revoked);
                }
                tokio::time::timeout(std::time::Duration::from_secs(2), output.send(value))
                    .await
                    .map_err(|_| PluginApiErrorCode::Busy)?
                    .map_err(|_| PluginApiErrorCode::Cancelled)?;
            }
            let Some(call) = response.call else {
                return Ok(());
            };
            if !call_ids.insert(call.call_id.clone())
                || (!opening_action
                    && matches!(
                        call.operation,
                        PluginApiOperation::NetworkStart { .. }
                            | PluginApiOperation::SerialOpen { .. }
                    ))
                || !protocol_call_allowed(binding, &call.operation)
            {
                return Err(PluginApiErrorCode::PermissionDenied);
            }
            let reply = self
                .invoke_api(
                    &installed,
                    ApiInvocation::Provider {
                        request_id: request_id.clone(),
                        owner: binding.owner.clone(),
                        operation_id: invocation.operation_identity().to_owned(),
                        authority: binding.authority.clone(),
                        transaction_authority: binding.transaction_authority.clone(),
                        consumer: binding.consumer.clone(),
                        explicit_user_action: opening_action,
                    },
                    &call,
                )
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)?;
            if matches!(
                call.operation,
                PluginApiOperation::NetworkSend { .. } | PluginApiOperation::SerialSend { .. }
            ) && let norishell_core_api::PluginApiOutcome::Failed { code } = &reply.outcome
            {
                return Err(*code);
            }
            current.event = PluginProtocolEventKind::ApiResult {
                call_id: call.call_id,
                reply,
            };
        }
        Err(PluginApiErrorCode::QuotaExceeded)
    }
}

fn protocol_call_allowed(binding: &ProtocolBinding, operation: &PluginApiOperation) -> bool {
    match operation {
        PluginApiOperation::NetworkStart { request, .. } => match request.operation {
            PluginNetworkOperation::Tcp {} => {
                binding.resources.contains(&PluginProtocolResource::Tcp)
            }
            PluginNetworkOperation::Tls {} => {
                binding.resources.contains(&PluginProtocolResource::Tls)
            }
            PluginNetworkOperation::WebSocket { .. } => binding
                .resources
                .contains(&PluginProtocolResource::Websocket),
            _ => false,
        },
        PluginApiOperation::SerialDevices { .. }
        | PluginApiOperation::SerialOpen { .. }
        | PluginApiOperation::SerialSend { .. } => {
            binding.resources.contains(&PluginProtocolResource::Serial)
        }
        PluginApiOperation::NetworkSend { .. }
        | PluginApiOperation::ResourceClose { .. }
        | PluginApiOperation::ResourcesList {}
        | PluginApiOperation::Describe {} => true,
        _ => false,
    }
}
