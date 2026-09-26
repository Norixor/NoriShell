//! Fixed network targets, destination-scoped authorization, and controlled credential injection.
use super::{
    api::ApiAccessReview, api_credentials::map_credential_error, api_invocation::ApiInvocation, *,
};
use crate::plugin_api::{
    ResourceFence, ResourceOwner,
    network::{NetworkDriver, prepare_endpoint, validate_start},
};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginApprovalOperation, PluginCredentialState,
    PluginNetworkEndpointRequest, PluginNetworkStartRequest, PreparedNetworkEndpoint,
};

pub(super) const NETWORK_TARGET_POLICY_IDENTITY: &str = "network.target.v1";

/// The decision is for the frozen destination. Paths, methods, request bodies,
/// credentials, and action IDs are not part of its remembered identity.
pub(super) fn network_target_scope(endpoint: &PreparedNetworkEndpoint) -> serde_json::Value {
    serde_json::json!({
        "version": NETWORK_TARGET_POLICY_IDENTITY,
        "scheme": endpoint.scheme,
        "host": endpoint.host,
        "port": endpoint.port,
        "resolvedIps": endpoint.resolved_ips,
    })
}

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
        let parsed_url = reqwest::Url::parse(&frozen.canonical_url)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        let exact_origin = parsed_url.origin().ascii_serialization();
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
        let exact_scope = network_target_scope(&frozen);
        let locale = self
            .active_instance(
                invocation.request_id().clone(),
                &owner.plugin_id,
                &owner.signer,
                owner.generation,
            )
            .map_err(|_| PluginApiErrorCode::Revoked)?
            .locale;
        let scope_summary = if locale.as_str() == "zh-CN" {
            "记住后，此插件可连接此网络目标的所有路径，发送和接收数据；请求可能读取或修改服务端数据。每次请求仍单独校验。"
        } else {
            "Remembering lets this plugin connect to every path on this network destination and send or receive data. Requests may read or change server data. Each request is still validated."
        };
        let display_host = if frozen.host.contains(':') {
            format!("[{}]", frozen.host)
        } else {
            frozen.host.clone()
        };
        let target_label = format!("{}://{}:{}", parsed_url.scheme(), display_host, frozen.port);
        let details = format!(
            "{scope_summary}\n\n{target_label}\n{}",
            frozen.resolved_ips.join(", ")
        );
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
                    policy_identity: Some(NETWORK_TARGET_POLICY_IDENTITY),
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
            let credential_origin = exact_origin.clone();
            Some(
                tauri::async_runtime::spawn_blocking(move || {
                    service.lease(&owner, &reference, &credential_origin, permission)
                })
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)?
                .map_err(map_credential_error)?,
            )
        } else if let Some(profile_id) = network.oauth_profile_id.as_deref() {
            let coordinator = self
                .ssh_sync
                .as_ref()
                .ok_or(PluginApiErrorCode::Unavailable)?;
            let admission_current = current.clone();
            let admission_fence = fence.clone();
            let lease_fence: crate::ssh_sync_exchange::ActionFence =
                Arc::new(move || admission_current() && admission_fence());
            Some(
                coordinator
                    .oauth_network_lease(
                        owner.plugin_id.as_str(),
                        &owner.signer,
                        profile_id,
                        &exact_origin,
                        lease_fence,
                    )
                    .await?,
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
            Some(self.api.blobs.clone()),
        )
        .map(|handle| PluginApiValue::NetworkStarted { handle })
    }
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{PluginNetworkScheme, PreparedNetworkEndpoint};

    use super::network_target_scope;

    fn endpoint() -> PreparedNetworkEndpoint {
        PreparedNetworkEndpoint {
            canonical_url: "https://sync.example.test/exchange".to_owned(),
            scheme: PluginNetworkScheme::Https,
            host: "sync.example.test".to_owned(),
            port: 443,
            path_and_query: "/exchange".to_owned(),
            resolved_ips: vec!["192.0.2.1".to_owned()],
        }
    }

    #[test]
    fn path_and_request_content_do_not_change_destination_permission() {
        let first = endpoint();
        let mut changed_path = first.clone();
        changed_path.canonical_url = "https://sync.example.test/other?revision=2".to_owned();
        changed_path.path_and_query = "/other?revision=2".to_owned();
        assert_eq!(
            network_target_scope(&first),
            network_target_scope(&changed_path)
        );
    }

    #[test]
    fn scheme_port_and_resolved_destination_change_permission() {
        let first = endpoint();
        let mut changed = first.clone();
        changed.scheme = PluginNetworkScheme::Http;
        assert_ne!(network_target_scope(&first), network_target_scope(&changed));
        changed = first.clone();
        changed.port = 8443;
        assert_ne!(network_target_scope(&first), network_target_scope(&changed));
        changed = first.clone();
        changed.resolved_ips = vec!["192.0.2.2".to_owned()];
        assert_ne!(network_target_scope(&first), network_target_scope(&changed));
    }
}
