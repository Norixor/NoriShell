//! Public DTOs for Core-brokered, independently connected plugin SSH exec resources.
//!
//! These request types carry only a host-scoped opaque handle and bounded bytes. Core resolves the
//! saved Host, credentials, route, Known Hosts verifier and all authority from trusted state.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::PluginProcessOutputStream;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoteExecStartRequest {
    pub host_handle: String,
    pub command: String,
    pub timeout_ms: u32,
}

impl fmt::Debug for PluginRemoteExecStartRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginRemoteExecStartRequest")
            .field("host_handle", &self.host_handle)
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRemoteExecSendRequest {
    pub handle: String,
    pub data_base64: String,
    #[serde(default)]
    pub close_stdin: bool,
}

impl fmt::Debug for PluginRemoteExecSendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginRemoteExecSendRequest")
            .field("handle", &self.handle)
            .field("close_stdin", &self.close_stdin)
            .finish_non_exhaustive()
    }
}

/// Resource events emitted by Core for one independently connected SSH exec channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginRemoteExecEvent {
    RemoteExecOutput {
        stream: PluginProcessOutputStream,
        data_base64: String,
    },
    RemoteExecExited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        exit_code: Option<i64>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_fields_and_eof_request_keep_the_camel_case_abi() {
        let eof: PluginRemoteExecSendRequest = serde_json::from_value(serde_json::json!({
            "handle": "018f0000-0000-7000-8000-000000000001",
            "dataBase64": ""
        }))
        .expect("closeStdin defaults safely");
        assert!(!eof.close_stdin);
        let event = PluginRemoteExecEvent::RemoteExecOutput {
            stream: PluginProcessOutputStream::Stdout,
            data_base64: "YQ==".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(event).expect("event serializes"),
            serde_json::json!({
                "kind": "remoteExecOutput",
                "stream": "stdout",
                "dataBase64": "YQ=="
            })
        );
    }
}
