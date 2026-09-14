use super::*;
use crate::ssh_session_service::SessionChannelLease;
use norishell_core_api::{
    PluginApprovedTerminalChannelLaunch, PluginDockerContainerShell, SshSessionTarget,
};

#[derive(Clone)]
pub(crate) struct ApprovedPluginTerminalStartup {
    pub(crate) command: String,
    pub(crate) parent: SessionChannelLease,
    pub(crate) is_current: crate::plugin_operations::OperationFence,
}

pub(super) struct PendingTerminalChannel {
    plugin_id: PluginId,
    target: SshSessionTarget,
    expires_at_unix_ms: i64,
    startup: ApprovedPluginTerminalStartup,
}

pub(super) fn startup_command(
    container_id: &str,
    shell: PluginDockerContainerShell,
) -> Option<String> {
    if container_id.len() != 64
        || !container_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let shell = match shell {
        PluginDockerContainerShell::Sh => "/bin/sh",
        PluginDockerContainerShell::Bash => "/bin/bash",
    };
    Some(format!("docker exec -it -- '{container_id}' '{shell}'"))
}

impl PluginService {
    pub(super) fn prepare_terminal_channel_launch(
        &self,
        plugin_id: PluginId,
        parent: SessionChannelLease,
        command: String,
        container_id: &str,
        is_current: crate::plugin_operations::OperationFence,
    ) -> CoreResult<PluginApprovedTerminalChannelLaunch> {
        if !parent.is_current() || !is_current() {
            return Err(plugin_conflict_error(RequestId::new(), None));
        }
        let authorization_token = PluginApprovalId::new();
        let target = parent.target.clone();
        let now = unix_time_ms();
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime
            .approved_terminal_channels
            .retain(|_, pending| pending.expires_at_unix_ms > now);
        if runtime.approved_terminal_channels.len() >= 32 {
            return Err(plugin_conflict_error(RequestId::new(), None));
        }
        runtime.approved_terminal_channels.insert(
            authorization_token.as_str().to_owned(),
            PendingTerminalChannel {
                plugin_id,
                target: target.clone(),
                expires_at_unix_ms: now + 30_000,
                startup: ApprovedPluginTerminalStartup {
                    command,
                    parent,
                    is_current,
                },
            },
        );
        Ok(PluginApprovedTerminalChannelLaunch {
            operation_id: norishell_core_api::OperationId::new(),
            authorization_token,
            target,
            label: format!("Docker · {}", &container_id[..12]),
        })
    }

    pub(crate) fn consume_terminal_launch_authorization(
        &self,
        request_id: RequestId,
        token: &PluginApprovalId,
        target: &SshSessionTarget,
    ) -> CoreResult<Option<ApprovedPluginTerminalStartup>> {
        let pending = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .approved_terminal_channels
            .remove(token.as_str());
        if let Some(pending) = pending {
            if pending.expires_at_unix_ms <= unix_time_ms()
                || &pending.target != target
                || !pending.startup.parent.is_current()
                || !(pending.startup.is_current)()
            {
                return Err(plugin_conflict_error(request_id, None));
            }
            return Ok(Some(pending.startup));
        }
        let SshSessionTarget::Host { host_id, .. } = target else {
            return Err(plugin_permission_error(request_id));
        };
        self.consume_host_session_authorization(
            request_id,
            token,
            host_id,
            PluginHostSessionKind::Terminal,
        )?;
        Ok(None)
    }

    pub(super) fn discard_terminal_channel_launches(&self, plugin_id: &PluginId) {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .approved_terminal_channels
            .retain(|_, pending| pending.plugin_id != *plugin_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_terminal_accepts_only_immutable_ids_and_fixed_shells() {
        let id = "a".repeat(64);
        assert_eq!(
            startup_command(&id, PluginDockerContainerShell::Sh),
            Some(format!("docker exec -it -- '{id}' '/bin/sh'"))
        );
        assert_eq!(
            startup_command(&id, PluginDockerContainerShell::Bash),
            Some(format!("docker exec -it -- '{id}' '/bin/bash'"))
        );
        for invalid in [
            "container-name",
            "abc; id",
            &"A".repeat(64),
            &"a".repeat(63),
            &"a".repeat(65),
        ] {
            assert!(startup_command(invalid, PluginDockerContainerShell::Sh).is_none());
        }
    }
}
