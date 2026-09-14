//! Typed, non-authorizing SFTP requests for the isolated plugin API.
//!
//! A plugin sees only Core-generated root, directory, entry, and cursor handles.
//! It never supplies a HostId, SSH/SFTP session ID, remote path, credential, or
//! host-key decision. Root authorization is deliberately outside this wire contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Every binary read is bounded independently of the caller payload limit.
pub const PLUGIN_SFTP_MAX_READ_BYTES: u16 = 16 * 1024;
/// Plugin uploads are streamed in bounded frames; base64 framing remains well
/// below the generic plugin-call limit.
pub const PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES: u16 = 16 * 1024;
pub const PLUGIN_SFTP_MAX_LIST_ENTRIES: u16 = 100;
/// A single approved root may stage at most this many plugin-supplied bytes at
/// once. Larger files must use an explicit future Core local-file capability.
pub const PLUGIN_SFTP_MAX_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// SFTP resource events are deliberately metadata-only. Binary data remains in
/// bounded `Read` replies or explicit upload chunks; it never enters the
/// shared resource-event queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSftpEvent {
    UploadStarted {
        upload_handle: String,
        #[ts(type = "number")]
        expected_bytes: u64,
    },
    UploadProgress {
        upload_handle: String,
        #[ts(type = "number")]
        received_bytes: u64,
        #[ts(type = "number")]
        expected_bytes: u64,
    },
    UploadCompleted {
        upload_handle: String,
        #[ts(type = "number")]
        total_bytes: u64,
    },
    DownloadProgress {
        entry_handle: String,
        #[ts(type = "number")]
        next_offset: u64,
        #[ts(type = "number")]
        total_bytes: u64,
        eof: bool,
    },
}

/// Every operation is against a previously opened Core-owned root resource.
/// Opening itself is a separate protected Plugin API operation, so this wire
/// contract has no Host, credential, raw path, or approval authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSftpOperation {
    /// With no `directory_handle`, lists the approved root. A child listing
    /// requires the exact parent directory and directory entry handles returned
    /// by a prior page. A continuation may not switch the selected child.
    List {
        root_handle: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        directory_handle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        child_entry_handle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        cursor: Option<String>,
        limit: u16,
    },
    Read {
        root_handle: String,
        directory_handle: String,
        entry_handle: String,
        offset: u64,
        length: u16,
    },
    /// Atomically replaces one previously listed regular file with a bounded
    /// binary payload after Core revalidates its exact listing precondition.
    WriteBinary {
        root_handle: String,
        directory_handle: String,
        entry_handle: String,
        precondition: PluginSftpObjectPrecondition,
        data_base64: String,
    },
    /// Creates a new remote file only if its target name remains absent. The
    /// returned upload handle accepts ordered bounded chunks before commit.
    UploadStart {
        root_handle: String,
        directory_handle: String,
        name: String,
        #[ts(type = "number")]
        expected_bytes: u64,
    },
    /// Replaces one previously listed regular file. The exact supplied facts
    /// form the compare-and-set precondition at start and commit.
    UploadReplaceStart {
        root_handle: String,
        directory_handle: String,
        entry_handle: String,
        precondition: PluginSftpObjectPrecondition,
        #[ts(type = "number")]
        expected_bytes: u64,
    },
    /// Appends exactly one bounded binary frame at `offset`. Core rejects gaps,
    /// duplicate offsets, and frames that exceed the approved total length.
    UploadChunk {
        root_handle: String,
        upload_handle: String,
        #[ts(type = "number")]
        offset: u64,
        data_base64: String,
    },
    /// Verifies the staged length and commits using the start mode's no-replace
    /// or precondition-checked atomic replacement semantics.
    UploadCommit {
        root_handle: String,
        upload_handle: String,
    },
    /// Explicitly removes a remote staging target. Closing the owning root also
    /// performs this cleanup before its independent transport disconnects.
    UploadAbort {
        root_handle: String,
        upload_handle: String,
    },
    CreateDirectory {
        root_handle: String,
        directory_handle: String,
        name: String,
    },
    CreateEmptyFile {
        root_handle: String,
        directory_handle: String,
        name: String,
    },
    RenameNoReplace {
        root_handle: String,
        source_directory_handle: String,
        source_entry_handle: String,
        source_precondition: PluginSftpObjectPrecondition,
        target_directory_handle: String,
        target_name: String,
    },
    Remove {
        root_handle: String,
        directory_handle: String,
        entry_handle: String,
        precondition: PluginSftpObjectPrecondition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginSftpResult {
    Opened {
        root_handle: String,
    },
    Page {
        directory_handle: String,
        entries: Vec<PluginSftpDirectoryEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        next_cursor: Option<String>,
    },
    Read {
        data_base64: String,
        offset: u64,
        next_offset: u64,
        total_bytes: u64,
        eof: bool,
        precondition: PluginSftpObjectPrecondition,
    },
    Written {
        #[ts(type = "number")]
        total_bytes: u64,
    },
    UploadStarted {
        upload_handle: String,
        #[ts(type = "number")]
        expected_bytes: u64,
    },
    UploadProgress {
        upload_handle: String,
        #[ts(type = "number")]
        received_bytes: u64,
        #[ts(type = "number")]
        expected_bytes: u64,
    },
    Uploaded {
        upload_handle: String,
        #[ts(type = "number")]
        total_bytes: u64,
    },
    UploadAborted {},
    Created {},
    Renamed {},
    Removed {},
}

/// Only regular files and real directories are projected. Symlinks and every
/// other remote object remain outside the plugin capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginSftpEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSftpDirectoryEntry {
    pub entry_handle: String,
    pub name: String,
    pub kind: PluginSftpEntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub modified_at_unix_ms: Option<i64>,
    pub precondition: PluginSftpObjectPrecondition,
}

