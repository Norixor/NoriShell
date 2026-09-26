//! Builds remembered operation scopes from Core-owned targets, never guest-supplied identity.

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
        _mutation: Option<&PluginHostMutationPatch>,
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
                    action_label,
                    host_id,
                    &snapshot.host.address,
                    snapshot.host.port,
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
        let host = self
            .hosts
            .with_plugin_repository(|repository| {
                let snapshot = repository.get_host_connection_snapshot(host_id)?;
                Ok(snapshot.host)
            })
            .ok()?;
        if host.state_version != *expected_host_state_version
            || host.address != lease.endpoint.address
            || host.port != lease.endpoint.port
        {
            return None;
        }
        let scope = if matches!(
            operation,
            PluginApprovalOperation::RemoteExecute | PluginApprovalOperation::TerminalInput
        ) {
            serde_json::json!({
                "hostId": host_id,
                "endpoint": lease.endpoint,
                "action": action_label,
                "command": payload,
            })
        } else {
            serde_json::json!({
                "hostId": host_id,
                "endpoint": lease.endpoint,
                "operation": operation,
                "target": payload,
            })
        };
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
                &scope,
            )
            .ok()
    }
}
