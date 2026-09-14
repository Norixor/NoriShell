//! Discovery yields short-lived device candidates; opening requires protected approval of frozen identity and parameters.
use super::{api::ApiAccessReview, api_invocation::ApiInvocation, *};
use crate::plugin_api::{
    ResourceConsumer, ResourceFence, ResourceOwner,
    serial::{DiscoveredSerialCandidate, SerialDriver},
};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginApprovalOperation, PluginSerialSendRequest,
    PluginSerialSettings,
};

pub(super) struct SerialCandidateRecord {
    owner: ResourceOwner,
    consumer: Option<ResourceConsumer>,
    expires_at: i64,
    candidate: DiscoveredSerialCandidate,
}

impl PluginService {
    pub(super) async fn list_api_serial(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &ResourceOwner,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::DeviceSerial,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let found = tauri::async_runtime::spawn_blocking(SerialDriver::discover)
            .await
            .map_err(|_| PluginApiErrorCode::Unavailable)??;
        if found.len() > 64 {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let consumer = invocation.resource_consumer();
        let now = unix_time_ms();
        let mut records = self
            .serial_candidates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        records.retain(|_, record| {
            record.expires_at > now && !(record.owner == *owner && record.consumer == consumer)
        });
        if records.len().saturating_add(found.len()) > 256 {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        let devices = found.iter().map(|value| value.candidate.clone()).collect();
        for candidate in found {
            records.insert(
                candidate.candidate.candidate_id.clone(),
                SerialCandidateRecord {
                    owner: owner.clone(),
                    consumer: consumer.clone(),
                    expires_at: now.saturating_add(120_000),
                    candidate,
                },
            );
        }
        Ok(PluginApiValue::SerialDevices { devices })
    }

    pub(super) async fn open_api_serial(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        candidate_id: &str,
        settings: PluginSerialSettings,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        settings.validate()?;
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::DeviceSerial,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let candidate = {
            let records = self
                .serial_candidates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            records
                .get(candidate_id)
                .filter(|record| {
                    record.owner == *owner
                        && record.consumer == invocation.resource_consumer()
                        && record.expires_at > unix_time_ms()
                })
                .map(|record| record.candidate.clone())
                .ok_or(PluginApiErrorCode::NotFound)?
        };
        let plan = candidate.freeze(candidate_id, settings)?;
        let exact_scope = serde_json::json!({ "device": plan.canonical_path(),
            "identity": plan.platform_identity(), "settings": settings });
        let details = serde_json::to_string_pretty(&exact_scope)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let opaque_id = candidate_id.chars().take(8).collect::<String>();
        let persisted_target_label = format!("Selected serial device · {opaque_id}");
        let fence = self
            .authorize_api_access(
                invocation,
                installed,
                owner,
                ApiAccessReview {
                    capability: PluginCapability::DeviceSerial,
                    operation: PluginApprovalOperation::SerialAccess,
                    target_label: candidate.candidate.metadata.label,
                    persisted_target_label,
                    details,
                    exact_scope,
                    target_fence: None,
                },
            )
            .await?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let fence = if let ApiInvocation::Provider { authority, .. }
        | ApiInvocation::Task { authority, .. } = invocation
        {
            let authority = authority.clone();
            Arc::new(move || authority() && fence()) as ResourceFence
        } else {
            fence
        };
        let _permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Revoked)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let handle = SerialDriver::start(
            &invocation.resource_registry(&self.api.resources),
            owner.clone(),
            plan,
            fence,
        )?;
        if !current() {
            return Err(PluginApiErrorCode::OutcomeUnknown);
        }
        Ok(PluginApiValue::SerialStarted { handle })
    }

    pub(super) async fn send_api_serial(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        request: &PluginSerialSendRequest,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::DeviceSerial,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                installed,
                PluginCapability::DeviceSerial,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let current = self.api_invocation_fence(owner, invocation);
        let lifetime =
            self.api_resource_fence(owner.clone(), Some((PluginCapability::DeviceSerial, epoch)));
        let fence: ResourceFence = Arc::new(move || current() && lifetime());
        SerialDriver::send(
            &invocation.resource_registry(&self.api.resources),
            owner,
            request.clone(),
            &fence,
        )
        .await?;
        Ok(PluginApiValue::SerialSent {
            handle: request.handle.clone(),
        })
    }
}
