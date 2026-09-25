use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    HostId, OperationId, RequestMeta, SftpSessionId, SshSessionId, TransferId, WireSequence,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpSessionState {
    Connecting,
    VerifyingHostKey,
    Authenticating,
    NeedsAuthentication,
    OpeningSubsystem,
    Ready,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpFailureCode {
    HostUnavailable,
    HostKeyRejected,
    HostKeyMismatch,
    VaultLocked,
    CredentialUnavailable,
    AuthenticationRejected,
    SubsystemRejected,
    TransportLost,
    UnsupportedPathEncoding,
    PermissionDenied,
    Protocol,
    CleanupUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpSessionFailure {
    pub code: SftpFailureCode,
    pub stage: String,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpSessionSummary {
    pub session_id: SftpSessionId,
    /// Present when the parent SSH session was opened from a saved Host.
    pub host_id: Option<HostId>,
    /// Present for a subsystem borrowed from an already-open user SSH session.
    pub parent_ssh_session: Option<SftpParentSshSession>,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub state: SftpSessionState,
    pub transfer_count: u32,
    pub active_transfer_count: u32,
    pub failure: Option<SftpSessionFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpParentSshSession {
    pub session_id: SshSessionId,
    pub generation: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpSessionOpenRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SftpSessionId,
    pub host_id: HostId,
    pub expected_host_state_version: WireSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpLocalBoundaryKind {
    UploadSource,
    DownloadTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalBoundaryRegisterRequest {
    pub meta: RequestMeta,
    pub kind: SftpLocalBoundaryKind,
    pub selected_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalBoundary {
    pub token: String,
    pub kind: SftpLocalBoundaryKind,
    pub display_name: String,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
}

/// An opaque Core-owned capability for one opened local directory. The path
/// supplied while registering it is never reused as transfer authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryCapability {
    pub directory_ref: String,
    pub revision: WireSequence,
    pub display_name: String,
    /// A bounded, valid UTF-8 local path which can be registered again after
    /// this opaque capability expires. It is absent for paths which cannot be
    /// represented safely; callers must never reconstruct it from display_name.
    pub rememberable_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryRegisterRequest {
    pub meta: RequestMeta,
    pub selected_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpLocalEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryEntry {
    pub entry_ref: String,
    pub display_name: String,
    pub kind: SftpLocalEntryKind,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    #[ts(type = "number | null")]
    pub modified_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryListRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub directory_ref: String,
    pub expected_revision: WireSequence,
    pub cursor: Option<Vec<u8>>,
    pub page_size: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryListing {
    pub directory_ref: String,
    pub revision: WireSequence,
    pub entries: Vec<SftpLocalDirectoryEntry>,
    pub next_cursor: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryOpenChildRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub parent_directory_ref: String,
    pub expected_parent_revision: WireSequence,
    pub entry_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryCreateChildRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub parent_directory_ref: String,
    pub expected_parent_revision: WireSequence,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpLocalDirectoryReleaseRequest {
    pub meta: RequestMeta,
    pub directory_ref: String,
    pub expected_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpSessionSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpSessionSnapshot {
    pub snapshot_revision: WireSequence,
    pub sessions: Vec<SftpSessionSummary>,
    pub transfers: Vec<SftpTransferSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpSessionDisconnectRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SftpSessionId,
    pub expected_generation: WireSequence,
}

/// Remote SFTP paths are opaque bytes on the wire. A concrete adapter that
/// cannot represent them must reject the operation; it must never use lossy
/// conversion or reinterpret them as local paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpRemotePath {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpRemoteEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpRemoteDirectoryEntry {
    pub entry_ref: String,
    pub path: SftpRemotePath,
    pub display_name: String,
    pub kind: SftpRemoteEntryKind,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    #[ts(type = "number | null")]
    pub modified_at_unix_ms: Option<i64>,
    pub permission_bits: Option<u32>,
}

/// Identity facts captured from the directory listing that authorized a
/// destructive mutation. Core re-runs lstat immediately before the mutation
/// and rejects stale facts instead of applying the user's intent to a replaced
/// remote object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpRemoteObjectPrecondition {
    pub kind: SftpRemoteEntryKind,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    #[ts(type = "number | null")]
    pub modified_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpDirectoryListRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SftpSessionId,
    pub expected_generation: WireSequence,
    pub path: SftpRemotePath,
    pub cursor: Option<Vec<u8>>,
    pub page_size: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpDirectoryListCancelRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SftpSessionId,
    pub expected_generation: WireSequence,
    pub path: SftpRemotePath,
    pub cursor: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpDirectoryListing {
    pub session_id: SftpSessionId,
    pub generation: WireSequence,
    pub directory_ref: String,
    pub path: SftpRemotePath,
    pub entries: Vec<SftpRemoteDirectoryEntry>,
    pub next_cursor: Option<Vec<u8>>,
}

/// Reads a file only through the opaque references returned by the current
/// directory listing. Core revalidates the entry immediately before the
/// bounded read, so a replaced remote object cannot be previewed accidentally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpFilePreviewRequest {
    pub meta: RequestMeta,
    pub session_id: SftpSessionId,
    pub expected_generation: WireSequence,
    pub directory_ref: String,
    pub entry_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpFilePreviewContent {
    Text {
        text: String,
        end_offset: WireSequence,
        editable: bool,
        truncated: bool,
        line_ending: SftpTextLineEnding,
    },
    Image {
        media_type: String,
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpTextLineEnding {
    Lf,
    CrLf,
    Cr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpFilePreview {
    pub session_id: SftpSessionId,
    pub generation: WireSequence,
    pub display_name: String,
    pub content: SftpFilePreviewContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpFileTailRequest {
    pub meta: RequestMeta,
    pub session_id: SftpSessionId,
    pub expected_generation: WireSequence,
    pub directory_ref: String,
    pub entry_ref: String,
    pub offset: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpFileTailResult {
    pub session_id: SftpSessionId,
    pub generation: WireSequence,
    pub start_offset: WireSequence,
    pub next_offset: WireSequence,
    pub total_size: WireSequence,
    pub reset: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpArchiveSource {
    pub path: SftpRemotePath,
    pub precondition: SftpRemoteObjectPrecondition,
    pub archive_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpFileMutation {
    CreateDirectory {
        path: SftpRemotePath,
    },
    CreateEmptyFile {
        path: SftpRemotePath,
    },
    WriteText {
        path: SftpRemotePath,
        precondition: SftpRemoteObjectPrecondition,
        text: String,
    },
    RenameNoReplace {
        source: SftpRemotePath,
        target: SftpRemotePath,
        source_precondition: SftpRemoteObjectPrecondition,
    },
    Delete {
        path: SftpRemotePath,
        precondition: SftpRemoteObjectPrecondition,
        irreversible_confirmed: bool,
    },
    SetPermissions {
        path: SftpRemotePath,
        precondition: SftpRemoteObjectPrecondition,
        expected_permission_bits: u32,
        mode: u32,
    },
    CreateZip {
        sources: Vec<SftpArchiveSource>,
        target: SftpRemotePath,
    },
    ExtractZip {
        source: SftpRemotePath,
        source_precondition: SftpRemoteObjectPrecondition,
        target_directory: SftpRemotePath,
    },
    DownloadUrl {
        url: String,
        target: SftpRemotePath,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpFileMutationRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SftpSessionId,
    pub expected_generation: WireSequence,
    pub mutation: SftpFileMutation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpFileMutationResult {
    pub session_id: SftpSessionId,
    pub generation: WireSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpTransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
pub enum SftpConflictPolicy {
    #[default]
    FailIfExists,
    ReplaceSafely,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpTransferEndpoint {
    LocalBoundaryToken { token: String, display_name: String },
    Remote { path: SftpRemotePath },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpTransferIntentSource {
    LocalDirectoryEntry {
        directory_ref: String,
        entry_ref: String,
    },
    RemoteFile {
        session_id: SftpSessionId,
        expected_generation: WireSequence,
        directory_ref: String,
        entry_ref: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpTransferIntentTarget {
    LocalDirectory {
        directory_ref: String,
    },
    RemoteDirectory {
        session_id: SftpSessionId,
        expected_generation: WireSequence,
        directory_ref: String,
    },
}

/// A pane-originated intent. Endpoint revisions are UI projection fences and
/// participate in the operation fingerprint; concrete Core capabilities and
/// SFTP generations remain the authority checked immediately before enqueue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentPrepareRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub source_pane_id: String,
    pub target_pane_id: String,
    pub source_endpoint_revision: WireSequence,
    pub target_endpoint_revision: WireSequence,
    pub source: SftpTransferIntentSource,
    pub target: SftpTransferIntentTarget,
    #[ts(type = "number")]
    pub expected_bytes: u64,
    pub conflict_policy: SftpConflictPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentPrepared {
    pub intent_token: String,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
    pub source_fence: SftpTransferEndpointFence,
    pub target_fence: SftpTransferEndpointFence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentEnqueueRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub intent_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpTransferEndpointFence {
    LocalCapability {
        directory_ref: String,
        revision: WireSequence,
    },
    RemoteSession {
        session_id: SftpSessionId,
        generation: WireSequence,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpTransferIntentDirection {
    Upload,
    Download,
    ServerToServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpTransferCommitOutcome {
    NotCommitted,
    Committed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpTransferIntentCleanupResidual {
    RemoteTemporaryTarget {
        session_id: SftpSessionId,
        generation: WireSequence,
        path: SftpRemotePath,
        display_path: String,
    },
    LocalTemporaryTarget {
        directory_ref: String,
        revision: WireSequence,
        display_name: String,
    },
    Unknown {
        display_name: String,
    },
}

/// Transfer intents are coordinated independently from each SftpSession actor.
/// This preserves both endpoint fences for remote-to-remote copy and prevents
/// either single-session summary from hiding the second live lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentSummary {
    pub transfer_id: TransferId,
    pub direction: SftpTransferIntentDirection,
    pub source_display_name: String,
    pub target_display_name: String,
    pub source_pane_id: String,
    pub target_pane_id: String,
    pub source_endpoint_revision: WireSequence,
    pub target_endpoint_revision: WireSequence,
    pub source_fence: SftpTransferEndpointFence,
    pub target_fence: SftpTransferEndpointFence,
    #[ts(type = "number")]
    pub expected_bytes: u64,
    #[ts(type = "number")]
    pub transferred_bytes: u64,
    #[ts(type = "number | null")]
    pub bytes_per_second: Option<u64>,
    #[ts(type = "number | null")]
    pub remaining_seconds: Option<u64>,
    pub state_revision: WireSequence,
    pub state: SftpTransferState,
    pub commit_outcome: SftpTransferCommitOutcome,
    pub failure_code: Option<SftpTransferFailureCode>,
    pub cleanup_residual: Option<SftpTransferIntentCleanupResidual>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentSnapshot {
    pub snapshot_revision: WireSequence,
    pub transfers: Vec<SftpTransferIntentSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentActionRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub expected_source_fence: SftpTransferEndpointFence,
    pub expected_target_fence: SftpTransferEndpointFence,
    pub expected_state_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentCleanupRetryRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub expected_source_fence: SftpTransferEndpointFence,
    pub expected_target_fence: SftpTransferEndpointFence,
    pub expected_state_revision: WireSequence,
    pub expected_target_host_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferIntentCleanupRetainRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub expected_source_fence: SftpTransferEndpointFence,
    pub expected_target_fence: SftpTransferEndpointFence,
    pub expected_state_revision: WireSequence,
    pub retain_remote_temporary_file_confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpTransferState {
    Queued,
    Preparing,
    Transferring,
    Verifying,
    Committing,
    Completed,
    PausedByDisconnect,
    Cancelling,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SftpTransferFailureCode {
    TargetExists,
    UnsafeReplaceUnsupported,
    CommitOutcomeUncertain,
    PermissionDenied,
    TransportLost,
    LengthMismatch,
    CleanupIncomplete,
    UnsupportedPathEncoding,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SftpCleanupResidual {
    RemoteTemporaryTarget {
        path: SftpRemotePath,
        display_path: String,
    },
    LocalTemporaryTarget {
        display_name: String,
    },
    Unknown {
        display_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferSummary {
    pub transfer_id: TransferId,
    pub session_id: SftpSessionId,
    pub generation: WireSequence,
    pub direction: SftpTransferDirection,
    pub source: SftpTransferEndpoint,
    pub target: SftpTransferEndpoint,
    #[ts(type = "number")]
    pub expected_bytes: u64,
    #[ts(type = "number")]
    pub transferred_bytes: u64,
    #[ts(type = "number | null")]
    pub bytes_per_second: Option<u64>,
    #[ts(type = "number | null")]
    pub remaining_seconds: Option<u64>,
    pub state_revision: WireSequence,
    pub state: SftpTransferState,
    pub commit_outcome: SftpTransferCommitOutcome,
    pub failure_code: Option<SftpTransferFailureCode>,
    pub cleanup_residual: Option<SftpCleanupResidual>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferEnqueueRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub session_id: SftpSessionId,
    pub expected_generation: WireSequence,
    pub direction: SftpTransferDirection,
    pub source: SftpTransferEndpoint,
    pub target: SftpTransferEndpoint,
    #[ts(type = "number")]
    pub expected_bytes: u64,
    pub conflict_policy: SftpConflictPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferActionRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpRemoteCleanupRetryRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub expected_host_state_version: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpRemoteCleanupRetainRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub transfer_id: TransferId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub retain_remote_temporary_file_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SftpSessionEvent {
    pub schema_version: u16,
    pub event_seq: WireSequence,
    pub session: SftpSessionSummary,
    pub transfer: Option<SftpTransferSummary>,
}

#[cfg(test)]
mod tests {
    use super::{
        SftpCleanupResidual, SftpFileMutation, SftpRemoteEntryKind, SftpRemoteObjectPrecondition,
        SftpRemotePath,
    };

    #[test]
    fn remote_path_preserves_non_utf8_bytes_on_the_wire() {
        let path = SftpRemotePath {
            bytes: vec![b'/', 0xff, b'a'],
        };
        let encoded = serde_json::to_string(&path).expect("serialize remote path");
        assert_eq!(encoded, "{\"bytes\":[47,255,97]}");
    }

    #[test]
    fn cleanup_residual_preserves_opaque_remote_path_bytes_and_safe_display() {
        let residual = SftpCleanupResidual::RemoteTemporaryTarget {
            path: SftpRemotePath {
                bytes: vec![b'/', 0xff, b'a'],
            },
            display_path: "/\\xffa".to_owned(),
        };
        let encoded = serde_json::to_value(residual).expect("serialize residual");
        assert_eq!(encoded["kind"], "remoteTemporaryTarget");
        assert_eq!(encoded["path"]["bytes"], serde_json::json!([47, 255, 97]));
        assert_eq!(encoded["displayPath"], "/\\xffa");
    }

    #[test]
    fn destructive_mutation_wire_carries_listing_identity_facts() {
        let mutation = SftpFileMutation::Delete {
            path: SftpRemotePath {
                bytes: b"/srv/file".to_vec(),
            },
            precondition: SftpRemoteObjectPrecondition {
                kind: SftpRemoteEntryKind::File,
                size: Some(42),
                modified_at_unix_ms: Some(1_700_000_000_000),
            },
            irreversible_confirmed: true,
        };
        let encoded = serde_json::to_value(mutation).expect("serialize mutation");
        assert_eq!(encoded["precondition"]["kind"], "file");
        assert_eq!(encoded["precondition"]["size"], 42);
        assert_eq!(
            encoded["precondition"]["modifiedAtUnixMs"],
            1_700_000_000_000_i64
        );
    }

    #[test]
    fn permission_mutation_wire_carries_original_bits_and_requested_rwx_mode() {
        let mutation = SftpFileMutation::SetPermissions {
            path: SftpRemotePath {
                bytes: b"/srv/file".to_vec(),
            },
            precondition: SftpRemoteObjectPrecondition {
                kind: SftpRemoteEntryKind::File,
                size: Some(42),
                modified_at_unix_ms: Some(1_700_000_000_000),
            },
            expected_permission_bits: 0o100640,
            mode: 0o600,
        };
        let encoded = serde_json::to_value(mutation).expect("serialize permission mutation");
        assert_eq!(encoded["kind"], "setPermissions");
        assert_eq!(encoded["expectedPermissionBits"], 0o100640);
        assert_eq!(encoded["mode"], 0o600);
    }
}
