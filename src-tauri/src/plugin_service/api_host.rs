//! Resolves opaque plugin Host handles without exposing Core connection or credential references.

use super::{api_invocation::ApiInvocation, *};
use crate::{
    connection_profile::resolve_long_lived_connection_profile,
    plugin_api::{ResourceFence, ResourceOwner},
};
use norishell_core_api::{HostId, PluginApiErrorCode};

#[derive(Clone, serde::Serialize)]
pub(super) struct ApiHostTarget {
    pub(super) host_id: HostId,
    pub(super) host_revision: WireSequence,
    pub(super) scope_revision: WireSequence,
    pub(super) label: String,
    pub(super) endpoint: String,
    pub(super) connection_revision: String,
}

impl PluginService {
    pub(super) fn resolve_api_host(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        handle: &PluginHostHandle,
    ) -> Result<(ApiHostTarget, ResourceFence), PluginApiErrorCode> {
        let owner = invocation.owner(installed);
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::HostSessionRequest,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let record = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .host_handles
            .get(handle.as_str())
            .cloned()
            .filter(|record| {
                record.plugin_id == owner.plugin_id
                    && record.instance_generation == owner.generation
            })
            .ok_or(PluginApiErrorCode::PermissionDenied)?;
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                installed,
                PluginCapability::HostSessionRequest,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let scope_fence = self.api_host_scope_fence(
            owner,
            record.host_id.clone(),
            record.scope_state_version,
            epoch,
        );
        if !scope_fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let snapshot = self
            .hosts
            .get_connection_snapshot(&record.host_id)
            .map_err(|_| PluginApiErrorCode::NotFound)?;
        let profile = resolve_long_lived_connection_profile(
            &self.hosts,
            &record.host_id,
            snapshot.host.state_version,
        )
        .map_err(|_| PluginApiErrorCode::InteractionRequired)?;
        Ok((
            ApiHostTarget {
                host_id: record.host_id,
                host_revision: snapshot.host.state_version,
                scope_revision: record.scope_state_version,
                label: snapshot.host.label,
                endpoint: format!(
                    "{}@{}:{}",
                    profile.connection.username,
                    profile.connection.endpoint.address,
                    profile.connection.endpoint.port
                ),
                connection_revision: profile.connection.revision_token,
            },
            scope_fence,
        ))
    }

    fn api_host_scope_fence(
        &self,
        owner: ResourceOwner,
        host_id: HostId,
        scope_revision: WireSequence,
        capability_epoch: WireSequence,
    ) -> ResourceFence {
        let runtime = self.api_resource_fence(
            owner.clone(),
            Some((PluginCapability::HostSessionRequest, capability_epoch)),
        );
        let hosts = self.hosts.clone();
        Arc::new(move || {
            runtime()
                && hosts
                    .with_plugin_repository(|repository| {
                        let installed = repository.get_plugin_installation(&owner.plugin_id)?;
                        let scope = repository.plugin_host_scope_set(
                            &owner.plugin_id,
                            &owner.signer,
                            u64::from(PLUGIN_PROTOCOL_MAJOR),
                        )?;
                        if scope.as_ref().is_none_or(|scope| {
                            scope.state_version != scope_revision
                                || !effective_plugin_scope(&installed, scope)
                        }) {
                            return Ok(false);
                        }
                        Ok(repository
                            .list_plugin_host_scope_grants(
                                &owner.plugin_id,
                                &owner.signer,
                                u64::from(PLUGIN_PROTOCOL_MAJOR),
                            )?
                            .iter()
                            .any(|grant| {
                                grant.host_id == host_id
                                    && grant.capability == PluginCapability::HostSessionRequest
                            }))
                    })
                    .unwrap_or(false)
        })
    }

    pub(super) fn api_host_admission_fence(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &ResourceOwner,
        target: &ApiHostTarget,
        scope_fence: ResourceFence,
    ) -> ResourceFence {
        let current = self.api_invocation_fence(owner, invocation);
        let hosts = self.hosts.clone();
        let target = target.clone();
        Arc::new(move || {
            current()
                && scope_fence()
                && resolve_long_lived_connection_profile(
                    &hosts,
                    &target.host_id,
                    target.host_revision,
                )
                .is_ok_and(|profile| {
                    profile.connection.revision_token == target.connection_revision
                })
        })
    }
}
