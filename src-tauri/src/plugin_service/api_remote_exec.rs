//! Exact-scope admission for plugin-owned SSH exec connections.

use super::{api::ApiAccessReview, api_invocation::ApiInvocation, *};
use crate::plugin_api::{
    ResourceFence, ResourceOwner,
    remote_exec::{CoreRemoteExecGrant, RemoteExecDriver, RemoteExecRuntime},
};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginRemoteExecSendRequest, PluginRemoteExecStartRequest,
};

impl PluginService {
    pub(super) async fn start_api_remote_exec(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        request: &PluginRemoteExecStartRequest,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::RemoteExecRequest,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let handle = PluginHostHandle::parse(request.host_handle.clone())
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let (target, host_scope) = self.resolve_api_host(invocation, installed, &handle)?;
        let admission =
            self.api_host_admission_fence(invocation, owner, &target, host_scope.clone());
        if !admission() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let details = format!(
            "{}\n{}\n\n{}\n\n{} ms",
            target.label, target.endpoint, request.command, request.timeout_ms
        );
        let permission = self.authorize_api_access(invocation, installed, owner, ApiAccessReview {
            capability: PluginCapability::RemoteExecRequest,
            operation: norishell_core_api::PluginApprovalOperation::RemoteExecute,
            target_label: target.label.clone(),
            persisted_target_label: target.label.clone(),
            details,
            exact_scope: serde_json::json!({ "host": target, "command": request.command, "timeoutMs": request.timeout_ms }),
            target_fence: Some(admission.clone()),
        }).await?;
        if !admission() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let runtime = self.remote_exec_runtime()?;
        let _permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Revoked)?;
        let resource_fence: ResourceFence = Arc::new(move || permission() && host_scope());
        let resources = invocation.resource_registry(&self.api.resources);
        let handle = RemoteExecDriver::start(
            &resources,
            CoreRemoteExecGrant {
                owner: owner.clone(),
                host_id: target.host_id,
                expected_host_revision: target.host_revision,
                expected_connection_revision: target.connection_revision,
                command: request.command.as_bytes().to_vec(),
                timeout_ms: request.timeout_ms,
                admission_fence: admission.clone(),
                resource_fence,
            },
            runtime,
        )
        .await?;
        if !admission() {
            return Err(PluginApiErrorCode::OutcomeUnknown);
        }
        Ok(PluginApiValue::RemoteExecStarted { handle })
    }

    pub(super) async fn send_api_remote_exec(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        request: &PluginRemoteExecSendRequest,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::RemoteExecRequest,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                installed,
                PluginCapability::RemoteExecRequest,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let permission = self.api_resource_fence(
            owner.clone(),
            Some((PluginCapability::RemoteExecRequest, epoch)),
        );
        let current = self.api_invocation_fence(owner, invocation);
        let fence: ResourceFence = Arc::new(move || permission() && current());
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let resources = invocation.resource_registry(&self.api.resources);
        RemoteExecDriver::send(&resources, owner, request.clone(), &fence).await?;
        if !fence() {
            return Err(PluginApiErrorCode::OutcomeUnknown);
        }
        Ok(PluginApiValue::RemoteExecSent {
            handle: request.handle.clone(),
        })
    }

    fn remote_exec_runtime(&self) -> Result<RemoteExecRuntime, PluginApiErrorCode> {
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        Ok(RemoteExecRuntime {
            hosts: self.hosts.clone(),
            vault: app
                .try_state::<crate::vault_service::VaultService>()
                .ok_or(PluginApiErrorCode::Unavailable)?
                .inner()
                .clone(),
            transient_credentials: app
                .try_state::<crate::transient_credential_service::TransientCredentialService>()
                .ok_or(PluginApiErrorCode::Unavailable)?
                .inner()
                .clone(),
            ssh_agent: app
                .try_state::<crate::ssh_agent_service::SshAgentService>()
                .ok_or(PluginApiErrorCode::Unavailable)?
                .inner()
                .clone(),
        })
    }
}
