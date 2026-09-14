//! Non-secret contracts for plugin-owned credentials; only protected input windows submit plaintext to Core.
use crate::WireSequence;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginCredentialOperation {
    Create {
        operation_id: String,
        idempotency_key: String,
        label: String,
        target: PluginCredentialTarget,
    },
    List {},
    Revoke {
        handle: String,
        expected_revision: WireSequence,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCredentialTarget {
    pub origin: String,
    pub injection: PluginCredentialInjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginCredentialInjection {
    Bearer {},
    Header { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCredentialSummary {
    pub handle: String,
    pub label: String,
    pub target: PluginCredentialTarget,
    pub revision: WireSequence,
    pub state: PluginCredentialState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginCredentialState {
    PendingVault,
    Ready,
    CleanupPending,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginCredentialResult {
    Created {
        credential: PluginCredentialSummary,
    },
    List {
        credentials: Vec<PluginCredentialSummary>,
    },
    Revoked {
        credential: PluginCredentialSummary,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNetworkCredentialRef {
    pub handle: String,
    pub expected_revision: WireSequence,
}
