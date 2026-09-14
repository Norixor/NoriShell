//! Typed, non-authorizing local-file requests for the isolated plugin API.
//!
//! `root_handle` is an opaque Core resource handle. A guest never supplies a
//! filesystem path outside that handle, nor an authority or a native path.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Requests are limited to 16 KiB payload/read chunks and a 64 MiB materialized file.
pub const PLUGIN_FILE_MAX_CHUNK_BYTES: u32 = 16 * 1024;
pub const PLUGIN_FILE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const PLUGIN_FILE_MAX_LIST_ENTRIES: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginFileOperation {
    Read {
        root_handle: String,
        relative_path: String,
        offset: u64,
    },
    /// A write is one bounded snapshot patch. When the target exists,
    /// `expected_fingerprint` is mandatory; an omitted value creates only a
    /// previously absent target. The service checks the fingerprint again just
    /// before its same-directory replace, but this is conflict detection rather
    /// than a cross-process compare-and-swap guarantee.
    Write {
        root_handle: String,
        relative_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        expected_fingerprint: Option<String>,
        offset: u64,
        data_base64: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        final_size: Option<u64>,
    },
    List {
        root_handle: String,
        relative_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        cursor: Option<String>,
        limit: u16,
    },
    /// The destination must not already exist. Rename is intentionally not an
    /// overwrite primitive.
    Rename {
        root_handle: String,
        from_path: String,
        to_path: String,
        expected_fingerprint: String,
    },
    Remove {
        root_handle: String,
        relative_path: String,
        expected_fingerprint: String,
        #[serde(default)]
        recursive: bool,
    },
    WatchStart {
        root_handle: String,
        relative_path: String,
        interval_ms: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginFileResult {
    Read {
        data_base64: String,
        fingerprint: String,
        eof: bool,
    },
    Written {
        fingerprint: String,
    },
    Listed {
        entries: Vec<PluginFileEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        next_cursor: Option<String>,
    },
    Renamed {
        fingerprint: String,
    },
    Removed {},
    WatchStarted {
        handle: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginFileEntry {
    pub name: String,
    pub kind: PluginFileEntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size: Option<u64>,
    /// Directory entries expose their current shallow fingerprint so a subsequent
    /// remove or rename can carry an explicit conflict-detection precondition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginFileEntryKind {
    File,
    Directory,
}

/// Emitted through the generic resource-event stream after watch polling
/// observes a material change. `None` means the watched entry no longer exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginFileWatchChange {
    pub root_handle: String,
    pub relative_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fingerprint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_contract_preserves_create_only_default_and_chunked_offsets() {
        let operation = PluginFileOperation::Write {
            root_handle: "root".to_owned(),
            relative_path: "notes/a.txt".to_owned(),
            expected_fingerprint: None,
            offset: u64::from(PLUGIN_FILE_MAX_CHUNK_BYTES),
            data_base64: "YQ==".to_owned(),
            final_size: Some(u64::from(PLUGIN_FILE_MAX_CHUNK_BYTES) + 1),
        };
        let encoded = serde_json::to_value(operation).unwrap();
        assert_eq!(encoded["kind"], "write");
        assert!(encoded.get("expectedFingerprint").is_none());
        assert_eq!(encoded["offset"], PLUGIN_FILE_MAX_CHUNK_BYTES);
    }

    #[test]
    fn file_requests_reject_guest_authority_fields() {
        assert!(
            serde_json::from_str::<PluginFileOperation>(
                r#"{"kind":"read","rootHandle":"r","relativePath":"a","offset":0,"path":"/secret"}"#
            )
            .is_err()
        );
    }
}
