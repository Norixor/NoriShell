//! Typed, plugin-private non-secret storage operations.
//!
//! Bytes cross the isolated JSON bridge as canonical standard Base64. Core
//! decodes them before persistence, so SQLite always stores the original BLOB
//! bytes rather than an encoded expansion.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginStorageOperation {
    KvGet {
        key: String,
    },
    KvSet {
        key: String,
        value_base64: String,
        expected_revision: u64,
    },
    KvDelete {
        key: String,
        expected_revision: u64,
    },
    KvList {
        prefix: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        cursor: Option<String>,
        limit: u16,
    },
    BlobRead {
        key: String,
        offset: u64,
        length: u16,
    },
    BlobWrite {
        key: String,
        offset: u64,
        data_base64: String,
        expected_revision: u64,
    },
    BlobDelete {
        key: String,
        expected_revision: u64,
    },
    CacheGet {
        key: String,
    },
    CacheSet {
        key: String,
        value_base64: String,
        ttl_ms: u64,
    },
    CacheDelete {
        key: String,
    },
    CacheClear {},
    SchemaGet {},
    SchemaCommit {
        expected_version: u64,
        new_version: u64,
        mutations: Vec<PluginStorageMutation>,
    },
}

/// A bounded non-secret migration batch. Core performs this declaration as a
/// single SQLite transaction; it never executes plugin-provided migration code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginStorageMutation {
    KvSet {
        key: String,
        value_base64: String,
        expected_revision: u64,
    },
    KvDelete {
        key: String,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginStorageResult {
    KvValue {
        state: PluginStorageState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        entry: Option<PluginStorageEntry>,
    },
    KvPage {
        state: PluginStorageState,
        entries: Vec<PluginStorageEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        next_cursor: Option<String>,
    },
    BlobRead {
        state: PluginStorageState,
        blob: PluginStorageBlobRead,
    },
    BlobWritten {
        state: PluginStorageState,
        key: String,
        revision: u64,
        total_bytes: u64,
    },
    CacheValue {
        state: PluginStorageState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        entry: Option<PluginStorageEntry>,
    },
    State {
        state: PluginStorageState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginStorageState {
    pub store_revision: u64,
    pub schema_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginStorageEntry {
    pub key: String,
    pub value_base64: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginStorageBlobRead {
    pub key: String,
    pub revision: u64,
    pub total_bytes: u64,
    pub offset: u64,
    pub data_base64: String,
    pub eof: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_operations_are_strict_and_do_not_accept_guest_identity() {
        let call = serde_json::from_str::<PluginStorageOperation>(
            r#"{"kind":"kvSet","key":"theme","valueBase64":"ZGFyaw==","expectedRevision":0}"#,
        )
        .expect("storage operation");
        assert!(matches!(call, PluginStorageOperation::KvSet { .. }));
        assert!(
            serde_json::from_str::<PluginStorageOperation>(
                r#"{"kind":"kvGet","pluginId":"other","key":"theme"}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn schema_batch_is_typed_and_round_trips() {
        let result = PluginStorageResult::KvPage {
            state: PluginStorageState {
                store_revision: 3,
                schema_version: 2,
            },
            entries: vec![PluginStorageEntry {
                key: "view".to_owned(),
                value_base64: "bGlzdA==".to_owned(),
                revision: 2,
            }],
            next_cursor: Some("cursor".to_owned()),
        };
        let encoded = serde_json::to_string(&result).expect("encode result");
        assert_eq!(
            serde_json::from_str::<PluginStorageResult>(&encoded).unwrap(),
            result
        );
    }
}
