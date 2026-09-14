//! Fixed network targets, exact authorization, and controlled injection of plugin-owned credentials.
use super::{
    api::ApiAccessReview, api_credentials::map_credential_error, api_invocation::ApiInvocation, *,
};
use crate::plugin_api::{
    ResourceFence, ResourceOwner,
    network::{NetworkDriver, prepare_endpoint, validate_start},
};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginApprovalOperation, PluginCredentialState,
    PluginNetworkEndpointRequest, PluginNetworkStartRequest,
};

impl PluginService {
    pub(super) async fn start_api_network(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        endpoint: &PluginNetworkEndpointRequest,
        network: &PluginNetworkStartRequest,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::NetworkDomain,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let credential_permission = network
            .credential
            .as_ref()
            .map(|_| self.api_credential_fence(invocation, installed, owner))
            .transpose()?;
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let frozen = prepare_endpoint(endpoint).await?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        validate_start(&frozen, network)?;
        let exact_origin = reqwest::Url::parse(&frozen.canonical_url)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?
            .origin()
            .ascii_serialization();
        let credential_metadata = if let Some(reference) = &network.credential {
            let service = self.api_credential_service()?;
            let owner = owner.clone();
            let transaction = self.api_transaction_fence(&owner, invocation);
            let credentials =
                tauri::async_runtime::spawn_blocking(move || service.list(&owner, transaction))
                    .await
                    .map_err(|_| PluginApiErrorCode::Unavailable)?
                    .map_err(map_credential_error)?;
            Some(
                credentials
                    .into_iter()
                    .find(|credential| {
                        credential.handle == reference.handle
                            && credential.revision == reference.expected_revision
                            && credential.state == PluginCredentialState::Ready
                            && credential.target.origin == exact_origin
                    })
                    .ok_or(PluginApiErrorCode::Revoked)?,
            )
        } else {
            None
        };
        let exact_scope = serde_json::json!({ "endpoint": frozen, "request": network, "credential": credential_metadata });
        let details = serde_json::to_string_pretty(&exact_scope)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let target_label = format!("{}:{}", frozen.host, frozen.port);
        let fence = self
            .authorize_api_access(
                invocation,
                installed,
                owner,
                ApiAccessReview {
                    capability: PluginCapability::NetworkDomain,
                    operation: PluginApprovalOperation::NetworkRequest,
                    target_label: target_label.clone(),
                    persisted_target_label: target_label,
                    details,
                    exact_scope,
                    target_fence: credential_permission.clone(),
                },
            )
            .await?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let credential = if let (Some(reference), Some(permission)) =
            (&network.credential, credential_permission)
        {
            let admission_current = current.clone();
            let admission_permission = permission.clone();
            let admission: ResourceFence =
                Arc::new(move || admission_current() && admission_permission());
            let locale = self
                .active_instance(
                    invocation.request_id().clone(),
                    &owner.plugin_id,
                    &owner.signer,
                    owner.generation,
                )
                .map_err(|_| PluginApiErrorCode::Revoked)?
                .locale;
            self.operations
                .as_ref()
                .ok_or(PluginApiErrorCode::Unavailable)?
                .ensure_credential_vault(
                    crate::plugin_operations::prompts::IndependentApproval {
                        plugin_id: owner.plugin_id.clone(),
                        instance_generation: owner.generation,
                        plugin_name: installed.name.clone(),
                        locale,
                        host_label: credential_metadata
                            .as_ref()
                            .map(|value| value.label.clone())
                            .unwrap_or_default(),
                        endpoint: exact_origin.clone(),
                        fence: admission,
                        remembered_policy: None,
                        unavailable_policy: norishell_core_api::PluginRememberPolicy::Unavailable,
                    },
                    self.api_vault_service()?,
                    invocation.explicit_user_action(),
                )
                .await?;
            let service = self.api_credential_service()?;
            let owner = owner.clone();
            let reference = reference.clone();
            Some(
                tauri::async_runtime::spawn_blocking(move || {
                    service.lease(&owner, &reference, &exact_origin, permission)
                })
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)?
                .map_err(map_credential_error)?,
            )
        } else {
            None
        };
        let fence = if let ApiInvocation::Provider { authority, .. }
        | ApiInvocation::Task { authority, .. } = invocation
        {
            let authority = authority.clone();
            let network_fence = fence;
            Arc::new(move || authority() && network_fence()) as ResourceFence
        } else {
            fence
        };
        let _permit = self
            .api_creation_permit(invocation.request_id().clone())
            .map_err(|_| PluginApiErrorCode::Revoked)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        NetworkDriver::start_with_credential(
            &invocation.resource_registry(&self.api.resources),
            owner.clone(),
            frozen,
            network.clone(),
            fence,
            credential,
        )
        .map(|handle| PluginApiValue::NetworkStarted { handle })
    }
}
