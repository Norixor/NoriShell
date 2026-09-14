//! Builds exact operation scopes from Core-owned targets, never guest-supplied identity.

use crate::{
    plugin_operation_policy::PreparedOperationPolicy, ssh_session_service::SessionChannelLease,
};
use norishell_core_api::{
    PluginApprovalOperation, PluginHostMutationPatch, PluginHostSessionKind, SshSessionTarget,
};

use super::*;

impl PluginService {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn host_operation_policy(
        &self,
        installed: &PluginInstalledRecord,
        capability: PluginCapability,
        operation: PluginApprovalOperation,
        action_label: &str,
        host_id: &norishell_core_api::HostId,
        mutation: Option<&PluginHostMutationPatch>,
        session_kind: Option<PluginHostSessionKind>,
    ) -> Option<PreparedOperationPolicy> {
        let snapshot = self
            .hosts
            .with_plugin_repository(|repository| repository.get_host_connection_snapshot(host_id))
            .ok()?;
        self.operation_policies
            .prepare(
                installed,
                capability,
                current_plugin_permission_binding(&installed.package_sha256),
                operation,
                action_label,
                &snapshot.host.label,
                &(
                    host_id,
                    &snapshot.host,
                    &snapshot.config.route_plan,
                    &snapshot.config.authentication_plan,
                    &snapshot.config.algorithm_policy,
                    mutation,
                    session_kind,
                ),
            )
            .ok()
    }

    pub(super) fn session_operation_policy(
        &self,
        installed: &PluginInstalledRecord,
        action_label: &str,
        operation: PluginApprovalOperation,
        lease: &SessionChannelLease,
        payload: &impl serde::Serialize,
    ) -> Option<PreparedOperationPolicy> {
        let SshSessionTarget::Host {
            host_id,
            expected_host_state_version,
        } = &lease.target
        else {
            return None;
        };
        let (host, config) = self
            .hosts
            .with_plugin_repository(|repository| {
                let snapshot = repository.get_host_connection_snapshot(host_id)?;
                Ok((snapshot.host, snapshot.config))
            })
            .ok()?;
        if host.state_version != *expected_host_state_version
            || host.address != lease.endpoint.address
            || host.port != lease.endpoint.port
        {
            return None;
        }
        self.operation_policies
            .prepare(
                installed,
                if operation == PluginApprovalOperation::TerminalInput {
                    PluginCapability::TerminalRequestInput
                } else {
                    PluginCapability::RemoteExecRequest
                },
                current_plugin_permission_binding(&installed.package_sha256),
                operation,
                action_label,
                &host.label,
                &(
                    host_id,
                    &lease.endpoint,
                    &config.route_plan,
                    &config.authentication_plan,
                    &config.algorithm_policy,
                    action_label,
                    payload,
                ),
            )
            .ok()
    }
}
