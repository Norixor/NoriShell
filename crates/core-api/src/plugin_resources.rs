//! Strict public DTOs for SSH-session-scoped plugin resource requests.
//!
//! A plugin submits only an opaque terminal handle. The Core resolves it to an already-open SSH
//! session and constructs the internal owner/action fences before calling the resource broker.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{OperationId, PluginDockerContainerShell, PluginId, SftpSessionSummary, WireSequence};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginResourceOperationRequest {
    pub terminal_handle: String,
    pub operation: PluginResourceOperation,
    pub reason: String,
}

impl std::fmt::Debug for PluginResourceOperationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginResourceOperationRequest")
            .field("terminal_handle", &self.terminal_handle)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginResourceOperation {
    /// Requests a new terminal channel in a container through the selected, already-open SSH
    /// session. The fixed shell enum and immutable container ID are revalidated by Core.
    DockerTerminalOpen {
        container_id: String,
        shell: PluginDockerContainerShell,
    },
    /// Requests standard host navigation. The result is an intent, not proof that navigation ran.
    SftpOpen {
        path: String,
        edit: bool,
    },
    /// Reads bounded increments from regular text files over a resource-private SFTP session.
    LogsRead {
        files: Vec<PluginLogReadRequest>,
    },
    ForwardStart {
        rule: PluginForwardRule,
    },
    ForwardList,
    ForwardStop {
        forward_handle: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLogReadRequest {
    pub path: String,
    /// `None` requests the latest bounded window. The returned `next_offset` is used for later
    /// incremental reads; a supplied offset remains a decimal string on the JSON wire.
    pub offset: Option<WireSequence>,
    pub max_bytes: u16,
}

/// A plugin never supplies the HostId in a forwarding rule. Core injects the already-authorized
/// Host after resolving the opaque Host handle and rechecking its action fence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginForwardRule {
    Local {
        local_bind_address: String,
        local_listen_port: u16,
        remote_target_host: String,
        remote_target_port: u16,
    },
    Remote {
        remote_bind_address: String,
        remote_listen_port: u16,
        local_target_host: String,
        local_target_port: u16,
    },
    Dynamic {
        local_bind_address: String,
        local_listen_port: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginResourceOperationResult {
    /// Core committed the approved launch intent. The terminal channel is created by the main
    /// application from a separate one-time token.
    TerminalLaunchRequested,
    NavigationRequested {
        request: PluginHostNavigationRequest,
    },
    LogsRead {
        files: Vec<PluginLogReadResult>,
        total_bytes: u32,
    },
    ForwardStarted {
        forward: PluginOwnedForwardSummary,
    },
    ForwardList {
        forwards: Vec<PluginOwnedForwardSummary>,
    },
    ForwardStopped {
        forward: PluginOwnedForwardSummary,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginHostNavigationRequest {
    Sftp { path: String, edit: bool },
}

/// Sent by Core to the main application after an authorized plugin page action commits.
/// This event is never accepted as plugin output and never returned to its guest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostNavigationEvent {
    pub operation_id: OperationId,
    pub plugin_id: PluginId,
    /// The already-open SFTP subsystem adopted from the selected SSH session. The UI attaches to
    /// this exact session/generation and must not open a second Host connection.
    pub sftp_session: SftpSessionSummary,
    pub request: PluginHostNavigationRequest,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLogReadResult {
    pub path: String,
    pub start_offset: WireSequence,
    pub next_offset: WireSequence,
    pub total_size: WireSequence,
    pub reset: bool,
    pub text: String,
    pub invalid_utf8_replaced: bool,
}

impl std::fmt::Debug for PluginLogReadResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginLogReadResult")
            .field("path", &self.path)
            .field("start_offset", &self.start_offset)
            .field("next_offset", &self.next_offset)
            .field("total_size", &self.total_size)
            .field("reset", &self.reset)
            .field("invalid_utf8_replaced", &self.invalid_utf8_replaced)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginOwnedForwardState {
    Starting,
    Running,
    Failed,
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginActualForwardBind {
    Local { address: String, port: u16 },
    Remote { address: String, port: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOwnedForwardSummary {
    pub forward_handle: String,
    pub generation: WireSequence,
    pub state: PluginOwnedForwardState,
    pub actual_bind: Option<PluginActualForwardBind>,
    pub child_count: u32,
    pub listener_to_target_bytes: WireSequence,
    pub target_to_listener_bytes: WireSequence,
    pub stable_error: Option<String>,
    pub cleanup_uncertain: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_request_is_terminal_scoped_and_rejects_plugin_supplied_session_identity() {
        let accepted = serde_json::json!({
            "terminalHandle": "opaque-terminal",
            "reason": "inspect logs",
            "operation": {
                "kind": "logsRead",
                "files": [{
                    "path": "/var/log/app.log",
                    "offset": null,
                    "maxBytes": 8192
                }]
            }
        });
        assert!(serde_json::from_value::<PluginResourceOperationRequest>(accepted).is_ok());

        let request = serde_json::json!({
            "terminalHandle": "opaque-terminal",
            "reason": "inspect logs",
            "operation": {
                "kind": "forwardStart",
                "rule": {
                    "kind": "dynamic",
                    "localBindAddress": "127.0.0.1",
                    "localListenPort": 1080,
                    "sessionId": "forbidden"
                }
            }
        });
        assert!(serde_json::from_value::<PluginResourceOperationRequest>(request).is_err());
    }

    #[test]
    fn docker_terminal_open_uses_only_an_immutable_container_id_and_fixed_shell_enum() {
        let operation = PluginResourceOperation::DockerTerminalOpen {
            container_id: "a".repeat(64),
            shell: PluginDockerContainerShell::Sh,
        };
        let value = serde_json::to_value(operation).unwrap();
        assert_eq!(value["kind"], "dockerTerminalOpen");
        assert_eq!(value["containerId"], "a".repeat(64));
        assert_eq!(value["shell"], "sh");
    }

    #[test]
    fn log_offsets_remain_strings_on_the_json_wire() {
        let request = PluginResourceOperation::LogsRead {
            files: vec![PluginLogReadRequest {
                path: "/var/log/app.log".to_owned(),
                offset: Some(WireSequence::new(9_007_199_254_740_992)),
                max_bytes: 8192,
            }],
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["files"][0]["offset"], "9007199254740992");
    }

    #[test]
    fn log_text_is_redacted_from_debug_output() {
        let result = PluginLogReadResult {
            path: "/var/log/app.log".to_owned(),
            start_offset: WireSequence::new(0),
            next_offset: WireSequence::new(6),
            total_size: WireSequence::new(6),
            reset: false,
            text: "secret".to_owned(),
            invalid_utf8_replaced: false,
        };
        assert!(!format!("{result:?}").contains("secret"));
    }
}
