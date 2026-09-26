//! Application broker for plugin-owned resources; independent of UI document revisions.

pub(crate) mod blobs;
pub(crate) mod data_states;
pub(crate) mod files;
pub(crate) mod network;
pub(crate) mod process;
pub(crate) mod remote_exec;
mod resources;
pub(crate) mod serial;
pub(crate) mod sftp;
pub(crate) mod storage;
pub(crate) mod subscriptions;
mod timers;

use std::sync::Arc;

use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginApiAvailability, PluginApiCall,
    PluginApiDescription, PluginApiErrorCode, PluginApiLimits, PluginApiMethod, PluginApiOperation,
    PluginApiOutcome, PluginApiReply, PluginApiValue,
};

pub(crate) use resources::{
    ResourceCommand, ResourceCommandReceiver, ResourceConsumer, ResourceEventWriter, ResourceOwner,
    ResourceRegistry,
};

pub(crate) const MAX_CALL_BYTES: u32 = 64 * 1024;
pub(crate) const MAX_CHUNK_BYTES: u32 = 16 * 1024;
pub(crate) const MAX_PLUGIN_RESOURCES: u16 = 32;
pub(crate) const MAX_TOTAL_RESOURCES: usize = 128;
pub(crate) const MAX_PENDING_EVENTS: u16 = 32;
pub(crate) type ResourceFence = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub(crate) struct PluginApi {
    pub resources: ResourceRegistry,
    pub blobs: blobs::ExchangeBlobStore,
    pub data_states: data_states::DataStateRegistry,
    pub files: files::PluginFileService,
    pub subscriptions: subscriptions::SubscriptionSources,
}

impl Default for PluginApi {
    fn default() -> Self {
        let resources = ResourceRegistry::default();
        Self {
            blobs: blobs::ExchangeBlobStore::default(),
            data_states: data_states::DataStateRegistry::default(),
            files: files::PluginFileService::new(resources.clone()),
            subscriptions: subscriptions::SubscriptionSources::default(),
            resources,
        }
    }
}

