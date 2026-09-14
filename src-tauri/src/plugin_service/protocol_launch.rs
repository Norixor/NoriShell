//! Queryable, claimable launch records; no network resource exists before a pane is projected.
use super::{api_invocation::ApiInvocation, protocols::ProtocolBinding, *};
use crate::plugin_api::{ResourceConsumer, ResourceFence, ResourceOwner};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginProtocolLaunchSummary, PluginProtocolOpen,
    PluginProtocolProvider, PluginSettingsValues,
};

#[derive(Clone)]
pub(super) struct ProtocolLaunch {
    pub summary: PluginProtocolLaunchSummary,
    pub binding: ProtocolBinding,
    pub provider: PluginProtocolProvider,
    pub configuration: PluginSettingsValues,
    pub schema_hash: String,
    pub session_id: Option<Uuid>,
    request_key: String,
    request: PluginProtocolOpen,
}

impl PluginService {
    pub(super) fn open_protocol_launch(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &ResourceOwner,
        call_id: &str,
        request: &PluginProtocolOpen,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        if !invocation.explicit_user_action()
            || matches!(invocation, ApiInvocation::Provider { .. })
        {
            return Err(PluginApiErrorCode::InteractionRequired);
        }
        let installed = self
            .has_capability(
                invocation.request_id().clone(),
                &owner.plugin_id,
                &owner.signer,
                PluginCapability::TerminalProvider,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let catalog = self
            .installer
            .read_protocol_catalog(&owner.plugin_id, &installed.active_version, &owner.package)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?
            .ok_or(PluginApiErrorCode::NotFound)?;
        let provider = catalog
            .catalog
            .providers
            .into_iter()
            .find(|provider| provider.id == request.provider_id)
            .ok_or(PluginApiErrorCode::NotFound)?;
        let configuration: PluginSettingsValues = serde_json::from_value(
            serde_json::to_value(&request.configuration)
                .map_err(|_| PluginApiErrorCode::InvalidRequest)?,
        )
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        norishell_plugin_platform::validate_settings_values(
            &provider.configuration,
            &configuration,
        )
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let label = request
            .label
            .as_ref()
            .filter(|label| !label.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| provider.label.en.clone());
        if label.len() > 256 || label.chars().any(char::is_control) {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let request_key = format!("{}:{call_id}", invocation.request_id().as_str());
        let now = unix_time_ms();
        let mut records = self
            .protocol_launches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Claimed records are short-lived response-retry history, not session ownership.
        // The actor and protocol_sessions retain live sessions independently.
        records.retain(|_, record| record.summary.expires_at_unix_ms > now);
        if let Some(record) = records
            .values()
            .find(|record| record.binding.owner == *owner && record.request_key == request_key)
        {
            return if record.request == *request {
                Ok(PluginApiValue::ProtocolLaunched {
                    launch_id: record.summary.launch_id.clone(),
                })
            } else {
                Err(PluginApiErrorCode::InvalidRequest)
            };
        }
        if records.len() >= 128 {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let summary = PluginProtocolLaunchSummary {
            launch_id: Uuid::new_v4().to_string(),
            plugin_id: owner.plugin_id.clone(),
            provider_id: provider.id.clone(),
            label,
            tab_id: Uuid::new_v4().to_string(),
            pane_id: Uuid::new_v4().to_string(),
            revision: WireSequence::new(1),
            claimed: false,
            expires_at_unix_ms: now.saturating_add(120_000),
        };
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                &installed,
                PluginCapability::TerminalProvider,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let authority: ResourceFence = self.api_resource_fence(
            owner.clone(),
            Some((PluginCapability::TerminalProvider, epoch)),
        );
        let binding = ProtocolBinding {
            owner: owner.clone(),
            consumer: ResourceConsumer {
                connection: Uuid::new_v4(),
                generation: WireSequence::new(1),
                stream: Uuid::new_v4(),
            },
            connection_handle: Uuid::new_v4().to_string(),
            provider_id: provider.id.clone(),
            resources: provider.resources.clone(),
            authority,
            transaction_authority: self.api_runtime_fence(owner.clone()),
        };
        records.insert(
            summary.launch_id.clone(),
            ProtocolLaunch {
                summary: summary.clone(),
                binding,
                provider,
                configuration,
                schema_hash: catalog.catalog_sha256,
                session_id: None,
                request_key,
                request: request.clone(),
            },
        );
        drop(records);
        if let Some(app) = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            use tauri::Emitter;
            let _ = app.emit_to("main", "plugin-protocol-launch", &summary);
        }
        Ok(PluginApiValue::ProtocolLaunched {
            launch_id: summary.launch_id,
        })
    }

    pub(crate) fn pending_protocol_launches(&self) -> Vec<PluginProtocolLaunchSummary> {
        let now = unix_time_ms();
        self.protocol_launches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|record| {
                record.summary.expires_at_unix_ms > now && (record.binding.authority)()
            })
            .map(|record| record.summary.clone())
            .collect()
    }
}
