use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{HostSummary, RequestMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum OpenSshImportDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshImportDiagnostic {
    pub code: String,
    pub severity: OpenSshImportDiagnosticSeverity,
    pub line: Option<u32>,
    pub message: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshImportEndpoint {
    pub address: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshImportJumpHop {
    pub endpoint: OpenSshImportEndpoint,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OpenSshImportRoutePreview {
    Direct,
    JumpChain { hops: Vec<OpenSshImportJumpHop> },
    HttpConnect { proxy: OpenSshImportEndpoint },
    Socks5 { proxy: OpenSshImportEndpoint },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshImportCandidate {
    pub candidate_id: String,
    pub alias: String,
    pub endpoint: Option<OpenSshImportEndpoint>,
    pub username: Option<String>,
    pub identity_file_hints: Vec<String>,
    pub route: OpenSshImportRoutePreview,
    pub diagnostics: Vec<OpenSshImportDiagnostic>,
    pub importable: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshConfigPreviewRequest {
    pub meta: RequestMeta,
    pub config_text: String,
}

impl std::fmt::Debug for OpenSshConfigPreviewRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenSshConfigPreviewRequest")
            .field("meta", &self.meta)
            .field("config_text", &"[REDACTED CONFIG TEXT]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshConfigPreviewResponse {
    pub snapshot_id: String,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
    pub candidates: Vec<OpenSshImportCandidate>,
    pub diagnostics: Vec<OpenSshImportDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshConfigCommitRequest {
    pub meta: RequestMeta,
    pub snapshot_id: String,
    pub candidate_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshConfigCommitResponse {
    pub hosts: Vec<HostSummary>,
}

#[cfg(test)]
mod tests {
    use super::OpenSshConfigPreviewRequest;
    use crate::{RequestId, RequestMeta};

    #[test]
    fn preview_debug_redacts_config_text() {
        let request = OpenSshConfigPreviewRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            config_text: "IdentityFile /Users/alice/.ssh/id_ed25519".to_owned(),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("IdentityFile"));
        assert!(!debug.contains("alice"));
    }
}
