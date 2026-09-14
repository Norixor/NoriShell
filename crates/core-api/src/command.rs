use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    ForwardSessionId, LocalSessionId, PluginId, RequestId, SftpSessionId, SshSessionId,
    TelnetSessionId, TransferId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RequestMeta {
    pub request_id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WindowCloseRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationExitRequest {
    pub meta: RequestMeta,
    /// Set only after the user reviewed the active resource list and
    /// explicitly confirmed that those resources may be disconnected.
    #[serde(default)]
    pub disconnect_active_resources: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ExitBlocker {
    PluginResource {
        plugin_id: PluginId,
        resource_kind: String,
        resource_id: String,
    },
    DesktopSession {
        session_id: String,
    },
    SshSession {
        session_id: SshSessionId,
    },
    LocalTerminal {
        session_id: LocalSessionId,
    },
    TelnetSession {
        session_id: TelnetSessionId,
    },
    SftpSession {
        session_id: SftpSessionId,
    },
    SftpTransfer {
        transfer_id: TransferId,
    },
    ForwardSession {
        session_id: ForwardSessionId,
    },
    PluginHost {
        plugin_id: PluginId,
        #[ts(type = "number")]
        process_id: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExitReadiness {
    pub can_exit: bool,
    pub blockers: Vec<ExitBlocker>,
}

#[cfg(test)]
mod tests {
    use super::{ApplicationExitRequest, ExitBlocker, ExitReadiness};
    use crate::{
        ForwardSessionId, LocalSessionId, PluginId, SftpSessionId, SshSessionId, TelnetSessionId,
        TransferId,
    };

    #[test]
    fn exit_readiness_serializes_every_active_resource_blocker() {
        let readiness = ExitReadiness {
            can_exit: false,
            blockers: vec![
                ExitBlocker::SshSession {
                    session_id: SshSessionId::new(),
                },
                ExitBlocker::LocalTerminal {
                    session_id: LocalSessionId::new(),
                },
                ExitBlocker::TelnetSession {
                    session_id: TelnetSessionId::new(),
                },
                ExitBlocker::SftpSession {
                    session_id: SftpSessionId::new(),
                },
                ExitBlocker::SftpTransfer {
                    transfer_id: TransferId::new(),
                },
                ExitBlocker::ForwardSession {
                    session_id: ForwardSessionId::new(),
                },
                ExitBlocker::PluginHost {
                    plugin_id: PluginId::parse("com.norishell.fixture").expect("plugin id"),
                    process_id: 42,
                },
            ],
        };

        let encoded = serde_json::to_string(&readiness).expect("serialize exit readiness");
        assert!(encoded.contains("\"kind\":\"sshSession\""));
        assert!(encoded.contains("\"kind\":\"localTerminal\""));
        assert!(encoded.contains("\"kind\":\"telnetSession\""));
        assert!(encoded.contains("\"kind\":\"sftpSession\""));
        assert!(encoded.contains("\"kind\":\"sftpTransfer\""));
        assert!(encoded.contains("\"kind\":\"forwardSession\""));
        assert!(encoded.contains("\"kind\":\"pluginHost\""));
        assert!(encoded.contains("\"sessionId\":"));
        assert!(encoded.contains("\"transferId\":"));
    }

    #[test]
    fn legacy_exit_request_defaults_to_non_destructive_readiness_check() {
        let request: ApplicationExitRequest = serde_json::from_value(serde_json::json!({
            "meta": { "requestId": "019d0000-0000-7000-8000-000000000501" }
        }))
        .expect("legacy request");
        assert!(!request.disconnect_active_resources);
    }
}