/// Facts captured by Core from a directory page. A destructive operation must
/// echo the exact object facts; Core also revalidates them at execution time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSftpObjectPrecondition {
    pub kind: PluginSftpEntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub modified_at_unix_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_never_accept_guest_host_or_path_authority() {
        let accepted = serde_json::json!({
            "kind": "read",
            "rootHandle": "root",
            "directoryHandle": "directory",
            "entryHandle": "entry",
            "offset": 0,
            "length": 1,
        });
        assert!(serde_json::from_value::<PluginSftpOperation>(accepted).is_ok());
        for request in [
            serde_json::json!({"kind": "read", "rootHandle": "root", "directoryHandle": "directory", "entryHandle": "entry", "offset": 0, "length": 1, "hostId": "host"}),
            serde_json::json!({"kind": "read", "rootHandle": "root", "directoryHandle": "directory", "entryHandle": "entry", "offset": 0, "length": 1, "path": "/etc"}),
            serde_json::json!({"kind": "read", "rootHandle": "root", "directoryHandle": "d", "entryHandle": "e", "offset": 0, "length": 1, "sessionId": "s"}),
        ] {
            assert!(serde_json::from_value::<PluginSftpOperation>(request).is_err());
        }
    }

    #[test]
    fn binary_read_bound_is_part_of_the_contract() {
        assert_eq!(PLUGIN_SFTP_MAX_READ_BYTES, 16 * 1024);
        assert_eq!(PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES, 16 * 1024);
    }

    #[test]
    fn resource_events_keep_the_camel_case_abi() {
        let event = PluginSftpEvent::UploadProgress {
            upload_handle: "upload".to_owned(),
            received_bytes: 4,
            expected_bytes: 8,
        };
        assert_eq!(
            serde_json::to_value(event).expect("SFTP event serializes"),
            serde_json::json!({
                "kind": "uploadProgress",
                "uploadHandle": "upload",
                "receivedBytes": 4,
                "expectedBytes": 8,
            })
        );
    }
}