impl PluginApi {
    pub async fn invoke(
        &self,
        owner: &ResourceOwner,
        call: &PluginApiCall,
        resource_fence: ResourceFence,
    ) -> PluginApiReply {
        let value = match &call.operation {
            PluginApiOperation::DataCatalog {} => Ok(PluginApiValue::DataCatalog {
                catalog: data_catalog(),
            }),
            PluginApiOperation::DataRead { .. }
            | PluginApiOperation::DataSnapshot { .. }
            | PluginApiOperation::DataInspect { .. }
            | PluginApiOperation::DataCompose { .. }
            | PluginApiOperation::DataReview { .. }
            | PluginApiOperation::DataApply { .. }
            | PluginApiOperation::DataExport { .. }
            | PluginApiOperation::DataCheckpoint { .. }
            | PluginApiOperation::DataRelease { .. } => Err(PluginApiErrorCode::PermissionDenied),
            PluginApiOperation::Describe {} => {
                Ok(PluginApiValue::Description { api: description() })
            }
            PluginApiOperation::Permissions {}
            | PluginApiOperation::PermissionRequest { .. }
            | PluginApiOperation::PermissionRevoke { .. }
            | PluginApiOperation::PermissionsForget {} => {
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::ResourcesList {} => Ok(PluginApiValue::Resources {
                resources: self.resources.list(owner),
            }),
            PluginApiOperation::ResourceClose { handle } => self
                .resources
                .close(owner, handle)
                .await
                .map(|()| PluginApiValue::Closed {
                    handle: handle.clone(),
                }),
            PluginApiOperation::TimerStart {
                delay_ms,
                interval_ms,
            } => timers::start_timer(
                &self.resources,
                owner.clone(),
                *delay_ms,
                *interval_ms,
                resource_fence,
            )
            .map(|handle| PluginApiValue::TimerStarted { handle }),
            PluginApiOperation::ResourceEvents {
                handle,
                limit,
                wait_ms,
            } => self
                .resources
                .take_events_wait(owner, handle, *limit, *wait_ms, &resource_fence)
                .await
                .map(|(events, backpressured)| PluginApiValue::ResourceEvents {
                    handle: handle.clone(),
                    events,
                    backpressured,
                }),
            PluginApiOperation::NetworkStart { .. } | PluginApiOperation::NetworkSend { .. } => {
                // The PluginService broker turns network starts into a frozen, protected approval
                // and routes later writes to an exact owner-scoped resource. This generic layer
                // must never make an unapproved network call as a fallback.
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::RemoteExecStart { .. }
            | PluginApiOperation::RemoteExecSend { .. } => {
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::ProcessStart { .. } | PluginApiOperation::ProcessSend { .. } => {
                // Process execution is admitted only by PluginService after Core has frozen an
                // exact executable plan and obtained the independent LocalProcess approval.
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::SftpOpen { .. } | PluginApiOperation::Sftp { .. } => {
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::SubscriptionStart { .. } => {
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::ProtocolOpen { .. } => Err(PluginApiErrorCode::InteractionRequired),
            PluginApiOperation::TaskStart { .. }
            | PluginApiOperation::TaskGet { .. }
            | PluginApiOperation::TaskList { .. }
            | PluginApiOperation::TaskCancel { .. }
            | PluginApiOperation::TaskResume { .. }
            | PluginApiOperation::AppRegister { .. }
            | PluginApiOperation::AppNotify { .. }
            | PluginApiOperation::AppNavigate { .. } => {
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::SerialDevices { .. }
            | PluginApiOperation::SerialOpen { .. }
            | PluginApiOperation::SerialSend { .. } => Err(PluginApiErrorCode::InteractionRequired),
            PluginApiOperation::Credential { .. } => Err(PluginApiErrorCode::InteractionRequired),
            PluginApiOperation::Storage { .. } => Err(PluginApiErrorCode::InteractionRequired),
            PluginApiOperation::FilePick { .. } | PluginApiOperation::File { .. } => {
                Err(PluginApiErrorCode::InteractionRequired)
            }
            PluginApiOperation::TerminalRequestInput { .. } => {
                Err(PluginApiErrorCode::InteractionRequired)
            }
        };
        PluginApiReply {
            call_id: call.call_id.clone(),
            outcome: match value {
                Ok(value) => PluginApiOutcome::Completed { value },
                Err(code) => PluginApiOutcome::Failed { code },
            },
        }
    }
}

pub(crate) fn validate_call(call: &PluginApiCall) -> Result<(), PluginApiErrorCode> {
    if call.call_id.is_empty()
        || call.call_id.len() > 80
        || !call
            .call_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || serde_json::to_vec(call).map_or(true, |value| value.len() > MAX_CALL_BYTES as usize)
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    match &call.operation {
        PluginApiOperation::DataRead { request } => request.validate()?,
        PluginApiOperation::DataSnapshot { request } => request.validate()?,
        PluginApiOperation::DataInspect { request } => request.validate()?,
        PluginApiOperation::DataCompose { request } => request.validate()?,
        PluginApiOperation::DataReview { request } => request.validate()?,
        PluginApiOperation::DataApply { request } => request.validate()?,
        PluginApiOperation::DataExport { request } => request.validate()?,
        PluginApiOperation::DataCheckpoint { request } => request.validate()?,
        PluginApiOperation::DataRelease { request } => request.validate()?,
        PluginApiOperation::PermissionRevoke { permission_id, .. }
            if uuid::Uuid::parse_str(permission_id).is_err() =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::SerialOpen {
            candidate_id,
            settings,
        } => {
            if uuid::Uuid::parse_str(candidate_id).is_err() {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            settings.validate()?;
        }
        PluginApiOperation::SerialSend { request }
            if uuid::Uuid::parse_str(&request.handle).is_err()
                || request.data_base64.is_empty()
                || request.data_base64.len()
                    > norishell_core_api::MAX_PLUGIN_SERIAL_SEND_BYTES * 2 =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::RemoteExecStart { request }
            if request.command.is_empty()
                || request.command.len() > 16 * 1024
                || request.command.contains('\0')
                || request.timeout_ms < 100
                || request.timeout_ms > 120_000
                || norishell_core_api::PluginHostHandle::parse(request.host_handle.clone())
                    .is_err() =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::RemoteExecSend { request }
            if uuid::Uuid::parse_str(&request.handle).is_err()
                || request.data_base64.len() > 24 * 1024
                || (request.data_base64.is_empty() && !request.close_stdin)
                || (!request.data_base64.is_empty() && request.close_stdin) =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::ResourceClose { handle }
        | PluginApiOperation::ResourceEvents { handle, .. }
            if uuid::Uuid::parse_str(handle).is_err() =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::ResourceEvents { limit, .. }
            if *limit == 0 || *limit > MAX_PENDING_EVENTS =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::ResourceEvents { wait_ms, .. } if *wait_ms > 30_000 => {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::TimerStart {
            delay_ms,
            interval_ms,
        } => timers::validate_timer(*delay_ms, *interval_ms)?,
        PluginApiOperation::Storage { operation } => storage::validate_operation(operation)?,
        PluginApiOperation::TerminalRequestInput {
            terminal_handle,
            payload,
            ..
        } if terminal_handle.is_empty()
            || terminal_handle.len() > 120
            || payload.is_empty()
            || payload.len() > MAX_CHUNK_BYTES as usize =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::NetworkStart { endpoint, request }
            if endpoint.endpoint.is_empty()
                || endpoint.endpoint.len() > 2_048
                || !(100..=120_000).contains(&request.timeout_ms) =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::NetworkSend { request }
            if uuid::Uuid::parse_str(&request.handle).is_err()
                || request.data_base64.is_empty()
                || request.data_base64.len() > MAX_CHUNK_BYTES as usize * 2 =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::ProcessStart {
            program,
            arguments,
            timeout_ms,
        } if program.is_empty()
            || program.len() > 4_096
            || arguments.len() > norishell_core_api::MAX_PLUGIN_PROCESS_ARGUMENTS
            || arguments.iter().any(|argument| {
                argument.len() > norishell_core_api::MAX_PLUGIN_PROCESS_ARGUMENT_BYTES
                    || argument.contains('\0')
            })
            || *timeout_ms == 0
            || *timeout_ms > norishell_core_api::MAX_PLUGIN_PROCESS_TIMEOUT_MS =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        PluginApiOperation::ProcessSend { request }
            if uuid::Uuid::parse_str(&request.handle).is_err()
                || request.data_base64.len() > MAX_CHUNK_BYTES as usize * 2
                || (request.data_base64.is_empty() && !request.close_stdin)
                || (request.close_stdin && !request.data_base64.is_empty()) =>
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        _ => {}
    }
    Ok(())
}

fn description() -> PluginApiDescription {
    let methods = [
        ("dataCatalog", None),
        ("dataRead", None),
        (
            "dataSnapshot",
            Some(norishell_core_api::PluginCapability::SshSync),
        ),
        (
            "dataInspect",
            Some(norishell_core_api::PluginCapability::SshSync),
        ),
        (
            "dataCompose",
            Some(norishell_core_api::PluginCapability::SshSync),
        ),
        (
            "dataApply",
            Some(norishell_core_api::PluginCapability::SshSync),
        ),
        (
            "dataExport",
            Some(norishell_core_api::PluginCapability::SshSync),
        ),
        (
            "dataCheckpoint",
            Some(norishell_core_api::PluginCapability::SshSync),
        ),
        (
            "dataRelease",
            Some(norishell_core_api::PluginCapability::SshSync),
        ),
        ("describe", None),
        ("permissions", None),
        ("permissionRequest", None),
        ("permissionRevoke", None),
        ("permissionsForget", None),
        (
            "filePick",
            Some(norishell_core_api::PluginCapability::LocalFiles),
        ),
        (
            "file",
            Some(norishell_core_api::PluginCapability::LocalFiles),
        ),
        (
            "sftpOpen",
            Some(norishell_core_api::PluginCapability::SftpRead),
        ),
        ("sftp", Some(norishell_core_api::PluginCapability::SftpRead)),
        (
            "remoteExecStart",
            Some(norishell_core_api::PluginCapability::RemoteExecRequest),
        ),
        (
            "remoteExecSend",
            Some(norishell_core_api::PluginCapability::RemoteExecRequest),
        ),
        (
            "credential",
            Some(norishell_core_api::PluginCapability::CredentialsPlugin),
        ),
        (
            "protocolOpen",
            Some(norishell_core_api::PluginCapability::TerminalProvider),
        ),
        (
            "serialDevices",
            Some(norishell_core_api::PluginCapability::DeviceSerial),
        ),
        (
            "serialOpen",
            Some(norishell_core_api::PluginCapability::DeviceSerial),
        ),
        (
            "serialSend",
            Some(norishell_core_api::PluginCapability::DeviceSerial),
        ),
        ("subscriptionStart", None),
        ("taskStart", None),
        ("taskGet", None),
        ("taskList", None),
        ("taskCancel", None),
        ("taskResume", None),
        ("appRegister", None),
        ("appNotify", None),
        ("appNavigate", None),
        ("resourcesList", None),
        ("resourceClose", None),
        ("timerStart", None),
        ("resourceEvents", None),
        (
            "storage",
            Some(norishell_core_api::PluginCapability::StoragePlugin),
        ),
        (
            "networkStart",
            Some(norishell_core_api::PluginCapability::NetworkDomain),
        ),
        (
            "networkSend",
            Some(norishell_core_api::PluginCapability::NetworkDomain),
        ),
        (
            "processStart",
            Some(norishell_core_api::PluginCapability::LocalProcess),
        ),
        (
            "processSend",
            Some(norishell_core_api::PluginCapability::LocalProcess),
        ),
        (
            "terminalRequestInput",
            Some(norishell_core_api::PluginCapability::TerminalRequestInput),
        ),
    ]
    .into_iter()
    .map(|(name, capability)| PluginApiMethod {
        name: name.to_owned(),
        capability,
        availability: PluginApiAvailability::Available,
    })
    .collect();
    PluginApiDescription {
        protocol_major: PLUGIN_PROTOCOL_MAJOR,
        protocol_minor: PLUGIN_PROTOCOL_MINOR,
        platform: std::env::consts::OS.to_owned(),
        methods,
        limits: PluginApiLimits {
            max_call_bytes: MAX_CALL_BYTES,
            max_chunk_bytes: MAX_CHUNK_BYTES,
            max_resources: MAX_PLUGIN_RESOURCES,
            max_pending_events: MAX_PENDING_EVENTS,
        },
    }
}

fn data_catalog() -> norishell_core_api::PluginDataCatalog {
    use norishell_core_api::{
        PluginCapability, PluginDataCatalog, PluginDataCategory, PluginDataCategoryDescriptor,
    };

    let descriptor = |category, required_capability| PluginDataCategoryDescriptor {
        category,
        required_capability,
        can_read: false,
        can_export: false,
        can_restore: false,
        available_groups: Vec::new(),
        unavailable_groups: Vec::new(),
    };
    PluginDataCatalog {
        categories: vec![
            PluginDataCategoryDescriptor {
                can_read: true,
                can_export: true,
                can_restore: true,
                ..descriptor(PluginDataCategory::Hosts, PluginCapability::SshSync)
            },
            PluginDataCategoryDescriptor {
                can_read: true,
                can_export: true,
                can_restore: true,
                ..descriptor(PluginDataCategory::Credentials, PluginCapability::SshSync)
            },
            PluginDataCategoryDescriptor {
                can_read: true,
                can_export: true,
                can_restore: true,
                ..descriptor(
                    PluginDataCategory::DesktopProfiles,
                    PluginCapability::SshSync,
                )
            },
            PluginDataCategoryDescriptor {
                can_read: true,
                available_groups: [
                    "application",
                    "appearance",
                    "interaction",
                    "highlights",
                    "shortcuts",
                    "files",
                    "desktop",
                    "commandNotifications",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                ..descriptor(
                    PluginDataCategory::AppPreferences,
                    PluginCapability::AppPreferencesRead,
                )
            },
            PluginDataCategoryDescriptor {
                can_read: true,
                ..descriptor(
                    PluginDataCategory::TerminalHistory,
                    PluginCapability::TerminalHistoryRead,
                )
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use norishell_core_api::{
        PluginApiCall, PluginApiOperation, PluginApiOutcome, PluginDataCategory,
        PluginDataReadRequest, PluginId, WireSequence,
    };

    #[tokio::test]
    async fn unbrokered_data_read_fails_closed() {
        let owner = super::ResourceOwner {
            plugin_id: PluginId::parse("example.read").expect("plugin id"),
            signer: "signer".to_owned(),
            package: "package".to_owned(),
            generation: WireSequence::new(1),
        };
        let call = PluginApiCall {
            call_id: "read".to_owned(),
            operation: PluginApiOperation::DataRead {
                request: PluginDataReadRequest {
                    category: PluginDataCategory::AppPreferences,
                    offset: 0,
                    limit: 1,
                },
            },
        };
        let reply = super::PluginApi::default()
            .invoke(&owner, &call, std::sync::Arc::new(|| true))
            .await;
        assert_eq!(
            reply.outcome,
            PluginApiOutcome::Failed {
                code: super::PluginApiErrorCode::PermissionDenied,
            }
        );
    }

    #[test]
    fn plugin_api_description_contains_each_method_once() {
        let description = super::description();
        let names: BTreeSet<_> = description
            .methods
            .iter()
            .map(|method| &method.name)
            .collect();
        assert_eq!(names.len(), description.methods.len());
        for name in ["remoteExecStart", "remoteExecSend"] {
            let method = description
                .methods
                .iter()
                .find(|method| method.name == name)
                .unwrap();
            assert_eq!(
                method.capability,
                Some(norishell_core_api::PluginCapability::RemoteExecRequest)
            );
        }
    }
}
