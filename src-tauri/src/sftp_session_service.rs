//! SFTP session actor and production transport boundary.
//!
//! Each resource opens exactly one SFTP subsystem. User-created sessions resolve an immutable Host
//! profile and establish their own SSH transport; plugin-created sessions receive a fenced child
//! channel on an existing user SSH transport. Login automation never enters this module.

mod file_utilities;
mod local_capability;
mod plugin_access;
mod remote_copy;
mod transfer_intent_cleanup;

pub(crate) use plugin_access::{PluginSftpAccess, PluginSftpBinaryRead, PluginSftpUpload};

use transfer_intent_cleanup::{IntentCleanupExitAuthorization, IntentCleanupReplay};

use file_utilities::{
    ArchiveEntry, MAX_ARCHIVE_DEPTH, MAX_ARCHIVE_ENTRIES, MAX_ARCHIVE_INPUT_BYTES,
    MAX_NETWORK_DOWNLOAD_BYTES, decode_zip, encode_zip, validate_archive_name,
};

use local_capability::{
    LocalBoundary, LocalDirectoryCapability, LocalDirectoryCursor, LocalDirectoryEntryReference,
    LocalObjectIdentity, LocalObjectKind, rememberable_local_child_path,
    rememberable_local_directory_path, safe_local_name_display,
};

#[cfg(any(unix, windows))]
use local_capability::{
    LocalBoundaryCapability, LocalDirectoryCapabilityHandle, NativeLocalBoundary,
    NativeLocalDirectoryCapability, map_local_entry,
};
#[cfg(unix)]
use local_capability::{NativeFileIdentity, openat_identity};

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    io,
    path::Path,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(any(unix, windows))]
use std::{ffi::CString, path::PathBuf};

use norishell_app_persistence::KnownHostObservation;
use norishell_core_api::{self as wire, HostId, WireSequence};
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    AuthenticatedTransport, HostKeyDecision as TransportHostKeyDecision, HostKeyVerifier,
    IngressFailureKind, ObservedHostKey, SftpTransport, SharedSessionChannels, SharedSftpChannel,
    TransportCloseHandle, TransportError, VerifyFuture,
};
use russh_sftp::{
    client::{
        DirectoryStream as RemoteDirectoryStream, SftpSession as RemoteSftpClient,
        fs::File as RemoteSftpFile,
    },
    protocol::{FileAttributes as RemoteFileAttributes, OpenFlags},
};
use sha2::{Digest, Sha256};
use tauri::State;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, watch};
use zeroize::Zeroizing;

use crate::{
    connection_profile::{
        ConnectionProfileError, ResolvedSshConnectionBase, ResolvedTransportKeepalivePolicy,
        connection_has_vault_credentials, connection_requires_vault,
        resolve_long_lived_connection_profile,
    },
    host_service::HostService,
    lifecycle::LifecycleState,
    ssh_agent_service::SshAgentService,
    ssh_connection_orchestrator::{
        ConnectionPhase, ConnectionRouteStage, KeyboardInteractiveRequest,
        RoutedTransportHeartbeat, SshConnectionInteraction, SshConnectionOrchestrator,
    },
    ssh_session_service::SessionChannelLease,
    transient_credential_service::TransientCredentialService,
    vault_service::VaultService,
};

const MAX_ID_BYTES: usize = 128;
const MAX_HOST_ID_BYTES: usize = 128;
const MAX_REMOTE_PATH_BYTES: usize = 4096;
const MAX_LOCAL_BOUNDARY_TOKEN_BYTES: usize = 512;
const MAX_DIRECTORY_PAGE_SIZE: u16 = 512;
#[cfg(test)]
const MAX_KEYBOARD_PROMPTS: usize = 32;
const MAX_LABEL_BYTES: usize = 4096;
const MAX_RESUME_PREFIX_BYTES: usize = 8 * 1024 * 1024;
const TRANSFER_CHUNK_BYTES: usize = 64 * 1024;
const SFTP_PROTOCOL_OPERATION_TIMEOUT: Duration = Duration::from_secs(20);
const SFTP_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SFTP_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const MUTATION_LEDGER_CAPACITY: usize = 1_024;
const SFTP_V3_HANDLE_STABLE_RESUME_COMMIT: bool = false;
const LOCAL_DIRECTORY_CAPABILITY_TTL: Duration = Duration::from_secs(30 * 60);
const DIRECTORY_REFERENCE_TTL: Duration = Duration::from_secs(5 * 60);
const TRANSFER_INTENT_TOKEN_TTL: Duration = Duration::from_secs(30);
const MAX_LOCAL_DIRECTORY_CAPABILITIES: usize = 128;
const MAX_DIRECTORY_CURSORS: usize = 512;
const MAX_DIRECTORY_ENTRY_REFS: usize = 8_192;
const MAX_DIRECTORY_PAGES_PER_CURSOR: u32 = 1_024;
const MAX_PREPARED_TRANSFER_INTENTS: usize = 1_024;
const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_TEXT_PREVIEW_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const MAX_TAIL_INITIAL_BYTES: usize = 256 * 1024;
const MAX_TAIL_CHUNK_BYTES: usize = 64 * 1024;

fn sftp_v3_handle_stable_resume_commit_supported() -> bool {
    SFTP_V3_HANDLE_STABLE_RESUME_COMMIT
}

fn validate_js_safe_transfer_size(expected_bytes: u64) -> SftpRuntimeResult<()> {
    if expected_bytes > MAX_JS_SAFE_INTEGER {
        return Err(SftpRuntimeError::InvalidInput);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SftpRuntimeError {
    InvalidInput,
    StaleGeneration,
    InvalidState,
    #[allow(
        dead_code,
        reason = "plugin-facing error mapping keeps the state-machine failure contract stable"
    )]
    ChallengeMismatch,
    Conflict,
    UnsafeReplaceUnsupported,
    ProgressRegression,
    LengthMismatch,
    CleanupIncomplete,
    ResumeEvidenceMismatch,
    UnsupportedPathEncoding,
}

pub(crate) type SftpRuntimeResult<T> = Result<T, SftpRuntimeError>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SftpSessionId(String);

impl SftpSessionId {
    pub(crate) fn parse(value: impl Into<String>) -> SftpRuntimeResult<Self> {
        bounded_id(value.into()).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TransferId(String);

impl TransferId {
    pub(crate) fn parse(value: impl Into<String>) -> SftpRuntimeResult<Self> {
        bounded_id(value.into()).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SftpGeneration(u64);

impl SftpGeneration {
    pub(crate) fn new(value: u64) -> SftpRuntimeResult<Self> {
        (value > 0)
            .then_some(Self(value))
            .ok_or(SftpRuntimeError::InvalidInput)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SftpRouteStage {
    #[allow(
        dead_code,
        reason = "the focused SFTP interaction tests exercise only target hops"
    )]
    Ingress,
    #[allow(
        dead_code,
        reason = "the focused SFTP interaction tests exercise only target hops"
    )]
    JumpHost {
        hop_index: u8,
        host_id: String,
        endpoint: String,
    },
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SftpSessionState {
    Idle,
    Connecting,
    #[allow(
        dead_code,
        reason = "the actor-only interaction model is retained for its focused state-machine tests"
    )]
    VerifyingHostKey,
    #[allow(
        dead_code,
        reason = "the actor-only interaction model is retained for its focused state-machine tests"
    )]
    Authenticating,
    #[allow(
        dead_code,
        reason = "the actor-only interaction model is retained for its focused state-machine tests"
    )]
    NeedsAuthentication,
    OpeningSubsystem,
    Ready,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SftpFailureCode {
    HostKeyRejected,
    HostKeyMismatch,
    #[allow(
        dead_code,
        reason = "the actor-only interaction model is retained for its focused state-machine tests"
    )]
    VaultLocked,
    CredentialUnavailable,
    AuthenticationRejected,
    SubsystemRejected,
    TransportLost,
    Protocol,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostKeyChallenge {
    pub(crate) challenge_id: String,
    pub(crate) generation: SftpGeneration,
    pub(crate) route_stage: SftpRouteStage,
    pub(crate) algorithm: String,
    pub(crate) fingerprint_sha256: String,
    pub(crate) trusted_fingerprint_sha256: Option<String>,
}

#[cfg(test)]
#[allow(
    dead_code,
    reason = "the focused SFTP interaction model retains both explicit host-key decision outcomes"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKeyDecision {
    AcceptAndStore,
    Reject,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthenticationRequirement {
    VaultLocked,
    CredentialUnavailable,
    #[allow(
        dead_code,
        reason = "the focused SFTP interaction tests cover the other requirement paths"
    )]
    KeyboardInteractive,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyboardInteractivePrompt {
    pub(crate) prompt_index: u8,
    pub(crate) label: String,
    pub(crate) echo: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyboardInteractiveChallenge {
    pub(crate) challenge_id: String,
    pub(crate) generation: SftpGeneration,
    pub(crate) route_stage: SftpRouteStage,
    pub(crate) credential_ref_id: String,
    pub(crate) attempt_index: u8,
    pub(crate) round_index: u8,
    pub(crate) prompts: Vec<KeyboardInteractivePrompt>,
    pub(crate) expires_at_unix_ms: i64,
}

/// Public authentication response contains references only. Hidden answers
/// stay in the resource actor's bounded secret store and are consumed once by
/// the SSH connection interaction.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyboardInteractiveResponse {
    pub(crate) challenge_id: String,
    pub(crate) generation: SftpGeneration,
    pub(crate) round_index: u8,
    pub(crate) answer_ref_ids: Vec<String>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SftpInteraction {
    HostKeyReview(HostKeyChallenge),
    AuthenticationRequired {
        generation: SftpGeneration,
        requirement: AuthenticationRequirement,
    },
    KeyboardInteractive(KeyboardInteractiveChallenge),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemotePath(Vec<u8>);

impl RemotePath {
    pub(crate) fn parse(bytes: impl Into<Vec<u8>>) -> SftpRuntimeResult<Self> {
        let bytes = bytes.into();
        if bytes.is_empty()
            || bytes.len() > MAX_REMOTE_PATH_BYTES
            || bytes.contains(&0)
            || bytes[0] != b'/'
        {
            return Err(SftpRuntimeError::InvalidInput);
        }
        Ok(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn parent(&self) -> &[u8] {
        let split = self.0.iter().rposition(|byte| *byte == b'/').unwrap_or(0);
        if split == 0 { b"/" } else { &self.0[..split] }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteDirectoryEntry {
    pub(crate) path: RemotePath,
    pub(crate) kind: RemoteEntryKind,
    pub(crate) size: Option<u64>,
    pub(crate) modified_at_unix_ms: Option<i64>,
    pub(crate) permission_bits: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteObjectPrecondition {
    pub(crate) kind: RemoteEntryKind,
    pub(crate) size: Option<u64>,
    pub(crate) modified_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveSourcePlan {
    pub(crate) path: RemotePath,
    pub(crate) precondition: RemoteObjectPrecondition,
    pub(crate) archive_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationFence {
    pub(crate) operation_id: String,
    pub(crate) idempotency_key: String,
}

impl OperationFence {
    /// The command layer must key its bounded ledger by both fields plus the
    /// complete non-secret plan fingerprint. Same operation with different
    /// parameters is a conflict; uncertain non-idempotent results are refreshed
    /// from protocol facts rather than automatically reissued.
    pub(crate) fn parse(
        operation_id: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> SftpRuntimeResult<Self> {
        Ok(Self {
            operation_id: bounded_id(operation_id.into())?,
            idempotency_key: bounded_id(idempotency_key.into())?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryListingPlan {
    pub(crate) operation: OperationFence,
    pub(crate) generation: SftpGeneration,
    pub(crate) path: RemotePath,
    pub(crate) cursor: Option<Vec<u8>>,
    pub(crate) page_size: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteKind {
    File,
    EmptyDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileMutationPlan {
    CreateDirectory {
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
    },
    CreateEmptyFile {
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
    },
    WriteText {
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
        precondition: RemoteObjectPrecondition,
        bytes: Vec<u8>,
    },
    RenameNoReplace {
        operation: OperationFence,
        generation: SftpGeneration,
        source: RemotePath,
        target: RemotePath,
        source_precondition: RemoteObjectPrecondition,
    },
    DeleteConfirmed {
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
        kind: DeleteKind,
        precondition: RemoteObjectPrecondition,
    },
    CreateZip {
        operation: OperationFence,
        generation: SftpGeneration,
        sources: Vec<ArchiveSourcePlan>,
        target: RemotePath,
    },
    ExtractZip {
        operation: OperationFence,
        generation: SftpGeneration,
        source: RemotePath,
        source_precondition: RemoteObjectPrecondition,
        target_directory: RemotePath,
    },
    DownloadUrl {
        operation: OperationFence,
        generation: SftpGeneration,
        url: String,
        target: RemotePath,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ConflictPolicy {
    #[default]
    FailIfExists,
    ReplaceSafely,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferEndpoint {
    LocalBoundaryToken(String),
    Remote(RemotePath),
}

impl TransferEndpoint {
    fn validate(&self) -> SftpRuntimeResult<()> {
        match self {
            Self::LocalBoundaryToken(token) => {
                if token.trim().is_empty()
                    || token.len() > MAX_LOCAL_BOUNDARY_TOKEN_BYTES
                    || token.bytes().any(|byte| byte == 0)
                {
                    return Err(SftpRuntimeError::InvalidInput);
                }
            }
            Self::Remote(_) => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferState {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferFailureCode {
    TargetExists,
    UnsafeReplaceUnsupported,
    PermissionDenied,
    TransportLost,
    LengthMismatch,
    CleanupIncomplete,
    UnsupportedPathEncoding,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemporaryTarget {
    LocalBoundaryToken(String),
    Remote(RemotePath),
}

impl TemporaryTarget {
    fn validate_for(&self, target: &TransferEndpoint) -> SftpRuntimeResult<()> {
        match (self, target) {
            (Self::Remote(temp), TransferEndpoint::Remote(target))
                if temp != target && temp.parent() == target.parent() =>
            {
                Ok(())
            }
            (Self::LocalBoundaryToken(temp), TransferEndpoint::LocalBoundaryToken(target))
                if !temp.trim().is_empty()
                    && temp.len() <= MAX_LOCAL_BOUNDARY_TOKEN_BYTES
                    && temp != target =>
            {
                Ok(())
            }
            _ => Err(SftpRuntimeError::InvalidInput),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CleanupOutcome {
    Cleaned,
    Residual { opaque_location: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResumeEvidence {
    pub(crate) temporary_target: TemporaryTarget,
    pub(crate) verified_length: u64,
    pub(crate) prefix_checksum_verified: bool,
}

pub(crate) struct VerifiedRemoteResume {
    evidence: ResumeEvidence,
    file: RemoteSftpFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitFacts {
    pub(crate) final_length: u64,
    pub(crate) atomic_no_replace: bool,
    pub(crate) atomic_replace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferRecord {
    pub(crate) transfer_id: TransferId,
    pub(crate) generation: SftpGeneration,
    pub(crate) direction: TransferDirection,
    pub(crate) source: TransferEndpoint,
    pub(crate) target: TransferEndpoint,
    pub(crate) expected_bytes: u64,
    pub(crate) transferred_bytes: u64,
    started_at: Option<std::time::Instant>,
    pub(crate) conflict_policy: ConflictPolicy,
    pub(crate) state: TransferState,
    pub(crate) state_revision: u64,
    pub(crate) temporary_target: Option<TemporaryTarget>,
    pub(crate) target_existed: bool,
    pub(crate) safe_commit_supported: bool,
    pub(crate) failure_code: Option<TransferFailureCode>,
    pub(crate) cleanup_residual: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferPlan {
    pub(crate) transfer_id: TransferId,
    pub(crate) generation: SftpGeneration,
    pub(crate) direction: TransferDirection,
    pub(crate) source: TransferEndpoint,
    pub(crate) target: TransferEndpoint,
    pub(crate) conflict_policy: ConflictPolicy,
    pub(crate) expected_bytes: u64,
    /// Both ordinary creation and confirmed replacement write a distinct
    /// temporary target first. Direct target truncation is never authorized.
    pub(crate) require_temporary_target: bool,
    /// A non-zero offset is only produced after the production adapter has
    /// re-read and matched the complete trusted prefix for the exact temporary
    /// target on the current session generation.
    pub(crate) resume_from: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SftpSessionSummary {
    pub(crate) session_id: SftpSessionId,
    pub(crate) host_id: Option<String>,
    pub(crate) parent_ssh_session: Option<wire::SftpParentSshSession>,
    pub(crate) generation: Option<SftpGeneration>,
    pub(crate) state_revision: u64,
    pub(crate) state: SftpSessionState,
    pub(crate) failure_code: Option<SftpFailureCode>,
    #[cfg(test)]
    pub(crate) active_interaction: Option<SftpInteraction>,
    pub(crate) transport_id: Option<String>,
    pub(crate) subsystem_id: Option<String>,
}

#[derive(Debug)]
pub(crate) enum SftpProductionError {
    VaultUnavailable,
    Profile(ConnectionProfileError),
    Connection {
        error: Box<TransportError>,
        #[allow(
            dead_code,
            reason = "route stage is retained for safe future diagnostics without exposing it in current error DTOs"
        )]
        route_stage: ConnectionRouteStage,
    },
    Transport(Box<TransportError>),
    Runtime(SftpRuntimeError),
    NonUtf8RemotePath,
    ShutdownIncomplete(
        #[allow(
            dead_code,
            reason = "shutdown residual facts remain available to a lifecycle diagnostic surface"
        )]
        SftpShutdownFailure,
    ),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SftpShutdownFailure {
    pub(crate) timed_out_transfers: Vec<String>,
    pub(crate) disconnect_failed_sessions: Vec<String>,
    pub(crate) cleanup_residuals: Vec<SftpShutdownResidual>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SftpShutdownResidual {
    pub(crate) transfer_id: String,
    pub(crate) opaque_location: String,
}

impl SftpShutdownFailure {
    fn is_empty(&self) -> bool {
        self.timed_out_transfers.is_empty()
            && self.disconnect_failed_sessions.is_empty()
            && self.cleanup_residuals.is_empty()
    }

    fn into_result(self) -> Result<(), SftpProductionError> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(SftpProductionError::ShutdownIncomplete(self))
        }
    }
}

impl From<SftpRuntimeError> for SftpProductionError {
    fn from(error: SftpRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

/// SFTP-specific view of one saved Host snapshot.
///
/// The resolver deliberately reuses only the resource-neutral SSH base and
/// Transport Keepalive selection. Login automation is never materialized.
pub(crate) struct ResolvedSftpConnectionProfile {
    pub(crate) connection: ResolvedSshConnectionBase,
    pub(crate) transport_keepalive: Option<ResolvedTransportKeepalivePolicy>,
    pub(crate) revision_token: String,
}

pub(crate) struct SftpTransportFactory<'a> {
    hosts: &'a HostService,
    vault: &'a VaultService,
    transient_credentials: &'a TransientCredentialService,
    ssh_agent: &'a SshAgentService,
}

impl<'a> SftpTransportFactory<'a> {
    pub(crate) fn new(
        hosts: &'a HostService,
        vault: &'a VaultService,
        transient_credentials: &'a TransientCredentialService,
        ssh_agent: &'a SshAgentService,
    ) -> Self {
        Self {
            hosts,
            vault,
            transient_credentials,
            ssh_agent,
        }
    }

    /// Resolves a saved Host without parsing or validating Login Automation or
    /// Monitoring. SFTP owns a long-lived resource transport and therefore
    /// snapshots only the resource-neutral SSH base and Transport Keepalive.
    pub(crate) fn resolve_saved_host(
        &self,
        host_id: &HostId,
        expected_host_state_version: WireSequence,
    ) -> Result<ResolvedSftpConnectionProfile, SftpProductionError> {
        let profile =
            resolve_long_lived_connection_profile(self.hosts, host_id, expected_host_state_version)
                .map_err(SftpProductionError::Profile)?;
        let revision_token = format!("sftp-v1:{}", profile.connection.revision_token);
        Ok(ResolvedSftpConnectionProfile {
            connection: profile.connection,
            transport_keepalive: profile.transport_keepalive,
            revision_token,
        })
    }

    /// Establishes a new authenticated SSH transport for this SFTP resource.
    /// The returned value cannot open a PTY, Shell, or login-automation path.
    pub(crate) async fn connect<V, I>(
        &self,
        profile: ResolvedSftpConnectionProfile,
        verifier: Arc<V>,
        interaction: &mut I,
    ) -> Result<AuthenticatedSftpConnection<V>, SftpProductionError>
    where
        V: HostKeyVerifier,
        I: SshConnectionInteraction,
    {
        if connection_requires_vault(&profile.connection) && !self.vault.is_unlocked() {
            return Err(SftpProductionError::VaultUnavailable);
        }
        let has_vault_credentials = connection_has_vault_credentials(&profile.connection);
        let keepalive_interval = profile
            .transport_keepalive
            .as_ref()
            .map(|policy| Duration::from_secs(u64::from(policy.interval_seconds)));
        let connected =
            SshConnectionOrchestrator::new(self.vault, self.transient_credentials, self.ssh_agent)
                .connect(
                    profile.connection,
                    verifier,
                    interaction,
                    keepalive_interval,
                )
                .await
                .map_err(|failure| {
                    if has_vault_credentials
                        && !self.vault.is_unlocked()
                        && vault_locked_after_credential_failure(&failure.error)
                    {
                        SftpProductionError::VaultUnavailable
                    } else {
                        SftpProductionError::Connection {
                            error: Box::new(failure.error),
                            route_stage: failure.route_stage,
                        }
                    }
                })?;
        Ok(AuthenticatedSftpConnection {
            transport: connected.transport,
            transport_heartbeats: connected.transport_heartbeats,
            transport_keepalive: profile.transport_keepalive,
        })
    }
}

/// Authenticated, resource-private SSH connection before subsystem open.
pub(crate) struct AuthenticatedSftpConnection<V>
where
    V: HostKeyVerifier,
{
    transport: AuthenticatedTransport<V>,
    pub(crate) transport_heartbeats: Vec<RoutedTransportHeartbeat>,
    pub(crate) transport_keepalive: Option<ResolvedTransportKeepalivePolicy>,
}

impl<V> AuthenticatedSftpConnection<V>
where
    V: HostKeyVerifier,
{
    /// Opens exactly one SFTP subsystem and advances the actor only from facts
    /// returned by the SSH/SFTP protocols. Any post-open actor rejection closes
    /// the newly-created transport before returning.
    pub(crate) async fn open_subsystem(
        self,
        actor: &mut SftpSessionActor,
        generation: SftpGeneration,
    ) -> Result<SftpProductionSession<V>, SftpProductionError> {
        let transport_id = uuid::Uuid::new_v4().to_string();
        actor.authenticated_transport_ready(generation, transport_id.clone())?;
        let subsystem = match self.transport.open_sftp().await {
            Ok(subsystem) => subsystem,
            Err(error) => {
                let failure = map_transport_failure(&error);
                actor.connection_failed(generation, failure)?;
                return Err(SftpProductionError::Transport(Box::new(error)));
            }
        };
        let subsystem_id = uuid::Uuid::new_v4().to_string();
        if let Err(error) = actor.subsystem_opened(generation, subsystem_id.clone()) {
            let _ = subsystem.disconnect().await;
            return Err(SftpProductionError::Runtime(error));
        }
        Ok(SftpProductionSession {
            generation,
            transport: SftpLiveTransport::Dedicated(subsystem),
            transport_heartbeats: self.transport_heartbeats,
            transport_keepalive: self.transport_keepalive,
        })
    }
}

pub(crate) struct DirectoryPage {
    pub(crate) entries: Vec<RemoteDirectoryEntry>,
    continuation: Option<RemoteDirectoryStream>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteTransferPreparationFacts {
    pub(crate) target_existed: bool,
    pub(crate) temporary_target_existed: bool,
    pub(crate) temporary_target_length: Option<u64>,
    /// The current SFTP v3 adapter can use the OpenSSH hardlink extension for
    /// atomic no-replace, but cannot prove an atomic replacement of an
    /// existing target.
    pub(crate) safe_commit_supported: bool,
}

/// Live SFTP subsystem for exactly one session generation.
pub(crate) struct SftpProductionSession<V>
where
    V: HostKeyVerifier,
{
    generation: SftpGeneration,
    transport: SftpLiveTransport<V>,
    pub(crate) transport_heartbeats: Vec<RoutedTransportHeartbeat>,
    pub(crate) transport_keepalive: Option<ResolvedTransportKeepalivePolicy>,
}

enum SftpLiveTransport<V>
where
    V: HostKeyVerifier,
{
    Dedicated(SftpTransport<V>),
    Shared(SharedSftpChannel),
}

impl<V> SftpLiveTransport<V>
where
    V: HostKeyVerifier,
{
    fn client(&self) -> &RemoteSftpClient {
        match self {
            Self::Dedicated(transport) => transport.client(),
            Self::Shared(channel) => channel.client(),
        }
    }

    fn dedicated_close_handle(&self) -> Option<TransportCloseHandle> {
        match self {
            Self::Dedicated(transport) => Some(transport.close_handle()),
            Self::Shared(_) => None,
        }
    }

    async fn disconnect(self) -> Result<(), TransportError> {
        match self {
            Self::Dedicated(transport) => transport.disconnect().await,
            Self::Shared(channel) if channel.is_closed() => Ok(()),
            Self::Shared(channel) => channel.close().await,
        }
    }
}

#[derive(Clone)]
struct SftpDirectoryClient {
    generation: SftpGeneration,
    client: RemoteSftpClient,
}

impl SftpDirectoryClient {
    async fn list_directory(
        &self,
        plan: &DirectoryListingPlan,
    ) -> Result<DirectoryPage, SftpProductionError> {
        if self.generation != plan.generation || plan.cursor.is_some() {
            return Err(SftpRuntimeError::StaleGeneration.into());
        }
        let path = remote_path_utf8(&plan.path)?;
        let mut stream = tokio::time::timeout(
            SFTP_PROTOCOL_OPERATION_TIMEOUT,
            self.client.open_dir_stream(path),
        )
        .await
        .map_err(|_| sftp_protocol_error())?
        .map_err(|_| sftp_protocol_error())?;
        let page = match tokio::time::timeout(
            SFTP_PROTOCOL_OPERATION_TIMEOUT,
            stream.next_page(usize::from(plan.page_size)),
        )
        .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(_)) | Err(_) => {
                let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, stream.close()).await;
                return Err(sftp_protocol_error());
            }
        };
        let page_entries = match map_remote_directory_page(&plan.path, page) {
            Ok(entries) => entries,
            Err(error) => {
                let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, stream.close()).await;
                return Err(error);
            }
        };
        let continuation = (!stream.is_complete()).then_some(stream);
        Ok(DirectoryPage {
            entries: page_entries,
            continuation,
        })
    }
}

impl<V> SftpProductionSession<V>
where
    V: HostKeyVerifier,
{
    fn directory_client(&self) -> SftpDirectoryClient {
        SftpDirectoryClient {
            generation: self.generation,
            client: self.transport.client().clone(),
        }
    }

    pub(crate) async fn execute_mutation(
        &self,
        plan: &FileMutationPlan,
    ) -> Result<(), SftpProductionError> {
        let generation = match plan {
            FileMutationPlan::CreateDirectory { generation, .. }
            | FileMutationPlan::CreateEmptyFile { generation, .. }
            | FileMutationPlan::WriteText { generation, .. }
            | FileMutationPlan::RenameNoReplace { generation, .. }
            | FileMutationPlan::DeleteConfirmed { generation, .. }
            | FileMutationPlan::CreateZip { generation, .. }
            | FileMutationPlan::ExtractZip { generation, .. }
            | FileMutationPlan::DownloadUrl { generation, .. } => *generation,
        };
        self.require_generation(generation)?;
        match plan {
            FileMutationPlan::CreateDirectory { path, .. } => {
                self.bounded_protocol(self.transport.client().create_dir(remote_path_utf8(path)?))
                    .await
            }
            FileMutationPlan::CreateEmptyFile { path, .. } => {
                let path = remote_path_utf8(path)?;
                let file = self
                    .bounded_protocol(self.transport.client().open_with_flags(
                        path,
                        OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                    ))
                    .await?;
                self.bounded_protocol(file.close()).await
            }
            FileMutationPlan::WriteText {
                operation,
                path,
                precondition,
                bytes,
                ..
            } => {
                let target_path = remote_path_utf8(path)?;
                let temporary_target = remote_edit_temporary_path(path, &operation.operation_id)?;
                let temporary_path = remote_path_utf8(&temporary_target)?;
                let metadata = self
                    .bounded_protocol(self.transport.client().symlink_metadata(target_path))
                    .await?;
                let observed = remote_precondition_from_metadata(&metadata);
                if &observed != precondition || observed.kind != RemoteEntryKind::File {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                if self
                    .bounded_protocol(self.transport.client().try_exists(temporary_path))
                    .await?
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                let expected_length =
                    u64::try_from(bytes.len()).map_err(|_| SftpRuntimeError::InvalidInput)?;
                let write_result = async {
                    let mut file = self
                        .bounded_protocol(self.transport.client().open_with_flags(
                            temporary_path,
                            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                        ))
                        .await?;
                    self.bounded_protocol(file.write_all(bytes)).await?;
                    self.bounded_protocol(file.sync_all()).await?;
                    let written = self.bounded_protocol(file.metadata()).await?.len();
                    self.bounded_protocol(file.close()).await?;
                    if written != expected_length {
                        return Err(SftpRuntimeError::LengthMismatch.into());
                    }
                    Ok::<(), SftpProductionError>(())
                }
                .await;
                if let Err(error) = write_result {
                    let _ = self
                        .cleanup_temporary_target(generation, &temporary_target)
                        .await;
                    return Err(error);
                }
                let commit_result = async {
                    let current = self
                        .bounded_protocol(self.transport.client().symlink_metadata(target_path))
                        .await?;
                    if remote_precondition_from_metadata(&current) != *precondition {
                        return Err(SftpRuntimeError::Conflict.into());
                    }
                    let replaced = self
                        .bounded_protocol(
                            self.transport
                                .client()
                                .posix_rename(temporary_path, target_path),
                        )
                        .await?;
                    if !replaced {
                        return Err(SftpRuntimeError::UnsafeReplaceUnsupported.into());
                    }
                    let final_length = self
                        .bounded_protocol(self.transport.client().symlink_metadata(target_path))
                        .await?
                        .len();
                    if final_length != expected_length {
                        return Err(SftpRuntimeError::LengthMismatch.into());
                    }
                    Ok::<(), SftpProductionError>(())
                }
                .await;
                if let Err(error) = commit_result {
                    let _ = self
                        .cleanup_temporary_target(generation, &temporary_target)
                        .await;
                    return Err(error);
                }
                Ok(())
            }
            FileMutationPlan::RenameNoReplace {
                source,
                target,
                source_precondition,
                ..
            } => {
                let source = remote_path_utf8(source)?;
                let target = remote_path_utf8(target)?;
                let metadata = self
                    .bounded_protocol(self.transport.client().symlink_metadata(source))
                    .await?;
                let observed = RemoteObjectPrecondition {
                    kind: remote_entry_kind_from_flags(
                        metadata.is_regular(),
                        metadata.is_dir(),
                        metadata.is_symlink(),
                    ),
                    size: metadata.size,
                    modified_at_unix_ms: metadata.mtime.map(|value| i64::from(value) * 1_000),
                };
                if &observed != source_precondition {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                if observed.kind != RemoteEntryKind::File {
                    return Err(SftpRuntimeError::UnsafeReplaceUnsupported.into());
                }
                if self
                    .bounded_protocol(self.transport.client().try_exists(target))
                    .await?
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                let linked = self
                    .bounded_protocol(self.transport.client().hardlink(source, target))
                    .await
                    .map_err(|_| SftpProductionError::Runtime(SftpRuntimeError::Conflict))?;
                if !linked {
                    return Err(SftpRuntimeError::UnsafeReplaceUnsupported.into());
                }
                self.bounded_protocol(self.transport.client().remove_file(source))
                    .await
                    .map_err(|_| SftpRuntimeError::CleanupIncomplete.into())
            }
            FileMutationPlan::DeleteConfirmed {
                path,
                kind,
                precondition,
                ..
            } => {
                let path = remote_path_utf8(path)?;
                let metadata = self
                    .bounded_protocol(self.transport.client().symlink_metadata(path))
                    .await?;
                let observed = RemoteObjectPrecondition {
                    kind: remote_entry_kind_from_flags(
                        metadata.is_regular(),
                        metadata.is_dir(),
                        metadata.is_symlink(),
                    ),
                    size: metadata.size,
                    modified_at_unix_ms: metadata.mtime.map(|value| i64::from(value) * 1_000),
                };
                if &observed != precondition
                    || !matches!(
                        (kind, &observed.kind),
                        (DeleteKind::EmptyDirectory, RemoteEntryKind::Directory)
                            | (DeleteKind::File, RemoteEntryKind::File)
                            | (DeleteKind::File, RemoteEntryKind::Symlink)
                            | (DeleteKind::File, RemoteEntryKind::Other)
                    )
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                match kind {
                    DeleteKind::File => {
                        self.bounded_protocol(self.transport.client().remove_file(path))
                            .await
                    }
                    DeleteKind::EmptyDirectory => {
                        self.bounded_protocol(self.transport.client().remove_dir(path))
                            .await
                    }
                }
            }
            FileMutationPlan::CreateZip {
                operation,
                sources,
                target,
                ..
            } => {
                let entries = self.collect_archive_entries(generation, sources).await?;
                let archive = tokio::task::spawn_blocking(move || encode_zip(entries))
                    .await
                    .map_err(|_| SftpRuntimeError::InvalidState)??;
                self.write_new_remote_file(generation, operation, target, &archive)
                    .await
            }
            FileMutationPlan::ExtractZip {
                source,
                source_precondition,
                target_directory,
                ..
            } => {
                let archive = self
                    .read_remote_file_preview(
                        generation,
                        source,
                        source_precondition,
                        0,
                        file_utilities::MAX_ARCHIVE_OUTPUT_BYTES,
                    )
                    .await?;
                let entries = tokio::task::spawn_blocking(move || decode_zip(archive))
                    .await
                    .map_err(|_| SftpRuntimeError::InvalidState)??;
                self.extract_archive_entries(generation, target_directory, entries)
                    .await
            }
            FileMutationPlan::DownloadUrl {
                operation,
                url,
                target,
                ..
            } => {
                self.download_url_to_remote_file(generation, operation, url, target)
                    .await
            }
        }
    }

    async fn collect_archive_entries(
        &self,
        generation: SftpGeneration,
        sources: &[ArchiveSourcePlan],
    ) -> Result<Vec<ArchiveEntry>, SftpProductionError> {
        self.require_generation(generation)?;
        let mut entries = Vec::new();
        let mut total_bytes = 0usize;
        let mut pending = Vec::new();
        for source in sources.iter().rev() {
            pending.push((
                source.path.clone(),
                source.archive_name.clone(),
                source.precondition.clone(),
                1usize,
                true,
            ));
        }
        while let Some((path, archive_name, expected, depth, is_root)) = pending.pop() {
            if depth > MAX_ARCHIVE_DEPTH || entries.len() >= MAX_ARCHIVE_ENTRIES {
                return Err(SftpRuntimeError::InvalidInput.into());
            }
            validate_archive_name(&archive_name)?;
            let metadata = self
                .bounded_protocol(
                    self.transport
                        .client()
                        .symlink_metadata(remote_path_utf8(&path)?),
                )
                .await?;
            let observed = remote_precondition_from_metadata(&metadata);
            if is_root && observed != expected {
                return Err(SftpRuntimeError::Conflict.into());
            }
            match observed.kind {
                RemoteEntryKind::File => {
                    let size = usize::try_from(metadata.len())
                        .map_err(|_| SftpRuntimeError::InvalidInput)?;
                    total_bytes = total_bytes
                        .checked_add(size)
                        .ok_or(SftpRuntimeError::InvalidInput)?;
                    if total_bytes > MAX_ARCHIVE_INPUT_BYTES {
                        return Err(SftpRuntimeError::InvalidInput.into());
                    }
                    let bytes = self
                        .read_remote_file_preview(
                            generation,
                            &path,
                            &observed,
                            0,
                            MAX_ARCHIVE_INPUT_BYTES
                                .saturating_sub(total_bytes)
                                .saturating_add(size),
                        )
                        .await?;
                    entries.push(ArchiveEntry {
                        name: archive_name,
                        is_directory: false,
                        bytes,
                    });
                }
                RemoteEntryKind::Directory => {
                    entries.push(ArchiveEntry {
                        name: archive_name.clone(),
                        is_directory: true,
                        bytes: Vec::new(),
                    });
                    let mut stream = self
                        .bounded_protocol(
                            self.transport
                                .client()
                                .open_dir_stream(remote_path_utf8(&path)?),
                        )
                        .await?;
                    loop {
                        let page = self.bounded_protocol(stream.next_page(256)).await?;
                        let child_entries = map_remote_directory_page(&path, page)?;
                        let complete = stream.is_complete();
                        for child in child_entries.into_iter().rev() {
                            let child_name = remote_file_name(&child.path)?;
                            let child_archive_name = format!("{archive_name}/{child_name}");
                            let child_precondition = RemoteObjectPrecondition {
                                kind: child.kind,
                                size: child.size,
                                modified_at_unix_ms: child.modified_at_unix_ms,
                            };
                            if matches!(
                                child_precondition.kind,
                                RemoteEntryKind::Symlink | RemoteEntryKind::Other
                            ) {
                                let _ = self.bounded_protocol(stream.close()).await;
                                return Err(SftpRuntimeError::InvalidInput.into());
                            }
                            pending.push((
                                child.path,
                                child_archive_name,
                                child_precondition,
                                depth.saturating_add(1),
                                false,
                            ));
                        }
                        if complete {
                            break;
                        }
                        if pending.len().saturating_add(entries.len()) > MAX_ARCHIVE_ENTRIES {
                            let _ = self.bounded_protocol(stream.close()).await;
                            return Err(SftpRuntimeError::InvalidInput.into());
                        }
                    }
                    let _ = self.bounded_protocol(stream.close()).await;
                }
                RemoteEntryKind::Symlink | RemoteEntryKind::Other => {
                    return Err(SftpRuntimeError::InvalidInput.into());
                }
            }
        }
        Ok(entries)
    }

    async fn write_new_remote_file(
        &self,
        generation: SftpGeneration,
        operation: &OperationFence,
        target: &RemotePath,
        bytes: &[u8],
    ) -> Result<(), SftpProductionError> {
        self.require_generation(generation)?;
        let target_path = remote_path_utf8(target)?;
        let temporary_target = remote_edit_temporary_path(target, &operation.operation_id)?;
        let temporary_path = remote_path_utf8(&temporary_target)?;
        if self
            .bounded_protocol(self.transport.client().try_exists(target_path))
            .await?
            || self
                .bounded_protocol(self.transport.client().try_exists(temporary_path))
                .await?
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let expected_length =
            u64::try_from(bytes.len()).map_err(|_| SftpRuntimeError::InvalidInput)?;
        let write_result = async {
            let mut file = self
                .bounded_protocol(self.transport.client().open_with_flags(
                    temporary_path,
                    OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                ))
                .await?;
            self.bounded_protocol(file.write_all(bytes)).await?;
            self.bounded_protocol(file.sync_all()).await?;
            let written = self.bounded_protocol(file.metadata()).await?.len();
            self.bounded_protocol(file.close()).await?;
            if written != expected_length {
                return Err(SftpRuntimeError::LengthMismatch.into());
            }
            Ok::<(), SftpProductionError>(())
        }
        .await;
        if let Err(error) = write_result {
            let _ = self
                .cleanup_temporary_target(generation, &temporary_target)
                .await;
            return Err(error);
        }
        let linked = self
            .bounded_protocol(
                self.transport
                    .client()
                    .hardlink(temporary_path, target_path),
            )
            .await
            .map_err(|_| SftpRuntimeError::Conflict)?;
        if !linked {
            let _ = self
                .cleanup_temporary_target(generation, &temporary_target)
                .await;
            return Err(SftpRuntimeError::UnsafeReplaceUnsupported.into());
        }
        self.bounded_protocol(self.transport.client().remove_file(temporary_path))
            .await
            .map_err(|_| SftpRuntimeError::CleanupIncomplete)?;
        let final_length = self
            .bounded_protocol(self.transport.client().symlink_metadata(target_path))
            .await?
            .len();
        if final_length != expected_length {
            return Err(SftpRuntimeError::LengthMismatch.into());
        }
        Ok(())
    }

    async fn extract_archive_entries(
        &self,
        generation: SftpGeneration,
        target_directory: &RemotePath,
        entries: Vec<ArchiveEntry>,
    ) -> Result<(), SftpProductionError> {
        self.require_generation(generation)?;
        let root = remote_path_utf8(target_directory)?;
        if self
            .bounded_protocol(self.transport.client().try_exists(root))
            .await?
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        self.bounded_protocol(self.transport.client().create_dir(root))
            .await?;
        let mut created_directories = BTreeSet::from([String::new()]);
        for entry in entries {
            let components = entry.name.split('/').collect::<Vec<_>>();
            let parent_count = if entry.is_directory {
                components.len()
            } else {
                components.len().saturating_sub(1)
            };
            for index in 1..=parent_count {
                let relative = components[..index].join("/");
                if created_directories.insert(relative.clone()) {
                    let path = remote_archive_child_path(target_directory, &relative)?;
                    self.bounded_protocol(
                        self.transport.client().create_dir(remote_path_utf8(&path)?),
                    )
                    .await
                    .map_err(|_| SftpRuntimeError::CleanupIncomplete)?;
                }
            }
            if !entry.is_directory {
                let target = remote_archive_child_path(target_directory, &entry.name)?;
                let target_path = remote_path_utf8(&target)?;
                let mut file = self
                    .bounded_protocol(self.transport.client().open_with_flags(
                        target_path,
                        OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                    ))
                    .await
                    .map_err(|_| SftpRuntimeError::CleanupIncomplete)?;
                if self
                    .bounded_protocol(file.write_all(&entry.bytes))
                    .await
                    .is_err()
                    || self.bounded_protocol(file.sync_all()).await.is_err()
                    || self.bounded_protocol(file.close()).await.is_err()
                {
                    return Err(SftpRuntimeError::CleanupIncomplete.into());
                }
            }
        }
        Ok(())
    }

    async fn download_url_to_remote_file(
        &self,
        generation: SftpGeneration,
        operation: &OperationFence,
        url: &str,
        target: &RemotePath,
    ) -> Result<(), SftpProductionError> {
        self.require_generation(generation)?;
        let parsed = reqwest::Url::parse(url).map_err(|_| SftpRuntimeError::InvalidInput)?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.host_str().is_none()
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 || attempt.url().scheme() != "https" {
                    attempt.error("redirect rejected by SFTP download policy")
                } else {
                    attempt.follow()
                }
            }))
            .timeout(Duration::from_secs(5 * 60))
            .build()
            .map_err(|_| SftpRuntimeError::InvalidState)?;
        let mut response = client
            .get(parsed)
            .send()
            .await
            .map_err(|_| SftpRuntimeError::InvalidState)?
            .error_for_status()
            .map_err(|_| SftpRuntimeError::InvalidInput)?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_NETWORK_DOWNLOAD_BYTES)
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let target_path = remote_path_utf8(target)?;
        let temporary_target = remote_edit_temporary_path(target, &operation.operation_id)?;
        let temporary_path = remote_path_utf8(&temporary_target)?;
        if self
            .bounded_protocol(self.transport.client().try_exists(target_path))
            .await?
            || self
                .bounded_protocol(self.transport.client().try_exists(temporary_path))
                .await?
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let mut file = self
            .bounded_protocol(self.transport.client().open_with_flags(
                temporary_path,
                OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            ))
            .await?;
        let mut downloaded = 0u64;
        let write_result = async {
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| SftpRuntimeError::InvalidState)?
            {
                downloaded = downloaded
                    .checked_add(
                        u64::try_from(chunk.len()).map_err(|_| SftpRuntimeError::InvalidInput)?,
                    )
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                if downloaded > MAX_NETWORK_DOWNLOAD_BYTES {
                    return Err(SftpRuntimeError::InvalidInput.into());
                }
                self.bounded_protocol(file.write_all(&chunk)).await?;
            }
            self.bounded_protocol(file.sync_all()).await?;
            let written = self.bounded_protocol(file.metadata()).await?.len();
            self.bounded_protocol(file.close()).await?;
            if written != downloaded {
                return Err(SftpRuntimeError::LengthMismatch.into());
            }
            Ok::<(), SftpProductionError>(())
        }
        .await;
        if let Err(error) = write_result {
            let _ = self
                .cleanup_temporary_target(generation, &temporary_target)
                .await;
            return Err(error);
        }
        let linked = self
            .bounded_protocol(
                self.transport
                    .client()
                    .hardlink(temporary_path, target_path),
            )
            .await
            .map_err(|_| SftpRuntimeError::Conflict)?;
        if !linked {
            let _ = self
                .cleanup_temporary_target(generation, &temporary_target)
                .await;
            return Err(SftpRuntimeError::UnsafeReplaceUnsupported.into());
        }
        Ok(self
            .bounded_protocol(self.transport.client().remove_file(temporary_path))
            .await
            .map_err(|_| SftpRuntimeError::CleanupIncomplete)?)
    }

    /// Reads target/temp existence and length from the SFTP server. The v3
    /// adapter reports safe commit as unsupported instead of inferring it from
    /// a successful ordinary rename.
    pub(crate) async fn inspect_transfer_targets(
        &self,
        generation: SftpGeneration,
        target: &RemotePath,
        temporary_target: &RemotePath,
    ) -> Result<RemoteTransferPreparationFacts, SftpProductionError> {
        self.require_generation(generation)?;
        if target == temporary_target || target.parent() != temporary_target.parent() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let target = remote_path_utf8(target)?;
        let temporary_target = remote_path_utf8(temporary_target)?;
        let target_existed = self
            .bounded_protocol(self.transport.client().try_exists(target))
            .await?;
        let temporary_target_existed = self
            .bounded_protocol(self.transport.client().try_exists(temporary_target))
            .await?;
        let temporary_target_length = if temporary_target_existed {
            let metadata = self
                .bounded_protocol(self.transport.client().symlink_metadata(temporary_target))
                .await?;
            if !metadata.is_regular() {
                return Err(SftpRuntimeError::InvalidInput.into());
            }
            Some(metadata.len())
        } else {
            None
        };
        Ok(RemoteTransferPreparationFacts {
            target_existed,
            temporary_target_existed,
            temporary_target_length,
            safe_commit_supported: false,
        })
    }

    pub(crate) async fn verify_remote_file(
        &self,
        generation: SftpGeneration,
        path: &RemotePath,
        expected: &RemoteObjectPrecondition,
    ) -> Result<(), SftpProductionError> {
        self.require_generation(generation)?;
        let metadata = self
            .bounded_protocol(
                self.transport
                    .client()
                    .symlink_metadata(remote_path_utf8(path)?),
            )
            .await?;
        let observed = RemoteObjectPrecondition {
            kind: remote_entry_kind_from_flags(
                metadata.is_regular(),
                metadata.is_dir(),
                metadata.is_symlink(),
            ),
            size: metadata.size,
            modified_at_unix_ms: metadata.mtime.map(|value| i64::from(value) * 1_000),
        };
        if &observed != expected || observed.kind != RemoteEntryKind::File {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(())
    }

    pub(crate) async fn read_remote_file_preview(
        &self,
        generation: SftpGeneration,
        path: &RemotePath,
        expected: &RemoteObjectPrecondition,
        start_offset: u64,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, SftpProductionError> {
        self.verify_remote_file(generation, path, expected).await?;
        let expected_size = expected.size.ok_or(SftpRuntimeError::InvalidInput)?;
        if start_offset > expected_size {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let mut source = self
            .bounded_protocol(self.transport.client().open(remote_path_utf8(path)?))
            .await?;
        self.bounded_protocol(source.seek(io::SeekFrom::Start(start_offset)))
            .await?;
        let mut limited = (&mut source).take((maximum_bytes as u64).saturating_add(1));
        let mut bytes = Vec::with_capacity(maximum_bytes.min(64 * 1024));
        self.bounded_protocol(limited.read_to_end(&mut bytes))
            .await?;
        let _ = self.bounded_protocol(source.close()).await;
        if bytes.len() > maximum_bytes {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        Ok(bytes)
    }

    pub(crate) async fn read_remote_file_tail_limited(
        &self,
        generation: SftpGeneration,
        path: &RemotePath,
        offset: u64,
        maximum_bytes: usize,
    ) -> Result<(u64, u64, u64, bool, Vec<u8>), SftpProductionError> {
        if maximum_bytes == 0 || maximum_bytes > MAX_TAIL_CHUNK_BYTES {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        self.require_generation(generation)?;
        let path = remote_path_utf8(path)?;
        let metadata = self
            .bounded_protocol(self.transport.client().symlink_metadata(path))
            .await?;
        if !metadata.is_regular() {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let total_size = metadata.len();
        validate_js_safe_transfer_size(total_size)?;
        let (start_offset, length, reset) =
            tail_read_window_limited(total_size, offset, maximum_bytes);
        if start_offset == total_size {
            return Ok((start_offset, start_offset, total_size, reset, Vec::new()));
        }
        let mut source = self
            .bounded_protocol(self.transport.client().open(path))
            .await?;
        self.bounded_protocol(source.seek(io::SeekFrom::Start(start_offset)))
            .await?;
        let capacity = usize::try_from(length).map_err(|_| SftpRuntimeError::InvalidInput)?;
        let mut bytes = Vec::with_capacity(capacity);
        self.bounded_protocol((&mut source).take(length).read_to_end(&mut bytes))
            .await?;
        let _ = self.bounded_protocol(source.close()).await;
        let bytes_read = u64::try_from(bytes.len()).map_err(|_| SftpRuntimeError::InvalidInput)?;
        let next_offset = start_offset.saturating_add(bytes_read);
        Ok((start_offset, next_offset, total_size, reset, bytes))
    }

    /// Removes only the exact temporary target and verifies the result with a
    /// second protocol query. A failed or uncertain cleanup is reported as a
    /// residual, never as a successful cancellation.
    pub(crate) async fn cleanup_temporary_target(
        &self,
        generation: SftpGeneration,
        temporary_target: &RemotePath,
    ) -> Result<CleanupOutcome, SftpProductionError> {
        self.require_generation(generation)?;
        let path = remote_path_utf8(temporary_target)?;
        let existed = self
            .bounded_protocol(self.transport.client().try_exists(path))
            .await?;
        if !existed {
            return Ok(CleanupOutcome::Cleaned);
        }
        if self
            .bounded_protocol(self.transport.client().remove_file(path))
            .await
            .is_err()
        {
            return Ok(CleanupOutcome::Residual {
                opaque_location: path.to_owned(),
            });
        }
        match self
            .bounded_protocol(self.transport.client().try_exists(path))
            .await
        {
            Ok(false) => Ok(CleanupOutcome::Cleaned),
            Ok(true) | Err(_) => Ok(CleanupOutcome::Residual {
                opaque_location: path.to_owned(),
            }),
        }
    }

    /// Re-reads the complete bounded temporary prefix and compares it with the
    /// trusted local prefix supplied by the local-file boundary. No progress
    /// percentage or cached counter is treated as resume evidence.
    pub(crate) async fn verify_resume_prefix(
        &self,
        generation: SftpGeneration,
        temporary_target: RemotePath,
        expected_prefix: &[u8],
    ) -> Result<VerifiedRemoteResume, SftpProductionError> {
        self.require_generation(generation)?;
        if expected_prefix.len() > MAX_RESUME_PREFIX_BYTES {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let path = remote_path_utf8(&temporary_target)?;
        let mut file = self
            .bounded_protocol(
                self.transport
                    .client()
                    .open_with_flags(path, OpenFlags::READ | OpenFlags::WRITE),
            )
            .await?;
        let metadata = self.bounded_protocol(file.metadata()).await?;
        if !metadata.is_regular() {
            let _ = file.close().await;
            return Err(SftpRuntimeError::ResumeEvidenceMismatch.into());
        }
        let verified_length = metadata.len();
        let expected_length = u64::try_from(expected_prefix.len())
            .map_err(|_| SftpProductionError::Runtime(SftpRuntimeError::InvalidInput))?;
        if verified_length != expected_length {
            let _ = file.close().await;
            return Err(SftpRuntimeError::ResumeEvidenceMismatch.into());
        }
        let mut bytes = Vec::with_capacity(expected_prefix.len());
        tokio::time::timeout(
            SFTP_PROTOCOL_OPERATION_TIMEOUT,
            (&mut file)
                .take(verified_length.saturating_add(1))
                .read_to_end(&mut bytes),
        )
        .await
        .map_err(|_| sftp_protocol_error())?
        .map_err(|_| sftp_protocol_error())?;
        if bytes.as_slice() != expected_prefix {
            let _ = file.close().await;
            return Err(SftpRuntimeError::ResumeEvidenceMismatch.into());
        }
        file.seek(io::SeekFrom::Start(verified_length))
            .await
            .map_err(|_| sftp_protocol_error())?;
        Ok(VerifiedRemoteResume {
            evidence: ResumeEvidence {
                temporary_target: TemporaryTarget::Remote(temporary_target),
                verified_length,
                prefix_checksum_verified: true,
            },
            file,
        })
    }

    /// SFTP v3 only exposes path-based hardlink/rename for final commit. Even
    /// when a READ|WRITE handle remains stable after a path replacement, the
    /// final path lookup cannot be proven to name that handle. Resume therefore
    /// remains fail-closed until the adapter gains a handle-stable commit.
    fn supports_handle_stable_resume_commit(&self) -> bool {
        sftp_v3_handle_stable_resume_commit_supported()
    }

    fn uses_shared_transport(&self) -> bool {
        matches!(&self.transport, SftpLiveTransport::Shared(_))
    }

    pub(crate) async fn disconnect(self) -> Result<(), SftpProductionError> {
        self.transport
            .disconnect()
            .await
            .map_err(|error| SftpProductionError::Transport(Box::new(error)))
    }

    fn require_generation(&self, generation: SftpGeneration) -> Result<(), SftpProductionError> {
        if self.generation == generation {
            Ok(())
        } else {
            Err(SftpRuntimeError::StaleGeneration.into())
        }
    }

    async fn bounded_protocol<T, E, F>(&self, future: F) -> Result<T, SftpProductionError>
    where
        F: Future<Output = Result<T, E>>,
    {
        tokio::time::timeout(SFTP_PROTOCOL_OPERATION_TIMEOUT, future)
            .await
            .map_err(|_| sftp_protocol_error())?
            .map_err(|_| sftp_protocol_error())
    }
}

/// Serial state owner for exactly one SFTP resource.
///
/// A concrete adapter must open an SFTP subsystem through a protocol implementation and feed only
/// fenced facts into this actor. It may own a dedicated connection or a child channel borrowed
/// from a user-owned SSH session. The actor has no API for Terminal attachments, ShellHeartbeat,
/// or login automation.
pub(crate) struct SftpSessionActor {
    summary: SftpSessionSummary,
    connection_revision_token: Option<String>,
    transfers: BTreeMap<TransferId, TransferRecord>,
}

impl SftpSessionActor {
    pub(crate) fn new(
        session_id: SftpSessionId,
        host_id: impl Into<String>,
    ) -> SftpRuntimeResult<Self> {
        let host_id = host_id.into();
        if host_id.trim().is_empty() || host_id.len() > MAX_HOST_ID_BYTES {
            return Err(SftpRuntimeError::InvalidInput);
        }
        Ok(Self {
            summary: SftpSessionSummary {
                session_id,
                host_id: Some(host_id),
                parent_ssh_session: None,
                generation: None,
                state_revision: 0,
                state: SftpSessionState::Idle,
                failure_code: None,
                #[cfg(test)]
                active_interaction: None,
                transport_id: None,
                subsystem_id: None,
            },
            connection_revision_token: None,
            transfers: BTreeMap::new(),
        })
    }

    fn new_on_ssh_session(
        session_id: SftpSessionId,
        host_id: Option<HostId>,
        parent: wire::SftpParentSshSession,
    ) -> SftpRuntimeResult<Self> {
        let mut actor = Self::new(
            session_id,
            host_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| parent.session_id.to_string()),
        )?;
        actor.summary.host_id = host_id.map(|value| value.to_string());
        actor.summary.parent_ssh_session = Some(parent);
        Ok(actor)
    }

    pub(crate) fn summary(&self) -> &SftpSessionSummary {
        &self.summary
    }

    pub(crate) fn transfers(&self) -> impl Iterator<Item = &TransferRecord> {
        self.transfers.values()
    }

    pub(crate) fn start(
        &mut self,
        connection_revision_token: impl Into<String>,
    ) -> SftpRuntimeResult<SftpGeneration> {
        if matches!(
            self.summary.state,
            SftpSessionState::Connecting
                | SftpSessionState::VerifyingHostKey
                | SftpSessionState::Authenticating
                | SftpSessionState::OpeningSubsystem
                | SftpSessionState::Ready
                | SftpSessionState::Disconnecting
        ) {
            return Err(SftpRuntimeError::InvalidState);
        }
        let revision = connection_revision_token.into();
        if revision.trim().is_empty() || revision.len() > MAX_ID_BYTES * 8 {
            return Err(SftpRuntimeError::InvalidInput);
        }
        let next = self
            .summary
            .generation
            .map(|generation| generation.get().saturating_add(1))
            .unwrap_or(1);
        let generation = SftpGeneration::new(next)?;
        self.pause_active_transfers();
        self.summary.generation = Some(generation);
        self.clear_active_interaction();
        self.summary.transport_id = None;
        self.summary.subsystem_id = None;
        self.connection_revision_token = Some(revision);
        self.transition(SftpSessionState::Connecting, None);
        Ok(generation)
    }

    pub(crate) fn authenticated_transport_ready(
        &mut self,
        generation: SftpGeneration,
        transport_id: impl Into<String>,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        if !matches!(
            self.summary.state,
            SftpSessionState::Connecting | SftpSessionState::Authenticating
        ) {
            return Err(SftpRuntimeError::InvalidState);
        }
        self.summary.transport_id = Some(bounded_id(transport_id.into())?);
        self.transition(SftpSessionState::OpeningSubsystem, None);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn phase_changed(
        &mut self,
        generation: SftpGeneration,
        state: SftpSessionState,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        self.require_connection_pending()?;
        if !matches!(
            state,
            SftpSessionState::Connecting
                | SftpSessionState::VerifyingHostKey
                | SftpSessionState::Authenticating
        ) {
            return Err(SftpRuntimeError::InvalidInput);
        }
        self.transition(state, None);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn host_key_observed(
        &mut self,
        generation: SftpGeneration,
        challenge_id: impl Into<String>,
        route_stage: SftpRouteStage,
        algorithm: impl Into<String>,
        fingerprint_sha256: impl Into<String>,
        trusted_fingerprint_sha256: Option<String>,
    ) -> SftpRuntimeResult<HostKeyChallenge> {
        self.require_generation(generation)?;
        self.require_connection_pending()?;
        if self.summary.active_interaction.is_some() {
            return Err(SftpRuntimeError::InvalidState);
        }
        let challenge = HostKeyChallenge {
            challenge_id: bounded_id(challenge_id.into())?,
            generation,
            route_stage,
            algorithm: bounded_label(algorithm.into())?,
            fingerprint_sha256: bounded_label(fingerprint_sha256.into())?,
            trusted_fingerprint_sha256,
        };
        self.transition(SftpSessionState::VerifyingHostKey, None);
        self.summary.active_interaction = Some(SftpInteraction::HostKeyReview(challenge.clone()));
        Ok(challenge)
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "the focused SFTP interaction model retains the explicit host-key decision transition"
    )]
    pub(crate) fn decide_host_key(
        &mut self,
        generation: SftpGeneration,
        challenge_id: &str,
        decision: HostKeyDecision,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        let Some(SftpInteraction::HostKeyReview(challenge)) =
            self.summary.active_interaction.as_ref()
        else {
            return Err(SftpRuntimeError::InvalidState);
        };
        if challenge.challenge_id != challenge_id || challenge.generation != generation {
            return Err(SftpRuntimeError::ChallengeMismatch);
        }
        self.clear_active_interaction();
        match decision {
            HostKeyDecision::AcceptAndStore => {
                self.transition(SftpSessionState::Connecting, None);
                Ok(())
            }
            HostKeyDecision::Reject => {
                self.transition(
                    SftpSessionState::Failed,
                    Some(SftpFailureCode::HostKeyRejected),
                );
                Ok(())
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn host_key_mismatch(
        &mut self,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        self.require_connection_pending()?;
        self.clear_active_interaction();
        self.pause_active_transfers();
        self.transition(
            SftpSessionState::Failed,
            Some(SftpFailureCode::HostKeyMismatch),
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn require_authentication(
        &mut self,
        generation: SftpGeneration,
        requirement: AuthenticationRequirement,
    ) -> SftpRuntimeResult<SftpInteraction> {
        self.require_generation(generation)?;
        self.require_connection_pending()?;
        if self.summary.active_interaction.is_some() {
            return Err(SftpRuntimeError::InvalidState);
        }
        let failure = match requirement {
            AuthenticationRequirement::VaultLocked => Some(SftpFailureCode::VaultLocked),
            AuthenticationRequirement::CredentialUnavailable => {
                Some(SftpFailureCode::CredentialUnavailable)
            }
            AuthenticationRequirement::KeyboardInteractive => None,
        };
        let interaction = SftpInteraction::AuthenticationRequired {
            generation,
            requirement,
        };
        self.summary.active_interaction = Some(interaction.clone());
        self.transition(SftpSessionState::NeedsAuthentication, failure);
        Ok(interaction)
    }

    /// Continues only after the owning service has satisfied the exact typed
    /// requirement (for example, a successful Vault unlock). No secret value
    /// crosses this state-machine boundary.
    #[cfg(test)]
    pub(crate) fn authentication_requirement_satisfied(
        &mut self,
        generation: SftpGeneration,
        requirement: AuthenticationRequirement,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        let expected = SftpInteraction::AuthenticationRequired {
            generation,
            requirement,
        };
        if self.summary.active_interaction.as_ref() != Some(&expected) {
            return Err(SftpRuntimeError::ChallengeMismatch);
        }
        self.clear_active_interaction();
        self.transition(SftpSessionState::Authenticating, None);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn keyboard_interactive_challenge(
        &mut self,
        challenge: KeyboardInteractiveChallenge,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(challenge.generation)?;
        self.require_connection_pending()?;
        if self.summary.active_interaction.is_some() {
            return Err(SftpRuntimeError::InvalidState);
        }
        if challenge.challenge_id.trim().is_empty()
            || challenge.prompts.is_empty()
            || challenge.prompts.len() > MAX_KEYBOARD_PROMPTS
            || challenge.expires_at_unix_ms <= 0
        {
            return Err(SftpRuntimeError::InvalidInput);
        }
        for (index, prompt) in challenge.prompts.iter().enumerate() {
            if prompt.prompt_index != u8::try_from(index).unwrap_or(u8::MAX)
                || prompt.label.len() > MAX_LABEL_BYTES
            {
                return Err(SftpRuntimeError::InvalidInput);
            }
        }
        self.summary.active_interaction = Some(SftpInteraction::KeyboardInteractive(challenge));
        self.transition(SftpSessionState::NeedsAuthentication, None);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn respond_keyboard_interactive(
        &mut self,
        response: &KeyboardInteractiveResponse,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(response.generation)?;
        let Some(SftpInteraction::KeyboardInteractive(challenge)) =
            self.summary.active_interaction.as_ref()
        else {
            return Err(SftpRuntimeError::InvalidState);
        };
        if response.challenge_id != challenge.challenge_id
            || response.round_index != challenge.round_index
            || response.answer_ref_ids.len() != challenge.prompts.len()
            || response
                .answer_ref_ids
                .iter()
                .any(|value| value.trim().is_empty() || value.len() > MAX_ID_BYTES)
        {
            return Err(SftpRuntimeError::ChallengeMismatch);
        }
        let mut unique = response.answer_ref_ids.clone();
        unique.sort();
        unique.dedup();
        if unique.len() != response.answer_ref_ids.len() {
            return Err(SftpRuntimeError::ChallengeMismatch);
        }
        self.clear_active_interaction();
        self.transition(SftpSessionState::Authenticating, None);
        Ok(())
    }

    pub(crate) fn subsystem_opened(
        &mut self,
        generation: SftpGeneration,
        subsystem_id: impl Into<String>,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        if self.summary.state != SftpSessionState::OpeningSubsystem
            || self.summary.transport_id.is_none()
        {
            return Err(SftpRuntimeError::InvalidState);
        }
        self.summary.subsystem_id = Some(bounded_id(subsystem_id.into())?);
        self.clear_active_interaction();
        self.transition(SftpSessionState::Ready, None);
        Ok(())
    }

    pub(crate) fn directory_listing(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
        cursor: Option<Vec<u8>>,
        page_size: u16,
    ) -> SftpRuntimeResult<DirectoryListingPlan> {
        self.require_ready(generation)?;
        if page_size == 0 || page_size > MAX_DIRECTORY_PAGE_SIZE {
            return Err(SftpRuntimeError::InvalidInput);
        }
        if cursor.as_ref().is_some_and(|value| value.len() > 1024) {
            return Err(SftpRuntimeError::InvalidInput);
        }
        Ok(DirectoryListingPlan {
            operation,
            generation,
            path,
            cursor,
            page_size,
        })
    }

    pub(crate) fn mkdir(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        Ok(FileMutationPlan::CreateDirectory {
            operation,
            generation,
            path,
        })
    }

    pub(crate) fn create_empty_file(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        Ok(FileMutationPlan::CreateEmptyFile {
            operation,
            generation,
            path,
        })
    }

    pub(crate) fn write_text(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
        precondition: RemoteObjectPrecondition,
        text: String,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        let bytes = text.into_bytes();
        if bytes.len() > MAX_TEXT_PREVIEW_BYTES || bytes.contains(&0) {
            return Err(SftpRuntimeError::InvalidInput);
        }
        if precondition.kind != RemoteEntryKind::File {
            return Err(SftpRuntimeError::InvalidInput);
        }
        Ok(FileMutationPlan::WriteText {
            operation,
            generation,
            path,
            precondition,
            bytes,
        })
    }

    pub(crate) fn rename(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        source: RemotePath,
        target: RemotePath,
        source_precondition: RemoteObjectPrecondition,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        if source == target {
            return Err(SftpRuntimeError::InvalidInput);
        }
        Ok(FileMutationPlan::RenameNoReplace {
            operation,
            generation,
            source,
            target,
            source_precondition,
        })
    }

    pub(crate) fn delete(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        path: RemotePath,
        kind: DeleteKind,
        precondition: RemoteObjectPrecondition,
        irreversible_confirmed: bool,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        if !irreversible_confirmed {
            return Err(SftpRuntimeError::InvalidState);
        }
        Ok(FileMutationPlan::DeleteConfirmed {
            operation,
            generation,
            path,
            kind,
            precondition,
        })
    }

    pub(crate) fn create_zip(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        sources: Vec<ArchiveSourcePlan>,
        target: RemotePath,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        if sources.is_empty() || sources.len() > MAX_ARCHIVE_ENTRIES {
            return Err(SftpRuntimeError::InvalidInput);
        }
        let mut names = BTreeSet::new();
        for source in &sources {
            validate_archive_name(&source.archive_name)?;
            if !names.insert(source.archive_name.clone())
                || !matches!(
                    source.precondition.kind,
                    RemoteEntryKind::File | RemoteEntryKind::Directory
                )
            {
                return Err(SftpRuntimeError::InvalidInput);
            }
        }
        Ok(FileMutationPlan::CreateZip {
            operation,
            generation,
            sources,
            target,
        })
    }

    pub(crate) fn extract_zip(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        source: RemotePath,
        source_precondition: RemoteObjectPrecondition,
        target_directory: RemotePath,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        if source_precondition.kind != RemoteEntryKind::File || source == target_directory {
            return Err(SftpRuntimeError::InvalidInput);
        }
        Ok(FileMutationPlan::ExtractZip {
            operation,
            generation,
            source,
            source_precondition,
            target_directory,
        })
    }

    pub(crate) fn download_url(
        &self,
        operation: OperationFence,
        generation: SftpGeneration,
        url: String,
        target: RemotePath,
    ) -> SftpRuntimeResult<FileMutationPlan> {
        self.require_ready(generation)?;
        if url.len() > 4096 || url.bytes().any(|byte| byte == 0) {
            return Err(SftpRuntimeError::InvalidInput);
        }
        Ok(FileMutationPlan::DownloadUrl {
            operation,
            generation,
            url,
            target,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn enqueue_transfer(
        &mut self,
        transfer_id: TransferId,
        generation: SftpGeneration,
        direction: TransferDirection,
        source: TransferEndpoint,
        target: TransferEndpoint,
        expected_bytes: u64,
        conflict_policy: ConflictPolicy,
    ) -> SftpRuntimeResult<TransferRecord> {
        self.require_ready(generation)?;
        source.validate()?;
        target.validate()?;
        if self.transfers.contains_key(&transfer_id)
            || !matches!(
                (&direction, &source, &target),
                (
                    TransferDirection::Upload,
                    TransferEndpoint::LocalBoundaryToken(_),
                    TransferEndpoint::Remote(_)
                ) | (
                    TransferDirection::Download,
                    TransferEndpoint::Remote(_),
                    TransferEndpoint::LocalBoundaryToken(_)
                )
            )
        {
            return Err(SftpRuntimeError::InvalidInput);
        }
        let record = TransferRecord {
            transfer_id: transfer_id.clone(),
            generation,
            direction,
            source,
            target,
            expected_bytes,
            transferred_bytes: 0,
            started_at: None,
            conflict_policy,
            state: TransferState::Queued,
            state_revision: 1,
            temporary_target: None,
            target_existed: false,
            safe_commit_supported: false,
            failure_code: None,
            cleanup_residual: None,
        };
        self.transfers.insert(transfer_id, record.clone());
        Ok(record)
    }

    pub(crate) fn begin_transfer(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<TransferPlan> {
        self.require_ready(generation)?;
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Queued {
            return Err(SftpRuntimeError::InvalidState);
        }
        transfer_transition(record, TransferState::Preparing, None);
        record.started_at = Some(std::time::Instant::now());
        Ok(TransferPlan {
            transfer_id: record.transfer_id.clone(),
            generation,
            direction: record.direction,
            source: record.source.clone(),
            target: record.target.clone(),
            conflict_policy: record.conflict_policy,
            expected_bytes: record.expected_bytes,
            require_temporary_target: true,
            resume_from: 0,
        })
    }

    pub(crate) fn rebind_queued_transfer(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<()> {
        self.require_ready(generation)?;
        let record = self
            .transfers
            .get_mut(transfer_id)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if record.state != TransferState::Queued {
            return Err(SftpRuntimeError::InvalidState);
        }
        if record.generation != generation {
            record.generation = generation;
            record.state_revision = record.state_revision.saturating_add(1);
        }
        Ok(())
    }

    pub(crate) fn transfer_prepared(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        temporary_target: TemporaryTarget,
        target_existed: bool,
        safe_commit_supported: bool,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Preparing {
            return Err(SftpRuntimeError::InvalidState);
        }
        temporary_target.validate_for(&record.target)?;
        record.temporary_target = Some(temporary_target);
        record.target_existed = target_existed;
        record.safe_commit_supported = safe_commit_supported;
        if target_existed && record.conflict_policy == ConflictPolicy::FailIfExists {
            transfer_transition(
                record,
                TransferState::Failed,
                Some(TransferFailureCode::TargetExists),
            );
            return Err(SftpRuntimeError::Conflict);
        }
        if target_existed
            && record.conflict_policy == ConflictPolicy::ReplaceSafely
            && !safe_commit_supported
        {
            transfer_transition(
                record,
                TransferState::Failed,
                Some(TransferFailureCode::UnsafeReplaceUnsupported),
            );
            return Err(SftpRuntimeError::UnsafeReplaceUnsupported);
        }
        transfer_transition(record, TransferState::Transferring, None);
        Ok(())
    }

    pub(crate) fn record_progress(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        transferred_bytes: u64,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Transferring
            || transferred_bytes < record.transferred_bytes
            || transferred_bytes > record.expected_bytes
        {
            return Err(SftpRuntimeError::ProgressRegression);
        }
        record.transferred_bytes = transferred_bytes;
        record.state_revision = record.state_revision.saturating_add(1);
        Ok(())
    }

    pub(crate) fn begin_verification(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        observed_length: u64,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Transferring {
            return Err(SftpRuntimeError::InvalidState);
        }
        if record.transferred_bytes != record.expected_bytes
            || observed_length != record.expected_bytes
        {
            transfer_transition(
                record,
                TransferState::Failed,
                Some(TransferFailureCode::LengthMismatch),
            );
            return Err(SftpRuntimeError::LengthMismatch);
        }
        transfer_transition(record, TransferState::Verifying, None);
        Ok(())
    }

    pub(crate) fn begin_commit(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Verifying || record.temporary_target.is_none() {
            return Err(SftpRuntimeError::InvalidState);
        }
        transfer_transition(record, TransferState::Committing, None);
        Ok(())
    }

    pub(crate) fn complete_transfer(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        facts: CommitFacts,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Committing {
            return Err(SftpRuntimeError::InvalidState);
        }
        let atomic_commit_proven = if record.target_existed {
            record.conflict_policy == ConflictPolicy::ReplaceSafely && facts.atomic_replace
        } else {
            facts.atomic_no_replace
        };
        if facts.final_length != record.expected_bytes || !atomic_commit_proven {
            transfer_transition(
                record,
                TransferState::Failed,
                Some(if facts.final_length != record.expected_bytes {
                    TransferFailureCode::LengthMismatch
                } else {
                    TransferFailureCode::UnsafeReplaceUnsupported
                }),
            );
            return Err(if facts.final_length != record.expected_bytes {
                SftpRuntimeError::LengthMismatch
            } else {
                SftpRuntimeError::UnsafeReplaceUnsupported
            });
        }
        record.temporary_target = None;
        transfer_transition(record, TransferState::Completed, None);
        Ok(())
    }

    pub(crate) fn begin_cancel(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if matches!(
            record.state,
            TransferState::Completed | TransferState::Cancelled | TransferState::Cancelling
        ) {
            return Err(SftpRuntimeError::InvalidState);
        }
        transfer_transition(record, TransferState::Cancelling, None);
        Ok(())
    }

    pub(crate) fn fail_transfer(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        failure: TransferFailureCode,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if matches!(
            record.state,
            TransferState::Completed | TransferState::Cancelled | TransferState::Failed
        ) {
            return Err(SftpRuntimeError::InvalidState);
        }
        transfer_transition(
            record,
            if failure == TransferFailureCode::TransportLost {
                TransferState::PausedByDisconnect
            } else {
                TransferState::Failed
            },
            Some(failure),
        );
        Ok(())
    }

    pub(crate) fn fail_transfer_after_cleanup(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        failure: TransferFailureCode,
        cleanup: CleanupOutcome,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if matches!(
            record.state,
            TransferState::Completed | TransferState::Cancelled | TransferState::Failed
        ) {
            return Err(SftpRuntimeError::InvalidState);
        }
        let failure = match cleanup {
            CleanupOutcome::Cleaned => {
                record.temporary_target = None;
                record.cleanup_residual = None;
                failure
            }
            CleanupOutcome::Residual { opaque_location } => {
                record.cleanup_residual = Some(bounded_label(opaque_location)?);
                TransferFailureCode::CleanupIncomplete
            }
        };
        transfer_transition(
            record,
            if failure == TransferFailureCode::TransportLost {
                TransferState::PausedByDisconnect
            } else {
                TransferState::Failed
            },
            Some(failure),
        );
        Ok(())
    }

    pub(crate) fn finish_cancel(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        cleanup: CleanupOutcome,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Cancelling {
            return Err(SftpRuntimeError::InvalidState);
        }
        match cleanup {
            CleanupOutcome::Cleaned => {
                record.temporary_target = None;
                record.cleanup_residual = None;
                transfer_transition(record, TransferState::Cancelled, None);
                Ok(())
            }
            CleanupOutcome::Residual { opaque_location } => {
                record.cleanup_residual = Some(bounded_label(opaque_location)?);
                transfer_transition(
                    record,
                    TransferState::Failed,
                    Some(TransferFailureCode::CleanupIncomplete),
                );
                Err(SftpRuntimeError::CleanupIncomplete)
            }
        }
    }

    fn resolve_cleanup_residual(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<()> {
        let record = self.transfer_mut(transfer_id, generation)?;
        if record.state != TransferState::Failed
            || record.failure_code != Some(TransferFailureCode::CleanupIncomplete)
            || record.cleanup_residual.is_none()
        {
            return Err(SftpRuntimeError::InvalidState);
        }
        record.temporary_target = None;
        record.cleanup_residual = None;
        transfer_transition(record, TransferState::Cancelled, None);
        Ok(())
    }

    pub(crate) fn resume_transfer(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        evidence: ResumeEvidence,
    ) -> SftpRuntimeResult<()> {
        self.require_ready(generation)?;
        let record = self
            .transfers
            .get_mut(transfer_id)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if record.state != TransferState::PausedByDisconnect
            || record.temporary_target.as_ref() != Some(&evidence.temporary_target)
            || !evidence.prefix_checksum_verified
            || evidence.verified_length != record.transferred_bytes
            || evidence.verified_length > record.expected_bytes
        {
            return Err(SftpRuntimeError::ResumeEvidenceMismatch);
        }
        // Explicit resume is the only transition allowed to rebind a paused
        // transfer. Once rebound, delayed facts from the previous transport
        // generation fail the ordinary transfer fence.
        record.generation = generation;
        transfer_transition(record, TransferState::Transferring, None);
        Ok(())
    }

    pub(crate) fn resumed_transfer_plan(
        &self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<TransferPlan> {
        self.require_ready(generation)?;
        let record = self
            .transfers
            .get(transfer_id)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if record.generation != generation
            || record.state != TransferState::Transferring
            || record.transferred_bytes == 0
            || record.temporary_target.is_none()
        {
            return Err(SftpRuntimeError::InvalidState);
        }
        Ok(TransferPlan {
            transfer_id: record.transfer_id.clone(),
            generation,
            direction: record.direction,
            source: record.source.clone(),
            target: record.target.clone(),
            conflict_policy: record.conflict_policy,
            expected_bytes: record.expected_bytes,
            require_temporary_target: true,
            resume_from: record.transferred_bytes,
        })
    }

    pub(crate) fn resume_verification_failed(
        &mut self,
        transfer_id: &TransferId,
        cleanup: CleanupOutcome,
    ) -> SftpRuntimeResult<()> {
        let record = self
            .transfers
            .get_mut(transfer_id)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if record.state != TransferState::PausedByDisconnect {
            return Err(SftpRuntimeError::InvalidState);
        }
        match cleanup {
            CleanupOutcome::Cleaned => {
                record.temporary_target = None;
                record.cleanup_residual = None;
                transfer_transition(
                    record,
                    TransferState::Failed,
                    Some(TransferFailureCode::LengthMismatch),
                );
            }
            CleanupOutcome::Residual { opaque_location } => {
                record.cleanup_residual = Some(bounded_label(opaque_location)?);
                transfer_transition(
                    record,
                    TransferState::Failed,
                    Some(TransferFailureCode::CleanupIncomplete),
                );
            }
        }
        Ok(())
    }

    pub(crate) fn restart_transfer(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        cleanup: CleanupOutcome,
    ) -> SftpRuntimeResult<()> {
        self.require_ready(generation)?;
        let record = self
            .transfers
            .get_mut(transfer_id)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if !matches!(
            record.state,
            TransferState::Failed | TransferState::PausedByDisconnect
        ) {
            return Err(SftpRuntimeError::InvalidState);
        }
        if let CleanupOutcome::Residual { opaque_location } = cleanup {
            record.cleanup_residual = Some(bounded_label(opaque_location)?);
            transfer_transition(
                record,
                TransferState::Failed,
                Some(TransferFailureCode::CleanupIncomplete),
            );
            return Err(SftpRuntimeError::CleanupIncomplete);
        }
        record.generation = generation;
        record.transferred_bytes = 0;
        record.started_at = None;
        record.temporary_target = None;
        record.target_existed = false;
        record.safe_commit_supported = false;
        record.cleanup_residual = None;
        transfer_transition(record, TransferState::Queued, None);
        Ok(())
    }

    pub(crate) fn disconnect(&mut self, generation: SftpGeneration) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        self.clear_active_interaction();
        self.pause_active_transfers();
        self.transition(SftpSessionState::Disconnecting, None);
        Ok(())
    }

    pub(crate) fn closed(&mut self, generation: SftpGeneration) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        if self.summary.state != SftpSessionState::Disconnecting {
            return Err(SftpRuntimeError::InvalidState);
        }
        self.summary.transport_id = None;
        self.summary.subsystem_id = None;
        self.transition(SftpSessionState::Closed, None);
        Ok(())
    }

    pub(crate) fn transport_lost(&mut self, generation: SftpGeneration) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        self.clear_active_interaction();
        self.summary.transport_id = None;
        self.summary.subsystem_id = None;
        self.pause_active_transfers();
        self.transition(
            SftpSessionState::Failed,
            Some(SftpFailureCode::TransportLost),
        );
        Ok(())
    }

    pub(crate) fn connection_failed(
        &mut self,
        generation: SftpGeneration,
        failure: SftpFailureCode,
    ) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        if !matches!(
            self.summary.state,
            SftpSessionState::Connecting
                | SftpSessionState::VerifyingHostKey
                | SftpSessionState::Authenticating
                | SftpSessionState::NeedsAuthentication
                | SftpSessionState::OpeningSubsystem
        ) {
            return Err(SftpRuntimeError::InvalidState);
        }
        self.clear_active_interaction();
        self.summary.transport_id = None;
        self.summary.subsystem_id = None;
        self.pause_active_transfers();
        self.transition(SftpSessionState::Failed, Some(failure));
        Ok(())
    }

    fn clear_active_interaction(&mut self) {
        #[cfg(test)]
        {
            self.summary.active_interaction = None;
        }
    }

    fn require_generation(&self, generation: SftpGeneration) -> SftpRuntimeResult<()> {
        if self.summary.generation == Some(generation) {
            Ok(())
        } else {
            Err(SftpRuntimeError::StaleGeneration)
        }
    }

    fn require_ready(&self, generation: SftpGeneration) -> SftpRuntimeResult<()> {
        self.require_generation(generation)?;
        if self.summary.state == SftpSessionState::Ready && self.summary.subsystem_id.is_some() {
            Ok(())
        } else {
            Err(SftpRuntimeError::InvalidState)
        }
    }

    #[cfg(test)]
    fn require_connection_pending(&self) -> SftpRuntimeResult<()> {
        if matches!(
            self.summary.state,
            SftpSessionState::Connecting
                | SftpSessionState::VerifyingHostKey
                | SftpSessionState::Authenticating
                | SftpSessionState::NeedsAuthentication
        ) {
            Ok(())
        } else {
            Err(SftpRuntimeError::InvalidState)
        }
    }

    fn transfer_mut(
        &mut self,
        transfer_id: &TransferId,
        generation: SftpGeneration,
    ) -> SftpRuntimeResult<&mut TransferRecord> {
        let record = self
            .transfers
            .get_mut(transfer_id)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if record.generation == generation {
            Ok(record)
        } else {
            Err(SftpRuntimeError::StaleGeneration)
        }
    }

    fn pause_active_transfers(&mut self) {
        for record in self.transfers.values_mut() {
            if matches!(
                record.state,
                TransferState::Preparing
                    | TransferState::Transferring
                    | TransferState::Verifying
                    | TransferState::Committing
            ) {
                transfer_transition(
                    record,
                    TransferState::PausedByDisconnect,
                    Some(TransferFailureCode::TransportLost),
                );
            } else if record.state == TransferState::Cancelling {
                record.cleanup_residual = record
                    .temporary_target
                    .as_ref()
                    .map(|target| format!("cleanup outcome unknown for {target:?}"));
                transfer_transition(
                    record,
                    TransferState::Failed,
                    Some(TransferFailureCode::CleanupIncomplete),
                );
            }
        }
    }

    fn transition(&mut self, state: SftpSessionState, failure: Option<SftpFailureCode>) {
        self.summary.state = state;
        self.summary.failure_code = failure;
        self.summary.state_revision = self.summary.state_revision.saturating_add(1);
    }
}

fn transfer_transition(
    record: &mut TransferRecord,
    state: TransferState,
    failure: Option<TransferFailureCode>,
) {
    record.state = state;
    record.failure_code = failure;
    record.state_revision = record.state_revision.saturating_add(1);
}

fn bounded_id(value: String) -> SftpRuntimeResult<String> {
    if value.trim().is_empty() || value.len() > MAX_ID_BYTES || value.bytes().any(|byte| byte == 0)
    {
        Err(SftpRuntimeError::InvalidInput)
    } else {
        Ok(value)
    }
}

fn bounded_label(value: String) -> SftpRuntimeResult<String> {
    if value.len() > MAX_LABEL_BYTES || value.bytes().any(|byte| byte == 0) {
        Err(SftpRuntimeError::InvalidInput)
    } else {
        Ok(value)
    }
}

fn remote_path_utf8(path: &RemotePath) -> Result<&str, SftpProductionError> {
    std::str::from_utf8(path.as_bytes()).map_err(|_| SftpProductionError::NonUtf8RemotePath)
}

fn sftp_protocol_error() -> SftpProductionError {
    SftpProductionError::Transport(Box::new(TransportError::SftpProtocol))
}

fn remote_entry_kind_from_flags(regular: bool, directory: bool, symlink: bool) -> RemoteEntryKind {
    if regular {
        RemoteEntryKind::File
    } else if directory {
        RemoteEntryKind::Directory
    } else if symlink {
        RemoteEntryKind::Symlink
    } else {
        RemoteEntryKind::Other
    }
}

fn decode_remote_precondition(
    precondition: wire::SftpRemoteObjectPrecondition,
) -> RemoteObjectPrecondition {
    RemoteObjectPrecondition {
        kind: match precondition.kind {
            wire::SftpRemoteEntryKind::File => RemoteEntryKind::File,
            wire::SftpRemoteEntryKind::Directory => RemoteEntryKind::Directory,
            wire::SftpRemoteEntryKind::Symlink => RemoteEntryKind::Symlink,
            wire::SftpRemoteEntryKind::Other => RemoteEntryKind::Other,
        },
        size: precondition.size,
        modified_at_unix_ms: precondition.modified_at_unix_ms,
    }
}

fn mutation_fingerprint(request: &wire::SftpFileMutationRequest) -> Vec<u8> {
    fn field(output: &mut Vec<u8>, value: &[u8]) {
        output.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
        output.extend_from_slice(value);
    }
    fn precondition(output: &mut Vec<u8>, value: &wire::SftpRemoteObjectPrecondition) {
        output.push(match value.kind {
            wire::SftpRemoteEntryKind::File => 1,
            wire::SftpRemoteEntryKind::Directory => 2,
            wire::SftpRemoteEntryKind::Symlink => 3,
            wire::SftpRemoteEntryKind::Other => 4,
        });
        match value.size {
            Some(size) => {
                output.push(1);
                output.extend_from_slice(&size.to_be_bytes());
            }
            None => output.push(0),
        }
        match value.modified_at_unix_ms {
            Some(modified) => {
                output.push(1);
                output.extend_from_slice(&modified.to_be_bytes());
            }
            None => output.push(0),
        }
    }

    let mut output = b"sftp-mutation-v1".to_vec();
    field(&mut output, request.session_id.as_str().as_bytes());
    output.extend_from_slice(&request.expected_generation.get().to_be_bytes());
    match &request.mutation {
        wire::SftpFileMutation::CreateDirectory { path } => {
            output.push(1);
            field(&mut output, &path.bytes);
        }
        wire::SftpFileMutation::CreateEmptyFile { path } => {
            output.push(4);
            field(&mut output, &path.bytes);
        }
        wire::SftpFileMutation::WriteText {
            path,
            precondition: expected,
            text,
        } => {
            output.push(5);
            field(&mut output, &path.bytes);
            precondition(&mut output, expected);
            field(&mut output, &Sha256::digest(text.as_bytes()));
        }
        wire::SftpFileMutation::RenameNoReplace {
            source,
            target,
            source_precondition,
        } => {
            output.push(2);
            field(&mut output, &source.bytes);
            field(&mut output, &target.bytes);
            precondition(&mut output, source_precondition);
        }
        wire::SftpFileMutation::Delete {
            path,
            precondition: expected,
            irreversible_confirmed,
        } => {
            output.push(3);
            field(&mut output, &path.bytes);
            precondition(&mut output, expected);
            output.push(u8::from(*irreversible_confirmed));
        }
        wire::SftpFileMutation::CreateZip { sources, target } => {
            output.push(6);
            field(&mut output, &target.bytes);
            for source in sources {
                field(&mut output, &source.path.bytes);
                precondition(&mut output, &source.precondition);
                field(&mut output, source.archive_name.as_bytes());
            }
        }
        wire::SftpFileMutation::ExtractZip {
            source,
            source_precondition,
            target_directory,
        } => {
            output.push(7);
            field(&mut output, &source.bytes);
            precondition(&mut output, source_precondition);
            field(&mut output, &target_directory.bytes);
        }
        wire::SftpFileMutation::DownloadUrl { url, target } => {
            output.push(8);
            field(&mut output, &Sha256::digest(url.as_bytes()));
            field(&mut output, &target.bytes);
        }
    }
    output
}

fn open_fingerprint(request: &wire::SftpSessionOpenRequest) -> Vec<u8> {
    let mut output = b"sftp-open-v1".to_vec();
    for value in [
        request.session_id.as_str().as_bytes(),
        request.host_id.as_str().as_bytes(),
    ] {
        output.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
        output.extend_from_slice(value);
    }
    output.extend_from_slice(&request.expected_host_state_version.get().to_be_bytes());
    output
}

fn cleanup_retry_fingerprint(request: &wire::SftpRemoteCleanupRetryRequest) -> Vec<u8> {
    let mut value = Vec::new();
    value.extend_from_slice(request.transfer_id.as_str().as_bytes());
    value.extend_from_slice(&request.expected_generation.get().to_be_bytes());
    value.extend_from_slice(&request.expected_state_revision.get().to_be_bytes());
    value.extend_from_slice(&request.expected_host_state_version.get().to_be_bytes());
    value
}

fn cleanup_retain_fingerprint(request: &wire::SftpRemoteCleanupRetainRequest) -> Vec<u8> {
    let mut value = Vec::new();
    value.extend_from_slice(request.transfer_id.as_str().as_bytes());
    value.extend_from_slice(&request.expected_generation.get().to_be_bytes());
    value.extend_from_slice(&request.expected_state_revision.get().to_be_bytes());
    value.push(u8::from(request.retain_remote_temporary_file_confirmed));
    value
}

fn fingerprint_field(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    output.extend_from_slice(value);
}

fn local_directory_list_fingerprint(request: &wire::SftpLocalDirectoryListRequest) -> Vec<u8> {
    let mut value = b"sftp-local-directory-list-v1".to_vec();
    fingerprint_field(&mut value, request.directory_ref.as_bytes());
    value.extend_from_slice(&request.expected_revision.get().to_be_bytes());
    fingerprint_field(&mut value, request.cursor.as_deref().unwrap_or_default());
    value.extend_from_slice(&request.page_size.to_be_bytes());
    value
}

fn remote_directory_list_fingerprint(request: &wire::SftpDirectoryListRequest) -> Vec<u8> {
    let mut value = b"sftp-remote-directory-list-v1".to_vec();
    fingerprint_field(&mut value, request.session_id.as_str().as_bytes());
    value.extend_from_slice(&request.expected_generation.get().to_be_bytes());
    fingerprint_field(&mut value, &request.path.bytes);
    fingerprint_field(&mut value, request.cursor.as_deref().unwrap_or_default());
    value.extend_from_slice(&request.page_size.to_be_bytes());
    value
}

fn local_directory_open_fingerprint(request: &wire::SftpLocalDirectoryOpenChildRequest) -> Vec<u8> {
    let mut value = b"sftp-local-directory-open-v1".to_vec();
    fingerprint_field(&mut value, request.parent_directory_ref.as_bytes());
    value.extend_from_slice(&request.expected_parent_revision.get().to_be_bytes());
    fingerprint_field(&mut value, request.entry_ref.as_bytes());
    value
}

fn local_directory_create_fingerprint(
    request: &wire::SftpLocalDirectoryCreateChildRequest,
) -> Vec<u8> {
    let mut value = b"sftp-local-directory-create-v1".to_vec();
    fingerprint_field(&mut value, request.parent_directory_ref.as_bytes());
    value.extend_from_slice(&request.expected_parent_revision.get().to_be_bytes());
    fingerprint_field(&mut value, request.name.as_bytes());
    value
}

fn remote_directory_cancel_fingerprint(request: &wire::SftpDirectoryListCancelRequest) -> Vec<u8> {
    let mut fingerprint = Vec::new();
    fingerprint_field(&mut fingerprint, request.session_id.as_str().as_bytes());
    fingerprint_field(
        &mut fingerprint,
        &request.expected_generation.get().to_be_bytes(),
    );
    fingerprint_field(&mut fingerprint, &request.path.bytes);
    fingerprint_field(&mut fingerprint, &request.cursor);
    fingerprint
}

fn transfer_intent_prepare_fingerprint(
    request: &wire::SftpTransferIntentPrepareRequest,
) -> Vec<u8> {
    let mut value = b"sftp-transfer-intent-prepare-v2".to_vec();
    fingerprint_field(&mut value, request.source_pane_id.as_bytes());
    fingerprint_field(&mut value, request.target_pane_id.as_bytes());
    value.extend_from_slice(&request.source_endpoint_revision.get().to_be_bytes());
    value.extend_from_slice(&request.target_endpoint_revision.get().to_be_bytes());
    match &request.source {
        wire::SftpTransferIntentSource::LocalDirectoryEntry {
            directory_ref,
            entry_ref,
        } => {
            value.push(1);
            fingerprint_field(&mut value, directory_ref.as_bytes());
            fingerprint_field(&mut value, entry_ref.as_bytes());
        }
        wire::SftpTransferIntentSource::RemoteFile {
            session_id,
            expected_generation,
            directory_ref,
            entry_ref,
        } => {
            value.push(2);
            fingerprint_field(&mut value, session_id.as_str().as_bytes());
            value.extend_from_slice(&expected_generation.get().to_be_bytes());
            fingerprint_field(&mut value, directory_ref.as_bytes());
            fingerprint_field(&mut value, entry_ref.as_bytes());
        }
    }
    match &request.target {
        wire::SftpTransferIntentTarget::LocalDirectory { directory_ref } => {
            value.push(1);
            fingerprint_field(&mut value, directory_ref.as_bytes());
        }
        wire::SftpTransferIntentTarget::RemoteDirectory {
            session_id,
            expected_generation,
            directory_ref,
        } => {
            value.push(2);
            fingerprint_field(&mut value, session_id.as_str().as_bytes());
            value.extend_from_slice(&expected_generation.get().to_be_bytes());
            fingerprint_field(&mut value, directory_ref.as_bytes());
        }
    }
    value.extend_from_slice(&request.expected_bytes.to_be_bytes());
    value.push(match request.conflict_policy {
        wire::SftpConflictPolicy::FailIfExists => 1,
        wire::SftpConflictPolicy::ReplaceSafely => 2,
    });
    value
}

fn transfer_intent_enqueue_fingerprint(
    request: &wire::SftpTransferIntentEnqueueRequest,
) -> Vec<u8> {
    let mut value = b"sftp-transfer-intent-enqueue-v2".to_vec();
    fingerprint_field(&mut value, request.transfer_id.as_str().as_bytes());
    fingerprint_field(&mut value, request.intent_token.as_bytes());
    value
}

fn fingerprint_transfer_endpoint_fence(
    value: &mut Vec<u8>,
    fence: &wire::SftpTransferEndpointFence,
) {
    match fence {
        wire::SftpTransferEndpointFence::LocalCapability {
            directory_ref,
            revision,
        } => {
            value.push(1);
            fingerprint_field(value, directory_ref.as_bytes());
            value.extend_from_slice(&revision.get().to_be_bytes());
        }
        wire::SftpTransferEndpointFence::RemoteSession {
            session_id,
            generation,
        } => {
            value.push(2);
            fingerprint_field(value, session_id.as_str().as_bytes());
            value.extend_from_slice(&generation.get().to_be_bytes());
        }
    }
}

fn transfer_intent_action_fingerprint(request: &wire::SftpTransferIntentActionRequest) -> Vec<u8> {
    let mut value = b"sftp-transfer-intent-action-v1".to_vec();
    fingerprint_field(&mut value, request.transfer_id.as_str().as_bytes());
    fingerprint_transfer_endpoint_fence(&mut value, &request.expected_source_fence);
    fingerprint_transfer_endpoint_fence(&mut value, &request.expected_target_fence);
    value.extend_from_slice(&request.expected_state_revision.get().to_be_bytes());
    value
}

fn validate_name_component(name: &[u8]) -> SftpRuntimeResult<()> {
    if name.is_empty()
        || name.len() > 255
        || name == b"."
        || name == b".."
        || name.contains(&b'/')
        || name.contains(&0)
    {
        Err(SftpRuntimeError::InvalidInput)
    } else {
        Ok(())
    }
}

fn remote_name_bytes(path: &[u8]) -> SftpRuntimeResult<&[u8]> {
    let name = path.rsplit(|byte| *byte == b'/').next().unwrap_or_default();
    validate_name_component(name)?;
    Ok(name)
}

fn remote_child_path(directory: &RemotePath, name: &[u8]) -> SftpRuntimeResult<RemotePath> {
    validate_name_component(name)?;
    let mut bytes = directory.as_bytes().to_vec();
    if bytes.last() != Some(&b'/') {
        bytes.push(b'/');
    }
    bytes.extend_from_slice(name);
    RemotePath::parse(bytes)
}

fn map_transport_failure(error: &TransportError) -> SftpFailureCode {
    match error {
        TransportError::HostKeyRejected => SftpFailureCode::HostKeyRejected,
        TransportError::HostKeyMismatch { .. } => SftpFailureCode::HostKeyMismatch,
        TransportError::AuthenticationRejected
        | TransportError::AuthenticationIncomplete
        | TransportError::AuthenticationTimeout
        | TransportError::InvalidKeyboardInteractiveResponse
        | TransportError::InvalidPrivateKey
        | TransportError::SshAgentKeyUnavailable
        | TransportError::SshAgentUnavailable => SftpFailureCode::AuthenticationRejected,
        TransportError::SftpSubsystemRejected | TransportError::SftpSubsystemTimeout => {
            SftpFailureCode::SubsystemRejected
        }
        TransportError::ConnectionLost => SftpFailureCode::TransportLost,
        _ => SftpFailureCode::Protocol,
    }
}

#[derive(Clone)]
struct TrustedSftpHostKeyVerifier {
    hosts: HostService,
}

impl HostKeyVerifier for TrustedSftpHostKeyVerifier {
    fn verify(&self, endpoint: Endpoint, observed: ObservedHostKey) -> VerifyFuture {
        let hosts = self.hosts.clone();
        Box::pin(async move {
            match hosts.observe_known_host(
                endpoint.normalized_address(),
                endpoint.port(),
                &observed.algorithm,
                &observed.public_key_blob,
            ) {
                Ok(KnownHostObservation::Trusted(_)) => hosts
                    .record_known_host_verified(
                        endpoint.normalized_address(),
                        endpoint.port(),
                        &observed.algorithm,
                        &observed.public_key_blob,
                    )
                    .map(|_| TransportHostKeyDecision::Trusted)
                    .map_err(|_| TransportError::HostKeyVerificationFailed),
                Ok(KnownHostObservation::Unknown(_)) => Ok(TransportHostKeyDecision::Rejected),
                Ok(KnownHostObservation::Mismatch { trusted, .. })
                | Ok(KnownHostObservation::AlgorithmChanged { trusted, .. }) => {
                    Ok(TransportHostKeyDecision::Mismatch {
                        trusted_fingerprint: trusted.fingerprint_sha256,
                    })
                }
                Err(_) => Err(TransportError::HostKeyVerificationFailed),
            }
        })
    }
}

struct NonInteractiveSftpConnectionInteraction;

impl SshConnectionInteraction for NonInteractiveSftpConnectionInteraction {
    async fn phase_changed(
        &mut self,
        _phase: ConnectionPhase,
        _route_stage: ConnectionRouteStage,
    ) -> Result<(), TransportError> {
        Ok(())
    }

    async fn answer_keyboard_interactive(
        &mut self,
        _request: KeyboardInteractiveRequest,
    ) -> Result<Vec<Zeroizing<String>>, TransportError> {
        Err(TransportError::AuthenticationRejected)
    }
}

struct SftpServiceRecord {
    actor: SftpSessionActor,
    live: Option<SftpProductionSession<TrustedSftpHostKeyVerifier>>,
    // A shared child channel was consumed by a bounded close attempt whose
    // outcome was not clean. Keep this fact until the parent SSH transport
    // ends; otherwise a later `live: None` retry could claim a false close.
    shared_cleanup_incomplete: bool,
    shared_parent_channels: Option<SharedSessionChannels>,
    heartbeat_tasks: Option<SftpHeartbeatTasks>,
    pending_transfers: VecDeque<TransferId>,
    active_transfer_id: Option<TransferId>,
    open_operation_id: String,
    open_idempotency_key: String,
    open_fingerprint: Vec<u8>,
}

fn close_shared_child_record_after_parent_termination(
    record: &mut SftpServiceRecord,
    generation: SftpGeneration,
    parent_transport_closed: bool,
) -> bool {
    if !parent_transport_closed || record.actor.summary().generation != Some(generation) {
        return false;
    }
    record.heartbeat_tasks.take();
    record.live.take();
    record.shared_cleanup_incomplete = false;
    if record.actor.summary().state != SftpSessionState::Closed {
        let _ = record.actor.disconnect(generation);
        let _ = record.actor.closed(generation);
    }
    true
}

fn shared_sftp_record(
    actor: SftpSessionActor,
    live: Option<SftpProductionSession<TrustedSftpHostKeyVerifier>>,
    lease: SessionChannelLease,
    shared_cleanup_incomplete: bool,
) -> SftpServiceRecord {
    let heartbeat_tasks = (live.is_some() || shared_cleanup_incomplete).then(|| {
        let parent = lease.clone();
        SftpHeartbeatTasks::start_with_close_future(
            actor.summary().generation.expect("shared SFTP generation"),
            async move { parent.wait_cancelled().await },
            None,
            Vec::new(),
        )
    });
    SftpServiceRecord {
        actor,
        live,
        shared_cleanup_incomplete,
        shared_parent_channels: Some(lease.channels.clone()),
        heartbeat_tasks,
        pending_transfers: VecDeque::new(),
        active_transfer_id: None,
        open_operation_id: uuid::Uuid::now_v7().to_string(),
        open_idempotency_key: uuid::Uuid::now_v7().to_string(),
        open_fingerprint: format!("shared:{}:{}", lease.session_id, lease.generation.get())
            .into_bytes(),
    }
}

#[derive(Clone)]
enum MutationLedgerOutcome {
    Success(wire::SftpFileMutationResult),
    DeterministicFailure(SftpRuntimeError),
    Uncertain,
}

struct MutationLedgerEntry {
    idempotency_key: String,
    fingerprint: Vec<u8>,
    outcome: Option<MutationLedgerOutcome>,
    finished: watch::Sender<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupOperationKind {
    RetryRemote,
    RetainForExit,
}

#[derive(Clone)]
struct CleanupOperationRecord {
    idempotency_key: String,
    fingerprint: Vec<u8>,
    kind: CleanupOperationKind,
    result: wire::SftpTransferSummary,
}

#[derive(Clone)]
struct LocalDirectoryListLedgerRecord {
    idempotency_key: String,
    fingerprint: Vec<u8>,
    result: wire::SftpLocalDirectoryListing,
}

#[derive(Clone)]
struct LocalDirectoryOpenLedgerRecord {
    idempotency_key: String,
    fingerprint: Vec<u8>,
    result: wire::SftpLocalDirectoryCapability,
}

#[derive(Clone)]
struct RemoteDirectoryCancelLedgerRecord {
    idempotency_key: String,
    fingerprint: Vec<u8>,
}

#[derive(Clone)]
struct RemoteDirectoryListLedgerRecord {
    idempotency_key: String,
    fingerprint: Vec<u8>,
    result: wire::SftpDirectoryListing,
}

#[derive(Default)]
struct CleanupOperationLedger {
    entries: BTreeMap<String, CleanupOperationRecord>,
    order: VecDeque<String>,
}

impl CleanupOperationLedger {
    fn replay(
        &self,
        operation_id: &str,
        idempotency_key: &str,
        fingerprint: &[u8],
        kind: CleanupOperationKind,
    ) -> SftpRuntimeResult<Option<wire::SftpTransferSummary>> {
        let Some(record) = self.entries.get(operation_id) else {
            return Ok(None);
        };
        if record.idempotency_key != idempotency_key
            || record.fingerprint != fingerprint
            || record.kind != kind
        {
            return Err(SftpRuntimeError::Conflict);
        }
        Ok(Some(record.result.clone()))
    }

    fn record(
        &mut self,
        operation_id: String,
        idempotency_key: String,
        fingerprint: Vec<u8>,
        kind: CleanupOperationKind,
        result: wire::SftpTransferSummary,
    ) {
        while self.entries.len() >= MUTATION_LEDGER_CAPACITY {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.order.push_back(operation_id.clone());
        self.entries.insert(
            operation_id.clone(),
            CleanupOperationRecord {
                idempotency_key,
                fingerprint,
                kind,
                result,
            },
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CleanupExitAuthorization {
    generation: SftpGeneration,
    state_revision: u64,
}

#[derive(Default)]
struct MutationLedger {
    entries: BTreeMap<String, MutationLedgerEntry>,
    order: VecDeque<String>,
}

enum MutationLedgerClaim {
    Execute,
    Replay(MutationLedgerOutcome),
    Wait(watch::Receiver<bool>),
}

impl MutationLedger {
    fn claim(
        &mut self,
        operation_id: &str,
        idempotency_key: &str,
        fingerprint: &[u8],
    ) -> SftpRuntimeResult<MutationLedgerClaim> {
        if let Some(entry) = self.entries.get(operation_id) {
            if entry.idempotency_key != idempotency_key || entry.fingerprint != fingerprint {
                return Err(SftpRuntimeError::Conflict);
            }
            return Ok(match &entry.outcome {
                Some(outcome) => MutationLedgerClaim::Replay(outcome.clone()),
                None => MutationLedgerClaim::Wait(entry.finished.subscribe()),
            });
        }
        while self.entries.len() >= MUTATION_LEDGER_CAPACITY {
            let Some(oldest) = self.order.pop_front() else {
                return Err(SftpRuntimeError::Conflict);
            };
            if self
                .entries
                .get(&oldest)
                .is_some_and(|entry| entry.outcome.is_some())
            {
                self.entries.remove(&oldest);
            } else {
                self.order.push_back(oldest);
                return Err(SftpRuntimeError::Conflict);
            }
        }
        self.order.push_back(operation_id.to_owned());
        let (finished, _) = watch::channel(false);
        self.entries.insert(
            operation_id.to_owned(),
            MutationLedgerEntry {
                idempotency_key: idempotency_key.to_owned(),
                fingerprint: fingerprint.to_vec(),
                outcome: None,
                finished,
            },
        );
        Ok(MutationLedgerClaim::Execute)
    }

    fn finish(&mut self, operation_id: &str, outcome: MutationLedgerOutcome) {
        if let Some(entry) = self.entries.get_mut(operation_id) {
            entry.outcome = Some(outcome);
            entry.finished.send_replace(true);
        }
    }
}

#[derive(Clone)]
enum SftpHeartbeatLiveness {
    Disabled,
    Watching(watch::Receiver<bool>),
}

struct SftpHeartbeatTasks {
    generation: SftpGeneration,
    liveness: SftpHeartbeatLiveness,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl SftpHeartbeatTasks {
    fn start(
        generation: SftpGeneration,
        mut transport_close: TransportCloseHandle,
        policy: Option<&ResolvedTransportKeepalivePolicy>,
        heartbeats: Vec<RoutedTransportHeartbeat>,
    ) -> Self {
        Self::start_with_close_future(
            generation,
            async move { transport_close.wait_closed().await },
            policy,
            heartbeats,
        )
    }

    fn start_with_close_future<F>(
        generation: SftpGeneration,
        transport_close: F,
        policy: Option<&ResolvedTransportKeepalivePolicy>,
        heartbeats: Vec<RoutedTransportHeartbeat>,
    ) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (failure_tx, failure) = watch::channel(false);
        let close_failure = failure_tx.clone();
        let mut tasks = vec![tokio::spawn(async move {
            transport_close.await;
            close_failure.send_replace(true);
        })];
        if let Some(policy) = policy {
            let interval = Duration::from_secs(u64::from(policy.interval_seconds));
            let reply_timeout = Duration::from_secs(u64::from(policy.reply_timeout_seconds));
            let failure_threshold = policy.failure_threshold;
            tasks.extend(heartbeats.into_iter().map(|heartbeat| {
                let failure_tx = failure_tx.clone();
                tokio::spawn(async move {
                    let mut next_due = heartbeat.first_due;
                    let mut failures = 0_u8;
                    loop {
                        tokio::time::sleep_until(next_due).await;
                        let result =
                            tokio::time::timeout(reply_timeout, heartbeat.handle.request_reply())
                                .await;
                        failures = if matches!(result, Ok(Ok(()))) {
                            0
                        } else {
                            failures.saturating_add(1)
                        };
                        if failures >= failure_threshold {
                            failure_tx.send_replace(true);
                            break;
                        }
                        next_due = tokio::time::Instant::now() + interval;
                    }
                })
            }));
        }
        Self {
            generation,
            liveness: SftpHeartbeatLiveness::Watching(failure),
            tasks,
        }
    }

    fn has_failed(&self) -> bool {
        match &self.liveness {
            SftpHeartbeatLiveness::Disabled => false,
            SftpHeartbeatLiveness::Watching(failure) => *failure.borrow(),
        }
    }

    fn subscribe(&self) -> SftpHeartbeatLiveness {
        self.liveness.clone()
    }
}

impl Drop for SftpHeartbeatTasks {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

#[derive(Debug, Clone)]
struct RemoteDirectoryReference {
    session_id: String,
    generation: u64,
    path: RemotePath,
    expires_at: std::time::Instant,
}

/// Single-use Core continuation owning one live SFTP directory handle. It is
/// closed at EOF, explicit cancellation, TTL eviction, generation loss, or
/// service shutdown.
struct RemoteDirectoryCursor {
    directory_ref: String,
    session_id: String,
    generation: u64,
    path: RemotePath,
    stream: RemoteDirectoryCursorStream,
    expires_at: std::time::Instant,
    pages_read: u32,
    entries_read: usize,
}

#[derive(Clone)]
struct ActiveRemoteDirectoryCursor {
    session_id: String,
    generation: u64,
    path: RemotePath,
    cancel_requested: watch::Sender<bool>,
    close_result: watch::Receiver<Option<bool>>,
}

struct RemoteDirectoryPageControl {
    cursor_ref: String,
    cancel_requested: watch::Receiver<bool>,
    close_result: watch::Sender<Option<bool>>,
}

enum RemoteDirectoryCursorStream {
    Live(RemoteDirectoryStream),
    #[cfg(test)]
    Fixture(VecDeque<RemoteDirectoryEntry>),
}

impl RemoteDirectoryCursor {
    async fn take_page(
        &mut self,
        page_size: u16,
    ) -> Result<Vec<RemoteDirectoryEntry>, SftpProductionError> {
        if self.pages_read >= MAX_DIRECTORY_PAGES_PER_CURSOR
            || self.entries_read >= MAX_DIRECTORY_ENTRY_REFS
        {
            self.close_in_place().await;
            return Err(sftp_protocol_error());
        }
        let entries = match &mut self.stream {
            RemoteDirectoryCursorStream::Live(stream) => {
                let page = tokio::time::timeout(
                    SFTP_PROTOCOL_OPERATION_TIMEOUT,
                    stream.next_page(usize::from(page_size)),
                )
                .await;
                let page = match page {
                    Ok(Ok(page)) => page,
                    Ok(Err(_)) | Err(_) => {
                        self.close_in_place().await;
                        return Err(sftp_protocol_error());
                    }
                };
                match map_remote_directory_page(&self.path, page) {
                    Ok(entries) => entries,
                    Err(error) => {
                        self.close_in_place().await;
                        return Err(error);
                    }
                }
            }
            #[cfg(test)]
            RemoteDirectoryCursorStream::Fixture(entries) => {
                let take = usize::from(page_size).min(entries.len());
                entries.drain(..take).collect()
            }
        };
        if self.entries_read.saturating_add(entries.len()) > MAX_DIRECTORY_ENTRY_REFS {
            self.close_in_place().await;
            return Err(sftp_protocol_error());
        }
        self.pages_read = self.pages_read.saturating_add(1);
        self.entries_read = self.entries_read.saturating_add(entries.len());
        Ok(entries)
    }

    fn is_complete(&self) -> bool {
        match &self.stream {
            RemoteDirectoryCursorStream::Live(stream) => stream.is_complete(),
            #[cfg(test)]
            RemoteDirectoryCursorStream::Fixture(entries) => entries.is_empty(),
        }
    }

    async fn close_in_place(&mut self) -> bool {
        match &mut self.stream {
            RemoteDirectoryCursorStream::Live(stream) => {
                matches!(
                    tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, stream.close()).await,
                    Ok(Ok(()))
                )
            }
            #[cfg(test)]
            RemoteDirectoryCursorStream::Fixture(_) => true,
        }
    }

    async fn close(mut self) {
        let _ = self.close_in_place().await;
    }
}

#[derive(Debug, Clone)]
struct RemoteEntryReference {
    directory_ref: String,
    session_id: String,
    generation: u64,
    path: RemotePath,
    name: Vec<u8>,
    precondition: RemoteObjectPrecondition,
    display_name: String,
    expires_at: std::time::Instant,
}

#[derive(Clone)]
struct ActiveTransferControl {
    cancel_requested: watch::Sender<bool>,
    finished: watch::Sender<bool>,
    task: Arc<std::sync::Mutex<Option<tokio::task::AbortHandle>>>,
}

struct TransferSchedulingPause {
    paused: Arc<AtomicBool>,
    resume_on_drop: bool,
}

impl TransferSchedulingPause {
    fn new(paused: Arc<AtomicBool>) -> Self {
        paused.store(true, Ordering::Release);
        Self {
            paused,
            resume_on_drop: true,
        }
    }

    fn keep_paused(&mut self) {
        self.resume_on_drop = false;
    }
}

impl Drop for TransferSchedulingPause {
    fn drop(&mut self) {
        if self.resume_on_drop {
            self.paused.store(false, Ordering::Release);
        }
    }
}

enum TransferTaskOutcome {
    Finished(Result<(), TransferFailureCode>),
    Cancelled,
    TransportLost,
}

struct TransferTask {
    session_key: String,
    transfer_id: TransferId,
    plan: TransferPlan,
    boundary: LocalBoundary,
    live: SftpProductionSession<TrustedSftpHostKeyVerifier>,
    liveness: SftpHeartbeatLiveness,
    control: ActiveTransferControl,
    verified_resume_file: Option<RemoteSftpFile>,
}

#[derive(Clone)]
struct TransferIntentRecord {
    operation_id: String,
    idempotency_key: String,
    enqueue_fingerprint: Vec<u8>,
    prepared: PreparedTransferIntent,
    summary: wire::SftpTransferIntentSummary,
    legacy_session_id: Option<String>,
    started_at: Option<std::time::Instant>,
    cancel_operation: Option<(String, String, Vec<u8>)>,
}

#[derive(Clone)]
enum ResolvedIntentSource {
    Local {
        directory_ref: String,
        revision: u64,
        name: Vec<u8>,
        identity: LocalObjectIdentity,
    },
    Remote {
        session_id: String,
        generation: u64,
        path: RemotePath,
        name: Vec<u8>,
        precondition: RemoteObjectPrecondition,
    },
}

#[derive(Clone)]
enum ResolvedIntentTarget {
    Local {
        directory_ref: String,
        revision: u64,
        name: Vec<u8>,
    },
    Remote {
        session_id: String,
        generation: u64,
        path: RemotePath,
    },
}

#[derive(Clone)]
struct PreparedTransferIntent {
    source_pane_id: String,
    target_pane_id: String,
    source_endpoint_revision: u64,
    target_endpoint_revision: u64,
    source: ResolvedIntentSource,
    target: ResolvedIntentTarget,
    source_fence: wire::SftpTransferEndpointFence,
    target_fence: wire::SftpTransferEndpointFence,
    source_display_name: String,
    target_display_name: String,
    expected_bytes: u64,
    conflict_policy: ConflictPolicy,
    expires_at: std::time::Instant,
}

#[derive(Clone)]
struct PreparedTransferIntentToken {
    operation_id: String,
    idempotency_key: String,
    fingerprint: Vec<u8>,
    prepared: PreparedTransferIntent,
    expires_at_unix_ms: i64,
    consumed: bool,
}

#[derive(Clone)]
struct RemoteCopyPlan {
    transfer_id: TransferId,
    source_session_key: String,
    source_generation: SftpGeneration,
    source_path: RemotePath,
    source_precondition: RemoteObjectPrecondition,
    target_session_key: String,
    target_generation: SftpGeneration,
    target_path: RemotePath,
    expected_bytes: u64,
    conflict_policy: ConflictPolicy,
}

struct RemoteCopyTask {
    plan: RemoteCopyPlan,
    source_live: SftpProductionSession<TrustedSftpHostKeyVerifier>,
    target_live: SftpProductionSession<TrustedSftpHostKeyVerifier>,
    source_liveness: SftpHeartbeatLiveness,
    target_liveness: SftpHeartbeatLiveness,
    control: ActiveTransferControl,
}

enum RemoteCopyTaskOutcome {
    Finished(Result<RemoteCopyExecution, RemoteCopyExecutionFailure>),
    Cancelled,
    SourceLost,
    TargetLost,
}

struct RemoteCopyExecution {
    cleanup_residual: Option<wire::SftpTransferIntentCleanupResidual>,
}

struct RemoteCopyExecutionFailure {
    failure_code: wire::SftpTransferFailureCode,
    commit_outcome: wire::SftpTransferCommitOutcome,
    cleanup_residual: Option<wire::SftpTransferIntentCleanupResidual>,
}

/// A deliberately small, Core-internal transfer projection for native
/// notifications. It carries only opaque resource fences and never exposes a
/// path, display name, endpoint, or transfer byte count to the notification
/// dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotificationTransferKind {
    Direct,
    Intent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotificationTransferState {
    Active,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NotificationTransferSummary {
    pub(crate) kind: NotificationTransferKind,
    pub(crate) transfer_id: wire::TransferId,
    /// Every SFTP transfer has at least one remote endpoint. The optional
    /// form keeps the internal projection conservative if a future intent
    /// kind has no remote session fence.
    pub(crate) generation: Option<WireSequence>,
    pub(crate) state_revision: WireSequence,
    pub(crate) state: NotificationTransferState,
}

#[derive(Clone)]
pub(crate) struct SftpSessionService {
    hosts: HostService,
    vault: VaultService,
    transient_credentials: TransientCredentialService,
    ssh_agent: SshAgentService,
    records: Arc<Mutex<BTreeMap<String, SftpServiceRecord>>>,
    local_boundaries: Arc<Mutex<BTreeMap<String, LocalBoundary>>>,
    local_directories: Arc<Mutex<BTreeMap<String, LocalDirectoryCapability>>>,
    local_directory_entries: Arc<Mutex<BTreeMap<String, LocalDirectoryEntryReference>>>,
    local_directory_cursors: Arc<Mutex<BTreeMap<String, LocalDirectoryCursor>>>,
    remote_directory_refs: Arc<Mutex<BTreeMap<String, RemoteDirectoryReference>>>,
    remote_directory_cursors: Arc<Mutex<BTreeMap<String, RemoteDirectoryCursor>>>,
    active_remote_directory_cursors: Arc<Mutex<BTreeMap<String, ActiveRemoteDirectoryCursor>>>,
    remote_directory_cursor_scheduler: Arc<Mutex<()>>,
    remote_entry_refs: Arc<Mutex<BTreeMap<String, RemoteEntryReference>>>,
    local_directory_list_ledger: Arc<Mutex<BTreeMap<String, LocalDirectoryListLedgerRecord>>>,
    local_directory_open_ledger: Arc<Mutex<BTreeMap<String, LocalDirectoryOpenLedgerRecord>>>,
    remote_directory_cancel_ledger: Arc<Mutex<BTreeMap<String, RemoteDirectoryCancelLedgerRecord>>>,
    remote_directory_list_ledger: Arc<Mutex<BTreeMap<String, RemoteDirectoryListLedgerRecord>>>,
    remote_directory_list_operations: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
    active_transfers: Arc<Mutex<BTreeMap<String, ActiveTransferControl>>>,
    transfer_intents: Arc<Mutex<BTreeMap<String, TransferIntentRecord>>>,
    prepared_transfer_intents: Arc<Mutex<BTreeMap<String, PreparedTransferIntentToken>>>,
    public_intent_transfer_ids: Arc<Mutex<BTreeSet<String>>>,
    pending_remote_copies: Arc<Mutex<VecDeque<String>>>,
    remote_copy_scheduler: Arc<Mutex<()>>,
    active_transfer_intents: Arc<Mutex<BTreeMap<String, ActiveTransferControl>>>,
    transfer_boundaries: Arc<Mutex<BTreeMap<String, LocalBoundary>>>,
    mutation_ledger: Arc<Mutex<MutationLedger>>,
    cleanup_operation_ledger: Arc<Mutex<CleanupOperationLedger>>,
    cleanup_exit_authorizations: Arc<Mutex<BTreeMap<String, CleanupExitAuthorization>>>,
    intent_cleanup_exit_authorizations:
        Arc<Mutex<BTreeMap<String, IntentCleanupExitAuthorization>>>,
    intent_cleanup_replays: Arc<Mutex<BTreeMap<String, IntentCleanupReplay>>>,
    transfer_scheduling_paused: Arc<AtomicBool>,
}

impl SftpSessionService {
    pub(crate) fn production(
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: SshAgentService,
    ) -> Self {
        Self {
            hosts,
            vault,
            transient_credentials,
            ssh_agent,
            records: Arc::new(Mutex::new(BTreeMap::new())),
            local_boundaries: Arc::new(Mutex::new(BTreeMap::new())),
            local_directories: Arc::new(Mutex::new(BTreeMap::new())),
            local_directory_entries: Arc::new(Mutex::new(BTreeMap::new())),
            local_directory_cursors: Arc::new(Mutex::new(BTreeMap::new())),
            remote_directory_refs: Arc::new(Mutex::new(BTreeMap::new())),
            remote_directory_cursors: Arc::new(Mutex::new(BTreeMap::new())),
            active_remote_directory_cursors: Arc::new(Mutex::new(BTreeMap::new())),
            remote_directory_cursor_scheduler: Arc::new(Mutex::new(())),
            remote_entry_refs: Arc::new(Mutex::new(BTreeMap::new())),
            local_directory_list_ledger: Arc::new(Mutex::new(BTreeMap::new())),
            local_directory_open_ledger: Arc::new(Mutex::new(BTreeMap::new())),
            remote_directory_cancel_ledger: Arc::new(Mutex::new(BTreeMap::new())),
            remote_directory_list_ledger: Arc::new(Mutex::new(BTreeMap::new())),
            remote_directory_list_operations: Arc::new(Mutex::new(BTreeMap::new())),
            active_transfers: Arc::new(Mutex::new(BTreeMap::new())),
            transfer_intents: Arc::new(Mutex::new(BTreeMap::new())),
            prepared_transfer_intents: Arc::new(Mutex::new(BTreeMap::new())),
            public_intent_transfer_ids: Arc::new(Mutex::new(BTreeSet::new())),
            pending_remote_copies: Arc::new(Mutex::new(VecDeque::new())),
            remote_copy_scheduler: Arc::new(Mutex::new(())),
            active_transfer_intents: Arc::new(Mutex::new(BTreeMap::new())),
            transfer_boundaries: Arc::new(Mutex::new(BTreeMap::new())),
            mutation_ledger: Arc::new(Mutex::new(MutationLedger::default())),
            cleanup_operation_ledger: Arc::new(Mutex::new(CleanupOperationLedger::default())),
            cleanup_exit_authorizations: Arc::new(Mutex::new(BTreeMap::new())),
            intent_cleanup_exit_authorizations: Arc::new(Mutex::new(BTreeMap::new())),
            intent_cleanup_replays: Arc::new(Mutex::new(BTreeMap::new())),
            transfer_scheduling_paused: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) async fn open(
        &self,
        request: &wire::SftpSessionOpenRequest,
    ) -> Result<wire::SftpSessionSummary, SftpProductionError> {
        self.open_with_connection_gate(request, None, None).await
    }

    /// The regular UI entry does not carry a separately approved full profile
    /// revision. Core-only callers that do must use `plugin_open_saved_host`,
    /// which enters this shared implementation with an exact revision gate.
    async fn open_with_connection_gate(
        &self,
        request: &wire::SftpSessionOpenRequest,
        expected_connection_revision: Option<&str>,
        admission_fence: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<wire::SftpSessionSummary, SftpProductionError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let session_key = request.session_id.to_string();
        let operation_id = request.operation_id.to_string();
        let fingerprint = open_fingerprint(request);
        let mut records = self.records.lock().await;
        if let Some(record) = records.get(&session_key)
            && record.open_operation_id == operation_id
        {
            if record.open_idempotency_key != request.idempotency_key
                || record.open_fingerprint != fingerprint
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            return map_sftp_summary(record);
        }
        let factory = SftpTransportFactory::new(
            &self.hosts,
            &self.vault,
            &self.transient_credentials,
            &self.ssh_agent,
        );
        let profile =
            factory.resolve_saved_host(&request.host_id, request.expected_host_state_version)?;
        if expected_connection_revision.is_some_and(|expected| {
            expected.is_empty() || profile.connection.revision_token != expected
        }) || admission_fence.is_some_and(|fence| !fence())
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let mut existing_queue = VecDeque::new();
        let mut actor = if let Some(mut record) = records.remove(&session_key) {
            if record.actor.summary().host_id.as_deref() != Some(request.host_id.as_str())
                || !matches!(
                    record.actor.summary().state,
                    SftpSessionState::Failed | SftpSessionState::Closed
                )
                || record.shared_cleanup_incomplete
                || record.active_transfer_id.is_some()
            {
                records.insert(session_key, record);
                return Err(SftpRuntimeError::InvalidState.into());
            }
            record.heartbeat_tasks.take();
            if let Some(live) = record.live.take() {
                let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await;
            }
            existing_queue = record.pending_transfers;
            record.actor
        } else {
            SftpSessionActor::new(
                SftpSessionId::parse(session_key.clone())?,
                request.host_id.to_string(),
            )?
        };
        let generation = actor.start(profile.revision_token.clone())?;
        let verifier = Arc::new(TrustedSftpHostKeyVerifier {
            hosts: self.hosts.clone(),
        });
        let mut interaction = NonInteractiveSftpConnectionInteraction;
        let connection = match factory.connect(profile, verifier, &mut interaction).await {
            Ok(connection) => connection,
            Err(error) => {
                let _ = actor.connection_failed(generation, map_production_failure(&error));
                let record = SftpServiceRecord {
                    actor,
                    live: None,
                    shared_cleanup_incomplete: false,
                    shared_parent_channels: None,
                    heartbeat_tasks: None,
                    pending_transfers: existing_queue,
                    active_transfer_id: None,
                    open_operation_id: operation_id,
                    open_idempotency_key: request.idempotency_key.clone(),
                    open_fingerprint: fingerprint,
                };
                let summary = map_sftp_summary(&record)?;
                records.insert(session_key, record);
                return Ok(summary);
            }
        };
        let mut live = match connection.open_subsystem(&mut actor, generation).await {
            Ok(live) => Some(live),
            Err(error) => {
                if actor.summary().state != SftpSessionState::Failed {
                    let _ = actor.connection_failed(generation, map_production_failure(&error));
                }
                None
            }
        };
        let heartbeat_tasks = live.as_mut().and_then(|live| {
            live.transport.dedicated_close_handle().map(|close| {
                SftpHeartbeatTasks::start(
                    generation,
                    close,
                    live.transport_keepalive.as_ref(),
                    std::mem::take(&mut live.transport_heartbeats),
                )
            })
        });
        let record = SftpServiceRecord {
            actor,
            live,
            shared_cleanup_incomplete: false,
            shared_parent_channels: None,
            heartbeat_tasks,
            pending_transfers: existing_queue,
            active_transfer_id: None,
            open_operation_id: operation_id,
            open_idempotency_key: request.idempotency_key.clone(),
            open_fingerprint: fingerprint,
        };
        let summary = map_sftp_summary(&record)?;
        records.insert(session_key, record);
        drop(records);
        self.start_next_queued_transfer(request.session_id.as_str())
            .await;
        Ok(summary)
    }

    /// Adopts one SFTP subsystem opened on an existing user-owned SSH transport. This path never
    /// resolves credentials, authenticates, or creates another network connection. Closing the
    /// child record closes only its subsystem channel; the parent SSH session remains owned by
    /// [`crate::ssh_session_service::SshSessionService`].
    pub(crate) async fn open_on_ssh_session(
        &self,
        session_id: wire::SftpSessionId,
        lease: SessionChannelLease,
    ) -> Result<wire::SftpSessionSummary, SftpProductionError> {
        if !lease.is_current() {
            return Err(SftpRuntimeError::StaleGeneration.into());
        }
        let session_key = session_id.to_string();
        let mut records = self.records.lock().await;
        if records.contains_key(&session_key) {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let host_id = match &lease.target {
            wire::SshSessionTarget::Host { host_id, .. } => Some(host_id.clone()),
            wire::SshSessionTarget::QuickConnect { .. } => None,
        };
        let parent = wire::SftpParentSshSession {
            session_id: lease.session_id.clone(),
            generation: lease.generation,
        };
        let revision_token = format!(
            "shared-ssh-v1:{}:{}:{}",
            lease.session_id,
            lease.generation.get(),
            lease.parent_channel_id
        );
        let mut actor = SftpSessionActor::new_on_ssh_session(
            SftpSessionId::parse(session_id.to_string())?,
            host_id,
            parent,
        )?;
        let generation = actor.start(revision_token.clone())?;
        let transport_id = format!("shared:{}:{}", lease.session_id, lease.generation.get());
        actor.authenticated_transport_ready(generation, transport_id.clone())?;

        let channel = match lease.channels.open_sftp_channel().await {
            Ok(channel) if lease.is_current() => channel,
            Ok(channel) => {
                let shared_cleanup_incomplete = channel.close().await.is_err();
                actor.connection_failed(generation, SftpFailureCode::TransportLost)?;
                let parent_channels = lease.channels.clone();
                let record = shared_sftp_record(actor, None, lease, shared_cleanup_incomplete);
                let summary = map_sftp_summary(&record)?;
                records.insert(session_key, record);
                drop(records);
                let service = self.clone();
                let child_session_id = summary.session_id.clone();
                let child_generation = summary.generation;
                tokio::spawn(async move {
                    parent_channels.wait_closed().await;
                    service
                        .close_shared_child_after_parent_termination(
                            child_session_id,
                            child_generation,
                        )
                        .await;
                });
                return Ok(summary);
            }
            Err(error) => {
                actor.connection_failed(generation, map_transport_failure(&error))?;
                let record = shared_sftp_record(actor, None, lease, false);
                let summary = map_sftp_summary(&record)?;
                records.insert(session_key, record);
                return Ok(summary);
            }
        };
        let subsystem_id = uuid::Uuid::now_v7().to_string();
        actor.subsystem_opened(generation, subsystem_id.clone())?;
        let live = SftpProductionSession {
            generation,
            transport: SftpLiveTransport::Shared(channel),
            transport_heartbeats: Vec::new(),
            transport_keepalive: None,
        };
        let parent_channels = lease.channels.clone();
        let record = shared_sftp_record(actor, Some(live), lease, false);
        let summary = map_sftp_summary(&record)?;
        records.insert(session_key, record);
        drop(records);
        let service = self.clone();
        let child_session_id = summary.session_id.clone();
        let child_generation = summary.generation;
        tokio::spawn(async move {
            parent_channels.wait_closed().await;
            service
                .close_shared_child_after_parent_termination(child_session_id, child_generation)
                .await;
        });
        Ok(summary)
    }

    async fn snapshot(&self) -> Result<wire::SftpSessionSnapshot, SftpProductionError> {
        // Intent-created local<->remote transfers still execute in the
        // single-session actor for compatibility, but their only public
        // transfer projection is SftpTransferIntentSnapshot. Hold this owner
        // fence while reading actors so a concurrently enqueued intent cannot
        // appear briefly in both snapshots.
        let public_intent_transfer_ids = self.public_intent_transfer_ids.lock().await;
        let mut records = self.records.lock().await;
        for (session_key, record) in records.iter_mut() {
            self.reconcile_record_liveness(session_key, record).await;
        }
        let sessions = records
            .values()
            .map(map_sftp_summary)
            .collect::<Result<Vec<_>, _>>()?;
        let transfers = records
            .values()
            .flat_map(|record| {
                record
                    .actor
                    .transfers()
                    .filter(|transfer| {
                        !public_intent_transfer_ids.contains(transfer.transfer_id.as_str())
                    })
                    .map(move |transfer| (record, transfer))
            })
            .map(|(record, transfer)| map_transfer_summary(record, transfer))
            .collect::<Result<Vec<_>, _>>()?;
        let revision = sessions
            .iter()
            .map(|summary| summary.state_revision.get())
            .max()
            .unwrap_or(0);
        Ok(wire::SftpSessionSnapshot {
            snapshot_revision: wire::WireSequence::new(revision),
            sessions,
            transfers,
        })
    }

    async fn register_local_directory(
        &self,
        request: wire::SftpLocalDirectoryRegisterRequest,
    ) -> Result<wire::SftpLocalDirectoryCapability, SftpProductionError> {
        if request.selected_path.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = request;
            return Err(SftpRuntimeError::InvalidState.into());
        }
        #[cfg(any(unix, windows))]
        {
            let selected = PathBuf::from(&request.selected_path);
            let capability = NativeLocalDirectoryCapability::register(&selected)
                .map_err(|_| SftpRuntimeError::InvalidInput)?;
            let capability_id = uuid::Uuid::now_v7().to_string();
            let revision = 1;
            let display_name = local_display_name(&selected);
            let rememberable_path = rememberable_local_directory_path(&selected);
            let mut capabilities = self.local_directories.lock().await;
            capabilities.retain(|_, capability| {
                !capability.revoked.load(Ordering::Acquire)
                    && capability.expires_at > std::time::Instant::now()
            });
            if capabilities.len() >= MAX_LOCAL_DIRECTORY_CAPABILITIES {
                return Err(SftpRuntimeError::InvalidState.into());
            }
            capabilities.insert(
                capability_id.clone(),
                LocalDirectoryCapability {
                    revision,
                    capability: Arc::new(LocalDirectoryCapabilityHandle::Native(capability)),
                    rememberable_path: rememberable_path.clone(),
                    revoked: Arc::new(AtomicBool::new(false)),
                    expires_at: std::time::Instant::now() + LOCAL_DIRECTORY_CAPABILITY_TTL,
                },
            );
            Ok(wire::SftpLocalDirectoryCapability {
                directory_ref: capability_id,
                revision: WireSequence::new(revision),
                display_name,
                rememberable_path,
            })
        }
    }

    async fn list_local_directory(
        &self,
        request: wire::SftpLocalDirectoryListRequest,
    ) -> Result<wire::SftpLocalDirectoryListing, SftpProductionError> {
        if request.idempotency_key.trim().is_empty()
            || request.directory_ref.trim().is_empty()
            || request.page_size == 0
            || request.page_size > MAX_DIRECTORY_PAGE_SIZE
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = request;
            // Unsupported platforms must not fall back to ambient path access.
            return Err(SftpRuntimeError::InvalidState.into());
        }
        #[cfg(any(unix, windows))]
        {
            let operation_id = request.operation_id.to_string();
            let fingerprint = local_directory_list_fingerprint(&request);
            if let Some(replay) = self
                .local_directory_list_ledger
                .lock()
                .await
                .get(&operation_id)
                .cloned()
            {
                if replay.idempotency_key != request.idempotency_key
                    || replay.fingerprint != fingerprint
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                self.require_local_directory(
                    &replay.result.directory_ref,
                    replay.result.revision.get(),
                )
                .await?;
                return Ok(replay.result);
            }
            let directory_capability = self
                .local_directories
                .lock()
                .await
                .get(&request.directory_ref)
                .cloned()
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if directory_capability.revision != request.expected_revision.get()
                || directory_capability.revoked.load(Ordering::Acquire)
                || directory_capability.expires_at <= std::time::Instant::now()
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            let cursor_state = if let Some(cursor) = request.cursor.as_deref() {
                let cursor_ref = std::str::from_utf8(cursor)
                    .map_err(|_| SftpRuntimeError::InvalidInput)?
                    .to_owned();
                let cursor = self
                    .local_directory_cursors
                    .lock()
                    .await
                    .remove(&cursor_ref)
                    .ok_or(SftpRuntimeError::Conflict)?;
                if cursor.directory_ref != request.directory_ref
                    || cursor.directory_revision != request.expected_revision.get()
                    || cursor.expires_at <= std::time::Instant::now()
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                Some(cursor)
            } else {
                None
            };
            let (entries, next_stream) = match directory_capability.capability.as_ref() {
                LocalDirectoryCapabilityHandle::Native(capability) => capability
                    .list_page(
                        cursor_state.map(|cursor| cursor.stream),
                        usize::from(request.page_size),
                        &directory_capability.revoked,
                    )
                    .map_err(|_| SftpRuntimeError::Conflict)?,
            };
            let mut entry_refs = self.local_directory_entries.lock().await;
            entry_refs.retain(|_, entry| entry.expires_at > std::time::Instant::now());
            if entry_refs.len().saturating_add(entries.len()) > MAX_DIRECTORY_ENTRY_REFS {
                return Err(SftpRuntimeError::InvalidState.into());
            }
            if entries
                .iter()
                .any(|(_, identity)| identity.is_regular() && identity.size > MAX_JS_SAFE_INTEGER)
            {
                return Err(SftpRuntimeError::InvalidInput.into());
            }
            let page_entries = entries
                .into_iter()
                .map(|(name, identity)| {
                    let entry_ref = uuid::Uuid::now_v7().to_string();
                    entry_refs.insert(
                        entry_ref.clone(),
                        LocalDirectoryEntryReference {
                            directory_ref: request.directory_ref.clone(),
                            directory_revision: request.expected_revision.get(),
                            name: name.clone(),
                            identity: LocalObjectIdentity::from_native(&identity),
                            expires_at: std::time::Instant::now() + DIRECTORY_REFERENCE_TTL,
                        },
                    );
                    map_local_entry(entry_ref, &name, &identity)
                })
                .collect::<Vec<_>>();
            drop(entry_refs);
            let next_cursor = if let Some(stream) = next_stream {
                let cursor_ref = uuid::Uuid::now_v7().to_string();
                let mut cursors = self.local_directory_cursors.lock().await;
                cursors.retain(|_, cursor| cursor.expires_at > std::time::Instant::now());
                if cursors.len() >= MAX_DIRECTORY_CURSORS {
                    return Err(SftpRuntimeError::InvalidState.into());
                }
                cursors.insert(
                    cursor_ref.clone(),
                    LocalDirectoryCursor {
                        directory_ref: request.directory_ref.clone(),
                        directory_revision: request.expected_revision.get(),
                        stream,
                        expires_at: std::time::Instant::now() + DIRECTORY_REFERENCE_TTL,
                    },
                );
                Some(cursor_ref.into_bytes())
            } else {
                None
            };
            let result = wire::SftpLocalDirectoryListing {
                directory_ref: request.directory_ref,
                revision: request.expected_revision,
                entries: page_entries,
                next_cursor,
            };
            let mut ledger = self.local_directory_list_ledger.lock().await;
            if ledger.len() >= MUTATION_LEDGER_CAPACITY
                && let Some(oldest) = ledger.keys().next().cloned()
            {
                ledger.remove(&oldest);
            }
            ledger.insert(
                operation_id,
                LocalDirectoryListLedgerRecord {
                    idempotency_key: request.idempotency_key,
                    fingerprint,
                    result: result.clone(),
                },
            );
            Ok(result)
        }
    }

    async fn open_local_child_directory(
        &self,
        request: wire::SftpLocalDirectoryOpenChildRequest,
    ) -> Result<wire::SftpLocalDirectoryCapability, SftpProductionError> {
        if request.idempotency_key.trim().is_empty()
            || request.parent_directory_ref.trim().is_empty()
            || request.entry_ref.trim().is_empty()
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = request;
            return Err(SftpRuntimeError::InvalidState.into());
        }
        #[cfg(any(unix, windows))]
        {
            let operation_id = request.operation_id.to_string();
            let fingerprint = local_directory_open_fingerprint(&request);
            if let Some(replay) = self
                .local_directory_open_ledger
                .lock()
                .await
                .get(&operation_id)
                .cloned()
            {
                if replay.idempotency_key != request.idempotency_key
                    || replay.fingerprint != fingerprint
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                self.require_local_directory(
                    &replay.result.directory_ref,
                    replay.result.revision.get(),
                )
                .await?;
                return Ok(replay.result);
            }
            let parent = self
                .local_directories
                .lock()
                .await
                .get(&request.parent_directory_ref)
                .cloned()
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if parent.revision != request.expected_parent_revision.get()
                || parent.revoked.load(Ordering::Acquire)
                || parent.expires_at <= std::time::Instant::now()
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            let entry = self
                .local_directory_entries
                .lock()
                .await
                .get(&request.entry_ref)
                .cloned()
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if entry.directory_ref != request.parent_directory_ref
                || entry.directory_revision != request.expected_parent_revision.get()
                || entry.expires_at <= std::time::Instant::now()
                || entry.identity.kind != LocalObjectKind::Directory
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            let child = match parent.capability.as_ref() {
                LocalDirectoryCapabilityHandle::Native(parent) => parent
                    .open_child(&entry.name, &entry.identity)
                    .map_err(|_| SftpRuntimeError::Conflict)?,
            };
            let capability_id = uuid::Uuid::now_v7().to_string();
            let revision = 1;
            let display_name = safe_local_name_display(&entry.name);
            let rememberable_path =
                rememberable_local_child_path(parent.rememberable_path.as_deref(), &entry.name);
            let mut capabilities = self.local_directories.lock().await;
            capabilities.retain(|_, capability| {
                !capability.revoked.load(Ordering::Acquire)
                    && capability.expires_at > std::time::Instant::now()
            });
            if capabilities.len() >= MAX_LOCAL_DIRECTORY_CAPABILITIES {
                return Err(SftpRuntimeError::InvalidState.into());
            }
            capabilities.insert(
                capability_id.clone(),
                LocalDirectoryCapability {
                    revision,
                    capability: Arc::new(LocalDirectoryCapabilityHandle::Native(child)),
                    rememberable_path: rememberable_path.clone(),
                    revoked: Arc::new(AtomicBool::new(false)),
                    expires_at: std::time::Instant::now() + LOCAL_DIRECTORY_CAPABILITY_TTL,
                },
            );
            let result = wire::SftpLocalDirectoryCapability {
                directory_ref: capability_id,
                revision: WireSequence::new(revision),
                display_name,
                rememberable_path,
            };
            let mut ledger = self.local_directory_open_ledger.lock().await;
            if ledger.len() >= MUTATION_LEDGER_CAPACITY
                && let Some(oldest) = ledger.keys().next().cloned()
            {
                ledger.remove(&oldest);
            }
            ledger.insert(
                operation_id,
                LocalDirectoryOpenLedgerRecord {
                    idempotency_key: request.idempotency_key,
                    fingerprint,
                    result: result.clone(),
                },
            );
            Ok(result)
        }
    }

    async fn create_local_child_directory(
        &self,
        request: wire::SftpLocalDirectoryCreateChildRequest,
    ) -> Result<wire::SftpLocalDirectoryCapability, SftpProductionError> {
        if request.idempotency_key.trim().is_empty()
            || request.parent_directory_ref.trim().is_empty()
            || request.name.is_empty()
            || request.name == "."
            || request.name == ".."
            || request.name.contains('/')
            || request.name.contains('\0')
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        #[cfg(not(unix))]
        {
            let _ = request;
            return Err(SftpRuntimeError::InvalidState.into());
        }
        #[cfg(unix)]
        {
            let operation_id = request.operation_id.to_string();
            let fingerprint = local_directory_create_fingerprint(&request);
            if let Some(replay) = self
                .local_directory_open_ledger
                .lock()
                .await
                .get(&operation_id)
                .cloned()
            {
                if replay.idempotency_key != request.idempotency_key
                    || replay.fingerprint != fingerprint
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                self.require_local_directory(
                    &replay.result.directory_ref,
                    replay.result.revision.get(),
                )
                .await?;
                return Ok(replay.result);
            }
            let parent = self
                .require_local_directory(
                    &request.parent_directory_ref,
                    request.expected_parent_revision.get(),
                )
                .await?;
            let child = match parent.capability.as_ref() {
                LocalDirectoryCapabilityHandle::Native(parent) => parent
                    .create_child(request.name.as_bytes())
                    .map_err(|_| SftpRuntimeError::Conflict)?,
            };
            let capability_id = uuid::Uuid::now_v7().to_string();
            let revision = 1;
            let display_name = request.name;
            let rememberable_path = rememberable_local_child_path(
                parent.rememberable_path.as_deref(),
                display_name.as_bytes(),
            );
            let mut capabilities = self.local_directories.lock().await;
            capabilities.retain(|_, capability| {
                !capability.revoked.load(Ordering::Acquire)
                    && capability.expires_at > std::time::Instant::now()
            });
            if capabilities.len() >= MAX_LOCAL_DIRECTORY_CAPABILITIES {
                return Err(SftpRuntimeError::InvalidState.into());
            }
            capabilities.insert(
                capability_id.clone(),
                LocalDirectoryCapability {
                    revision,
                    capability: Arc::new(LocalDirectoryCapabilityHandle::Native(child)),
                    rememberable_path: rememberable_path.clone(),
                    revoked: Arc::new(AtomicBool::new(false)),
                    expires_at: std::time::Instant::now() + LOCAL_DIRECTORY_CAPABILITY_TTL,
                },
            );
            let result = wire::SftpLocalDirectoryCapability {
                directory_ref: capability_id,
                revision: WireSequence::new(revision),
                display_name,
                rememberable_path,
            };
            let mut ledger = self.local_directory_open_ledger.lock().await;
            if ledger.len() >= MUTATION_LEDGER_CAPACITY
                && let Some(oldest) = ledger.keys().next().cloned()
            {
                ledger.remove(&oldest);
            }
            ledger.insert(
                operation_id,
                LocalDirectoryOpenLedgerRecord {
                    idempotency_key: request.idempotency_key,
                    fingerprint,
                    result: result.clone(),
                },
            );
            Ok(result)
        }
    }

    async fn release_local_directory(
        &self,
        request: wire::SftpLocalDirectoryReleaseRequest,
    ) -> Result<(), SftpProductionError> {
        let mut capabilities = self.local_directories.lock().await;
        let capability = capabilities
            .get(&request.directory_ref)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if capability.revision != request.expected_revision.get() {
            return Err(SftpRuntimeError::Conflict.into());
        }
        capability.revoked.store(true, Ordering::Release);
        capabilities.remove(&request.directory_ref);
        drop(capabilities);
        self.local_directory_entries
            .lock()
            .await
            .retain(|_, entry| entry.directory_ref != request.directory_ref);
        self.local_directory_cursors
            .lock()
            .await
            .retain(|_, cursor| cursor.directory_ref != request.directory_ref);
        Ok(())
    }

    pub(crate) async fn wire_summaries(
        &self,
    ) -> Result<Vec<wire::SftpSessionSummary>, SftpProductionError> {
        let mut records = self.records.lock().await;
        for (session_key, record) in records.iter_mut() {
            self.reconcile_record_liveness(session_key, record).await;
        }
        records
            .values()
            .map(map_sftp_summary)
            .collect::<Result<Vec<_>, _>>()
    }

    pub(crate) async fn list_directory(
        &self,
        request: wire::SftpDirectoryListRequest,
    ) -> Result<wire::SftpDirectoryListing, SftpProductionError> {
        if request.idempotency_key.trim().is_empty()
            || request.page_size == 0
            || request.page_size > MAX_DIRECTORY_PAGE_SIZE
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let operation_id = request.operation_id.to_string();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = remote_directory_list_fingerprint(&request);
        let operation_lock = {
            let mut operations = self.remote_directory_list_operations.lock().await;
            if !operations.contains_key(&operation_id)
                && operations.len() >= MUTATION_LEDGER_CAPACITY
            {
                let removable = operations
                    .iter()
                    .find_map(|(key, lock)| (Arc::strong_count(lock) == 1).then_some(key.clone()))
                    .ok_or(SftpRuntimeError::InvalidState)?;
                operations.remove(&removable);
            }
            operations
                .entry(operation_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _operation_guard = operation_lock.lock().await;
        if let Some(replay) = self
            .remote_directory_list_ledger
            .lock()
            .await
            .get(&operation_id)
            .cloned()
        {
            if replay.idempotency_key != idempotency_key || replay.fingerprint != fingerprint {
                return Err(SftpRuntimeError::Conflict.into());
            }
            self.require_remote_intent_endpoint(
                &replay.result.session_id,
                replay.result.generation,
            )
            .await?;
            return Ok(replay.result);
        }
        let session_key = request.session_id.to_string();
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let path = RemotePath::parse(request.path.bytes.clone())?;
        let requested_cursor = request.cursor.clone();
        self.prune_remote_directory_cursors().await;
        let (plan, directory_client) = {
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&session_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            self.reconcile_record_liveness(&session_key, record).await;
            let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
            let plan = record.actor.directory_listing(
                OperationFence::parse(request.operation_id.to_string(), request.idempotency_key)?,
                generation,
                path.clone(),
                requested_cursor.clone(),
                request.page_size,
            )?;
            let directory_client = requested_cursor.is_none().then(|| live.directory_client());
            (plan, directory_client)
        };
        let (directory_ref, page_entries, continuation) = if requested_cursor.is_none() {
            let page = directory_client
                .ok_or(SftpRuntimeError::InvalidState)?
                .list_directory(&plan)
                .await?;
            let directory_ref = uuid::Uuid::now_v7().to_string();
            let entries_read = page.entries.len();
            let continuation = page.continuation.map(|stream| RemoteDirectoryCursor {
                directory_ref: directory_ref.clone(),
                session_id: session_key.clone(),
                generation: generation.get(),
                path: path.clone(),
                stream: RemoteDirectoryCursorStream::Live(stream),
                expires_at: std::time::Instant::now() + DIRECTORY_REFERENCE_TTL,
                pages_read: 1,
                entries_read,
            });
            (directory_ref, page.entries, continuation)
        } else {
            let cursor_ref = std::str::from_utf8(
                requested_cursor
                    .as_deref()
                    .ok_or(SftpRuntimeError::InvalidInput)?,
            )
            .map_err(|_| SftpRuntimeError::InvalidInput)?
            .to_owned();
            let scheduler = self.remote_directory_cursor_scheduler.lock().await;
            let mut cursor = {
                let mut cursors = self.remote_directory_cursors.lock().await;
                cursors
                    .remove(&cursor_ref)
                    .ok_or(SftpRuntimeError::Conflict)?
            };
            if cursor.session_id != session_key
                || cursor.generation != generation.get()
                || cursor.path != path
                || cursor.expires_at <= std::time::Instant::now()
            {
                drop(scheduler);
                cursor.close().await;
                return Err(SftpRuntimeError::Conflict.into());
            }
            let (cancel_requested, cancel_receiver) = watch::channel(false);
            let (close_result, close_result_receiver) = watch::channel(None);
            self.active_remote_directory_cursors.lock().await.insert(
                cursor_ref.clone(),
                ActiveRemoteDirectoryCursor {
                    session_id: session_key.clone(),
                    generation: generation.get(),
                    path: path.clone(),
                    cancel_requested,
                    close_result: close_result_receiver,
                },
            );
            drop(scheduler);
            let directory_ref = cursor.directory_ref.clone();
            let mut control = RemoteDirectoryPageControl {
                cursor_ref,
                cancel_requested: cancel_receiver,
                close_result,
            };
            let page_result = {
                let page = cursor.take_page(request.page_size);
                tokio::pin!(page);
                tokio::select! {
                    result = &mut page => Some(result),
                    _ = wait_for_true(&mut control.cancel_requested) => None,
                }
            };
            let cancellation_requested = {
                let _scheduler = self.remote_directory_cursor_scheduler.lock().await;
                let requested = page_result.is_none() || *control.cancel_requested.borrow();
                self.active_remote_directory_cursors
                    .lock()
                    .await
                    .remove(&control.cursor_ref);
                requested
            };
            if cancellation_requested {
                let closed = cursor.close_in_place().await;
                control.close_result.send_replace(Some(closed));
                return Err(if closed {
                    SftpRuntimeError::Conflict.into()
                } else {
                    SftpRuntimeError::CleanupIncomplete.into()
                });
            }
            let entries = match page_result.expect("non-cancelled directory page has a result") {
                Ok(entries) => entries,
                Err(error) => {
                    let closed = cursor.close_in_place().await;
                    control.close_result.send_replace(Some(closed));
                    return Err(error);
                }
            };
            control.close_result.send_replace(Some(true));
            let continuation = (!cursor.is_complete()).then_some(cursor);
            (directory_ref, entries, continuation)
        };
        if page_entries
            .iter()
            .any(|entry| entry.size.is_some_and(|size| size > MAX_JS_SAFE_INTEGER))
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let expires_at = continuation
            .as_ref()
            .map(|cursor| cursor.expires_at)
            .unwrap_or_else(|| std::time::Instant::now() + DIRECTORY_REFERENCE_TTL);
        let mut directories = self.remote_directory_refs.lock().await;
        directories.retain(|_, reference| reference.expires_at > std::time::Instant::now());
        if let Some(reference) = directories.get_mut(&directory_ref) {
            if reference.session_id != session_key
                || reference.generation != generation.get()
                || reference.path != path
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            reference.expires_at = reference.expires_at.min(expires_at);
        } else {
            if requested_cursor.is_some() {
                return Err(SftpRuntimeError::Conflict.into());
            }
            if directories.len() >= MAX_DIRECTORY_CURSORS {
                return Err(SftpRuntimeError::InvalidState.into());
            }
            directories.insert(
                directory_ref.clone(),
                RemoteDirectoryReference {
                    session_id: session_key.clone(),
                    generation: generation.get(),
                    path: path.clone(),
                    expires_at,
                },
            );
        }
        drop(directories);
        let mut entry_refs = self.remote_entry_refs.lock().await;
        entry_refs.retain(|_, reference| reference.expires_at > std::time::Instant::now());
        if entry_refs.len().saturating_add(page_entries.len()) > MAX_DIRECTORY_ENTRY_REFS {
            return Err(SftpRuntimeError::InvalidState.into());
        }
        let entries = page_entries
            .into_iter()
            .map(|entry| {
                let entry_ref = uuid::Uuid::now_v7().to_string();
                let precondition = RemoteObjectPrecondition {
                    kind: entry.kind.clone(),
                    size: entry.size,
                    modified_at_unix_ms: entry.modified_at_unix_ms,
                };
                let mapped = map_remote_entry(entry_ref.clone(), entry.clone())?;
                entry_refs.insert(
                    entry_ref,
                    RemoteEntryReference {
                        directory_ref: directory_ref.clone(),
                        session_id: session_key.clone(),
                        generation: generation.get(),
                        path: entry.path,
                        name: remote_name_bytes(mapped.path.bytes.as_slice())?.to_vec(),
                        precondition,
                        display_name: mapped.display_name.clone(),
                        expires_at,
                    },
                );
                Ok(mapped)
            })
            .collect::<Result<Vec<_>, SftpProductionError>>()?;
        drop(entry_refs);
        let next_cursor = if let Some(mut continuation) = continuation {
            continuation.expires_at = continuation.expires_at.min(expires_at);
            let cursor_ref = uuid::Uuid::now_v7().to_string();
            let mut continuation = Some(continuation);
            let inserted = {
                let _scheduler = self.remote_directory_cursor_scheduler.lock().await;
                let mut cursors = self.remote_directory_cursors.lock().await;
                if cursors.len() >= MAX_DIRECTORY_CURSORS {
                    false
                } else {
                    cursors.insert(
                        cursor_ref.clone(),
                        continuation.take().expect("continuation is inserted once"),
                    );
                    true
                }
            };
            if !inserted {
                let continuation = continuation.expect("full cursor map retains continuation");
                continuation.close().await;
                return Err(SftpRuntimeError::InvalidState.into());
            }
            Some(cursor_ref.into_bytes())
        } else {
            None
        };
        let result = wire::SftpDirectoryListing {
            session_id: request.session_id,
            generation: request.expected_generation,
            directory_ref,
            path: wire::SftpRemotePath { bytes: path.0 },
            entries,
            next_cursor,
        };
        let mut ledger = self.remote_directory_list_ledger.lock().await;
        if ledger.len() >= MUTATION_LEDGER_CAPACITY
            && let Some(oldest) = ledger.keys().next().cloned()
        {
            ledger.remove(&oldest);
        }
        ledger.insert(
            operation_id.clone(),
            RemoteDirectoryListLedgerRecord {
                idempotency_key,
                fingerprint,
                result: result.clone(),
            },
        );
        self.remote_directory_list_operations
            .lock()
            .await
            .remove(&operation_id);
        Ok(result)
    }

    async fn cancel_directory_listing(
        &self,
        request: wire::SftpDirectoryListCancelRequest,
    ) -> Result<(), SftpProductionError> {
        if request.idempotency_key.trim().is_empty() || request.cursor.is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let operation_id = request.operation_id.to_string();
        let fingerprint = remote_directory_cancel_fingerprint(&request);
        if let Some(replay) = self
            .remote_directory_cancel_ledger
            .lock()
            .await
            .get(&operation_id)
            .cloned()
        {
            if replay.idempotency_key != request.idempotency_key
                || replay.fingerprint != fingerprint
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            return Ok(());
        }
        let cursor_ref =
            std::str::from_utf8(&request.cursor).map_err(|_| SftpRuntimeError::InvalidInput)?;
        enum CancelTarget {
            Idle(RemoteDirectoryCursor),
            Active(watch::Receiver<Option<bool>>),
        }
        let target = {
            let _scheduler = self.remote_directory_cursor_scheduler.lock().await;
            let mut cursors = self.remote_directory_cursors.lock().await;
            if let Some(observed) = cursors.get(cursor_ref) {
                if observed.session_id != request.session_id.as_str()
                    || observed.generation != request.expected_generation.get()
                    || observed.path.as_bytes() != request.path.bytes
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                CancelTarget::Idle(
                    cursors
                        .remove(cursor_ref)
                        .ok_or(SftpRuntimeError::Conflict)?,
                )
            } else {
                drop(cursors);
                let active = self.active_remote_directory_cursors.lock().await;
                let observed = active.get(cursor_ref).ok_or(SftpRuntimeError::Conflict)?;
                if observed.session_id != request.session_id.as_str()
                    || observed.generation != request.expected_generation.get()
                    || observed.path.as_bytes() != request.path.bytes
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                observed.cancel_requested.send_replace(true);
                CancelTarget::Active(observed.close_result.clone())
            }
        };
        let closed = match target {
            CancelTarget::Idle(mut cursor) => cursor.close_in_place().await,
            CancelTarget::Active(mut result) => {
                tokio::time::timeout(SFTP_PROTOCOL_OPERATION_TIMEOUT, async {
                    loop {
                        if let Some(closed) = *result.borrow() {
                            return closed;
                        }
                        if result.changed().await.is_err() {
                            return false;
                        }
                    }
                })
                .await
                .unwrap_or(false)
            }
        };
        if !closed {
            return Err(SftpRuntimeError::CleanupIncomplete.into());
        }
        let mut ledger = self.remote_directory_cancel_ledger.lock().await;
        if ledger.len() >= MUTATION_LEDGER_CAPACITY
            && let Some(oldest) = ledger.keys().next().cloned()
        {
            ledger.remove(&oldest);
        }
        ledger.insert(
            operation_id,
            RemoteDirectoryCancelLedgerRecord {
                idempotency_key: request.idempotency_key,
                fingerprint,
            },
        );
        Ok(())
    }

    async fn preview_file(
        &self,
        request: wire::SftpFilePreviewRequest,
    ) -> Result<wire::SftpFilePreview, SftpProductionError> {
        if request.directory_ref.trim().is_empty() || request.entry_ref.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let session_key = request.session_id.to_string();
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let entry = self
            .remote_entry_refs
            .lock()
            .await
            .get(&request.entry_ref)
            .cloned()
            .ok_or(SftpRuntimeError::Conflict)?;
        if entry.directory_ref != request.directory_ref
            || entry.session_id != session_key
            || entry.generation != generation.get()
            || entry.expires_at <= std::time::Instant::now()
            || entry.precondition.kind != RemoteEntryKind::File
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let preview_kind = preview_kind_for_entry(&entry.path, &entry.display_name)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        let size = entry
            .precondition
            .size
            .ok_or(SftpRuntimeError::InvalidInput)?;
        validate_js_safe_transfer_size(size)?;
        let (start_offset, maximum_bytes, truncated) = match preview_kind {
            PreviewKind::Text if size > MAX_TEXT_PREVIEW_BYTES as u64 => (
                size.saturating_sub(MAX_TAIL_INITIAL_BYTES as u64),
                MAX_TAIL_INITIAL_BYTES,
                true,
            ),
            PreviewKind::Text => (0, MAX_TEXT_PREVIEW_BYTES, false),
            PreviewKind::Image(_) if size <= MAX_IMAGE_PREVIEW_BYTES as u64 => {
                (0, MAX_IMAGE_PREVIEW_BYTES, false)
            }
            PreviewKind::Image(_) => return Err(SftpRuntimeError::InvalidInput.into()),
        };
        let bytes = {
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&session_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            self.reconcile_record_liveness(&session_key, record).await;
            let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
            live.read_remote_file_preview(
                generation,
                &entry.path,
                &entry.precondition,
                start_offset,
                maximum_bytes,
            )
            .await?
        };
        let content = match preview_kind {
            PreviewKind::Text => {
                if bytes.contains(&0) {
                    return Err(SftpRuntimeError::InvalidInput.into());
                }
                let line_ending = detect_text_line_ending(&bytes);
                let text = if truncated {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    String::from_utf8(bytes).map_err(|_| SftpRuntimeError::InvalidInput)?
                };
                wire::SftpFilePreviewContent::Text {
                    text,
                    end_offset: WireSequence::new(size),
                    editable: !truncated,
                    truncated,
                    line_ending,
                }
            }
            PreviewKind::Image(expected_media_type) => {
                let observed_media_type = image_media_type(&bytes)
                    .filter(|media_type| *media_type == expected_media_type)
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                wire::SftpFilePreviewContent::Image {
                    media_type: observed_media_type.to_owned(),
                    bytes,
                }
            }
        };
        Ok(wire::SftpFilePreview {
            session_id: request.session_id,
            generation: request.expected_generation,
            display_name: entry.display_name,
            content,
        })
    }

    pub(crate) async fn tail_file(
        &self,
        request: wire::SftpFileTailRequest,
    ) -> Result<wire::SftpFileTailResult, SftpProductionError> {
        self.tail_file_limited(request, MAX_TAIL_CHUNK_BYTES).await
    }

    pub(crate) async fn tail_file_limited(
        &self,
        request: wire::SftpFileTailRequest,
        maximum_bytes: usize,
    ) -> Result<wire::SftpFileTailResult, SftpProductionError> {
        if maximum_bytes == 0 || maximum_bytes > MAX_TAIL_CHUNK_BYTES {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        if request.directory_ref.trim().is_empty() || request.entry_ref.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let session_key = request.session_id.to_string();
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let entry = {
            let now = std::time::Instant::now();
            let mut entries = self.remote_entry_refs.lock().await;
            let entry = entries
                .get_mut(&request.entry_ref)
                .ok_or(SftpRuntimeError::Conflict)?;
            if entry.directory_ref != request.directory_ref
                || entry.session_id != session_key
                || entry.generation != generation.get()
                || entry.expires_at <= now
                || entry.precondition.kind != RemoteEntryKind::File
                || preview_kind_for_entry(&entry.path, &entry.display_name)
                    != Some(PreviewKind::Text)
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            entry.expires_at = now + DIRECTORY_REFERENCE_TTL;
            entry.clone()
        };
        let (start_offset, next_offset, total_size, reset, bytes) = {
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&session_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            self.reconcile_record_liveness(&session_key, record).await;
            let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
            live.read_remote_file_tail_limited(
                generation,
                &entry.path,
                request.offset.get(),
                maximum_bytes,
            )
            .await?
        };
        Ok(wire::SftpFileTailResult {
            session_id: request.session_id,
            generation: request.expected_generation,
            start_offset: WireSequence::new(start_offset),
            next_offset: WireSequence::new(next_offset),
            total_size: WireSequence::new(total_size),
            reset,
            bytes,
        })
    }

    async fn prune_remote_directory_cursors(&self) {
        let now = std::time::Instant::now();
        let expired = {
            let _scheduler = self.remote_directory_cursor_scheduler.lock().await;
            let mut cursors = self.remote_directory_cursors.lock().await;
            let keys = cursors
                .iter()
                .filter_map(|(key, cursor)| (cursor.expires_at <= now).then_some(key.clone()))
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| cursors.remove(&key))
                .collect::<Vec<_>>()
        };
        for cursor in expired {
            cursor.close().await;
        }
    }

    async fn close_remote_directory_cursors_for_session(&self, session_key: &str) {
        let (removed, active) = {
            let _scheduler = self.remote_directory_cursor_scheduler.lock().await;
            let mut cursors = self.remote_directory_cursors.lock().await;
            let keys = cursors
                .iter()
                .filter_map(|(key, cursor)| {
                    (cursor.session_id == session_key).then_some(key.clone())
                })
                .collect::<Vec<_>>();
            let removed = keys
                .into_iter()
                .filter_map(|key| cursors.remove(&key))
                .collect::<Vec<_>>();
            drop(cursors);
            let active = self
                .active_remote_directory_cursors
                .lock()
                .await
                .values()
                .filter(|cursor| cursor.session_id == session_key)
                .cloned()
                .collect::<Vec<_>>();
            for cursor in &active {
                cursor.cancel_requested.send_replace(true);
            }
            (removed, active)
        };
        for cursor in removed {
            cursor.close().await;
        }
        for mut cursor in active.into_iter().map(|cursor| cursor.close_result) {
            let _ = tokio::time::timeout(SFTP_PROTOCOL_OPERATION_TIMEOUT, async {
                while cursor.borrow().is_none() && cursor.changed().await.is_ok() {}
            })
            .await;
        }
    }

    async fn close_all_remote_directory_cursors(&self) {
        let (cursors, active) = {
            let _scheduler = self.remote_directory_cursor_scheduler.lock().await;
            let mut cursors = self.remote_directory_cursors.lock().await;
            let cursors = std::mem::take(&mut *cursors)
                .into_values()
                .collect::<Vec<_>>();
            let active = self
                .active_remote_directory_cursors
                .lock()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            for cursor in &active {
                cursor.cancel_requested.send_replace(true);
            }
            (cursors, active)
        };
        let _ = tokio::time::timeout(SFTP_CLEANUP_TIMEOUT, async move {
            for cursor in cursors {
                cursor.close().await;
            }
            for mut cursor in active.into_iter().map(|cursor| cursor.close_result) {
                while cursor.borrow().is_none() && cursor.changed().await.is_ok() {}
            }
        })
        .await;
    }

    async fn reconcile_record_liveness(&self, session_key: &str, record: &mut SftpServiceRecord) {
        let failed_generation = record
            .heartbeat_tasks
            .as_ref()
            .filter(|owner| owner.has_failed())
            .map(|owner| owner.generation);
        let Some(generation) = failed_generation else {
            return;
        };
        record.heartbeat_tasks.take();
        let parent_transport_closed = record
            .shared_parent_channels
            .as_ref()
            .is_some_and(SharedSessionChannels::is_closed);
        if record.shared_parent_channels.is_some() {
            if close_shared_child_record_after_parent_termination(
                record,
                generation,
                parent_transport_closed,
            ) {
                let service = self.clone();
                let session_key = session_key.to_owned();
                tokio::spawn(async move {
                    service
                        .close_remote_directory_cursors_for_session(&session_key)
                        .await;
                });
                return;
            }

            let closing_shared_child = record
                .live
                .as_ref()
                .is_some_and(SftpProductionSession::uses_shared_transport);
            if record.actor.summary().generation == Some(generation) {
                let _ = record.actor.disconnect(generation);
            }
            let clean_close = if let Some(live) = record.live.take() {
                matches!(
                    tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await,
                    Ok(Ok(()))
                )
            } else {
                !record.shared_cleanup_incomplete
            };
            if clean_close {
                record.shared_cleanup_incomplete = false;
                if record.actor.summary().state == SftpSessionState::Disconnecting {
                    let _ = record.actor.closed(generation);
                }
            } else {
                record.shared_cleanup_incomplete |= closing_shared_child;
                if record.actor.summary().state != SftpSessionState::Closed {
                    let _ = record.actor.transport_lost(generation);
                }
            }
        } else {
            if let Some(live) = record.live.take() {
                let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await;
            }
            if record.actor.summary().generation == Some(generation)
                && !matches!(
                    record.actor.summary().state,
                    SftpSessionState::Closed | SftpSessionState::Failed
                )
            {
                let _ = record.actor.transport_lost(generation);
            }
        }
        let service = self.clone();
        let session_key = session_key.to_owned();
        tokio::spawn(async move {
            service
                .close_remote_directory_cursors_for_session(&session_key)
                .await;
        });
    }

    async fn close_shared_child_after_parent_termination(
        &self,
        session_id: wire::SftpSessionId,
        expected_generation: wire::WireSequence,
    ) {
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(session_id.as_str()) else {
            return;
        };
        let Ok(generation) = SftpGeneration::new(expected_generation.get()) else {
            return;
        };
        let parent_transport_closed = record
            .shared_parent_channels
            .as_ref()
            .is_some_and(SharedSessionChannels::is_closed);
        if !close_shared_child_record_after_parent_termination(
            record,
            generation,
            parent_transport_closed,
        ) {
            return;
        }
        drop(records);
        self.close_remote_directory_cursors_for_session(session_id.as_str())
            .await;
    }

    async fn mutate_file(
        &self,
        request: wire::SftpFileMutationRequest,
    ) -> Result<wire::SftpFileMutationResult, SftpProductionError> {
        let operation_id = request.operation_id.to_string();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = mutation_fingerprint(&request);
        loop {
            match self
                .claim_mutation(&operation_id, &idempotency_key, &fingerprint)
                .await?
            {
                MutationLedgerClaim::Execute => break,
                MutationLedgerClaim::Replay(MutationLedgerOutcome::Success(result)) => {
                    return Ok(result);
                }
                MutationLedgerClaim::Replay(MutationLedgerOutcome::DeterministicFailure(error)) => {
                    return Err(error.into());
                }
                MutationLedgerClaim::Replay(MutationLedgerOutcome::Uncertain) => {
                    return Err(SftpRuntimeError::InvalidState.into());
                }
                MutationLedgerClaim::Wait(mut finished) => wait_for_true(&mut finished).await,
            }
        }
        let result = self.execute_mutation_once(request).await;
        let outcome = match &result {
            Ok(result) => MutationLedgerOutcome::Success(result.clone()),
            Err(SftpProductionError::Runtime(error)) => {
                MutationLedgerOutcome::DeterministicFailure(error.clone())
            }
            Err(_) => MutationLedgerOutcome::Uncertain,
        };
        self.finish_mutation(&operation_id, outcome).await;
        result
    }

    async fn execute_mutation_once(
        &self,
        request: wire::SftpFileMutationRequest,
    ) -> Result<wire::SftpFileMutationResult, SftpProductionError> {
        let session_key = request.session_id.to_string();
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let operation =
            OperationFence::parse(request.operation_id.to_string(), request.idempotency_key)?;
        let (plan, live) = {
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&session_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            self.reconcile_record_liveness(&session_key, record).await;
            let plan = match request.mutation {
                wire::SftpFileMutation::CreateDirectory { path } => {
                    record
                        .actor
                        .mkdir(operation, generation, RemotePath::parse(path.bytes)?)?
                }
                wire::SftpFileMutation::CreateEmptyFile { path } => record
                    .actor
                    .create_empty_file(operation, generation, RemotePath::parse(path.bytes)?)?,
                wire::SftpFileMutation::WriteText {
                    path,
                    precondition,
                    text,
                } => record.actor.write_text(
                    operation,
                    generation,
                    RemotePath::parse(path.bytes)?,
                    decode_remote_precondition(precondition),
                    text,
                )?,
                wire::SftpFileMutation::RenameNoReplace {
                    source,
                    target,
                    source_precondition,
                } => record.actor.rename(
                    operation,
                    generation,
                    RemotePath::parse(source.bytes)?,
                    RemotePath::parse(target.bytes)?,
                    decode_remote_precondition(source_precondition),
                )?,
                wire::SftpFileMutation::Delete {
                    path,
                    precondition,
                    irreversible_confirmed,
                } => {
                    let kind = match precondition.kind {
                        wire::SftpRemoteEntryKind::Directory => DeleteKind::EmptyDirectory,
                        wire::SftpRemoteEntryKind::File
                        | wire::SftpRemoteEntryKind::Symlink
                        | wire::SftpRemoteEntryKind::Other => DeleteKind::File,
                    };
                    record.actor.delete(
                        operation,
                        generation,
                        RemotePath::parse(path.bytes)?,
                        kind,
                        decode_remote_precondition(precondition),
                        irreversible_confirmed,
                    )?
                }
                wire::SftpFileMutation::CreateZip { sources, target } => {
                    let sources = sources
                        .into_iter()
                        .map(|source| {
                            Ok(ArchiveSourcePlan {
                                path: RemotePath::parse(source.path.bytes)?,
                                precondition: decode_remote_precondition(source.precondition),
                                archive_name: source.archive_name,
                            })
                        })
                        .collect::<SftpRuntimeResult<Vec<_>>>()?;
                    record.actor.create_zip(
                        operation,
                        generation,
                        sources,
                        RemotePath::parse(target.bytes)?,
                    )?
                }
                wire::SftpFileMutation::ExtractZip {
                    source,
                    source_precondition,
                    target_directory,
                } => record.actor.extract_zip(
                    operation,
                    generation,
                    RemotePath::parse(source.bytes)?,
                    decode_remote_precondition(source_precondition),
                    RemotePath::parse(target_directory.bytes)?,
                )?,
                wire::SftpFileMutation::DownloadUrl { url, target } => record.actor.download_url(
                    operation,
                    generation,
                    url,
                    RemotePath::parse(target.bytes)?,
                )?,
            };
            let live = record.live.take().ok_or(SftpRuntimeError::InvalidState)?;
            (plan, live)
        };
        let result = live.execute_mutation(&plan).await;
        let mut live = Some(live);
        {
            let mut records = self.records.lock().await;
            if let Some(record) = records.get_mut(&session_key) {
                let heartbeat_failed = record
                    .heartbeat_tasks
                    .as_ref()
                    .is_some_and(SftpHeartbeatTasks::has_failed);
                if heartbeat_failed || matches!(result, Err(SftpProductionError::Transport(_))) {
                    record.heartbeat_tasks.take();
                    if record.actor.summary().generation == Some(generation) {
                        let _ = record.actor.transport_lost(generation);
                    }
                } else if record.live.is_none()
                    && record.actor.summary().generation == Some(generation)
                    && record.actor.summary().state == SftpSessionState::Ready
                {
                    record.live = live.take();
                }
            }
        }
        if let Some(live) = live {
            let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await;
        }
        result?;
        Ok(wire::SftpFileMutationResult {
            session_id: request.session_id,
            generation: request.expected_generation,
        })
    }

    async fn claim_mutation(
        &self,
        operation_id: &str,
        idempotency_key: &str,
        fingerprint: &[u8],
    ) -> SftpRuntimeResult<MutationLedgerClaim> {
        let mut ledger = self.mutation_ledger.lock().await;
        ledger.claim(operation_id, idempotency_key, fingerprint)
    }

    async fn finish_mutation(&self, operation_id: &str, outcome: MutationLedgerOutcome) {
        let mut ledger = self.mutation_ledger.lock().await;
        ledger.finish(operation_id, outcome);
    }

    fn start_next_queued_transfer<'a>(
        &'a self,
        session_key: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.transfer_scheduling_paused.load(Ordering::Acquire) {
                return;
            }
            loop {
                let transfer_id = {
                    let mut records = self.records.lock().await;
                    let Some(record) = records.get_mut(session_key) else {
                        return;
                    };
                    if record.active_transfer_id.is_some()
                        || record.actor.summary().state != SftpSessionState::Ready
                    {
                        return;
                    }
                    let Some(transfer_id) = record.pending_transfers.pop_front() else {
                        return;
                    };
                    record.active_transfer_id = Some(transfer_id.clone());
                    transfer_id
                };
                let boundary = self
                    .transfer_boundaries
                    .lock()
                    .await
                    .get(transfer_id.as_str())
                    .cloned();
                let start = {
                    let mut records = self.records.lock().await;
                    let Some(record) = records.get_mut(session_key) else {
                        return;
                    };
                    if record.active_transfer_id.as_ref() != Some(&transfer_id) {
                        continue;
                    }
                    let generation = match record.actor.summary().generation {
                        Some(generation) => generation,
                        None => {
                            record.active_transfer_id = None;
                            continue;
                        }
                    };
                    let Some(boundary) = boundary else {
                        let transfer_generation = record
                            .actor
                            .transfers()
                            .find(|transfer| transfer.transfer_id == transfer_id)
                            .map(|transfer| transfer.generation);
                        if let Some(transfer_generation) = transfer_generation {
                            let _ = record.actor.fail_transfer(
                                &transfer_id,
                                transfer_generation,
                                TransferFailureCode::Protocol,
                            );
                        }
                        record.active_transfer_id = None;
                        continue;
                    };
                    if record.live.is_none() {
                        let transfer_generation = record
                            .actor
                            .transfers()
                            .find(|transfer| transfer.transfer_id == transfer_id)
                            .map(|transfer| transfer.generation);
                        if let Some(transfer_generation) = transfer_generation {
                            let _ = record.actor.fail_transfer(
                                &transfer_id,
                                transfer_generation,
                                TransferFailureCode::Protocol,
                            );
                        }
                        record.active_transfer_id = None;
                        continue;
                    }
                    let plan = record
                        .actor
                        .rebind_queued_transfer(&transfer_id, generation)
                        .and_then(|()| record.actor.begin_transfer(&transfer_id, generation));
                    let plan = match plan {
                        Ok(plan) => plan,
                        Err(_) => {
                            let _ = record.actor.fail_transfer(
                                &transfer_id,
                                generation,
                                TransferFailureCode::Protocol,
                            );
                            record.active_transfer_id = None;
                            continue;
                        }
                    };
                    let liveness = record
                        .heartbeat_tasks
                        .as_ref()
                        .map(SftpHeartbeatTasks::subscribe)
                        .unwrap_or(SftpHeartbeatLiveness::Disabled);
                    let Some(live) = record.live.take() else {
                        let _ = record.actor.fail_transfer(
                            &transfer_id,
                            generation,
                            TransferFailureCode::Protocol,
                        );
                        record.active_transfer_id = None;
                        continue;
                    };
                    Some((plan, boundary, live, liveness))
                };
                let Some((plan, boundary, live, liveness)) = start else {
                    continue;
                };
                let transfer_key = transfer_id.as_str().to_owned();
                let (cancel_requested, _) = watch::channel(false);
                let (finished, _) = watch::channel(false);
                let control = ActiveTransferControl {
                    cancel_requested,
                    finished,
                    task: Arc::new(std::sync::Mutex::new(None)),
                };
                self.active_transfers
                    .lock()
                    .await
                    .insert(transfer_key, control.clone());
                let service = self.clone();
                let session_key = session_key.to_owned();
                let task_slot = control.task.clone();
                let task = tokio::spawn(Box::pin(async move {
                    service
                        .run_transfer(TransferTask {
                            session_key,
                            transfer_id,
                            plan,
                            boundary,
                            live,
                            liveness,
                            control,
                            verified_resume_file: None,
                        })
                        .await;
                }));
                if let Ok(mut slot) = task_slot.lock() {
                    *slot = Some(task.abort_handle());
                }
                return;
            }
        })
    }

    async fn enqueue_transfer(
        &self,
        request: wire::SftpTransferEnqueueRequest,
    ) -> Result<wire::SftpTransferSummary, SftpProductionError> {
        validate_js_safe_transfer_size(request.expected_bytes)?;
        let session_key = request.session_id.to_string();
        let transfer_key = request.transfer_id.to_string();
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let direction = match request.direction {
            wire::SftpTransferDirection::Upload => TransferDirection::Upload,
            wire::SftpTransferDirection::Download => TransferDirection::Download,
        };
        let source = decode_transfer_endpoint(request.source)?;
        let target = decode_transfer_endpoint(request.target)?;
        let conflict_policy = match request.conflict_policy {
            wire::SftpConflictPolicy::FailIfExists => ConflictPolicy::FailIfExists,
            wire::SftpConflictPolicy::ReplaceSafely => ConflictPolicy::ReplaceSafely,
        };
        let boundary_token = match (&direction, &source, &target) {
            (TransferDirection::Upload, TransferEndpoint::LocalBoundaryToken(token), _) => {
                token.clone()
            }
            (TransferDirection::Download, _, TransferEndpoint::LocalBoundaryToken(token)) => {
                token.clone()
            }
            _ => return Err(SftpRuntimeError::InvalidInput.into()),
        };
        let boundary = self
            .local_boundaries
            .lock()
            .await
            .remove(&boundary_token)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        let expected_kind = match direction {
            TransferDirection::Upload => wire::SftpLocalBoundaryKind::UploadSource,
            TransferDirection::Download => wire::SftpLocalBoundaryKind::DownloadTarget,
        };
        if boundary.kind != expected_kind
            || (direction == TransferDirection::Upload
                && boundary.size != Some(request.expected_bytes))
        {
            self.local_boundaries
                .lock()
                .await
                .insert(boundary_token.clone(), boundary);
            return Err(SftpRuntimeError::InvalidInput.into());
        }

        self.transfer_boundaries
            .lock()
            .await
            .insert(transfer_key.clone(), boundary.clone());

        let summary = {
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&session_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            self.reconcile_record_liveness(&session_key, record).await;
            let transfer_id = TransferId::parse(transfer_key.clone())?;
            let enqueue = record.actor.enqueue_transfer(
                transfer_id.clone(),
                generation,
                direction,
                source,
                target,
                request.expected_bytes,
                conflict_policy,
            );
            match enqueue {
                Ok(_) => {
                    record.pending_transfers.push_back(transfer_id.clone());
                    record
                        .actor
                        .transfers()
                        .find(|transfer| transfer.transfer_id == transfer_id)
                        .ok_or(SftpRuntimeError::InvalidState)
                        .and_then(|transfer| {
                            map_transfer_summary(record, transfer).map_err(|error| match error {
                                SftpProductionError::Runtime(runtime) => runtime,
                                _ => SftpRuntimeError::InvalidState,
                            })
                        })
                }
                Err(error) => Err(error),
            }
        };
        let summary = match summary {
            Ok(summary) => summary,
            Err(error) => {
                self.transfer_boundaries.lock().await.remove(&transfer_key);
                self.local_boundaries
                    .lock()
                    .await
                    .insert(boundary_token, boundary);
                return Err(error.into());
            }
        };
        self.start_next_queued_transfer(&session_key).await;
        Ok(summary)
    }

    async fn prepare_transfer_intent(
        &self,
        request: wire::SftpTransferIntentPrepareRequest,
    ) -> Result<wire::SftpTransferIntentPrepared, SftpProductionError> {
        if request.idempotency_key.trim().is_empty()
            || request.source_pane_id.trim().is_empty()
            || request.target_pane_id.trim().is_empty()
            || request.source_pane_id.len() > MAX_ID_BYTES
            || request.target_pane_id.len() > MAX_ID_BYTES
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        validate_js_safe_transfer_size(request.expected_bytes)?;
        let operation_id = request.operation_id.to_string();
        let fingerprint = transfer_intent_prepare_fingerprint(&request);
        let now = std::time::Instant::now();
        let mut tokens = self.prepared_transfer_intents.lock().await;
        tokens.retain(|_, token| token.prepared.expires_at > now);
        if let Some((token, replay)) = tokens
            .iter()
            .find(|(_, token)| token.operation_id == operation_id)
        {
            if replay.idempotency_key != request.idempotency_key
                || replay.fingerprint != fingerprint
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            return Ok(wire::SftpTransferIntentPrepared {
                intent_token: token.clone(),
                expires_at_unix_ms: replay.expires_at_unix_ms,
                source_fence: replay.prepared.source_fence.clone(),
                target_fence: replay.prepared.target_fence.clone(),
            });
        }
        if tokens.len() >= MAX_PREPARED_TRANSFER_INTENTS {
            return Err(SftpRuntimeError::InvalidState.into());
        }
        // Keep the operation ledger and token creation under one mutex guard.
        // Resolution can await remote/local facts, but releasing this guard
        // here would allow two concurrent calls with the same operation to
        // both pass the replay check and mint independently consumable tokens.
        let (source, source_fence, source_display_name) = self
            .resolve_intent_source(&request.source, request.expected_bytes)
            .await?;
        let source_name = match &source {
            ResolvedIntentSource::Local { name, .. }
            | ResolvedIntentSource::Remote { name, .. } => name.clone(),
        };
        let (target, target_fence, target_display_name) = self
            .resolve_intent_target(&request.target, &source_name)
            .await?;
        if matches!(
            (&source, &target),
            (
                ResolvedIntentSource::Remote { session_id: source, .. },
                ResolvedIntentTarget::Remote { session_id: target, .. }
            ) if source == target
        ) {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let expires_at = now + TRANSFER_INTENT_TOKEN_TTL;
        let expires_at_unix_ms = std::time::SystemTime::now()
            .checked_add(TRANSFER_INTENT_TOKEN_TTL)
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|value| i64::try_from(value.as_millis()).ok())
            .ok_or(SftpRuntimeError::InvalidState)?;
        let prepared = PreparedTransferIntent {
            source_pane_id: request.source_pane_id,
            target_pane_id: request.target_pane_id,
            source_endpoint_revision: request.source_endpoint_revision.get(),
            target_endpoint_revision: request.target_endpoint_revision.get(),
            source,
            target,
            source_fence: source_fence.clone(),
            target_fence: target_fence.clone(),
            source_display_name,
            target_display_name,
            expected_bytes: request.expected_bytes,
            conflict_policy: match request.conflict_policy {
                wire::SftpConflictPolicy::FailIfExists => ConflictPolicy::FailIfExists,
                wire::SftpConflictPolicy::ReplaceSafely => ConflictPolicy::ReplaceSafely,
            },
            expires_at,
        };
        let intent_token = uuid::Uuid::now_v7().to_string();
        tokens.insert(
            intent_token.clone(),
            PreparedTransferIntentToken {
                operation_id,
                idempotency_key: request.idempotency_key,
                fingerprint,
                prepared,
                expires_at_unix_ms,
                consumed: false,
            },
        );
        Ok(wire::SftpTransferIntentPrepared {
            intent_token,
            expires_at_unix_ms,
            source_fence,
            target_fence,
        })
    }

    async fn enqueue_transfer_intent(
        &self,
        request: wire::SftpTransferIntentEnqueueRequest,
    ) -> Result<wire::SftpTransferIntentSummary, SftpProductionError> {
        if request.idempotency_key.trim().is_empty() || request.intent_token.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let transfer_key = request.transfer_id.to_string();
        let operation_id = request.operation_id.to_string();
        let fingerprint = transfer_intent_enqueue_fingerprint(&request);
        {
            let intents = self.transfer_intents.lock().await;
            if let Some(existing) = intents.get(&transfer_key) {
                if existing.operation_id != operation_id
                    || existing.idempotency_key != request.idempotency_key
                    || existing.enqueue_fingerprint != fingerprint
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                return Ok(existing.summary.clone());
            }
            if intents.len() >= MUTATION_LEDGER_CAPACITY {
                return Err(SftpRuntimeError::InvalidState.into());
            }
        }
        let prepared = {
            let mut tokens = self.prepared_transfer_intents.lock().await;
            let token = tokens
                .get_mut(&request.intent_token)
                .ok_or(SftpRuntimeError::Conflict)?;
            if token.consumed || token.prepared.expires_at <= std::time::Instant::now() {
                return Err(SftpRuntimeError::Conflict.into());
            }
            token.consumed = true;
            token.prepared.clone()
        };
        self.revalidate_prepared_intent(&prepared).await?;
        let direction = match (&prepared.source, &prepared.target) {
            (ResolvedIntentSource::Local { .. }, ResolvedIntentTarget::Remote { .. }) => {
                wire::SftpTransferIntentDirection::Upload
            }
            (ResolvedIntentSource::Remote { .. }, ResolvedIntentTarget::Local { .. }) => {
                wire::SftpTransferIntentDirection::Download
            }
            (ResolvedIntentSource::Remote { .. }, ResolvedIntentTarget::Remote { .. }) => {
                wire::SftpTransferIntentDirection::ServerToServer
            }
            _ => return Err(SftpRuntimeError::InvalidInput.into()),
        };
        let base_summary = wire::SftpTransferIntentSummary {
            transfer_id: request.transfer_id.clone(),
            direction,
            source_display_name: prepared.source_display_name.clone(),
            target_display_name: prepared.target_display_name.clone(),
            source_pane_id: prepared.source_pane_id.clone(),
            target_pane_id: prepared.target_pane_id.clone(),
            source_endpoint_revision: WireSequence::new(prepared.source_endpoint_revision),
            target_endpoint_revision: WireSequence::new(prepared.target_endpoint_revision),
            source_fence: prepared.source_fence.clone(),
            target_fence: prepared.target_fence.clone(),
            expected_bytes: prepared.expected_bytes,
            transferred_bytes: 0,
            bytes_per_second: None,
            remaining_seconds: None,
            state_revision: WireSequence::new(1),
            state: wire::SftpTransferState::Queued,
            commit_outcome: wire::SftpTransferCommitOutcome::NotCommitted,
            failure_code: None,
            cleanup_residual: None,
        };
        let legacy = match (&prepared.source, &prepared.target) {
            (
                ResolvedIntentSource::Local {
                    directory_ref,
                    revision,
                    name,
                    identity,
                },
                ResolvedIntentTarget::Remote {
                    session_id,
                    generation,
                    path,
                },
            ) => {
                let boundary = self
                    .derive_local_upload_boundary(
                        directory_ref,
                        *revision,
                        name,
                        identity,
                        prepared.expected_bytes,
                    )
                    .await?;
                Some((
                    session_id.clone(),
                    boundary,
                    wire::SftpTransferDirection::Upload,
                    wire::SftpTransferEndpoint::LocalBoundaryToken {
                        token: format!("intent-local:{}", request.transfer_id.as_str()),
                        display_name: prepared.source_display_name.clone(),
                    },
                    wire::SftpTransferEndpoint::Remote {
                        path: wire::SftpRemotePath {
                            bytes: path.as_bytes().to_vec(),
                        },
                    },
                    *generation,
                ))
            }
            (
                ResolvedIntentSource::Remote {
                    session_id,
                    generation,
                    path,
                    ..
                },
                ResolvedIntentTarget::Local {
                    directory_ref,
                    revision,
                    name,
                },
            ) => {
                let boundary = self
                    .derive_local_download_boundary(directory_ref, *revision, name)
                    .await?;
                Some((
                    session_id.clone(),
                    boundary,
                    wire::SftpTransferDirection::Download,
                    wire::SftpTransferEndpoint::Remote {
                        path: wire::SftpRemotePath {
                            bytes: path.as_bytes().to_vec(),
                        },
                    },
                    wire::SftpTransferEndpoint::LocalBoundaryToken {
                        token: format!("intent-local:{}", request.transfer_id.as_str()),
                        display_name: prepared.target_display_name.clone(),
                    },
                    *generation,
                ))
            }
            _ => None,
        };
        if let Some((session_id, boundary, direction, source, target, generation)) = legacy {
            {
                let mut public_intent_transfer_ids = self.public_intent_transfer_ids.lock().await;
                if !public_intent_transfer_ids.insert(transfer_key.clone()) {
                    return Err(SftpRuntimeError::Conflict.into());
                }
            }
            let boundary_token = match (&source, &target) {
                (wire::SftpTransferEndpoint::LocalBoundaryToken { token, .. }, _) => token.clone(),
                (_, wire::SftpTransferEndpoint::LocalBoundaryToken { token, .. }) => token.clone(),
                _ => {
                    self.public_intent_transfer_ids
                        .lock()
                        .await
                        .remove(&transfer_key);
                    return Err(SftpRuntimeError::InvalidState.into());
                }
            };
            self.local_boundaries
                .lock()
                .await
                .insert(boundary_token, boundary);
            let legacy = self
                .enqueue_transfer(wire::SftpTransferEnqueueRequest {
                    meta: request.meta,
                    operation_id: request.operation_id,
                    idempotency_key: request.idempotency_key.clone(),
                    transfer_id: request.transfer_id,
                    session_id: wire::SftpSessionId::parse(&session_id)
                        .map_err(|_| SftpRuntimeError::InvalidInput)?,
                    expected_generation: WireSequence::new(generation),
                    direction,
                    source,
                    target,
                    expected_bytes: prepared.expected_bytes,
                    conflict_policy: match prepared.conflict_policy {
                        ConflictPolicy::FailIfExists => wire::SftpConflictPolicy::FailIfExists,
                        ConflictPolicy::ReplaceSafely => wire::SftpConflictPolicy::ReplaceSafely,
                    },
                })
                .await;
            let legacy = match legacy {
                Ok(legacy) => legacy,
                Err(error) => {
                    self.public_intent_transfer_ids
                        .lock()
                        .await
                        .remove(&transfer_key);
                    return Err(error);
                }
            };
            let summary = merge_legacy_intent_summary(base_summary, &legacy);
            self.transfer_intents.lock().await.insert(
                transfer_key,
                TransferIntentRecord {
                    operation_id,
                    idempotency_key: request.idempotency_key,
                    enqueue_fingerprint: fingerprint,
                    prepared,
                    summary: summary.clone(),
                    legacy_session_id: Some(session_id),
                    started_at: None,
                    cancel_operation: None,
                },
            );
            return Ok(summary);
        }
        {
            let queue = self.pending_remote_copies.lock().await;
            if queue.len() >= MUTATION_LEDGER_CAPACITY {
                return Err(SftpRuntimeError::InvalidState.into());
            }
        }
        self.transfer_intents.lock().await.insert(
            transfer_key.clone(),
            TransferIntentRecord {
                operation_id,
                idempotency_key: request.idempotency_key,
                enqueue_fingerprint: fingerprint,
                prepared,
                summary: base_summary.clone(),
                legacy_session_id: None,
                started_at: None,
                cancel_operation: None,
            },
        );
        let mut queue = self.pending_remote_copies.lock().await;
        queue.push_back(transfer_key);
        drop(queue);
        self.start_next_remote_copy().await;
        Ok(base_summary)
    }

    async fn snapshot_transfer_intents(
        &self,
    ) -> Result<wire::SftpTransferIntentSnapshot, SftpProductionError> {
        let legacy = {
            let records = self.records.lock().await;
            records
                .values()
                .flat_map(|record| {
                    record
                        .actor
                        .transfers()
                        .filter_map(|transfer| {
                            map_transfer_summary(record, transfer)
                                .ok()
                                .map(|summary| (transfer.transfer_id.as_str().to_owned(), summary))
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<BTreeMap<_, _>>()
        };
        let mut intents = self.transfer_intents.lock().await;
        for (transfer_key, intent) in intents.iter_mut() {
            if intent.legacy_session_id.is_some()
                && let Some(summary) = legacy.get(transfer_key)
            {
                intent.summary = merge_legacy_intent_summary(intent.summary.clone(), summary);
            }
        }
        let transfers = intents
            .values()
            .map(|intent| intent.summary.clone())
            .collect::<Vec<_>>();
        let revision = transfers
            .iter()
            .map(|summary| summary.state_revision.get())
            .max()
            .unwrap_or(0);
        Ok(wire::SftpTransferIntentSnapshot {
            snapshot_revision: WireSequence::new(revision),
            transfers,
        })
    }

    /// Returns a sanitized, retained transfer projection for the native
    /// notification worker. Both ordinary transfers and transfer intents keep
    /// their terminal actor facts after a fast transfer finishes, so a poll
    /// that lands after completion still sees one opaque terminal state. The
    /// caller owns its own baseline and never receives file names or paths.
    pub(crate) async fn notification_transfer_snapshot(
        &self,
    ) -> Result<Vec<NotificationTransferSummary>, ()> {
        let regular = self.snapshot().await.map_err(|_| ())?;
        let intents = self.snapshot_transfer_intents().await.map_err(|_| ())?;
        // A local<->remote intent can still have a legacy transfer actor. Its
        // public notification fact must come from the intent exactly once.
        let intent_ids = intents
            .transfers
            .iter()
            .map(|summary| summary.transfer_id.to_string())
            .collect::<BTreeSet<_>>();
        let mut summaries = regular
            .transfers
            .into_iter()
            .filter(|summary| !intent_ids.contains(summary.transfer_id.as_str()))
            .map(|summary| NotificationTransferSummary {
                kind: NotificationTransferKind::Direct,
                transfer_id: summary.transfer_id,
                generation: Some(summary.generation),
                state_revision: summary.state_revision,
                state: notification_transfer_state(summary.state),
            })
            .chain(intents.transfers.into_iter().map(|summary| {
                let generation = notification_intent_generation(&summary);
                NotificationTransferSummary {
                    kind: NotificationTransferKind::Intent,
                    transfer_id: summary.transfer_id,
                    generation,
                    state_revision: summary.state_revision,
                    state: notification_transfer_state(summary.state),
                }
            }))
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            notification_transfer_kind_name(left.kind)
                .cmp(notification_transfer_kind_name(right.kind))
                .then_with(|| left.transfer_id.as_str().cmp(right.transfer_id.as_str()))
                .then_with(|| left.generation.cmp(&right.generation))
        });
        Ok(summaries)
    }

    async fn cancel_transfer_intent(
        &self,
        request: wire::SftpTransferIntentActionRequest,
    ) -> Result<wire::SftpTransferIntentSummary, SftpProductionError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let transfer_key = request.transfer_id.to_string();
        let operation_id = request.operation_id.to_string();
        let fingerprint = transfer_intent_action_fingerprint(&request);
        let (legacy_session_id, legacy_summary) = {
            let mut intents = self.transfer_intents.lock().await;
            let intent = intents
                .get_mut(&transfer_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if intent.summary.source_fence != request.expected_source_fence
                || intent.summary.target_fence != request.expected_target_fence
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            if let Some((replay_operation, replay_key, replay_fingerprint)) =
                intent.cancel_operation.as_ref()
                && replay_operation == &operation_id
            {
                if replay_key != &request.idempotency_key || replay_fingerprint != &fingerprint {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                return Ok(intent.summary.clone());
            }
            if intent.summary.state_revision != request.expected_state_revision
                || matches!(
                    intent.summary.state,
                    wire::SftpTransferState::Completed
                        | wire::SftpTransferState::Cancelled
                        | wire::SftpTransferState::Failed
                )
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            intent.cancel_operation =
                Some((operation_id, request.idempotency_key.clone(), fingerprint));
            (intent.legacy_session_id.clone(), intent.summary.clone())
        };
        if let Some(session_id) = legacy_session_id {
            let legacy = {
                let records = self.records.lock().await;
                let record = records
                    .get(&session_id)
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                let transfer_id = TransferId::parse(transfer_key.clone())?;
                let transfer = record
                    .actor
                    .transfers()
                    .find(|transfer| transfer.transfer_id == transfer_id)
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                map_transfer_summary(record, transfer)?
            };
            let cancelled = self
                .cancel_transfer(wire::SftpTransferActionRequest {
                    meta: request.meta,
                    operation_id: request.operation_id,
                    idempotency_key: request.idempotency_key,
                    transfer_id: request.transfer_id,
                    expected_generation: legacy.generation,
                    expected_state_revision: legacy.state_revision,
                })
                .await?;
            let mut intents = self.transfer_intents.lock().await;
            let intent = intents
                .get_mut(&transfer_key)
                .ok_or(SftpRuntimeError::InvalidState)?;
            intent.summary = merge_legacy_intent_summary(legacy_summary, &cancelled);
            return Ok(intent.summary.clone());
        }
        let _scheduler_guard = self.remote_copy_scheduler.lock().await;
        self.pending_remote_copies
            .lock()
            .await
            .retain(|queued| queued != &transfer_key);
        if let Some(control) = self
            .active_transfer_intents
            .lock()
            .await
            .get(&transfer_key)
            .cloned()
        {
            let mut intents = self.transfer_intents.lock().await;
            let intent = intents
                .get_mut(&transfer_key)
                .ok_or(SftpRuntimeError::InvalidState)?;
            if intent.summary.state != wire::SftpTransferState::Committing {
                intent.summary.state = wire::SftpTransferState::Cancelling;
                intent.summary.state_revision =
                    WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
            }
            let summary = intent.summary.clone();
            drop(intents);
            control.cancel_requested.send_replace(true);
            return Ok(summary);
        }
        let mut intents = self.transfer_intents.lock().await;
        let intent = intents
            .get_mut(&transfer_key)
            .ok_or(SftpRuntimeError::InvalidState)?;
        intent.summary.state = wire::SftpTransferState::Cancelled;
        intent.summary.state_revision =
            WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
        Ok(intent.summary.clone())
    }

    async fn resolve_intent_source(
        &self,
        source: &wire::SftpTransferIntentSource,
        expected_bytes: u64,
    ) -> Result<
        (
            ResolvedIntentSource,
            wire::SftpTransferEndpointFence,
            String,
        ),
        SftpProductionError,
    > {
        match source {
            wire::SftpTransferIntentSource::LocalDirectoryEntry {
                directory_ref,
                entry_ref,
            } => {
                let entry = self
                    .local_directory_entries
                    .lock()
                    .await
                    .get(entry_ref)
                    .cloned()
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                let capability = self
                    .require_local_directory(directory_ref, entry.directory_revision)
                    .await?;
                if entry.directory_ref != *directory_ref
                    || entry.expires_at <= std::time::Instant::now()
                    || entry.identity.kind != LocalObjectKind::File
                    || entry.identity.size != expected_bytes
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                self.verify_local_entry(&capability, &entry).await?;
                Ok((
                    ResolvedIntentSource::Local {
                        directory_ref: directory_ref.clone(),
                        revision: entry.directory_revision,
                        name: entry.name.clone(),
                        identity: entry.identity,
                    },
                    wire::SftpTransferEndpointFence::LocalCapability {
                        directory_ref: directory_ref.clone(),
                        revision: WireSequence::new(entry.directory_revision),
                    },
                    safe_local_name_display(&entry.name),
                ))
            }
            wire::SftpTransferIntentSource::RemoteFile {
                session_id,
                expected_generation,
                directory_ref,
                entry_ref,
            } => {
                let entry = self
                    .remote_entry_refs
                    .lock()
                    .await
                    .get(entry_ref)
                    .cloned()
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                validate_remote_intent_entry(
                    &entry,
                    directory_ref,
                    session_id.as_str(),
                    expected_generation.get(),
                    expected_bytes,
                    std::time::Instant::now(),
                )?;
                self.require_remote_intent_endpoint(session_id, *expected_generation)
                    .await?;
                self.verify_remote_intent_source(
                    session_id.as_str(),
                    expected_generation.get(),
                    &entry.path,
                    &entry.precondition,
                )
                .await?;
                Ok((
                    ResolvedIntentSource::Remote {
                        session_id: session_id.to_string(),
                        generation: expected_generation.get(),
                        path: entry.path,
                        name: entry.name,
                        precondition: entry.precondition,
                    },
                    wire::SftpTransferEndpointFence::RemoteSession {
                        session_id: session_id.clone(),
                        generation: *expected_generation,
                    },
                    entry.display_name,
                ))
            }
        }
    }

    async fn resolve_intent_target(
        &self,
        target: &wire::SftpTransferIntentTarget,
        source_name: &[u8],
    ) -> Result<
        (
            ResolvedIntentTarget,
            wire::SftpTransferEndpointFence,
            String,
        ),
        SftpProductionError,
    > {
        match target {
            wire::SftpTransferIntentTarget::LocalDirectory { directory_ref } => {
                validate_name_component(source_name)?;
                let capability = self
                    .local_directories
                    .lock()
                    .await
                    .get(directory_ref)
                    .cloned()
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                let capability = self
                    .require_local_directory(directory_ref, capability.revision)
                    .await?;
                Ok((
                    ResolvedIntentTarget::Local {
                        directory_ref: directory_ref.clone(),
                        revision: capability.revision,
                        name: source_name.to_vec(),
                    },
                    wire::SftpTransferEndpointFence::LocalCapability {
                        directory_ref: directory_ref.clone(),
                        revision: WireSequence::new(capability.revision),
                    },
                    safe_local_name_display(source_name),
                ))
            }
            wire::SftpTransferIntentTarget::RemoteDirectory {
                session_id,
                expected_generation,
                directory_ref,
            } => {
                validate_name_component(source_name)?;
                let directory = self
                    .remote_directory_refs
                    .lock()
                    .await
                    .get(directory_ref)
                    .cloned()
                    .ok_or(SftpRuntimeError::InvalidInput)?;
                if directory.session_id != session_id.as_str()
                    || directory.generation != expected_generation.get()
                    || directory.expires_at <= std::time::Instant::now()
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                self.require_remote_intent_endpoint(session_id, *expected_generation)
                    .await?;
                Ok((
                    ResolvedIntentTarget::Remote {
                        session_id: session_id.to_string(),
                        generation: expected_generation.get(),
                        path: remote_child_path(&directory.path, source_name)?,
                    },
                    wire::SftpTransferEndpointFence::RemoteSession {
                        session_id: session_id.clone(),
                        generation: *expected_generation,
                    },
                    safe_local_name_display(source_name),
                ))
            }
        }
    }

    async fn require_local_directory(
        &self,
        directory_ref: &str,
        revision: u64,
    ) -> Result<LocalDirectoryCapability, SftpProductionError> {
        let capability = self
            .local_directories
            .lock()
            .await
            .get(directory_ref)
            .cloned()
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if capability.revision != revision
            || capability.revoked.load(Ordering::Acquire)
            || capability.expires_at <= std::time::Instant::now()
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(capability)
    }

    async fn verify_local_entry(
        &self,
        capability: &LocalDirectoryCapability,
        entry: &LocalDirectoryEntryReference,
    ) -> Result<(), SftpProductionError> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (capability, entry);
            Err(SftpRuntimeError::InvalidState.into())
        }
        #[cfg(any(unix, windows))]
        match capability.capability.as_ref() {
            LocalDirectoryCapabilityHandle::Native(capability) => {
                let name =
                    CString::new(entry.name.clone()).map_err(|_| SftpRuntimeError::InvalidInput)?;
                let observed = capability
                    .entry_identity(&name)
                    .map_err(|_| SftpRuntimeError::Conflict)?;
                if !entry.identity.matches_native(&observed) {
                    return Err(SftpRuntimeError::Conflict.into());
                }
                Ok(())
            }
        }
    }

    async fn verify_remote_intent_source(
        &self,
        session_id: &str,
        generation: u64,
        path: &RemotePath,
        precondition: &RemoteObjectPrecondition,
    ) -> Result<(), SftpProductionError> {
        let client = {
            let records = self.records.lock().await;
            let record = records
                .get(session_id)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if record.actor.summary().state != SftpSessionState::Ready
                || record.actor.summary().generation.map(SftpGeneration::get) != Some(generation)
            {
                return Err(SftpRuntimeError::StaleGeneration.into());
            }
            record
                .live
                .as_ref()
                .ok_or(SftpRuntimeError::InvalidState)?
                .transport
                .client()
                .clone()
        };
        let metadata = tokio::time::timeout(
            SFTP_PROTOCOL_OPERATION_TIMEOUT,
            client.symlink_metadata(remote_path_utf8(path)?),
        )
        .await
        .map_err(|_| sftp_protocol_error())?
        .map_err(|_| sftp_protocol_error())?;
        let observed = RemoteObjectPrecondition {
            kind: remote_entry_kind_from_flags(
                metadata.is_regular(),
                metadata.is_dir(),
                metadata.is_symlink(),
            ),
            size: metadata.size,
            modified_at_unix_ms: metadata.mtime.map(|value| i64::from(value) * 1_000),
        };
        if &observed != precondition || observed.kind != RemoteEntryKind::File {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(())
    }

    async fn revalidate_prepared_intent(
        &self,
        prepared: &PreparedTransferIntent,
    ) -> Result<(), SftpProductionError> {
        match &prepared.source {
            ResolvedIntentSource::Local {
                directory_ref,
                revision,
                name,
                identity,
            } => {
                let capability = self
                    .require_local_directory(directory_ref, *revision)
                    .await?;
                self.verify_local_entry(
                    &capability,
                    &LocalDirectoryEntryReference {
                        directory_ref: directory_ref.clone(),
                        directory_revision: *revision,
                        name: name.clone(),
                        identity: identity.clone(),
                        expires_at: prepared.expires_at,
                    },
                )
                .await?;
            }
            ResolvedIntentSource::Remote {
                session_id,
                generation,
                path,
                precondition,
                ..
            } => {
                self.verify_remote_intent_source(session_id, *generation, path, precondition)
                    .await?;
            }
        }
        match &prepared.target {
            ResolvedIntentTarget::Local {
                directory_ref,
                revision,
                name,
            } => {
                self.require_local_directory(directory_ref, *revision)
                    .await?;
                validate_name_component(name)?;
            }
            ResolvedIntentTarget::Remote {
                session_id,
                generation,
                ..
            } => {
                let session_id = wire::SftpSessionId::parse(session_id)
                    .map_err(|_| SftpRuntimeError::InvalidInput)?;
                self.require_remote_intent_endpoint(&session_id, WireSequence::new(*generation))
                    .await?;
            }
        }
        Ok(())
    }

    async fn require_remote_intent_endpoint(
        &self,
        session_id: &wire::SftpSessionId,
        expected_generation: WireSequence,
    ) -> Result<(), SftpProductionError> {
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(session_id.as_str())
            .ok_or(SftpRuntimeError::InvalidInput)?;
        self.reconcile_record_liveness(session_id.as_str(), record)
            .await;
        if record.actor.summary().state != SftpSessionState::Ready
            || record.actor.summary().generation.map(SftpGeneration::get)
                != Some(expected_generation.get())
            || record.live.is_none()
        {
            return Err(SftpRuntimeError::StaleGeneration.into());
        }
        Ok(())
    }

    async fn derive_local_upload_boundary(
        &self,
        directory_ref: &str,
        expected_revision: u64,
        name_bytes: &[u8],
        precondition: &LocalObjectIdentity,
        expected_bytes: u64,
    ) -> Result<LocalBoundary, SftpProductionError> {
        let capability = self
            .local_directories
            .lock()
            .await
            .get(directory_ref)
            .cloned()
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if capability.revision != expected_revision
            || capability.revoked.load(Ordering::Acquire)
            || capability.expires_at <= std::time::Instant::now()
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (capability, name_bytes, precondition, expected_bytes);
            Err(SftpRuntimeError::InvalidState.into())
        }
        #[cfg(any(unix, windows))]
        match capability.capability.as_ref() {
            LocalDirectoryCapabilityHandle::Native(capability) => capability
                .derive_upload_boundary(name_bytes, precondition, expected_bytes)
                .map_err(|_| SftpRuntimeError::Conflict.into()),
        }
    }

    async fn derive_local_download_boundary(
        &self,
        directory_ref: &str,
        expected_revision: u64,
        name_bytes: &[u8],
    ) -> Result<LocalBoundary, SftpProductionError> {
        let capability = self
            .local_directories
            .lock()
            .await
            .get(directory_ref)
            .cloned()
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if capability.revision != expected_revision
            || capability.revoked.load(Ordering::Acquire)
            || capability.expires_at <= std::time::Instant::now()
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        #[cfg(not(unix))]
        {
            let _ = (capability, name_bytes);
            Err(SftpRuntimeError::InvalidState.into())
        }
        #[cfg(unix)]
        match capability.capability.as_ref() {
            LocalDirectoryCapabilityHandle::Native(capability) => capability
                .derive_download_boundary(name_bytes)
                .map_err(|_| SftpRuntimeError::Conflict.into()),
        }
    }

    fn start_next_remote_copy<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if self.transfer_scheduling_paused.load(Ordering::Acquire) {
                return;
            }
            let scheduler_guard = self.remote_copy_scheduler.lock().await;
            let Some(transfer_key) = self.pending_remote_copies.lock().await.front().cloned()
            else {
                return;
            };
            let plan = {
                let intents = self.transfer_intents.lock().await;
                let Some(intent) = intents.get(&transfer_key) else {
                    self.pending_remote_copies.lock().await.pop_front();
                    return;
                };
                let (
                    ResolvedIntentSource::Remote {
                        session_id: source_session_key,
                        generation: source_generation,
                        path: source_path,
                        precondition: source_precondition,
                        ..
                    },
                    ResolvedIntentTarget::Remote {
                        session_id: target_session_key,
                        generation: target_generation,
                        path: target_path,
                    },
                ) = (&intent.prepared.source, &intent.prepared.target)
                else {
                    return;
                };
                let (Ok(transfer_id), Ok(source_generation), Ok(target_generation)) = (
                    TransferId::parse(transfer_key.clone()),
                    SftpGeneration::new(*source_generation),
                    SftpGeneration::new(*target_generation),
                ) else {
                    drop(intents);
                    drop(scheduler_guard);
                    self.fail_queued_remote_copy(
                        &transfer_key,
                        wire::SftpTransferFailureCode::Protocol,
                    )
                    .await;
                    return;
                };
                RemoteCopyPlan {
                    transfer_id,
                    source_session_key: source_session_key.clone(),
                    source_generation,
                    source_path: source_path.clone(),
                    source_precondition: source_precondition.clone(),
                    target_session_key: target_session_key.clone(),
                    target_generation,
                    target_path: target_path.clone(),
                    expected_bytes: intent.prepared.expected_bytes,
                    conflict_policy: intent.prepared.conflict_policy,
                }
            };
            if plan.source_session_key == plan.target_session_key {
                drop(scheduler_guard);
                self.fail_queued_remote_copy(
                    &transfer_key,
                    wire::SftpTransferFailureCode::Protocol,
                )
                .await;
                return;
            }
            let lease_attempt = {
                let mut records = self.records.lock().await;
                (|| {
                    let Some(mut source_record) = records.remove(&plan.source_session_key) else {
                        return Err(wire::SftpTransferFailureCode::TransportLost);
                    };
                    let Some(mut target_record) = records.remove(&plan.target_session_key) else {
                        records.insert(plan.source_session_key.clone(), source_record);
                        return Err(wire::SftpTransferFailureCode::TransportLost);
                    };
                    let fences_current = source_record.actor.summary().state
                        == SftpSessionState::Ready
                        && target_record.actor.summary().state == SftpSessionState::Ready
                        && source_record.actor.summary().generation == Some(plan.source_generation)
                        && target_record.actor.summary().generation == Some(plan.target_generation);
                    if !fences_current {
                        records.insert(plan.source_session_key.clone(), source_record);
                        records.insert(plan.target_session_key.clone(), target_record);
                        return Err(wire::SftpTransferFailureCode::TransportLost);
                    }
                    if source_record.active_transfer_id.is_some()
                        || target_record.active_transfer_id.is_some()
                    {
                        records.insert(plan.source_session_key.clone(), source_record);
                        records.insert(plan.target_session_key.clone(), target_record);
                        return Ok(None);
                    }
                    if source_record.live.is_none() || target_record.live.is_none() {
                        records.insert(plan.source_session_key.clone(), source_record);
                        records.insert(plan.target_session_key.clone(), target_record);
                        return Err(wire::SftpTransferFailureCode::Protocol);
                    }
                    source_record.active_transfer_id = Some(plan.transfer_id.clone());
                    target_record.active_transfer_id = Some(plan.transfer_id.clone());
                    let source_liveness = source_record
                        .heartbeat_tasks
                        .as_ref()
                        .map(SftpHeartbeatTasks::subscribe)
                        .unwrap_or(SftpHeartbeatLiveness::Disabled);
                    let target_liveness = target_record
                        .heartbeat_tasks
                        .as_ref()
                        .map(SftpHeartbeatTasks::subscribe)
                        .unwrap_or(SftpHeartbeatLiveness::Disabled);
                    let source_live = source_record.live.take().expect("checked source live");
                    let target_live = target_record.live.take().expect("checked target live");
                    records.insert(plan.source_session_key.clone(), source_record);
                    records.insert(plan.target_session_key.clone(), target_record);
                    Ok(Some((
                        source_live,
                        target_live,
                        source_liveness,
                        target_liveness,
                    )))
                })()
            };
            let leases = match lease_attempt {
                Ok(Some(leases)) => leases,
                Ok(None) => return,
                Err(failure) => {
                    drop(scheduler_guard);
                    self.fail_queued_remote_copy(&transfer_key, failure).await;
                    return;
                }
            };
            self.pending_remote_copies
                .lock()
                .await
                .retain(|queued| queued != &transfer_key);
            {
                let mut intents = self.transfer_intents.lock().await;
                if let Some(intent) = intents.get_mut(&transfer_key) {
                    intent.started_at = Some(std::time::Instant::now());
                    intent.summary.state = wire::SftpTransferState::Preparing;
                    intent.summary.state_revision =
                        WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
                }
            }
            let (cancel_requested, _) = watch::channel(false);
            let (finished, _) = watch::channel(false);
            let control = ActiveTransferControl {
                cancel_requested,
                finished,
                task: Arc::new(std::sync::Mutex::new(None)),
            };
            self.active_transfer_intents
                .lock()
                .await
                .insert(transfer_key.clone(), control.clone());
            let service = self.clone();
            let task_slot = control.task.clone();
            let task = tokio::spawn(async move {
                service
                    .run_remote_copy(RemoteCopyTask {
                        plan,
                        source_live: leases.0,
                        target_live: leases.1,
                        source_liveness: leases.2,
                        target_liveness: leases.3,
                        control,
                    })
                    .await;
            });
            if let Ok(mut slot) = task_slot.lock() {
                *slot = Some(task.abort_handle());
            }
        })
    }

    async fn fail_queued_remote_copy(
        &self,
        transfer_key: &str,
        failure: wire::SftpTransferFailureCode,
    ) {
        self.pending_remote_copies
            .lock()
            .await
            .retain(|queued| queued != transfer_key);
        if let Some(intent) = self.transfer_intents.lock().await.get_mut(transfer_key) {
            intent.summary.state = wire::SftpTransferState::Failed;
            intent.summary.failure_code = Some(failure);
            intent.summary.state_revision =
                WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
        }
        self.start_next_remote_copy().await;
    }

    async fn run_remote_copy(&self, task: RemoteCopyTask) {
        let RemoteCopyTask {
            plan,
            source_live,
            target_live,
            source_liveness,
            target_liveness,
            control,
        } = task;
        let mut cancel = control.cancel_requested.subscribe();
        let mut source_lost = match source_liveness {
            SftpHeartbeatLiveness::Disabled => None,
            SftpHeartbeatLiveness::Watching(receiver) => Some(receiver),
        };
        let mut target_lost = match target_liveness {
            SftpHeartbeatLiveness::Disabled => None,
            SftpHeartbeatLiveness::Watching(receiver) => Some(receiver),
        };
        let outcome = {
            let execution = self.execute_remote_copy(&plan, &source_live, &target_live);
            tokio::pin!(execution);
            tokio::select! {
                biased;
                _ = wait_for_true(&mut cancel) => RemoteCopyTaskOutcome::Cancelled,
                _ = async {
                    if let Some(receiver) = source_lost.as_mut() {
                        wait_for_true(receiver).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => RemoteCopyTaskOutcome::SourceLost,
                _ = async {
                    if let Some(receiver) = target_lost.as_mut() {
                        wait_for_true(receiver).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => RemoteCopyTaskOutcome::TargetLost,
                result = &mut execution => RemoteCopyTaskOutcome::Finished(result),
            }
        };
        let was_cancelled = matches!(&outcome, RemoteCopyTaskOutcome::Cancelled);
        let source_was_lost = matches!(&outcome, RemoteCopyTaskOutcome::SourceLost);
        let interrupted_resolution = match &outcome {
            RemoteCopyTaskOutcome::Cancelled
            | RemoteCopyTaskOutcome::SourceLost
            | RemoteCopyTaskOutcome::TargetLost => {
                Some(self.resolve_remote_copy_interruption(&plan).await)
            }
            RemoteCopyTaskOutcome::Finished(_) => None,
        };
        let (failure, commit_outcome, residual, source_failed, target_failed) = match outcome {
            RemoteCopyTaskOutcome::Finished(Ok(success)) => {
                let cleanup_failed = success.cleanup_residual.is_some();
                (
                    cleanup_failed.then_some(wire::SftpTransferFailureCode::CleanupIncomplete),
                    wire::SftpTransferCommitOutcome::Committed,
                    success.cleanup_residual,
                    false,
                    false,
                )
            }
            RemoteCopyTaskOutcome::Finished(Err(failure)) => (
                Some(failure.failure_code),
                failure.commit_outcome,
                failure.cleanup_residual,
                false,
                false,
            ),
            RemoteCopyTaskOutcome::Cancelled | RemoteCopyTaskOutcome::SourceLost => {
                let (commit_outcome, residual) =
                    interrupted_resolution.expect("interrupted remote copy has a resolution");
                let interruption_failure = if source_was_lost {
                    Some(wire::SftpTransferFailureCode::TransportLost)
                } else if residual.is_some() {
                    Some(wire::SftpTransferFailureCode::CleanupIncomplete)
                } else {
                    None
                };
                (interruption_failure, commit_outcome, residual, true, true)
            }
            RemoteCopyTaskOutcome::TargetLost => {
                let (commit_outcome, residual) =
                    interrupted_resolution.expect("interrupted remote copy has a resolution");
                (
                    Some(wire::SftpTransferFailureCode::TransportLost),
                    commit_outcome,
                    residual,
                    true,
                    true,
                )
            }
        };
        {
            let mut intents = self.transfer_intents.lock().await;
            if let Some(intent) = intents.get_mut(plan.transfer_id.as_str())
                && remote_copy_intent_matches_plan(intent, &plan)
            {
                intent.summary.commit_outcome = commit_outcome;
                intent.summary.cleanup_residual = residual;
                intent.summary.failure_code = failure;
                intent.summary.state = if intent.summary.failure_code.is_some() {
                    wire::SftpTransferState::Failed
                } else if was_cancelled
                    && intent.summary.commit_outcome
                        == wire::SftpTransferCommitOutcome::NotCommitted
                    && intent.summary.cleanup_residual.is_none()
                {
                    wire::SftpTransferState::Cancelled
                } else if intent.summary.commit_outcome
                    == wire::SftpTransferCommitOutcome::Committed
                    && intent.summary.cleanup_residual.is_none()
                {
                    wire::SftpTransferState::Completed
                } else {
                    wire::SftpTransferState::Failed
                };
                intent.summary.state_revision =
                    WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
            }
        }
        self.restore_remote_copy_lease(
            &plan,
            source_live,
            target_live,
            source_failed,
            target_failed,
        )
        .await;
        self.active_transfer_intents
            .lock()
            .await
            .remove(plan.transfer_id.as_str());
        control.finished.send_replace(true);
        self.start_next_queued_transfer(&plan.source_session_key)
            .await;
        self.start_next_queued_transfer(&plan.target_session_key)
            .await;
        self.start_next_remote_copy().await;
    }

    async fn execute_remote_copy(
        &self,
        plan: &RemoteCopyPlan,
        source_live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
        target_live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
    ) -> Result<RemoteCopyExecution, RemoteCopyExecutionFailure> {
        remote_copy::execute(self, plan, source_live, target_live).await
    }

    async fn resolve_remote_copy_interruption(
        &self,
        plan: &RemoteCopyPlan,
    ) -> (
        wire::SftpTransferCommitOutcome,
        Option<wire::SftpTransferIntentCleanupResidual>,
    ) {
        let state = self
            .transfer_intents
            .lock()
            .await
            .get(plan.transfer_id.as_str())
            .map(|intent| intent.summary.state);
        remote_copy_interruption_resolution(plan, state)
    }

    async fn update_remote_copy_state(
        &self,
        plan: &RemoteCopyPlan,
        state: wire::SftpTransferState,
    ) {
        if let Some(intent) = self
            .transfer_intents
            .lock()
            .await
            .get_mut(plan.transfer_id.as_str())
            && remote_copy_intent_matches_plan(intent, plan)
            && matches!(
                (intent.summary.state, state),
                (
                    wire::SftpTransferState::Preparing,
                    wire::SftpTransferState::Transferring
                ) | (
                    wire::SftpTransferState::Transferring,
                    wire::SftpTransferState::Verifying
                ) | (
                    wire::SftpTransferState::Verifying,
                    wire::SftpTransferState::Committing
                )
            )
        {
            intent.summary.state = state;
            intent.summary.state_revision =
                WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
        }
    }

    async fn record_remote_copy_progress(&self, plan: &RemoteCopyPlan, copied: u64) {
        if let Some(intent) = self
            .transfer_intents
            .lock()
            .await
            .get_mut(plan.transfer_id.as_str())
            && remote_copy_intent_matches_plan(intent, plan)
            && intent.summary.state == wire::SftpTransferState::Transferring
            && copied >= intent.summary.transferred_bytes
            && copied <= intent.summary.expected_bytes
        {
            intent.summary.transferred_bytes = copied;
            intent.summary.state_revision =
                WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
            if let Some(started_at) = intent.started_at {
                let elapsed = started_at.elapsed().as_secs_f64();
                if elapsed > 0.0 {
                    let speed = (copied as f64 / elapsed) as u64;
                    intent.summary.bytes_per_second = Some(speed);
                    intent.summary.remaining_seconds = (speed > 0).then(|| {
                        intent
                            .summary
                            .expected_bytes
                            .saturating_sub(copied)
                            .div_ceil(speed)
                    });
                }
            }
        }
    }

    async fn restore_remote_copy_lease(
        &self,
        plan: &RemoteCopyPlan,
        source_live: SftpProductionSession<TrustedSftpHostKeyVerifier>,
        target_live: SftpProductionSession<TrustedSftpHostKeyVerifier>,
        source_failed: bool,
        target_failed: bool,
    ) {
        let mut source_live = Some(source_live);
        let mut target_live = Some(target_live);
        {
            let mut records = self.records.lock().await;
            if let Some(source) = records.get_mut(&plan.source_session_key) {
                if source.active_transfer_id.as_ref() == Some(&plan.transfer_id) {
                    source.active_transfer_id = None;
                }
                if source_failed {
                    source.heartbeat_tasks.take();
                    let _ = source.actor.transport_lost(plan.source_generation);
                } else if source.actor.summary().state == SftpSessionState::Ready
                    && source.actor.summary().generation == Some(plan.source_generation)
                    && source.live.is_none()
                {
                    source.live = source_live.take();
                }
            }
            if let Some(target) = records.get_mut(&plan.target_session_key) {
                if target.active_transfer_id.as_ref() == Some(&plan.transfer_id) {
                    target.active_transfer_id = None;
                }
                if target_failed {
                    target.heartbeat_tasks.take();
                    let _ = target.actor.transport_lost(plan.target_generation);
                } else if target.actor.summary().state == SftpSessionState::Ready
                    && target.actor.summary().generation == Some(plan.target_generation)
                    && target.live.is_none()
                {
                    target.live = target_live.take();
                }
            }
        }
        if let Some(live) = source_live {
            let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await;
        }
        if let Some(live) = target_live {
            let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await;
        }
    }

    async fn cancel_transfer(
        &self,
        request: wire::SftpTransferActionRequest,
    ) -> Result<wire::SftpTransferSummary, SftpProductionError> {
        let transfer_id = TransferId::parse(request.transfer_id.to_string())?;
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let mut records = self.records.lock().await;
        let record = records
            .values_mut()
            .find(|record| {
                record
                    .actor
                    .transfers()
                    .any(|transfer| transfer.transfer_id == transfer_id)
            })
            .ok_or(SftpRuntimeError::InvalidInput)?;
        let current_revision = record
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .map(|transfer| transfer.state_revision)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if current_revision != request.expected_state_revision.get() {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let unattended_transfer = record
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .cloned()
            .ok_or(SftpRuntimeError::InvalidInput)?;
        record.actor.begin_cancel(&transfer_id, generation)?;
        let summary = record
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .ok_or(SftpRuntimeError::InvalidState)
            .and_then(|transfer| {
                map_transfer_summary(record, transfer).map_err(|_| SftpRuntimeError::InvalidState)
            })?;
        drop(records);
        let controls = self.active_transfers.lock().await;
        if let Some(control) = controls.get(transfer_id.as_str()) {
            control.cancel_requested.send_replace(true);
            return Ok(summary);
        }
        drop(controls);
        let unattended_cleanup = self.cleanup_unattended_cancel(&unattended_transfer).await;
        let mut records = self.records.lock().await;
        let record = records
            .values_mut()
            .find(|record| {
                record
                    .actor
                    .transfers()
                    .any(|transfer| transfer.transfer_id == transfer_id)
            })
            .ok_or(SftpRuntimeError::InvalidInput)?;
        record
            .pending_transfers
            .retain(|pending| pending != &transfer_id);
        match record
            .actor
            .finish_cancel(&transfer_id, generation, unattended_cleanup)
        {
            Ok(()) | Err(SftpRuntimeError::CleanupIncomplete) => {}
            Err(error) => return Err(error.into()),
        }
        let transfer = record
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .ok_or(SftpRuntimeError::InvalidState)?;
        let summary = map_transfer_summary(record, transfer)?;
        drop(records);
        if summary.state == wire::SftpTransferState::Cancelled {
            self.transfer_boundaries
                .lock()
                .await
                .remove(transfer_id.as_str());
        }
        Ok(summary)
    }

    async fn cleanup_unattended_cancel(&self, transfer: &TransferRecord) -> CleanupOutcome {
        match transfer.temporary_target.as_ref() {
            Some(TemporaryTarget::Remote(path)) => CleanupOutcome::Residual {
                opaque_location: safe_remote_path_display(path.as_bytes()),
            },
            Some(TemporaryTarget::LocalBoundaryToken(_))
                if transfer.direction == TransferDirection::Download
                    && matches!(&transfer.target, TransferEndpoint::LocalBoundaryToken(_)) =>
            {
                let boundary = self
                    .transfer_boundaries
                    .lock()
                    .await
                    .get(transfer.transfer_id.as_str())
                    .cloned();
                let Some(boundary) = boundary else {
                    return CleanupOutcome::Residual {
                        opaque_location: "selected local temporary file".to_owned(),
                    };
                };
                #[cfg(unix)]
                {
                    let Ok(capability) = boundary.native() else {
                        return CleanupOutcome::Residual {
                            opaque_location: "selected local temporary file".to_owned(),
                        };
                    };
                    let Ok(temporary_name) = capability.temporary_name(&transfer.transfer_id)
                    else {
                        return CleanupOutcome::Residual {
                            opaque_location: "selected local temporary file".to_owned(),
                        };
                    };
                    capability.remove_temporary(&temporary_name)
                }
                #[cfg(not(unix))]
                {
                    let _ = boundary.unsupported();
                    CleanupOutcome::Residual {
                        opaque_location: "selected local temporary file".to_owned(),
                    }
                }
            }
            Some(TemporaryTarget::LocalBoundaryToken(_)) => CleanupOutcome::Residual {
                opaque_location: "selected local temporary file".to_owned(),
            },
            None => CleanupOutcome::Cleaned,
        }
    }

    async fn resume_transfer(
        &self,
        request: wire::SftpTransferActionRequest,
    ) -> Result<wire::SftpTransferSummary, SftpProductionError> {
        let transfer_id = TransferId::parse(request.transfer_id.to_string())?;
        let transfer_key = transfer_id.as_str().to_owned();
        let current_generation = SftpGeneration::new(request.expected_generation.get())?;
        let boundary = self
            .transfer_boundaries
            .lock()
            .await
            .get(&transfer_key)
            .cloned()
            .ok_or(SftpRuntimeError::InvalidState)?;
        let mut verified_start = None;
        let (session_key, summary, restart_queued) = {
            let mut records = self.records.lock().await;
            let (session_key, record) = records
                .iter_mut()
                .find(|(_, record)| {
                    record
                        .actor
                        .transfers()
                        .any(|transfer| transfer.transfer_id == transfer_id)
                })
                .ok_or(SftpRuntimeError::InvalidInput)?;
            let transfer = record
                .actor
                .transfers()
                .find(|transfer| transfer.transfer_id == transfer_id)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if transfer.state_revision != request.expected_state_revision.get() {
                return Err(SftpRuntimeError::Conflict.into());
            }
            if record.actor.summary().generation != Some(current_generation)
                || record.actor.summary().state != SftpSessionState::Ready
                || record.active_transfer_id.is_some()
            {
                return Err(SftpRuntimeError::InvalidState.into());
            }
            let transfer = transfer.clone();
            let can_verify_upload = transfer.state == TransferState::PausedByDisconnect
                && transfer.direction == TransferDirection::Upload
                && transfer.transferred_bytes > 0
                && usize::try_from(transfer.transferred_bytes)
                    .ok()
                    .is_some_and(|length| length <= MAX_RESUME_PREFIX_BYTES)
                && matches!(transfer.temporary_target, Some(TemporaryTarget::Remote(_)));
            if can_verify_upload {
                let TemporaryTarget::Remote(temporary_target) =
                    transfer
                        .temporary_target
                        .clone()
                        .ok_or(SftpRuntimeError::InvalidState)?
                else {
                    return Err(SftpRuntimeError::InvalidState.into());
                };
                let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
                if !live.supports_handle_stable_resume_commit() {
                    record.actor.resume_verification_failed(
                        &transfer_id,
                        CleanupOutcome::Residual {
                            opaque_location: safe_remote_path_display(temporary_target.as_bytes()),
                        },
                    )?;
                    return Err(SftpRuntimeError::ResumeEvidenceMismatch.into());
                }
                #[cfg(not(any(unix, windows)))]
                let expected_prefix: SftpRuntimeResult<Vec<u8>> =
                    Err(SftpRuntimeError::InvalidState);
                #[cfg(any(unix, windows))]
                let expected_prefix: SftpRuntimeResult<Vec<u8>> = (|| {
                    use std::io::Read as _;

                    let length = usize::try_from(transfer.transferred_bytes)
                        .map_err(|_| SftpRuntimeError::InvalidInput)?;
                    let source = boundary
                        .native()
                        .map_err(|_| SftpRuntimeError::InvalidState)?
                        .open_upload_source(transfer.expected_bytes)
                        .map_err(|_| SftpRuntimeError::InvalidState)?;
                    let mut prefix = Vec::with_capacity(length);
                    source
                        .take(transfer.transferred_bytes)
                        .read_to_end(&mut prefix)
                        .map_err(|_| SftpRuntimeError::InvalidState)?;
                    if prefix.len() != length {
                        return Err(SftpRuntimeError::ResumeEvidenceMismatch);
                    }
                    Ok(prefix)
                })();
                let expected_prefix = expected_prefix?;
                let verified = live
                    .verify_resume_prefix(
                        current_generation,
                        temporary_target.clone(),
                        &expected_prefix,
                    )
                    .await;
                let verified = match verified {
                    Ok(verified)
                        if verified.evidence.prefix_checksum_verified
                            && verified.evidence.verified_length == transfer.transferred_bytes =>
                    {
                        verified
                    }
                    Ok(verified) => {
                        let _ = verified.file.close().await;
                        record.actor.resume_verification_failed(
                            &transfer_id,
                            CleanupOutcome::Residual {
                                opaque_location: safe_remote_path_display(
                                    temporary_target.as_bytes(),
                                ),
                            },
                        )?;
                        return Err(SftpRuntimeError::ResumeEvidenceMismatch.into());
                    }
                    Err(_) => {
                        record.actor.resume_verification_failed(
                            &transfer_id,
                            CleanupOutcome::Residual {
                                opaque_location: safe_remote_path_display(
                                    temporary_target.as_bytes(),
                                ),
                            },
                        )?;
                        return Err(SftpRuntimeError::ResumeEvidenceMismatch.into());
                    }
                };
                let VerifiedRemoteResume { evidence, file } = verified;
                record
                    .actor
                    .resume_transfer(&transfer_id, current_generation, evidence)?;
                let plan = record
                    .actor
                    .resumed_transfer_plan(&transfer_id, current_generation)?;
                let liveness = record
                    .heartbeat_tasks
                    .as_ref()
                    .map(SftpHeartbeatTasks::subscribe)
                    .unwrap_or(SftpHeartbeatLiveness::Disabled);
                let live = record.live.take().ok_or(SftpRuntimeError::InvalidState)?;
                record.active_transfer_id = Some(transfer_id.clone());
                let transfer = record
                    .actor
                    .transfers()
                    .find(|transfer| transfer.transfer_id == transfer_id)
                    .ok_or(SftpRuntimeError::InvalidState)?;
                let summary = map_transfer_summary(record, transfer)?;
                verified_start = Some((plan, live, liveness, file));
                (session_key.clone(), summary, false)
            } else {
                let cleanup_plan = TransferPlan {
                    transfer_id: transfer.transfer_id.clone(),
                    generation: current_generation,
                    direction: transfer.direction,
                    source: transfer.source.clone(),
                    target: transfer.target.clone(),
                    conflict_policy: transfer.conflict_policy,
                    expected_bytes: transfer.expected_bytes,
                    require_temporary_target: true,
                    resume_from: 0,
                };
                let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
                let cleanup = self
                    .bounded_transfer_cleanup(&cleanup_plan, &boundary, live)
                    .await;
                record
                    .actor
                    .restart_transfer(&transfer_id, current_generation, cleanup)?;
                record.pending_transfers.push_back(transfer_id.clone());
                let transfer = record
                    .actor
                    .transfers()
                    .find(|transfer| transfer.transfer_id == transfer_id)
                    .ok_or(SftpRuntimeError::InvalidState)?;
                let summary = map_transfer_summary(record, transfer)?;
                (session_key.clone(), summary, true)
            }
        };
        if let Some((plan, live, liveness, verified_resume_file)) = verified_start {
            let (cancel_requested, _) = watch::channel(false);
            let (finished, _) = watch::channel(false);
            let control = ActiveTransferControl {
                cancel_requested,
                finished,
                task: Arc::new(std::sync::Mutex::new(None)),
            };
            self.active_transfers
                .lock()
                .await
                .insert(transfer_key, control.clone());
            let service = self.clone();
            let task_session_key = session_key.clone();
            let task_slot = control.task.clone();
            let task = tokio::spawn(Box::pin(async move {
                service
                    .run_transfer(TransferTask {
                        session_key: task_session_key,
                        transfer_id,
                        plan,
                        boundary,
                        live,
                        liveness,
                        control,
                        verified_resume_file: Some(verified_resume_file),
                    })
                    .await;
            }));
            if let Ok(mut slot) = task_slot.lock() {
                *slot = Some(task.abort_handle());
            }
        } else if restart_queued {
            self.start_next_queued_transfer(&session_key).await;
        }
        Ok(summary)
    }

    async fn retry_remote_cleanup(
        &self,
        request: wire::SftpRemoteCleanupRetryRequest,
    ) -> Result<wire::SftpTransferSummary, SftpProductionError> {
        if request.idempotency_key.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let operation_id = request.operation_id.to_string();
        let fingerprint = cleanup_retry_fingerprint(&request);
        if let Some(result) = self.cleanup_operation_ledger.lock().await.replay(
            &operation_id,
            &request.idempotency_key,
            &fingerprint,
            CleanupOperationKind::RetryRemote,
        )? {
            return Ok(result);
        }
        let transfer_id = TransferId::parse(request.transfer_id.to_string())?;
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let (temporary_target, host_id) = {
            let records = self.records.lock().await;
            let record = records
                .values()
                .find(|record| {
                    record
                        .actor
                        .transfers()
                        .any(|transfer| transfer.transfer_id == transfer_id)
                })
                .ok_or(SftpRuntimeError::InvalidInput)?;
            let transfer = record
                .actor
                .transfers()
                .find(|transfer| transfer.transfer_id == transfer_id)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            if transfer.generation != generation
                || transfer.state_revision != request.expected_state_revision.get()
                || transfer.state != TransferState::Failed
                || transfer.failure_code != Some(TransferFailureCode::CleanupIncomplete)
                || transfer.cleanup_residual.is_none()
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            let TemporaryTarget::Remote(temporary_target) = transfer
                .temporary_target
                .as_ref()
                .ok_or(SftpRuntimeError::InvalidState)?
            else {
                return Err(SftpRuntimeError::InvalidState.into());
            };
            (
                temporary_target.clone(),
                HostId::parse(
                    record
                        .actor
                        .summary()
                        .host_id
                        .as_deref()
                        .ok_or(SftpRuntimeError::InvalidState)?,
                )
                .map_err(|_| SftpRuntimeError::InvalidState)?,
            )
        };
        let factory = SftpTransportFactory::new(
            &self.hosts,
            &self.vault,
            &self.transient_credentials,
            &self.ssh_agent,
        );
        let profile = factory.resolve_saved_host(&host_id, request.expected_host_state_version)?;
        let verifier = Arc::new(TrustedSftpHostKeyVerifier {
            hosts: self.hosts.clone(),
        });
        let mut interaction = NonInteractiveSftpConnectionInteraction;
        let connection = factory.connect(profile, verifier, &mut interaction).await?;
        let mut cleanup_actor = SftpSessionActor::new(
            SftpSessionId::parse(uuid::Uuid::now_v7().to_string())?,
            host_id.to_string(),
        )?;
        let cleanup_generation = cleanup_actor.start("explicit-remote-cleanup")?;
        let cleanup_session = connection
            .open_subsystem(&mut cleanup_actor, cleanup_generation)
            .await?;
        let cleanup = cleanup_session
            .cleanup_temporary_target(cleanup_generation, &temporary_target)
            .await;
        let _ = cleanup_session.disconnect().await;
        if !matches!(cleanup?, CleanupOutcome::Cleaned) {
            return Err(SftpRuntimeError::CleanupIncomplete.into());
        }
        let mut records = self.records.lock().await;
        let record = records
            .values_mut()
            .find(|record| {
                record
                    .actor
                    .transfers()
                    .any(|transfer| transfer.transfer_id == transfer_id)
            })
            .ok_or(SftpRuntimeError::Conflict)?;
        let current = record
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .ok_or(SftpRuntimeError::Conflict)?;
        if current.generation != generation
            || current.state_revision != request.expected_state_revision.get()
            || current.cleanup_residual.is_none()
            || !matches!(
                current.temporary_target,
                Some(TemporaryTarget::Remote(ref path)) if path == &temporary_target
            )
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        record
            .actor
            .resolve_cleanup_residual(&transfer_id, generation)?;
        let transfer = record
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .ok_or(SftpRuntimeError::InvalidState)?;
        let result = map_transfer_summary(record, transfer)?;
        drop(records);
        self.transfer_boundaries
            .lock()
            .await
            .remove(transfer_id.as_str());
        self.cleanup_exit_authorizations
            .lock()
            .await
            .remove(transfer_id.as_str());
        self.cleanup_operation_ledger.lock().await.record(
            operation_id,
            request.idempotency_key,
            fingerprint,
            CleanupOperationKind::RetryRemote,
            result.clone(),
        );
        Ok(result)
    }

    async fn retain_remote_cleanup_for_exit(
        &self,
        request: wire::SftpRemoteCleanupRetainRequest,
    ) -> Result<wire::SftpTransferSummary, SftpProductionError> {
        if request.idempotency_key.trim().is_empty()
            || !request.retain_remote_temporary_file_confirmed
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let operation_id = request.operation_id.to_string();
        let fingerprint = cleanup_retain_fingerprint(&request);
        if let Some(result) = self.cleanup_operation_ledger.lock().await.replay(
            &operation_id,
            &request.idempotency_key,
            &fingerprint,
            CleanupOperationKind::RetainForExit,
        )? {
            return Ok(result);
        }
        let transfer_id = TransferId::parse(request.transfer_id.to_string())?;
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        let records = self.records.lock().await;
        let record = records
            .values()
            .find(|record| {
                record
                    .actor
                    .transfers()
                    .any(|transfer| transfer.transfer_id == transfer_id)
            })
            .ok_or(SftpRuntimeError::InvalidInput)?;
        let transfer = record
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if transfer.generation != generation
            || transfer.state_revision != request.expected_state_revision.get()
            || transfer.state != TransferState::Failed
            || transfer.failure_code != Some(TransferFailureCode::CleanupIncomplete)
            || transfer.cleanup_residual.is_none()
            || !matches!(transfer.temporary_target, Some(TemporaryTarget::Remote(_)))
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let result = map_transfer_summary(record, transfer)?;
        self.cleanup_exit_authorizations.lock().await.insert(
            transfer_id.as_str().to_owned(),
            CleanupExitAuthorization {
                generation,
                state_revision: transfer.state_revision,
            },
        );
        drop(records);
        self.cleanup_operation_ledger.lock().await.record(
            operation_id,
            request.idempotency_key,
            fingerprint,
            CleanupOperationKind::RetainForExit,
            result.clone(),
        );
        Ok(result)
    }

    async fn run_transfer(&self, task: TransferTask) {
        let TransferTask {
            session_key,
            transfer_id,
            plan,
            boundary,
            live,
            liveness,
            control,
            verified_resume_file,
        } = task;
        let transfer_key = transfer_id.as_str().to_owned();
        let outcome = {
            let mut cancel = control.cancel_requested.subscribe();
            let execution = async {
                match plan.direction {
                    TransferDirection::Upload => {
                        self.run_upload(
                            &session_key,
                            &transfer_id,
                            &plan,
                            &boundary,
                            &live,
                            verified_resume_file,
                        )
                        .await
                    }
                    TransferDirection::Download => {
                        self.run_download(
                            &session_key,
                            &transfer_id,
                            &plan,
                            &boundary,
                            &live,
                            &control,
                        )
                        .await
                    }
                }
            };
            tokio::pin!(execution);
            match liveness {
                SftpHeartbeatLiveness::Disabled => {
                    tokio::select! {
                        biased;
                        _ = wait_for_true(&mut cancel) => TransferTaskOutcome::Cancelled,
                        result = &mut execution => TransferTaskOutcome::Finished(result),
                    }
                }
                SftpHeartbeatLiveness::Watching(mut lost) => {
                    tokio::select! {
                        biased;
                        _ = wait_for_true(&mut cancel) => TransferTaskOutcome::Cancelled,
                        _ = wait_for_true(&mut lost) => TransferTaskOutcome::TransportLost,
                        result = &mut execution => TransferTaskOutcome::Finished(result),
                    }
                }
            }
        };
        let mut close_transport = false;
        match outcome {
            TransferTaskOutcome::Cancelled => {
                close_transport = true;
                let cleanup = self.bounded_transfer_cleanup(&plan, &boundary, &live).await;
                let mut records = self.records.lock().await;
                if let Some(record) = records.get_mut(&session_key) {
                    let _ = record
                        .actor
                        .finish_cancel(&transfer_id, plan.generation, cleanup);
                }
            }
            TransferTaskOutcome::TransportLost => {
                close_transport = true;
                let mut records = self.records.lock().await;
                if let Some(record) = records.get_mut(&session_key) {
                    let _ = record.actor.fail_transfer(
                        &transfer_id,
                        plan.generation,
                        TransferFailureCode::TransportLost,
                    );
                }
            }
            TransferTaskOutcome::Finished(Ok(())) => {}
            TransferTaskOutcome::Finished(Err(failure)) => {
                close_transport = matches!(
                    failure,
                    TransferFailureCode::TransportLost
                        | TransferFailureCode::Protocol
                        | TransferFailureCode::CleanupIncomplete
                );
                let cleanup = if failure == TransferFailureCode::TransportLost {
                    None
                } else {
                    Some(self.bounded_transfer_cleanup(&plan, &boundary, &live).await)
                };
                let mut records = self.records.lock().await;
                if let Some(record) = records.get_mut(&session_key) {
                    let current_state = record
                        .actor
                        .transfers()
                        .find(|transfer| transfer.transfer_id == transfer_id)
                        .map(|transfer| transfer.state);
                    if !matches!(
                        current_state,
                        Some(TransferState::Cancelled | TransferState::Failed)
                    ) {
                        if let Some(cleanup) = cleanup {
                            let _ = record.actor.fail_transfer_after_cleanup(
                                &transfer_id,
                                plan.generation,
                                failure,
                                cleanup,
                            );
                        } else {
                            let _ =
                                record
                                    .actor
                                    .fail_transfer(&transfer_id, plan.generation, failure);
                        }
                    }
                }
            }
        }
        if close_transport {
            let _ = tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await;
            let mut records = self.records.lock().await;
            if let Some(record) = records.get_mut(&session_key) {
                record.heartbeat_tasks.take();
                if record.actor.summary().generation == Some(plan.generation)
                    && !matches!(
                        record.actor.summary().state,
                        SftpSessionState::Closed | SftpSessionState::Failed
                    )
                {
                    let _ = record.actor.transport_lost(plan.generation);
                }
            }
        } else {
            let mut records = self.records.lock().await;
            if let Some(record) = records.get_mut(&session_key)
                && record.live.is_none()
                && !matches!(
                    record.actor.summary().state,
                    SftpSessionState::Closed | SftpSessionState::Disconnecting
                )
            {
                record.live = Some(live);
            }
        }
        {
            let mut records = self.records.lock().await;
            if let Some(record) = records.get_mut(&session_key)
                && record.active_transfer_id.as_ref() == Some(&transfer_id)
            {
                record.active_transfer_id = None;
            }
        }
        self.active_transfers.lock().await.remove(&transfer_key);
        let final_state = {
            let records = self.records.lock().await;
            records.get(&session_key).and_then(|record| {
                record
                    .actor
                    .transfers()
                    .find(|transfer| transfer.transfer_id == transfer_id)
                    .map(|transfer| transfer.state)
            })
        };
        if matches!(
            final_state,
            Some(TransferState::Completed | TransferState::Cancelled)
        ) {
            self.transfer_boundaries.lock().await.remove(&transfer_key);
        }
        control.finished.send_replace(true);
        self.start_next_queued_transfer(&session_key).await;
        self.start_next_remote_copy().await;
    }

    async fn bounded_transfer_cleanup(
        &self,
        plan: &TransferPlan,
        boundary: &LocalBoundary,
        live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
    ) -> CleanupOutcome {
        if plan.direction == TransferDirection::Upload && plan.resume_from > 0 {
            let TransferEndpoint::Remote(target) = &plan.target else {
                return CleanupOutcome::Residual {
                    opaque_location: "remote temporary file".to_owned(),
                };
            };
            return remote_temporary_target(target, &plan.transfer_id)
                .map(|temporary_target| CleanupOutcome::Residual {
                    opaque_location: safe_remote_path_display(temporary_target.as_bytes()),
                })
                .unwrap_or_else(|_| CleanupOutcome::Residual {
                    opaque_location: "remote temporary file".to_owned(),
                });
        }
        tokio::time::timeout(
            SFTP_CLEANUP_TIMEOUT,
            self.cleanup_transfer_artifact(plan, boundary, live),
        )
        .await
        .unwrap_or_else(|_| CleanupOutcome::Residual {
            opaque_location: "temporary target cleanup timed out".to_owned(),
        })
    }

    async fn cleanup_transfer_artifact(
        &self,
        plan: &TransferPlan,
        boundary: &LocalBoundary,
        live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
    ) -> CleanupOutcome {
        match (&plan.direction, &plan.target) {
            (TransferDirection::Upload, TransferEndpoint::Remote(target)) => {
                let Ok(temporary_target) = remote_temporary_target(target, &plan.transfer_id)
                else {
                    return CleanupOutcome::Cleaned;
                };
                live.cleanup_temporary_target(plan.generation, &temporary_target)
                    .await
                    .unwrap_or_else(|_| CleanupOutcome::Residual {
                        opaque_location: std::str::from_utf8(temporary_target.as_bytes())
                            .unwrap_or("remote temporary file")
                            .to_owned(),
                    })
            }
            (TransferDirection::Download, TransferEndpoint::LocalBoundaryToken(_)) => {
                #[cfg(unix)]
                {
                    let Ok(capability) = boundary.native() else {
                        return CleanupOutcome::Residual {
                            opaque_location: "local boundary unavailable".to_owned(),
                        };
                    };
                    let Ok(temporary_name) = capability.temporary_name(&plan.transfer_id) else {
                        return CleanupOutcome::Cleaned;
                    };
                    capability.remove_temporary(&temporary_name)
                }
                #[cfg(not(unix))]
                {
                    let _ = boundary.unsupported();
                    CleanupOutcome::Residual {
                        opaque_location: "local transfer unsupported on this platform".to_owned(),
                    }
                }
            }
            _ => CleanupOutcome::Cleaned,
        }
    }

    async fn run_upload(
        &self,
        session_key: &str,
        transfer_id: &TransferId,
        plan: &TransferPlan,
        boundary: &LocalBoundary,
        live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
        verified_resume_file: Option<RemoteSftpFile>,
    ) -> Result<(), TransferFailureCode> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (
                session_key,
                transfer_id,
                plan,
                boundary,
                live,
                verified_resume_file,
            );
            return Err(TransferFailureCode::PermissionDenied);
        }
        #[cfg(any(unix, windows))]
        {
            let TransferEndpoint::Remote(target) = &plan.target else {
                return Err(TransferFailureCode::Protocol);
            };
            let temporary_target = remote_temporary_target(target, transfer_id)
                .map_err(|_| TransferFailureCode::UnsupportedPathEncoding)?;
            if plan.resume_from == 0 {
                let facts = live
                    .inspect_transfer_targets(plan.generation, target, &temporary_target)
                    .await
                    .map_err(map_transfer_execution_failure)?;
                if facts.temporary_target_existed {
                    let cleanup = live
                        .cleanup_temporary_target(plan.generation, &temporary_target)
                        .await
                        .map_err(map_transfer_execution_failure)?;
                    if !matches!(cleanup, CleanupOutcome::Cleaned) {
                        return Err(TransferFailureCode::CleanupIncomplete);
                    }
                }
                {
                    let mut records = self.records.lock().await;
                    let record = records
                        .get_mut(session_key)
                        .ok_or(TransferFailureCode::Protocol)?;
                    record
                        .actor
                        .transfer_prepared(
                            transfer_id,
                            plan.generation,
                            TemporaryTarget::Remote(temporary_target.clone()),
                            facts.target_existed,
                            facts.safe_commit_supported,
                        )
                        .map_err(map_transfer_runtime_failure)?;
                }
            }

            #[cfg(any(unix, windows))]
            let source = boundary
                .native()?
                .open_upload_source(plan.expected_bytes)
                .map_err(|_| TransferFailureCode::PermissionDenied)?;
            #[cfg(not(any(unix, windows)))]
            let source = {
                boundary.unsupported()?;
                unreachable!()
            };
            let mut source = tokio::fs::File::from_std(source);
            if plan.resume_from > 0 {
                source
                    .seek(io::SeekFrom::Start(plan.resume_from))
                    .await
                    .map_err(|_| TransferFailureCode::PermissionDenied)?;
            }
            let temporary_path = remote_path_utf8(&temporary_target)
                .map_err(|_| TransferFailureCode::UnsupportedPathEncoding)?;
            let mut target_file = if plan.resume_from == 0 {
                if verified_resume_file.is_some() {
                    return Err(TransferFailureCode::Protocol);
                }
                bounded_transfer_io(
                    live.transport.client().open_with_flags(
                        temporary_path,
                        OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                    ),
                    TransferFailureCode::Protocol,
                )
                .await?
            } else {
                verified_resume_file.ok_or(TransferFailureCode::Protocol)?
            };
            let mut copied = plan.resume_from;
            let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
            loop {
                let read = source
                    .read(&mut buffer)
                    .await
                    .map_err(|_| TransferFailureCode::PermissionDenied)?;
                if read == 0 {
                    break;
                }
                bounded_transfer_io(
                    target_file.write_all(&buffer[..read]),
                    TransferFailureCode::Protocol,
                )
                .await?;
                copied = copied.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                self.record_transfer_progress(session_key, transfer_id, plan.generation, copied)
                    .await?;
            }
            bounded_transfer_io(target_file.sync_all(), TransferFailureCode::Protocol).await?;
            let observed_length =
                bounded_transfer_io(target_file.metadata(), TransferFailureCode::Protocol)
                    .await?
                    .len();
            bounded_transfer_io(target_file.close(), TransferFailureCode::Protocol).await?;
            if plan.resume_from > 0 {
                // The current SFTP v3 adapter cannot bind the path-based final
                // hardlink to this stable handle. Never commit a potentially
                // substituted path as the verified temporary object.
                return Err(TransferFailureCode::UnsafeReplaceUnsupported);
            }
            self.begin_transfer_commit(session_key, transfer_id, plan.generation, observed_length)
                .await?;
            let final_path = remote_path_utf8(target)
                .map_err(|_| TransferFailureCode::UnsupportedPathEncoding)?;
            let hardlinked = bounded_transfer_io(
                live.transport.client().hardlink(temporary_path, final_path),
                TransferFailureCode::TargetExists,
            )
            .await?;
            if !hardlinked {
                let _ = live
                    .cleanup_temporary_target(plan.generation, &temporary_target)
                    .await;
                return Err(TransferFailureCode::UnsafeReplaceUnsupported);
            }
            let final_length = bounded_transfer_io(
                live.transport.client().symlink_metadata(final_path),
                TransferFailureCode::Protocol,
            )
            .await?
            .len();
            if bounded_transfer_io(
                live.transport.client().remove_file(temporary_path),
                TransferFailureCode::CleanupIncomplete,
            )
            .await
            .is_err()
            {
                return Err(TransferFailureCode::CleanupIncomplete);
            }
            self.complete_transfer(
                session_key,
                transfer_id,
                plan.generation,
                CommitFacts {
                    final_length,
                    atomic_no_replace: true,
                    atomic_replace: false,
                },
            )
            .await
        }
    }

    async fn run_download(
        &self,
        session_key: &str,
        transfer_id: &TransferId,
        plan: &TransferPlan,
        boundary: &LocalBoundary,
        live: &SftpProductionSession<TrustedSftpHostKeyVerifier>,
        _control: &ActiveTransferControl,
    ) -> Result<(), TransferFailureCode> {
        #[cfg(not(unix))]
        {
            let _ = (session_key, transfer_id, plan, boundary, live, _control);
            return Err(TransferFailureCode::PermissionDenied);
        }
        #[cfg(unix)]
        {
            let TransferEndpoint::Remote(source_path) = &plan.source else {
                return Err(TransferFailureCode::Protocol);
            };
            let source_path = remote_path_utf8(source_path)
                .map_err(|_| TransferFailureCode::UnsupportedPathEncoding)?;
            let source_metadata = bounded_transfer_io(
                live.transport.client().symlink_metadata(source_path),
                TransferFailureCode::Protocol,
            )
            .await?;
            if !source_metadata.is_regular() || source_metadata.len() != plan.expected_bytes {
                return Err(TransferFailureCode::LengthMismatch);
            }
            #[cfg(unix)]
            let capability = boundary.native()?;
            #[cfg(not(unix))]
            {
                boundary.unsupported()?;
            }
            #[cfg(unix)]
            let target_identity = capability
                .target_identity()
                .map_err(|_| TransferFailureCode::PermissionDenied)?;
            #[cfg(unix)]
            let target_existed = target_identity.is_some();
            #[cfg(unix)]
            let temporary_name = capability
                .temporary_name(transfer_id)
                .map_err(|_| TransferFailureCode::UnsupportedPathEncoding)?;
            #[cfg(unix)]
            if openat_identity(&capability.parent, &temporary_name)
                .map_err(|_| TransferFailureCode::CleanupIncomplete)?
                .is_some()
                && !matches!(
                    capability.remove_temporary(&temporary_name),
                    CleanupOutcome::Cleaned
                )
            {
                return Err(TransferFailureCode::CleanupIncomplete);
            }
            {
                let mut records = self.records.lock().await;
                let record = records
                    .get_mut(session_key)
                    .ok_or(TransferFailureCode::Protocol)?;
                record
                    .actor
                    .transfer_prepared(
                        transfer_id,
                        plan.generation,
                        TemporaryTarget::LocalBoundaryToken(format!(
                            "local-temp:{}",
                            transfer_id.as_str()
                        )),
                        target_existed,
                        true,
                    )
                    .map_err(map_transfer_runtime_failure)?;
            }
            let mut source = bounded_transfer_io(
                live.transport.client().open(source_path),
                TransferFailureCode::Protocol,
            )
            .await?;
            #[cfg(unix)]
            let target = capability
                .create_temporary(&temporary_name)
                .map_err(|_| TransferFailureCode::PermissionDenied)?;
            #[cfg(unix)]
            let mut target = tokio::fs::File::from_std(target);
            let mut copied = 0_u64;
            let mut buffer = vec![0_u8; TRANSFER_CHUNK_BYTES];
            loop {
                let read =
                    bounded_transfer_io(source.read(&mut buffer), TransferFailureCode::Protocol)
                        .await?;
                if read == 0 {
                    break;
                }
                target
                    .write_all(&buffer[..read])
                    .await
                    .map_err(|_| TransferFailureCode::PermissionDenied)?;
                copied = copied.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                self.record_transfer_progress(session_key, transfer_id, plan.generation, copied)
                    .await?;
            }
            let _ = bounded_transfer_io(source.close(), TransferFailureCode::Protocol).await;
            target
                .sync_all()
                .await
                .map_err(|_| TransferFailureCode::PermissionDenied)?;
            #[cfg(unix)]
            let target = target.into_std().await;
            #[cfg(unix)]
            let temporary_identity = NativeFileIdentity::from_metadata(
                &target
                    .metadata()
                    .map_err(|_| TransferFailureCode::PermissionDenied)?,
            );
            #[cfg(unix)]
            let observed_length = temporary_identity.size;
            self.begin_transfer_commit(session_key, transfer_id, plan.generation, observed_length)
                .await?;
            #[cfg(unix)]
            let (atomic_no_replace, atomic_replace, final_length) = capability.commit_download(
                &temporary_name,
                &temporary_identity,
                target_identity.as_ref(),
            )?;
            self.complete_transfer(
                session_key,
                transfer_id,
                plan.generation,
                CommitFacts {
                    final_length,
                    atomic_no_replace,
                    atomic_replace,
                },
            )
            .await
        }
    }

    async fn record_transfer_progress(
        &self,
        session_key: &str,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        copied: u64,
    ) -> Result<(), TransferFailureCode> {
        let mut records = self.records.lock().await;
        records
            .get_mut(session_key)
            .ok_or(TransferFailureCode::Protocol)?
            .actor
            .record_progress(transfer_id, generation, copied)
            .map_err(map_transfer_runtime_failure)
    }

    async fn begin_transfer_commit(
        &self,
        session_key: &str,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        observed_length: u64,
    ) -> Result<(), TransferFailureCode> {
        let mut records = self.records.lock().await;
        let actor = &mut records
            .get_mut(session_key)
            .ok_or(TransferFailureCode::Protocol)?
            .actor;
        actor
            .begin_verification(transfer_id, generation, observed_length)
            .map_err(map_transfer_runtime_failure)?;
        actor
            .begin_commit(transfer_id, generation)
            .map_err(map_transfer_runtime_failure)
    }

    async fn complete_transfer(
        &self,
        session_key: &str,
        transfer_id: &TransferId,
        generation: SftpGeneration,
        facts: CommitFacts,
    ) -> Result<(), TransferFailureCode> {
        let mut records = self.records.lock().await;
        records
            .get_mut(session_key)
            .ok_or(TransferFailureCode::Protocol)?
            .actor
            .complete_transfer(transfer_id, generation, facts)
            .map_err(map_transfer_runtime_failure)
    }

    pub(crate) async fn disconnect(
        &self,
        request: wire::SftpSessionDisconnectRequest,
    ) -> Result<wire::SftpSessionSummary, SftpProductionError> {
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(request.session_id.as_str())
            .ok_or(SftpRuntimeError::InvalidInput)?;
        let generation = SftpGeneration::new(request.expected_generation.get())?;
        if record.actor.transfers().any(|transfer| {
            !matches!(
                transfer.state,
                TransferState::Completed | TransferState::Cancelled | TransferState::Failed
            )
        }) || record.active_transfer_id.is_some()
        {
            return Err(SftpRuntimeError::InvalidState.into());
        }
        if record.shared_cleanup_incomplete {
            if record.actor.summary().generation != Some(generation) {
                return Err(SftpRuntimeError::StaleGeneration.into());
            }
            return map_sftp_summary(record);
        }
        record.actor.disconnect(generation)?;
        self.close_remote_directory_cursors_for_session(request.session_id.as_str())
            .await;
        record.heartbeat_tasks.take();
        let mut clean_close = true;
        let closing_shared_child = record
            .live
            .as_ref()
            .is_some_and(SftpProductionSession::uses_shared_transport);
        if let Some(live) = record.live.take() {
            clean_close = matches!(
                tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await,
                Ok(Ok(()))
            );
        }
        if clean_close {
            record.actor.closed(generation)?;
        } else {
            record.shared_cleanup_incomplete = closing_shared_child;
            record.actor.transport_lost(generation)?;
        }
        map_sftp_summary(record)
    }

    async fn register_local_boundary(
        &self,
        kind: wire::SftpLocalBoundaryKind,
        selected_path: String,
    ) -> Result<wire::SftpLocalBoundary, SftpRuntimeError> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (kind, selected_path);
            // Unsupported platforms must not fall back to ambient path access.
            return Err(SftpRuntimeError::InvalidState);
        }
        #[cfg(any(unix, windows))]
        {
            let selected = PathBuf::from(selected_path);
            let native = NativeLocalBoundary::register(&selected, kind)
                .map_err(|_| SftpRuntimeError::InvalidInput)?;
            let size = native
                .upload_identity
                .as_ref()
                .map(|identity| identity.size);
            let boundary = LocalBoundary {
                kind,
                display_name: local_display_name(&selected),
                size,
                capability: Arc::new(LocalBoundaryCapability::Native(native)),
            };
            let token = uuid::Uuid::new_v4().to_string();
            let response = wire::SftpLocalBoundary {
                token: token.clone(),
                kind,
                display_name: boundary.display_name.clone(),
                size: boundary.size,
            };
            self.local_boundaries.lock().await.insert(token, boundary);
            Ok(response)
        }
    }

    pub(crate) fn exit_blockers(&self) -> (Vec<wire::SftpSessionId>, Vec<wire::TransferId>) {
        let Ok(records) = self.records.try_lock() else {
            // A resource operation is currently mutating the serial actor.
            // Treat that in-flight state as an exit blocker rather than racing
            // cleanup or reporting a false empty snapshot.
            return (vec![wire::SftpSessionId::new()], Vec::new());
        };
        let sessions = records
            .values()
            .filter(|record| {
                record.shared_cleanup_incomplete
                    || !matches!(
                        record.actor.summary().state,
                        SftpSessionState::Closed | SftpSessionState::Failed
                    )
            })
            .filter_map(|record| {
                wire::SftpSessionId::parse(record.actor.summary().session_id.as_str()).ok()
            })
            .collect();
        let authorizations = self.cleanup_exit_authorizations.try_lock().ok();
        let mut transfers = records
            .values()
            .flat_map(|record| record.actor.transfers())
            .filter(|transfer| {
                let retained_for_this_exit = authorizations.as_ref().is_some_and(|values| {
                    values
                        .get(transfer.transfer_id.as_str())
                        .is_some_and(|authorization| {
                            authorization.generation == transfer.generation
                                && authorization.state_revision == transfer.state_revision
                        })
                });
                (transfer.cleanup_residual.is_some() && !retained_for_this_exit)
                    || !matches!(
                        transfer.state,
                        TransferState::Completed | TransferState::Cancelled | TransferState::Failed
                    )
            })
            .filter_map(|transfer| wire::TransferId::parse(transfer.transfer_id.as_str()).ok())
            .collect::<Vec<_>>();
        let Ok(intents) = self.transfer_intents.try_lock() else {
            transfers.push(wire::TransferId::new());
            return (sessions, transfers);
        };
        let intent_authorizations = self.intent_cleanup_exit_authorizations.try_lock().ok();
        for transfer_id in intents.values().filter_map(|intent| {
            let retained_for_this_exit = intent_authorizations.as_ref().is_some_and(|values| {
                values
                    .get(intent.summary.transfer_id.as_str())
                    .is_some_and(|authorization| authorization.matches(&intent.summary))
            });
            let terminal = matches!(
                intent.summary.state,
                wire::SftpTransferState::Completed
                    | wire::SftpTransferState::Cancelled
                    | wire::SftpTransferState::Failed
            );
            let unresolved = intent.summary.cleanup_residual.is_some()
                || intent.summary.commit_outcome == wire::SftpTransferCommitOutcome::Uncertain;
            let blocked = !terminal || (unresolved && !retained_for_this_exit);
            blocked.then(|| intent.summary.transfer_id.clone())
        }) {
            if !transfers.contains(&transfer_id) {
                transfers.push(transfer_id);
            }
        }
        (sessions, transfers)
    }

    pub(crate) async fn shutdown_all(&self) -> Result<(), SftpProductionError> {
        self.shutdown_all_with_timeout(Duration::from_secs(5)).await
    }

    async fn shutdown_all_with_timeout(
        &self,
        shutdown_timeout: Duration,
    ) -> Result<(), SftpProductionError> {
        let mut scheduling_pause =
            TransferSchedulingPause::new(self.transfer_scheduling_paused.clone());
        self.close_all_remote_directory_cursors().await;
        self.retry_existing_cleanup_residuals().await;
        let mut failure = SftpShutdownFailure::default();
        let mut controls = self
            .active_transfers
            .lock()
            .await
            .iter()
            .map(|(transfer_id, control)| (transfer_id.clone(), control.clone()))
            .collect::<Vec<_>>();
        controls.extend(
            self.active_transfer_intents
                .lock()
                .await
                .iter()
                .map(|(transfer_id, control)| (transfer_id.clone(), control.clone())),
        );
        {
            let mut records = self.records.lock().await;
            for (transfer_key, _) in &controls {
                let Ok(transfer_id) = TransferId::parse(transfer_key.clone()) else {
                    continue;
                };
                for record in records.values_mut() {
                    let transfer = record
                        .actor
                        .transfers()
                        .find(|transfer| transfer.transfer_id == transfer_id);
                    if let Some(transfer) = transfer
                        && !matches!(
                            transfer.state,
                            TransferState::Completed
                                | TransferState::Cancelled
                                | TransferState::Failed
                                | TransferState::Cancelling
                        )
                    {
                        let _ = record.actor.begin_cancel(&transfer_id, transfer.generation);
                    }
                }
            }
        }
        for (_, control) in &controls {
            control.cancel_requested.send_replace(true);
        }
        let shutdown_deadline = tokio::time::Instant::now() + shutdown_timeout;
        let mut timed_out = Vec::new();
        for (transfer_key, control) in controls {
            let mut finished = control.finished.subscribe();
            if !*finished.borrow() {
                let remaining =
                    shutdown_deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero()
                    || tokio::time::timeout(remaining, wait_for_true(&mut finished))
                        .await
                        .is_err()
                {
                    let abort_handle = control
                        .task
                        .lock()
                        .ok()
                        .and_then(|slot| slot.as_ref().cloned());
                    if let Some(task) = abort_handle {
                        task.abort();
                        let _ = tokio::time::timeout(Duration::from_millis(250), async {
                            while !task.is_finished() {
                                tokio::task::yield_now().await;
                            }
                        })
                        .await;
                    }
                    failure.timed_out_transfers.push(transfer_key.clone());
                    timed_out.push(transfer_key);
                }
            }
        }
        {
            let mut active = self.active_transfers.lock().await;
            for transfer_key in &timed_out {
                active.remove(transfer_key);
            }
        }
        {
            let mut active = self.active_transfer_intents.lock().await;
            for transfer_key in &timed_out {
                active.remove(transfer_key);
            }
        }
        {
            let mut intents = self.transfer_intents.lock().await;
            for transfer_key in &timed_out {
                let Some(intent) = intents.get_mut(transfer_key) else {
                    continue;
                };
                if intent.summary.state == wire::SftpTransferState::Committing {
                    intent.summary.commit_outcome = wire::SftpTransferCommitOutcome::Uncertain;
                }
                intent.summary.state = wire::SftpTransferState::Failed;
                intent.summary.failure_code =
                    Some(wire::SftpTransferFailureCode::CleanupIncomplete);
                intent.summary.cleanup_residual = transfer_intent_target_residual(
                    &intent.prepared.target,
                    &intent.summary.transfer_id,
                );
                intent.summary.state_revision =
                    WireSequence::new(intent.summary.state_revision.get().saturating_add(1));
            }
        }
        let mut records = self.records.lock().await;
        for transfer_key in &timed_out {
            let Ok(transfer_id) = TransferId::parse(transfer_key.clone()) else {
                continue;
            };
            for record in records.values_mut() {
                let transfer = record
                    .actor
                    .transfers()
                    .find(|transfer| transfer.transfer_id == transfer_id);
                if let Some(transfer) = transfer {
                    let generation = transfer.generation;
                    if transfer.state == TransferState::Cancelling {
                        let _ = record.actor.finish_cancel(
                            &transfer_id,
                            generation,
                            CleanupOutcome::Residual {
                                opaque_location: "shutdown cleanup timed out".to_owned(),
                            },
                        );
                    }
                    record.heartbeat_tasks.take();
                    if !matches!(
                        record.actor.summary().state,
                        SftpSessionState::Closed | SftpSessionState::Failed
                    ) {
                        let _ = record.actor.transport_lost(generation);
                    }
                }
            }
        }
        for (session_key, record) in records.iter_mut() {
            let Some(generation) = record.actor.summary().generation else {
                continue;
            };
            let actor_needs_close = !matches!(
                record.actor.summary().state,
                SftpSessionState::Closed | SftpSessionState::Failed
            );
            record.heartbeat_tasks.take();
            if actor_needs_close {
                record.actor.disconnect(generation)?;
            }
            let closing_shared_child = record
                .live
                .as_ref()
                .is_some_and(SftpProductionSession::uses_shared_transport);
            let disconnect_failed = record.shared_cleanup_incomplete
                || if let Some(live) = record.live.take() {
                    !matches!(
                        tokio::time::timeout(SFTP_DISCONNECT_TIMEOUT, live.disconnect()).await,
                        Ok(Ok(()))
                    )
                } else {
                    false
                };
            if disconnect_failed {
                record.shared_cleanup_incomplete |= closing_shared_child;
                failure.disconnect_failed_sessions.push(session_key.clone());
                if record.actor.summary().state != SftpSessionState::Closed {
                    let _ = record.actor.transport_lost(generation);
                }
            } else if actor_needs_close {
                record.actor.closed(generation)?;
            }
        }
        failure.cleanup_residuals = records
            .values()
            .flat_map(|record| record.actor.transfers())
            .filter_map(|transfer| {
                let retained = self
                    .cleanup_exit_authorizations
                    .try_lock()
                    .ok()
                    .and_then(|authorizations| {
                        authorizations.get(transfer.transfer_id.as_str()).cloned()
                    })
                    .is_some_and(|authorization| {
                        authorization.generation == transfer.generation
                            && authorization.state_revision == transfer.state_revision
                    });
                if retained {
                    return None;
                }
                transfer
                    .cleanup_residual
                    .as_ref()
                    .map(|opaque_location| SftpShutdownResidual {
                        transfer_id: transfer.transfer_id.as_str().to_owned(),
                        opaque_location: opaque_location.clone(),
                    })
            })
            .collect();
        drop(records);
        failure
            .cleanup_residuals
            .extend(
                self.transfer_intents
                    .lock()
                    .await
                    .values()
                    .filter_map(|intent| {
                        let retained = self
                            .intent_cleanup_exit_authorizations
                            .try_lock()
                            .ok()
                            .and_then(|authorizations| {
                                authorizations
                                    .get(intent.summary.transfer_id.as_str())
                                    .cloned()
                            })
                            .is_some_and(|authorization| authorization.matches(&intent.summary));
                        if retained {
                            return None;
                        }
                        intent.summary.cleanup_residual.as_ref().map(|residual| {
                            SftpShutdownResidual {
                                transfer_id: intent.summary.transfer_id.to_string(),
                                opaque_location: transfer_intent_residual_display(residual),
                            }
                        })
                    }),
            );
        self.cleanup_exit_authorizations.lock().await.clear();
        self.intent_cleanup_exit_authorizations.lock().await.clear();
        let result = failure.into_result();
        if result.is_ok() {
            self.local_boundaries.lock().await.clear();
            let mut local_directories = self.local_directories.lock().await;
            for capability in local_directories.values() {
                capability.revoked.store(true, Ordering::Release);
            }
            local_directories.clear();
            drop(local_directories);
            self.local_directory_entries.lock().await.clear();
            self.local_directory_cursors.lock().await.clear();
            self.remote_directory_refs.lock().await.clear();
            self.remote_entry_refs.lock().await.clear();
            self.active_remote_directory_cursors.lock().await.clear();
            self.remote_directory_list_ledger.lock().await.clear();
            self.remote_directory_list_operations.lock().await.clear();
            self.remote_directory_cancel_ledger.lock().await.clear();
            self.intent_cleanup_replays.lock().await.clear();
            self.prepared_transfer_intents.lock().await.clear();
            self.pending_remote_copies.lock().await.clear();
            self.transfer_intents.lock().await.clear();
            self.public_intent_transfer_ids.lock().await.clear();
            self.transfer_boundaries.lock().await.clear();
            self.active_transfers.lock().await.clear();
            self.active_transfer_intents.lock().await.clear();
            scheduling_pause.keep_paused();
        }
        result
    }

    async fn retry_existing_cleanup_residuals(&self) {
        let candidates = {
            let records = self.records.lock().await;
            records
                .values()
                .flat_map(|record| record.actor.transfers())
                .filter(|transfer| transfer.cleanup_residual.is_some())
                .map(|transfer| {
                    (
                        transfer.transfer_id.clone(),
                        transfer.generation,
                        transfer.direction,
                        transfer.target.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        for (transfer_id, generation, direction, target) in candidates {
            if direction != TransferDirection::Download
                || !matches!(target, TransferEndpoint::LocalBoundaryToken(_))
            {
                // A remote temporary target cannot be safely cleaned after its
                // resource-private transport has been dropped. Preserve the
                // residual as a stable blocker instead of reconnecting during
                // application exit.
                continue;
            }
            let boundary = self
                .transfer_boundaries
                .lock()
                .await
                .get(transfer_id.as_str())
                .cloned();
            let Some(boundary) = boundary else {
                continue;
            };
            #[cfg(unix)]
            let cleanup = boundary
                .native()
                .and_then(|capability| {
                    capability
                        .temporary_name(&transfer_id)
                        .map_err(|_| TransferFailureCode::CleanupIncomplete)
                        .map(|name| capability.remove_temporary(&name))
                })
                .unwrap_or_else(|_| CleanupOutcome::Residual {
                    opaque_location: "local boundary unavailable".to_owned(),
                });
            #[cfg(not(unix))]
            let cleanup = {
                let _ = boundary.unsupported();
                CleanupOutcome::Residual {
                    opaque_location: "local transfer unsupported on this platform".to_owned(),
                }
            };
            if cleanup != CleanupOutcome::Cleaned {
                continue;
            }
            let resolved = {
                let mut records = self.records.lock().await;
                records.values_mut().any(|record| {
                    record
                        .actor
                        .resolve_cleanup_residual(&transfer_id, generation)
                        .is_ok()
                })
            };
            if resolved {
                self.transfer_boundaries
                    .lock()
                    .await
                    .remove(transfer_id.as_str());
            }
        }
    }
}

type CoreResult<T> = Result<T, Box<wire::CoreApiError>>;

#[tauri::command]
pub(crate) async fn sftp_session_open(
    request: wire::SftpSessionOpenRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpSessionSummary> {
    let _creation_permit = lifecycle.acquire_resource_creation(request.meta.request_id.clone())?;
    service
        .open(&request)
        .await
        .map_err(|error| map_sftp_core_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_session_snapshot(
    request: wire::SftpSessionSnapshotRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpSessionSnapshot> {
    service
        .snapshot()
        .await
        .map_err(|error| map_sftp_core_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_session_disconnect(
    request: wire::SftpSessionDisconnectRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpSessionSummary> {
    let request_id = request.meta.request_id.clone();
    service
        .disconnect(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_local_boundary_register(
    request: wire::SftpLocalBoundaryRegisterRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpLocalBoundary> {
    let _creation_permit = lifecycle.acquire_resource_creation(request.meta.request_id.clone())?;
    service
        .register_local_boundary(request.kind, request.selected_path)
        .await
        .map_err(|error| {
            map_sftp_core_error(request.meta.request_id, SftpProductionError::Runtime(error))
        })
}

#[tauri::command]
pub(crate) async fn sftp_local_directory_register(
    request: wire::SftpLocalDirectoryRegisterRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpLocalDirectoryCapability> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .register_local_directory(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_local_directory_list(
    request: wire::SftpLocalDirectoryListRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpLocalDirectoryListing> {
    let request_id = request.meta.request_id.clone();
    service
        .list_local_directory(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_local_directory_open_child(
    request: wire::SftpLocalDirectoryOpenChildRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpLocalDirectoryCapability> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .open_local_child_directory(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_local_directory_create_child(
    request: wire::SftpLocalDirectoryCreateChildRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpLocalDirectoryCapability> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .create_local_child_directory(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_local_directory_release(
    request: wire::SftpLocalDirectoryReleaseRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<()> {
    let request_id = request.meta.request_id.clone();
    service
        .release_local_directory(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_directory_list(
    request: wire::SftpDirectoryListRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpDirectoryListing> {
    let request_id = request.meta.request_id.clone();
    service
        .list_directory(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_directory_list_cancel(
    request: wire::SftpDirectoryListCancelRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<()> {
    let request_id = request.meta.request_id.clone();
    service
        .cancel_directory_listing(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_file_preview(
    request: wire::SftpFilePreviewRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpFilePreview> {
    let request_id = request.meta.request_id.clone();
    service
        .preview_file(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_file_tail(
    request: wire::SftpFileTailRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpFileTailResult> {
    let request_id = request.meta.request_id.clone();
    service
        .tail_file(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_file_mutate(
    request: wire::SftpFileMutationRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpFileMutationResult> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .mutate_file(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_enqueue(
    request: wire::SftpTransferEnqueueRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpTransferSummary> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .enqueue_transfer(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_intent_prepare(
    request: wire::SftpTransferIntentPrepareRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpTransferIntentPrepared> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .prepare_transfer_intent(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_intent_enqueue(
    request: wire::SftpTransferIntentEnqueueRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpTransferIntentSummary> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .enqueue_transfer_intent(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_intent_snapshot(
    request: wire::SftpTransferIntentSnapshotRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpTransferIntentSnapshot> {
    service
        .snapshot_transfer_intents()
        .await
        .map_err(|error| map_sftp_core_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_intent_cancel(
    request: wire::SftpTransferIntentActionRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpTransferIntentSummary> {
    let request_id = request.meta.request_id.clone();
    service
        .cancel_transfer_intent(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_intent_cleanup_retry(
    request: wire::SftpTransferIntentCleanupRetryRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpTransferIntentSummary> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .retry_transfer_intent_cleanup(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_intent_cleanup_retain(
    request: wire::SftpTransferIntentCleanupRetainRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpTransferIntentSummary> {
    let request_id = request.meta.request_id.clone();
    service
        .retain_transfer_intent_cleanup_for_exit(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_cancel(
    request: wire::SftpTransferActionRequest,
    service: State<'_, SftpSessionService>,
) -> CoreResult<wire::SftpTransferSummary> {
    let request_id = request.meta.request_id.clone();
    service
        .cancel_transfer(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_transfer_resume(
    request: wire::SftpTransferActionRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpTransferSummary> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .resume_transfer(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_remote_cleanup_retry(
    request: wire::SftpRemoteCleanupRetryRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpTransferSummary> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .retry_remote_cleanup(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

#[tauri::command]
pub(crate) async fn sftp_remote_cleanup_retain(
    request: wire::SftpRemoteCleanupRetainRequest,
    service: State<'_, SftpSessionService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<wire::SftpTransferSummary> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .retain_remote_cleanup_for_exit(request)
        .await
        .map_err(|error| map_sftp_core_error(request_id, error))
}

fn map_sftp_summary(
    record: &SftpServiceRecord,
) -> Result<wire::SftpSessionSummary, SftpProductionError> {
    let summary = record.actor.summary();
    let generation = summary.generation.ok_or(SftpRuntimeError::InvalidState)?;
    let transfer_count = record.actor.transfers().count();
    let active_transfer_count = record
        .actor
        .transfers()
        .filter(|transfer| {
            !matches!(
                transfer.state,
                TransferState::Completed | TransferState::Cancelled | TransferState::Failed
            )
        })
        .count();
    Ok(wire::SftpSessionSummary {
        session_id: wire::SftpSessionId::parse(summary.session_id.as_str())
            .map_err(|_| SftpRuntimeError::InvalidInput)?,
        host_id: summary
            .host_id
            .as_deref()
            .map(wire::HostId::parse)
            .transpose()
            .map_err(|_| SftpRuntimeError::InvalidInput)?,
        parent_ssh_session: summary.parent_ssh_session.clone(),
        generation: wire::WireSequence::new(generation.get()),
        state_revision: wire::WireSequence::new(summary.state_revision),
        state: map_session_state(summary.state),
        transfer_count: u32::try_from(transfer_count).unwrap_or(u32::MAX),
        active_transfer_count: u32::try_from(active_transfer_count).unwrap_or(u32::MAX),
        failure: summary.failure_code.map(|code| wire::SftpSessionFailure {
            code: map_session_failure(code),
            stage: "sftp-session".to_owned(),
            message_key: "errors.sftp.sessionFailed".to_owned(),
        }),
    })
}

fn map_session_state(state: SftpSessionState) -> wire::SftpSessionState {
    match state {
        SftpSessionState::Idle | SftpSessionState::Connecting => wire::SftpSessionState::Connecting,
        SftpSessionState::VerifyingHostKey => wire::SftpSessionState::VerifyingHostKey,
        SftpSessionState::Authenticating => wire::SftpSessionState::Authenticating,
        SftpSessionState::NeedsAuthentication => wire::SftpSessionState::NeedsAuthentication,
        SftpSessionState::OpeningSubsystem => wire::SftpSessionState::OpeningSubsystem,
        SftpSessionState::Ready => wire::SftpSessionState::Ready,
        SftpSessionState::Disconnecting => wire::SftpSessionState::Disconnecting,
        SftpSessionState::Closed => wire::SftpSessionState::Closed,
        SftpSessionState::Failed => wire::SftpSessionState::Failed,
    }
}

fn notification_transfer_state(state: wire::SftpTransferState) -> NotificationTransferState {
    match state {
        wire::SftpTransferState::Completed => NotificationTransferState::Completed,
        wire::SftpTransferState::Failed => NotificationTransferState::Failed,
        wire::SftpTransferState::Queued
        | wire::SftpTransferState::Preparing
        | wire::SftpTransferState::Transferring
        | wire::SftpTransferState::Verifying
        | wire::SftpTransferState::Committing
        | wire::SftpTransferState::PausedByDisconnect
        | wire::SftpTransferState::Cancelling
        | wire::SftpTransferState::Cancelled => NotificationTransferState::Active,
    }
}

fn notification_intent_generation(
    summary: &wire::SftpTransferIntentSummary,
) -> Option<WireSequence> {
    [&summary.source_fence, &summary.target_fence]
        .into_iter()
        .find_map(|fence| match fence {
            wire::SftpTransferEndpointFence::LocalCapability { .. } => None,
            wire::SftpTransferEndpointFence::RemoteSession { generation, .. } => Some(*generation),
        })
}

fn notification_transfer_kind_name(kind: NotificationTransferKind) -> &'static str {
    match kind {
        NotificationTransferKind::Direct => "transfer",
        NotificationTransferKind::Intent => "transferIntent",
    }
}

fn map_session_failure(code: SftpFailureCode) -> wire::SftpFailureCode {
    match code {
        SftpFailureCode::HostKeyRejected => wire::SftpFailureCode::HostKeyRejected,
        SftpFailureCode::HostKeyMismatch => wire::SftpFailureCode::HostKeyMismatch,
        SftpFailureCode::VaultLocked => wire::SftpFailureCode::VaultLocked,
        SftpFailureCode::CredentialUnavailable => wire::SftpFailureCode::CredentialUnavailable,
        SftpFailureCode::AuthenticationRejected => wire::SftpFailureCode::AuthenticationRejected,
        SftpFailureCode::SubsystemRejected => wire::SftpFailureCode::SubsystemRejected,
        SftpFailureCode::TransportLost => wire::SftpFailureCode::TransportLost,
        SftpFailureCode::Protocol => wire::SftpFailureCode::Protocol,
    }
}

fn map_remote_directory_page(
    parent: &RemotePath,
    page: russh_sftp::client::fs::ReadDir,
) -> Result<Vec<RemoteDirectoryEntry>, SftpProductionError> {
    page.map(|entry| {
        let file_name = entry.file_name();
        let metadata = entry.metadata();
        let kind = if metadata.is_regular() {
            RemoteEntryKind::File
        } else if metadata.is_dir() {
            RemoteEntryKind::Directory
        } else if metadata.is_symlink() {
            RemoteEntryKind::Symlink
        } else {
            RemoteEntryKind::Other
        };
        Ok(RemoteDirectoryEntry {
            path: remote_directory_child_path(parent, &file_name)?,
            kind,
            size: metadata.size,
            modified_at_unix_ms: metadata.mtime.map(|value| i64::from(value) * 1_000),
            permission_bits: metadata.permissions,
        })
    })
    .collect()
}

fn remote_directory_child_path(
    parent: &RemotePath,
    file_name: &str,
) -> Result<RemotePath, SftpProductionError> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.as_bytes().contains(&b'/')
        || file_name.as_bytes().contains(&0)
    {
        return Err(sftp_protocol_error());
    }
    let mut child_path = parent.as_bytes().to_vec();
    if !child_path.ends_with(b"/") {
        child_path.push(b'/');
    }
    child_path.extend_from_slice(file_name.as_bytes());
    RemotePath::parse(child_path).map_err(Into::into)
}

fn remote_file_name(path: &RemotePath) -> Result<String, SftpProductionError> {
    let name = path
        .as_bytes()
        .rsplit(|byte| *byte == b'/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or(SftpRuntimeError::InvalidInput)?;
    std::str::from_utf8(name)
        .map(ToOwned::to_owned)
        .map_err(|_| SftpProductionError::NonUtf8RemotePath)
}

fn remote_archive_child_path(
    parent: &RemotePath,
    relative: &str,
) -> Result<RemotePath, SftpProductionError> {
    validate_archive_name(relative)?;
    let mut bytes = parent.as_bytes().to_vec();
    if !bytes.ends_with(b"/") {
        bytes.push(b'/');
    }
    bytes.extend_from_slice(relative.as_bytes());
    RemotePath::parse(bytes).map_err(Into::into)
}

fn map_remote_entry(
    entry_ref: String,
    entry: RemoteDirectoryEntry,
) -> Result<wire::SftpRemoteDirectoryEntry, SftpProductionError> {
    let display_name = std::str::from_utf8(entry.path.as_bytes())
        .map_err(|_| SftpProductionError::NonUtf8RemotePath)?
        .rsplit('/')
        .next()
        .unwrap_or("/")
        .to_owned();
    Ok(wire::SftpRemoteDirectoryEntry {
        entry_ref,
        path: wire::SftpRemotePath {
            bytes: entry.path.0,
        },
        display_name,
        kind: match entry.kind {
            RemoteEntryKind::File => wire::SftpRemoteEntryKind::File,
            RemoteEntryKind::Directory => wire::SftpRemoteEntryKind::Directory,
            RemoteEntryKind::Symlink => wire::SftpRemoteEntryKind::Symlink,
            RemoteEntryKind::Other => wire::SftpRemoteEntryKind::Other,
        },
        size: entry.size,
        modified_at_unix_ms: entry.modified_at_unix_ms,
        permission_bits: entry.permission_bits,
    })
}

fn validate_remote_intent_entry(
    entry: &RemoteEntryReference,
    directory_ref: &str,
    session_id: &str,
    generation: u64,
    expected_bytes: u64,
    now: std::time::Instant,
) -> SftpRuntimeResult<()> {
    if entry.directory_ref != directory_ref
        || entry.session_id != session_id
        || entry.generation != generation
        || entry.expires_at <= now
        || entry.precondition.kind != RemoteEntryKind::File
        || entry.precondition.size != Some(expected_bytes)
    {
        return Err(SftpRuntimeError::Conflict);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewKind {
    Text,
    Image(&'static str),
}

fn preview_kind_for_entry(path: &RemotePath, display_name: &str) -> Option<PreviewKind> {
    preview_kind_for_name(display_name)
        .or_else(|| nginx_standard_site_path(path).then_some(PreviewKind::Text))
}

fn nginx_standard_site_path(path: &RemotePath) -> bool {
    const DIRECTORIES: [&[u8]; 2] = [b"/etc/nginx/sites-available/", b"/etc/nginx/sites-enabled/"];
    let Some(name) = DIRECTORIES
        .iter()
        .find_map(|directory| path.as_bytes().strip_prefix(*directory))
    else {
        return false;
    };

    !name.is_empty()
        && !matches!(name, b"." | b"..")
        && !name.contains(&b'/')
        && !name.iter().any(|byte| byte.is_ascii_control())
}

fn preview_kind_for_name(display_name: &str) -> Option<PreviewKind> {
    let lower = display_name.to_ascii_lowercase();
    let base_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    let extension = base_name.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        Some("png") => Some(PreviewKind::Image("image/png")),
        Some("jpg" | "jpeg") => Some(PreviewKind::Image("image/jpeg")),
        Some("gif") => Some(PreviewKind::Image("image/gif")),
        Some("webp") => Some(PreviewKind::Image("image/webp")),
        Some(
            "txt" | "md" | "markdown" | "log" | "json" | "jsonl" | "yaml" | "yml" | "toml" | "ini"
            | "conf" | "cfg" | "xml" | "csv" | "tsv" | "sh" | "bash" | "zsh" | "fish" | "ps1"
            | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "vue" | "css" | "scss" | "less"
            | "html" | "htm" | "svg" | "rs" | "py" | "rb" | "go" | "java" | "c" | "h" | "cpp"
            | "hpp" | "swift" | "kt" | "sql" | "env" | "gitignore",
        ) => Some(PreviewKind::Text),
        _ if matches!(
            base_name,
            "dockerfile" | "makefile" | "license" | "readme" | "hosts" | "known_hosts"
        ) =>
        {
            Some(PreviewKind::Text)
        }
        _ => None,
    }
}

fn detect_text_line_ending(bytes: &[u8]) -> wire::SftpTextLineEnding {
    if bytes.windows(2).any(|window| window == b"\r\n") {
        wire::SftpTextLineEnding::CrLf
    } else if bytes.contains(&b'\r') {
        wire::SftpTextLineEnding::Cr
    } else {
        wire::SftpTextLineEnding::Lf
    }
}

#[cfg(test)]
fn tail_read_window(total_size: u64, requested_offset: u64) -> (u64, u64, bool) {
    tail_read_window_limited(total_size, requested_offset, MAX_TAIL_CHUNK_BYTES)
}

fn tail_read_window_limited(
    total_size: u64,
    requested_offset: u64,
    maximum_bytes: usize,
) -> (u64, u64, bool) {
    let reset = requested_offset > total_size;
    let start_offset = if reset {
        total_size.saturating_sub(MAX_TAIL_INITIAL_BYTES as u64)
    } else {
        requested_offset
    };
    let length = total_size
        .saturating_sub(start_offset)
        .min(maximum_bytes as u64);
    (start_offset, length, reset)
}

fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn map_transfer_summary(
    record: &SftpServiceRecord,
    transfer: &TransferRecord,
) -> Result<wire::SftpTransferSummary, SftpProductionError> {
    Ok(wire::SftpTransferSummary {
        transfer_id: wire::TransferId::parse(transfer.transfer_id.as_str())
            .map_err(|_| SftpRuntimeError::InvalidInput)?,
        session_id: wire::SftpSessionId::parse(record.actor.summary().session_id.as_str())
            .map_err(|_| SftpRuntimeError::InvalidInput)?,
        generation: wire::WireSequence::new(transfer.generation.get()),
        direction: match transfer.direction {
            TransferDirection::Upload => wire::SftpTransferDirection::Upload,
            TransferDirection::Download => wire::SftpTransferDirection::Download,
        },
        source: map_transfer_endpoint(&transfer.source),
        target: map_transfer_endpoint(&transfer.target),
        expected_bytes: transfer.expected_bytes,
        transferred_bytes: transfer.transferred_bytes,
        bytes_per_second: transfer_average_bytes_per_second(transfer),
        remaining_seconds: transfer_remaining_seconds(transfer),
        state_revision: wire::WireSequence::new(transfer.state_revision),
        state: match transfer.state {
            TransferState::Queued => wire::SftpTransferState::Queued,
            TransferState::Preparing => wire::SftpTransferState::Preparing,
            TransferState::Transferring => wire::SftpTransferState::Transferring,
            TransferState::Verifying => wire::SftpTransferState::Verifying,
            TransferState::Committing => wire::SftpTransferState::Committing,
            TransferState::Completed => wire::SftpTransferState::Completed,
            TransferState::PausedByDisconnect => wire::SftpTransferState::PausedByDisconnect,
            TransferState::Cancelling => wire::SftpTransferState::Cancelling,
            TransferState::Cancelled => wire::SftpTransferState::Cancelled,
            TransferState::Failed => wire::SftpTransferState::Failed,
        },
        failure_code: transfer.failure_code.map(|code| match code {
            TransferFailureCode::TargetExists => wire::SftpTransferFailureCode::TargetExists,
            TransferFailureCode::UnsafeReplaceUnsupported => {
                wire::SftpTransferFailureCode::UnsafeReplaceUnsupported
            }
            TransferFailureCode::PermissionDenied => {
                wire::SftpTransferFailureCode::PermissionDenied
            }
            TransferFailureCode::TransportLost => wire::SftpTransferFailureCode::TransportLost,
            TransferFailureCode::LengthMismatch => wire::SftpTransferFailureCode::LengthMismatch,
            TransferFailureCode::CleanupIncomplete => {
                wire::SftpTransferFailureCode::CleanupIncomplete
            }
            TransferFailureCode::UnsupportedPathEncoding => {
                wire::SftpTransferFailureCode::UnsupportedPathEncoding
            }
            TransferFailureCode::Protocol => wire::SftpTransferFailureCode::Protocol,
        }),
        cleanup_residual: map_cleanup_residual(transfer),
    })
}

fn merge_legacy_intent_summary(
    mut summary: wire::SftpTransferIntentSummary,
    legacy: &wire::SftpTransferSummary,
) -> wire::SftpTransferIntentSummary {
    summary.transferred_bytes = legacy.transferred_bytes;
    summary.bytes_per_second = legacy.bytes_per_second;
    summary.remaining_seconds = legacy.remaining_seconds;
    summary.state_revision = legacy.state_revision;
    summary.state = legacy.state;
    summary.failure_code = legacy.failure_code;
    summary.commit_outcome = if legacy.state == wire::SftpTransferState::Completed {
        wire::SftpTransferCommitOutcome::Committed
    } else {
        wire::SftpTransferCommitOutcome::NotCommitted
    };
    summary.cleanup_residual = legacy
        .cleanup_residual
        .as_ref()
        .map(|residual| match residual {
            wire::SftpCleanupResidual::RemoteTemporaryTarget { path, display_path } => {
                let wire::SftpTransferEndpointFence::RemoteSession {
                    session_id,
                    generation,
                } = &summary.target_fence
                else {
                    return wire::SftpTransferIntentCleanupResidual::Unknown {
                        display_name: display_path.clone(),
                    };
                };
                wire::SftpTransferIntentCleanupResidual::RemoteTemporaryTarget {
                    session_id: session_id.clone(),
                    generation: *generation,
                    path: path.clone(),
                    display_path: display_path.clone(),
                }
            }
            wire::SftpCleanupResidual::LocalTemporaryTarget { display_name } => {
                let wire::SftpTransferEndpointFence::LocalCapability {
                    directory_ref,
                    revision,
                } = &summary.target_fence
                else {
                    return wire::SftpTransferIntentCleanupResidual::Unknown {
                        display_name: display_name.clone(),
                    };
                };
                wire::SftpTransferIntentCleanupResidual::LocalTemporaryTarget {
                    directory_ref: directory_ref.clone(),
                    revision: *revision,
                    display_name: display_name.clone(),
                }
            }
            wire::SftpCleanupResidual::Unknown { display_name } => {
                wire::SftpTransferIntentCleanupResidual::Unknown {
                    display_name: display_name.clone(),
                }
            }
        });
    summary
}

fn remote_copy_intent_matches_plan(intent: &TransferIntentRecord, plan: &RemoteCopyPlan) -> bool {
    let source_matches = matches!(
        &intent.summary.source_fence,
        wire::SftpTransferEndpointFence::RemoteSession {
            session_id,
            generation,
        } if session_id.as_str() == plan.source_session_key
            && generation.get() == plan.source_generation.get()
    );
    let target_matches = matches!(
        &intent.summary.target_fence,
        wire::SftpTransferEndpointFence::RemoteSession {
            session_id,
            generation,
        } if session_id.as_str() == plan.target_session_key
            && generation.get() == plan.target_generation.get()
    );
    source_matches && target_matches
}

fn remote_copy_failure(failure: TransferFailureCode) -> RemoteCopyExecutionFailure {
    RemoteCopyExecutionFailure {
        failure_code: match failure {
            TransferFailureCode::TargetExists => wire::SftpTransferFailureCode::TargetExists,
            TransferFailureCode::UnsafeReplaceUnsupported => {
                wire::SftpTransferFailureCode::UnsafeReplaceUnsupported
            }
            TransferFailureCode::PermissionDenied => {
                wire::SftpTransferFailureCode::PermissionDenied
            }
            TransferFailureCode::TransportLost => wire::SftpTransferFailureCode::TransportLost,
            TransferFailureCode::LengthMismatch => wire::SftpTransferFailureCode::LengthMismatch,
            TransferFailureCode::CleanupIncomplete => {
                wire::SftpTransferFailureCode::CleanupIncomplete
            }
            TransferFailureCode::UnsupportedPathEncoding => {
                wire::SftpTransferFailureCode::UnsupportedPathEncoding
            }
            TransferFailureCode::Protocol => wire::SftpTransferFailureCode::Protocol,
        },
        commit_outcome: wire::SftpTransferCommitOutcome::NotCommitted,
        cleanup_residual: None,
    }
}

fn remote_copy_interruption_resolution(
    plan: &RemoteCopyPlan,
    state: Option<wire::SftpTransferState>,
) -> (
    wire::SftpTransferCommitOutcome,
    Option<wire::SftpTransferIntentCleanupResidual>,
) {
    let Ok(temporary_target) = remote_temporary_target(&plan.target_path, &plan.transfer_id) else {
        return (
            wire::SftpTransferCommitOutcome::Uncertain,
            Some(wire::SftpTransferIntentCleanupResidual::Unknown {
                display_name: "remote temporary file".to_owned(),
            }),
        );
    };
    let commit_outcome = if state == Some(wire::SftpTransferState::Committing) {
        wire::SftpTransferCommitOutcome::Uncertain
    } else {
        wire::SftpTransferCommitOutcome::NotCommitted
    };
    let residual = remote_copy_cleanup_residual(
        plan,
        Some(&temporary_target),
        CleanupOutcome::Residual {
            opaque_location: safe_remote_path_display(temporary_target.as_bytes()),
        },
    );
    (commit_outcome, residual)
}

fn remote_copy_cleanup_residual(
    plan: &RemoteCopyPlan,
    temporary_target: Option<&RemotePath>,
    cleanup: CleanupOutcome,
) -> Option<wire::SftpTransferIntentCleanupResidual> {
    let CleanupOutcome::Residual { opaque_location } = cleanup else {
        return None;
    };
    let Some(path) = temporary_target else {
        return Some(wire::SftpTransferIntentCleanupResidual::Unknown {
            display_name: opaque_location,
        });
    };
    let session_id = wire::SftpSessionId::parse(&plan.target_session_key).ok()?;
    Some(
        wire::SftpTransferIntentCleanupResidual::RemoteTemporaryTarget {
            session_id,
            generation: WireSequence::new(plan.target_generation.get()),
            path: wire::SftpRemotePath {
                bytes: path.as_bytes().to_vec(),
            },
            display_path: safe_remote_path_display(path.as_bytes()),
        },
    )
}

fn transfer_intent_target_residual(
    target: &ResolvedIntentTarget,
    transfer_id: &wire::TransferId,
) -> Option<wire::SftpTransferIntentCleanupResidual> {
    match target {
        ResolvedIntentTarget::Remote {
            session_id,
            generation,
            path,
        } => {
            let transfer_id = TransferId::parse(transfer_id.to_string()).ok()?;
            let temporary_target = remote_temporary_target(path, &transfer_id).ok()?;
            Some(
                wire::SftpTransferIntentCleanupResidual::RemoteTemporaryTarget {
                    session_id: wire::SftpSessionId::parse(session_id).ok()?,
                    generation: WireSequence::new(*generation),
                    path: wire::SftpRemotePath {
                        bytes: temporary_target.as_bytes().to_vec(),
                    },
                    display_path: safe_remote_path_display(temporary_target.as_bytes()),
                },
            )
        }
        ResolvedIntentTarget::Local {
            directory_ref,
            revision,
            name,
        } => Some(
            wire::SftpTransferIntentCleanupResidual::LocalTemporaryTarget {
                directory_ref: directory_ref.clone(),
                revision: WireSequence::new(*revision),
                display_name: safe_remote_path_display(name),
            },
        ),
    }
}

fn transfer_intent_residual_display(residual: &wire::SftpTransferIntentCleanupResidual) -> String {
    match residual {
        wire::SftpTransferIntentCleanupResidual::RemoteTemporaryTarget { display_path, .. } => {
            display_path.clone()
        }
        wire::SftpTransferIntentCleanupResidual::LocalTemporaryTarget { display_name, .. }
        | wire::SftpTransferIntentCleanupResidual::Unknown { display_name } => display_name.clone(),
    }
}

fn remote_metadata_matches(
    metadata: &RemoteFileAttributes,
    expected: &RemoteObjectPrecondition,
) -> bool {
    metadata.is_regular()
        && expected.kind == RemoteEntryKind::File
        && metadata.size == expected.size
        && metadata.mtime.map(|value| i64::from(value) * 1_000) == expected.modified_at_unix_ms
}

fn remote_precondition_from_metadata(metadata: &RemoteFileAttributes) -> RemoteObjectPrecondition {
    RemoteObjectPrecondition {
        kind: remote_entry_kind_from_flags(
            metadata.is_regular(),
            metadata.is_dir(),
            metadata.is_symlink(),
        ),
        size: metadata.size,
        modified_at_unix_ms: metadata.mtime.map(|value| i64::from(value) * 1_000),
    }
}

fn map_cleanup_residual(transfer: &TransferRecord) -> Option<wire::SftpCleanupResidual> {
    let residual = transfer.cleanup_residual.as_ref()?;
    Some(match transfer.temporary_target.as_ref() {
        Some(TemporaryTarget::Remote(path)) => wire::SftpCleanupResidual::RemoteTemporaryTarget {
            path: wire::SftpRemotePath {
                bytes: path.as_bytes().to_vec(),
            },
            display_path: safe_remote_path_display(path.as_bytes()),
        },
        Some(TemporaryTarget::LocalBoundaryToken(_)) => {
            wire::SftpCleanupResidual::LocalTemporaryTarget {
                display_name: "selected local temporary file".to_owned(),
            }
        }
        None => wire::SftpCleanupResidual::Unknown {
            display_name: residual.clone(),
        },
    })
}

fn safe_remote_path_display(bytes: &[u8]) -> String {
    if let Ok(value) = std::str::from_utf8(bytes)
        && !value.chars().any(is_unsafe_display_character)
    {
        return value.chars().take(MAX_LABEL_BYTES).collect();
    }
    let mut display = String::new();
    for byte in bytes.iter().take(MAX_LABEL_BYTES / 4) {
        if (byte.is_ascii_graphic() || *byte == b' ') && *byte != b'\\' {
            display.push(char::from(*byte));
        } else if *byte == b'\\' {
            display.push_str("\\\\");
        } else {
            use std::fmt::Write as _;
            let _ = write!(display, "\\x{byte:02x}");
        }
    }
    if bytes.len() > MAX_LABEL_BYTES / 4 {
        display.push('…');
    }
    display
}

fn is_unsafe_display_character(value: char) -> bool {
    value.is_control()
        || matches!(
            value,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

fn map_transfer_endpoint(endpoint: &TransferEndpoint) -> wire::SftpTransferEndpoint {
    match endpoint {
        TransferEndpoint::LocalBoundaryToken(token) => {
            wire::SftpTransferEndpoint::LocalBoundaryToken {
                token: token.clone(),
                display_name: "selected-local-file".to_owned(),
            }
        }
        TransferEndpoint::Remote(path) => wire::SftpTransferEndpoint::Remote {
            path: wire::SftpRemotePath {
                bytes: path.as_bytes().to_vec(),
            },
        },
    }
}

fn transfer_average_bytes_per_second(transfer: &TransferRecord) -> Option<u64> {
    if transfer.transferred_bytes == 0 {
        return None;
    }
    let elapsed = transfer.started_at?.elapsed().as_secs_f64();
    if elapsed < 0.001 {
        return None;
    }
    Some((transfer.transferred_bytes as f64 / elapsed).round() as u64)
}

fn transfer_remaining_seconds(transfer: &TransferRecord) -> Option<u64> {
    if transfer.state == TransferState::Completed {
        return Some(0);
    }
    let speed = transfer_average_bytes_per_second(transfer)?;
    if speed == 0 || transfer.transferred_bytes >= transfer.expected_bytes {
        return None;
    }
    Some(
        transfer
            .expected_bytes
            .saturating_sub(transfer.transferred_bytes)
            .div_ceil(speed),
    )
}

/// A locked Vault can only explain failures while acquiring or applying a
/// credential. Host-key and route failures must retain their protocol origin.
fn vault_locked_after_credential_failure(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::AuthenticationRejected | TransportError::InvalidPrivateKey
    ) || matches!(
        error,
        TransportError::RouteIngress(route)
            if route.kind == IngressFailureKind::CredentialLocked
    )
}

fn map_production_failure(error: &SftpProductionError) -> SftpFailureCode {
    match error {
        SftpProductionError::VaultUnavailable => SftpFailureCode::VaultLocked,
        SftpProductionError::Profile(ConnectionProfileError::CredentialUnavailable) => {
            SftpFailureCode::CredentialUnavailable
        }
        SftpProductionError::Connection { error, .. } | SftpProductionError::Transport(error) => {
            map_transport_failure(error)
        }
        SftpProductionError::Profile(_) => SftpFailureCode::Protocol,
        SftpProductionError::NonUtf8RemotePath
        | SftpProductionError::Runtime(_)
        | SftpProductionError::ShutdownIncomplete(_) => SftpFailureCode::Protocol,
    }
}

fn map_sftp_core_error(
    request_id: wire::RequestId,
    error: SftpProductionError,
) -> Box<wire::CoreApiError> {
    let (category, retry_strategy) = match &error {
        SftpProductionError::Runtime(SftpRuntimeError::InvalidInput)
        | SftpProductionError::NonUtf8RemotePath => {
            (wire::ErrorCategory::Validation, wire::RetryStrategy::Never)
        }
        SftpProductionError::Runtime(SftpRuntimeError::StaleGeneration)
        | SftpProductionError::Runtime(SftpRuntimeError::Conflict) => (
            wire::ErrorCategory::Conflict,
            wire::RetryStrategy::RefreshSnapshot,
        ),
        SftpProductionError::VaultUnavailable
        | SftpProductionError::Profile(ConnectionProfileError::CredentialUnavailable) => (
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::WaitForUser,
        ),
        _ => (
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::RefreshSnapshot,
        ),
    };
    Box::new(wire::CoreApiError {
        code: "sftp.operation_failed".to_owned(),
        category,
        retry_strategy,
        message_key: "errors.sftp.operationFailed".to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}

fn decode_transfer_endpoint(
    endpoint: wire::SftpTransferEndpoint,
) -> Result<TransferEndpoint, SftpProductionError> {
    match endpoint {
        wire::SftpTransferEndpoint::LocalBoundaryToken { token, .. } => {
            Ok(TransferEndpoint::LocalBoundaryToken(token))
        }
        wire::SftpTransferEndpoint::Remote { path } => {
            Ok(TransferEndpoint::Remote(RemotePath::parse(path.bytes)?))
        }
    }
}

fn remote_temporary_target(
    target: &RemotePath,
    transfer_id: &TransferId,
) -> SftpRuntimeResult<RemotePath> {
    let target = std::str::from_utf8(target.as_bytes())
        .map_err(|_| SftpRuntimeError::UnsupportedPathEncoding)?;
    let (parent, name) = target
        .rsplit_once('/')
        .ok_or(SftpRuntimeError::InvalidInput)?;
    if name.is_empty() {
        return Err(SftpRuntimeError::InvalidInput);
    }
    let parent = if parent.is_empty() { "/" } else { parent };
    let separator = if parent == "/" { "" } else { "/" };
    RemotePath::parse(
        format!(
            "{parent}{separator}.{name}.norishell-{}.part",
            transfer_id.as_str()
        )
        .into_bytes(),
    )
}

fn remote_edit_temporary_path(
    target: &RemotePath,
    operation_id: &str,
) -> SftpRuntimeResult<RemotePath> {
    let target = std::str::from_utf8(target.as_bytes())
        .map_err(|_| SftpRuntimeError::UnsupportedPathEncoding)?;
    let (parent, name) = target
        .rsplit_once('/')
        .ok_or(SftpRuntimeError::InvalidInput)?;
    if name.is_empty() {
        return Err(SftpRuntimeError::InvalidInput);
    }
    let parent = if parent.is_empty() { "/" } else { parent };
    let separator = if parent == "/" { "" } else { "/" };
    RemotePath::parse(
        format!("{parent}{separator}.{name}.norishell-edit-{operation_id}.part").into_bytes(),
    )
}

fn map_transfer_execution_failure(error: SftpProductionError) -> TransferFailureCode {
    match error {
        SftpProductionError::NonUtf8RemotePath
        | SftpProductionError::Runtime(SftpRuntimeError::UnsupportedPathEncoding) => {
            TransferFailureCode::UnsupportedPathEncoding
        }
        SftpProductionError::Runtime(error) => map_transfer_runtime_failure(error),
        SftpProductionError::Transport(error)
            if matches!(error.as_ref(), TransportError::ConnectionLost) =>
        {
            TransferFailureCode::TransportLost
        }
        SftpProductionError::VaultUnavailable
        | SftpProductionError::Profile(_)
        | SftpProductionError::Connection { .. } => TransferFailureCode::TransportLost,
        SftpProductionError::Transport(_) | SftpProductionError::ShutdownIncomplete(_) => {
            TransferFailureCode::Protocol
        }
    }
}

fn map_transfer_runtime_failure(error: SftpRuntimeError) -> TransferFailureCode {
    match error {
        SftpRuntimeError::Conflict => TransferFailureCode::TargetExists,
        SftpRuntimeError::UnsafeReplaceUnsupported => TransferFailureCode::UnsafeReplaceUnsupported,
        SftpRuntimeError::LengthMismatch | SftpRuntimeError::ProgressRegression => {
            TransferFailureCode::LengthMismatch
        }
        SftpRuntimeError::CleanupIncomplete => TransferFailureCode::CleanupIncomplete,
        SftpRuntimeError::UnsupportedPathEncoding => TransferFailureCode::UnsupportedPathEncoding,
        SftpRuntimeError::InvalidInput
        | SftpRuntimeError::StaleGeneration
        | SftpRuntimeError::InvalidState
        | SftpRuntimeError::ChallengeMismatch
        | SftpRuntimeError::ResumeEvidenceMismatch => TransferFailureCode::Protocol,
    }
}

fn local_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("selected-file")
        .to_owned()
}

async fn bounded_transfer_io<T, E, F>(
    future: F,
    failure: TransferFailureCode,
) -> Result<T, TransferFailureCode>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::time::timeout(SFTP_PROTOCOL_OPERATION_TIMEOUT, future)
        .await
        .map_err(|_| failure)?
        .map_err(|_| failure)
}

async fn wait_for_true(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt as _;

    use super::*;

    #[test]
    fn vault_recovery_classifies_only_credential_failures() {
        assert!(vault_locked_after_credential_failure(
            &TransportError::AuthenticationRejected
        ));
        assert!(vault_locked_after_credential_failure(
            &TransportError::InvalidPrivateKey
        ));
        assert!(vault_locked_after_credential_failure(
            &TransportError::RouteIngress(norishell_ssh_transport::RouteIngressError {
                stage: norishell_ssh_transport::IngressStage::Configuration,
                kind: IngressFailureKind::CredentialLocked,
            })
        ));
        assert!(!vault_locked_after_credential_failure(
            &TransportError::HostKeyRejected
        ));
        assert!(!vault_locked_after_credential_failure(
            &TransportError::ConnectFailed
        ));
    }

    fn actor() -> (SftpSessionActor, SftpGeneration) {
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse("session-1").expect("session id"),
            "host-1",
        )
        .expect("actor");
        let generation = actor.start("base-revision-1").expect("start");
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .expect("authenticated transport");
        actor
            .subsystem_opened(generation, "subsystem-1")
            .expect("subsystem open");
        (actor, generation)
    }

    fn upload(
        actor: &mut SftpSessionActor,
        generation: SftpGeneration,
        policy: ConflictPolicy,
    ) -> TransferId {
        let id = TransferId::parse(format!("transfer-{}", actor.transfers.len() + 1))
            .expect("transfer id");
        actor
            .enqueue_transfer(
                id.clone(),
                generation,
                TransferDirection::Upload,
                TransferEndpoint::LocalBoundaryToken("picker-source-1".to_owned()),
                TransferEndpoint::Remote(RemotePath::parse(b"/srv/app.bin".to_vec()).unwrap()),
                8,
                policy,
            )
            .expect("queue transfer");
        id
    }

    fn download(actor: &mut SftpSessionActor, generation: SftpGeneration) -> TransferId {
        let id = TransferId::parse(wire::TransferId::new().to_string()).expect("transfer id");
        actor
            .enqueue_transfer(
                id.clone(),
                generation,
                TransferDirection::Download,
                TransferEndpoint::Remote(RemotePath::parse(b"/srv/app.bin".to_vec()).unwrap()),
                TransferEndpoint::LocalBoundaryToken("picker-target-1".to_owned()),
                8,
                ConflictPolicy::FailIfExists,
            )
            .expect("queue transfer");
        id
    }

    fn remote_temp() -> TemporaryTarget {
        TemporaryTarget::Remote(
            RemotePath::parse(b"/srv/.app.bin.norishell-part".to_vec()).expect("temp path"),
        )
    }

    fn operation(name: &str) -> OperationFence {
        OperationFence::parse(format!("operation-{name}"), format!("idempotency-{name}"))
            .expect("operation fence")
    }

    #[test]
    fn mutation_ledger_replays_results_and_conflicts_on_any_fingerprint_change() {
        let mut ledger = MutationLedger::default();
        assert!(matches!(
            ledger.claim("operation-1", "key-1", b"fingerprint-a"),
            Ok(MutationLedgerClaim::Execute)
        ));
        assert!(matches!(
            ledger.claim("operation-1", "key-1", b"fingerprint-a"),
            Ok(MutationLedgerClaim::Wait(_))
        ));
        assert!(matches!(
            ledger.claim("operation-1", "key-2", b"fingerprint-a"),
            Err(SftpRuntimeError::Conflict)
        ));
        assert!(matches!(
            ledger.claim("operation-1", "key-1", b"fingerprint-b"),
            Err(SftpRuntimeError::Conflict)
        ));

        let success = wire::SftpFileMutationResult {
            session_id: wire::SftpSessionId::parse("019d0000-0000-7000-8000-000000000001").unwrap(),
            generation: wire::WireSequence::new(1),
        };
        ledger.finish(
            "operation-1",
            MutationLedgerOutcome::Success(success.clone()),
        );
        assert!(matches!(
            ledger.claim("operation-1", "key-1", b"fingerprint-a"),
            Ok(MutationLedgerClaim::Replay(MutationLedgerOutcome::Success(result)))
                if result == success
        ));

        assert!(matches!(
            ledger.claim("operation-2", "key-2", b"fingerprint-c"),
            Ok(MutationLedgerClaim::Execute)
        ));
        ledger.finish(
            "operation-2",
            MutationLedgerOutcome::DeterministicFailure(SftpRuntimeError::Conflict),
        );
        assert!(matches!(
            ledger.claim("operation-2", "key-2", b"fingerprint-c"),
            Ok(MutationLedgerClaim::Replay(
                MutationLedgerOutcome::DeterministicFailure(SftpRuntimeError::Conflict)
            ))
        ));
    }

    #[tokio::test]
    async fn transfer_cancel_watch_preempts_a_hung_protocol_future() {
        let (cancel, mut cancelled) = watch::channel(false);
        let waiter = tokio::spawn(async move {
            tokio::select! {
                _ = wait_for_true(&mut cancelled) => "cancelled",
                _ = std::future::pending::<()>() => "hung",
            }
        });
        cancel.send_replace(true);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("cancel bounded")
                .expect("cancel task"),
            "cancelled"
        );
    }

    #[test]
    fn heartbeat_liveness_failure_converges_ready_generation_to_failed() {
        let (mut actor, generation) = actor();
        let (failed, receiver) = watch::channel(false);
        let owner = SftpHeartbeatTasks {
            generation,
            liveness: SftpHeartbeatLiveness::Watching(receiver),
            tasks: Vec::new(),
        };
        assert!(!owner.has_failed());
        failed.send_replace(true);
        assert!(owner.has_failed());
        actor.transport_lost(generation).unwrap();
        assert_eq!(actor.summary().state, SftpSessionState::Failed);
        assert_eq!(
            actor.summary().failure_code,
            Some(SftpFailureCode::TransportLost)
        );
    }

    #[tokio::test]
    async fn passive_transport_close_is_observed_when_heartbeat_is_disabled() {
        let (mut actor, generation) = actor();
        let (closed, mut close_observer) = watch::channel(false);
        let owner = SftpHeartbeatTasks::start_with_close_future(
            generation,
            async move {
                wait_for_true(&mut close_observer).await;
            },
            None,
            Vec::new(),
        );
        assert!(!owner.has_failed());
        closed.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !owner.has_failed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("passive close observer converges");
        actor.transport_lost(generation).unwrap();
        assert_eq!(actor.summary().state, SftpSessionState::Failed);
        assert_eq!(
            actor.summary().failure_code,
            Some(SftpFailureCode::TransportLost)
        );
    }

    #[tokio::test]
    async fn shared_cleanup_failure_requires_a_parent_close_fact() {
        let directory = tempfile::tempdir().unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let session_id = wire::SftpSessionId::new();
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse(session_id.to_string()).unwrap(),
            wire::HostId::new().to_string(),
        )
        .unwrap();
        let generation = actor.start("base-revision-1").unwrap();
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .unwrap();
        actor.subsystem_opened(generation, "subsystem-1").unwrap();
        actor.transport_lost(generation).unwrap();
        service.records.lock().await.insert(
            session_id.to_string(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: true,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: None,
                open_operation_id: "test-open".to_owned(),
                open_idempotency_key: "test-open-key".to_owned(),
                open_fingerprint: b"test-open".to_vec(),
            },
        );

        let retry = service
            .disconnect(wire::SftpSessionDisconnectRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                operation_id: wire::OperationId::new(),
                idempotency_key: "retry-shared-cleanup".to_owned(),
                session_id: session_id.clone(),
                expected_generation: WireSequence::new(generation.get()),
            })
            .await
            .unwrap();
        assert_eq!(retry.state, wire::SftpSessionState::Failed);
        assert_eq!(
            retry.failure.as_ref().map(|failure| failure.code),
            Some(wire::SftpFailureCode::TransportLost)
        );
        assert_eq!(
            service.exit_blockers().0.as_slice(),
            std::slice::from_ref(&session_id)
        );
        assert!(service.records.lock().await[session_id.as_str()].shared_cleanup_incomplete);

        {
            let mut records = service.records.lock().await;
            let record = records.get_mut(session_id.as_str()).unwrap();
            assert!(
                !close_shared_child_record_after_parent_termination(record, generation, false),
                "a plugin lease cancellation without a closed parent transport must not close the child"
            );
            assert!(record.shared_cleanup_incomplete);
            assert!(close_shared_child_record_after_parent_termination(
                record, generation, true
            ));
        }
        let closed = service
            .wire_summaries()
            .await
            .unwrap()
            .into_iter()
            .find(|summary| summary.session_id == session_id)
            .unwrap();
        assert_eq!(closed.state, wire::SftpSessionState::Closed);
        assert!(service.exit_blockers().0.is_empty());
        assert!(!service.records.lock().await[session_id.as_str()].shared_cleanup_incomplete);
    }

    #[test]
    fn shutdown_failure_is_structured_for_timeout_disconnect_and_residual() {
        let failure = SftpShutdownFailure {
            timed_out_transfers: vec!["transfer-timeout".to_owned()],
            disconnect_failed_sessions: vec!["session-timeout".to_owned()],
            cleanup_residuals: vec![SftpShutdownResidual {
                transfer_id: "transfer-residual".to_owned(),
                opaque_location: "remote temporary file".to_owned(),
            }],
        };
        assert!(matches!(
            failure.into_result(),
            Err(SftpProductionError::ShutdownIncomplete(SftpShutdownFailure {
                timed_out_transfers,
                disconnect_failed_sessions,
                cleanup_residuals,
            })) if timed_out_transfers == ["transfer-timeout"]
                && disconnect_failed_sessions == ["session-timeout"]
                && cleanup_residuals == [SftpShutdownResidual {
                    transfer_id: "transfer-residual".to_owned(),
                    opaque_location: "remote temporary file".to_owned(),
                }]
        ));
        assert!(SftpShutdownFailure::default().into_result().is_ok());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn shutdown_all_timeout_is_actionable_on_the_next_attempt() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().expect("temporary app data");
        let service = SftpSessionService::production(
            HostService::start(directory.path()).expect("host service"),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let (mut actor, generation) = actor();
        let transfer_id = download(&mut actor, generation);
        actor.begin_transfer(&transfer_id, generation).unwrap();
        actor
            .transfer_prepared(
                &transfer_id,
                generation,
                TemporaryTarget::LocalBoundaryToken(format!("local-temp:{}", transfer_id.as_str())),
                false,
                true,
            )
            .unwrap();
        let target = directory.path().join("download.bin");
        let unix =
            NativeLocalBoundary::register(&target, wire::SftpLocalBoundaryKind::DownloadTarget)
                .expect("download boundary");
        let temporary_name = unix.temporary_name(&transfer_id).expect("temporary name");
        let mut temporary = unix
            .create_temporary(&temporary_name)
            .expect("temporary file");
        temporary.write_all(b"partial").expect("partial bytes");
        drop(temporary);
        service.transfer_boundaries.lock().await.insert(
            transfer_id.as_str().to_owned(),
            LocalBoundary {
                kind: wire::SftpLocalBoundaryKind::DownloadTarget,
                display_name: "download.bin".to_owned(),
                size: None,
                capability: Arc::new(LocalBoundaryCapability::Native(unix)),
            },
        );
        service.records.lock().await.insert(
            "session-1".to_owned(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: Some(transfer_id.clone()),
                open_operation_id: "test-open".to_owned(),
                open_idempotency_key: "test-open-key".to_owned(),
                open_fingerprint: b"test-open".to_vec(),
            },
        );
        let (cancel_requested, _) = watch::channel(false);
        let (finished, _) = watch::channel(false);
        service.active_transfers.lock().await.insert(
            transfer_id.as_str().to_owned(),
            ActiveTransferControl {
                cancel_requested,
                finished,
                task: Arc::new(std::sync::Mutex::new(None)),
            },
        );

        let failure = service
            .shutdown_all_with_timeout(Duration::from_millis(1))
            .await
            .expect_err("timed-out transfer must fail application shutdown");
        assert!(matches!(
            failure,
            SftpProductionError::ShutdownIncomplete(SftpShutdownFailure {
                timed_out_transfers,
                cleanup_residuals,
                ..
            }) if timed_out_transfers == [transfer_id.as_str()]
                && cleanup_residuals.iter().any(|residual| {
                    residual.transfer_id == transfer_id.as_str()
                        && residual.opaque_location == "shutdown cleanup timed out"
                })
        ));
        let records = service.records.lock().await;
        let transfer = records["session-1"]
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .expect("preserved transfer record");
        assert_eq!(transfer.state, TransferState::Failed);
        assert_eq!(
            transfer.cleanup_residual.as_deref(),
            Some("shutdown cleanup timed out")
        );
        drop(records);
        assert!(
            !service
                .active_transfers
                .lock()
                .await
                .contains_key(transfer_id.as_str())
        );
        assert!(
            service
                .transfer_boundaries
                .lock()
                .await
                .contains_key(transfer_id.as_str())
        );
        let (_, transfer_blockers) = service.exit_blockers();
        assert_eq!(
            transfer_blockers,
            [wire::TransferId::parse(transfer_id.as_str()).unwrap()]
        );

        tokio::time::timeout(
            Duration::from_millis(250),
            service.shutdown_all_with_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("second shutdown must not wait on the dead control")
        .expect("local residual cleanup succeeds on retry");
        assert!(
            !service
                .transfer_boundaries
                .lock()
                .await
                .contains_key(transfer_id.as_str())
        );
        assert!(!target.exists());
        let records = service.records.lock().await;
        let transfer = records["session-1"]
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .expect("resolved transfer record");
        assert_eq!(transfer.state, TransferState::Cancelled);
        assert!(transfer.cleanup_residual.is_none());
    }

    #[tokio::test]
    async fn explicit_remote_retain_is_idempotent_fenced_and_only_permits_one_exit_attempt() {
        let directory = tempfile::tempdir().expect("temporary app data");
        let service = SftpSessionService::production(
            HostService::start(directory.path()).expect("host service"),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse(wire::SftpSessionId::new().to_string()).unwrap(),
            "host-1",
        )
        .unwrap();
        let generation = actor.start("base-revision-1").unwrap();
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .unwrap();
        actor.subsystem_opened(generation, "subsystem-1").unwrap();
        let transfer_id = TransferId::parse(wire::TransferId::new().to_string()).unwrap();
        actor
            .enqueue_transfer(
                transfer_id.clone(),
                generation,
                TransferDirection::Upload,
                TransferEndpoint::LocalBoundaryToken("picker-source".to_owned()),
                TransferEndpoint::Remote(RemotePath::parse(b"/srv/app.bin".to_vec()).unwrap()),
                8,
                ConflictPolicy::FailIfExists,
            )
            .unwrap();
        actor.begin_transfer(&transfer_id, generation).unwrap();
        actor
            .transfer_prepared(&transfer_id, generation, remote_temp(), false, true)
            .unwrap();
        actor.begin_cancel(&transfer_id, generation).unwrap();
        assert_eq!(
            actor.finish_cancel(
                &transfer_id,
                generation,
                CleanupOutcome::Residual {
                    opaque_location: "shutdown cleanup timed out".to_owned(),
                },
            ),
            Err(SftpRuntimeError::CleanupIncomplete)
        );
        let state_revision = actor.transfers().next().unwrap().state_revision;
        service.records.lock().await.insert(
            "session-1".to_owned(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: None,
                open_operation_id: "test-open".to_owned(),
                open_idempotency_key: "test-open-key".to_owned(),
                open_fingerprint: b"test-open".to_vec(),
            },
        );
        assert_eq!(service.exit_blockers().1.len(), 1);
        let request = wire::SftpRemoteCleanupRetainRequest {
            meta: wire::RequestMeta {
                request_id: wire::RequestId::new(),
            },
            operation_id: wire::OperationId::new(),
            idempotency_key: "retain-once".to_owned(),
            transfer_id: wire::TransferId::parse(transfer_id.as_str()).unwrap(),
            expected_generation: wire::WireSequence::new(generation.get()),
            expected_state_revision: wire::WireSequence::new(state_revision),
            retain_remote_temporary_file_confirmed: true,
        };
        let mut stale = request.clone();
        stale.expected_state_revision = wire::WireSequence::new(state_revision + 1);
        assert!(matches!(
            service.retain_remote_cleanup_for_exit(stale).await,
            Err(SftpProductionError::Runtime(SftpRuntimeError::Conflict))
        ));
        let mut unconfirmed = request.clone();
        unconfirmed.retain_remote_temporary_file_confirmed = false;
        assert!(matches!(
            service.retain_remote_cleanup_for_exit(unconfirmed).await,
            Err(SftpProductionError::Runtime(SftpRuntimeError::InvalidInput))
        ));
        let first = service
            .retain_remote_cleanup_for_exit(request.clone())
            .await
            .expect("explicit retain");
        let replay = service
            .retain_remote_cleanup_for_exit(request.clone())
            .await
            .expect("idempotent replay");
        assert_eq!(first, replay);
        let mut conflicting_replay = request;
        conflicting_replay.idempotency_key = "different-confirmation".to_owned();
        assert!(matches!(
            service
                .retain_remote_cleanup_for_exit(conflicting_replay)
                .await,
            Err(SftpProductionError::Runtime(SftpRuntimeError::Conflict))
        ));
        assert!(first.cleanup_residual.is_some());
        assert_eq!(first.state, wire::SftpTransferState::Failed);
        assert!(service.exit_blockers().1.is_empty());
        service
            .shutdown_all_with_timeout(Duration::from_millis(10))
            .await
            .expect("authorized residual permits this shutdown attempt");
        assert!(service.cleanup_exit_authorizations.lock().await.is_empty());
        assert_eq!(service.exit_blockers().1.len(), 1);
        let records = service.records.lock().await;
        let retained = records["session-1"]
            .actor
            .transfers()
            .find(|transfer| transfer.transfer_id == transfer_id)
            .expect("retained remote residual");
        assert_eq!(retained.state, TransferState::Failed);
        assert!(retained.cleanup_residual.is_some());
        assert!(matches!(
            retained.temporary_target,
            Some(TemporaryTarget::Remote(_))
        ));
    }

    #[test]
    fn non_utf8_remote_cleanup_path_uses_reversible_safe_display() {
        assert_eq!(safe_remote_path_display(&[b'/', 0xff, b'a']), "/\\xffa");
        assert_eq!(safe_remote_path_display(b"/\\xffa\0"), "/\\\\xffa\\x00");
        assert_eq!(
            safe_remote_path_display("/safe\u{202e}txt".as_bytes()),
            "/safe\\xe2\\x80\\xaetxt"
        );
        assert_eq!(safe_remote_path_display(b"/srv/.part"), "/srv/.part");
    }

    #[test]
    fn shutdown_timeout_records_residual_and_never_leaves_cancelling() {
        let (mut actor, generation) = actor();
        let transfer_id = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&transfer_id, generation).unwrap();
        actor
            .transfer_prepared(&transfer_id, generation, remote_temp(), false, true)
            .unwrap();
        actor.begin_cancel(&transfer_id, generation).unwrap();
        assert_eq!(
            actor.finish_cancel(
                &transfer_id,
                generation,
                CleanupOutcome::Residual {
                    opaque_location: "shutdown cleanup timed out".to_owned(),
                },
            ),
            Err(SftpRuntimeError::CleanupIncomplete)
        );
        actor.transport_lost(generation).unwrap();
        let transfer = actor.transfers().next().unwrap();
        assert_eq!(transfer.state, TransferState::Failed);
        assert_eq!(
            transfer.failure_code,
            Some(TransferFailureCode::CleanupIncomplete)
        );
        assert_eq!(actor.summary().state, SftpSessionState::Failed);
    }

    #[test]
    fn old_generation_cannot_operate_reconnected_session() {
        let (mut actor, first) = actor();
        actor.disconnect(first).expect("disconnect");
        actor.closed(first).expect("closed");
        let second = actor.start("base-revision-2").expect("restart");
        assert!(second > first);
        assert_eq!(
            actor.phase_changed(first, SftpSessionState::Authenticating),
            Err(SftpRuntimeError::StaleGeneration)
        );
    }

    #[test]
    fn host_key_mismatch_is_a_hard_failure() {
        let mut actor =
            SftpSessionActor::new(SftpSessionId::parse("session-1").unwrap(), "host-1").unwrap();
        let generation = actor.start("revision").unwrap();
        actor
            .host_key_observed(
                generation,
                "challenge-1",
                SftpRouteStage::Target,
                "ssh-ed25519",
                "SHA256:new",
                Some("SHA256:trusted".to_owned()),
            )
            .unwrap();
        actor.host_key_mismatch(generation).unwrap();
        assert_eq!(actor.summary.state, SftpSessionState::Failed);
        assert_eq!(
            actor.summary.failure_code,
            Some(SftpFailureCode::HostKeyMismatch)
        );
        assert!(actor.summary.active_interaction.is_none());
    }

    #[test]
    fn keyboard_interactive_response_contains_only_unique_refs_and_current_fences() {
        let mut actor =
            SftpSessionActor::new(SftpSessionId::parse("session-1").unwrap(), "host-1").unwrap();
        let generation = actor.start("revision").unwrap();
        actor
            .keyboard_interactive_challenge(KeyboardInteractiveChallenge {
                challenge_id: "challenge-1".to_owned(),
                generation,
                route_stage: SftpRouteStage::Target,
                credential_ref_id: "credential-1".to_owned(),
                attempt_index: 0,
                round_index: 1,
                prompts: vec![KeyboardInteractivePrompt {
                    prompt_index: 0,
                    label: "Password:".to_owned(),
                    echo: false,
                }],
                expires_at_unix_ms: 100,
            })
            .unwrap();
        actor
            .respond_keyboard_interactive(&KeyboardInteractiveResponse {
                challenge_id: "challenge-1".to_owned(),
                generation,
                round_index: 1,
                answer_ref_ids: vec!["answer-ref-1".to_owned()],
            })
            .unwrap();
        assert_eq!(actor.summary.state, SftpSessionState::Authenticating);
        assert!(actor.summary.active_interaction.is_none());
    }

    #[test]
    fn vault_unlock_continuation_is_typed_and_generation_fenced() {
        let mut actor =
            SftpSessionActor::new(SftpSessionId::parse("session-1").unwrap(), "host-1").unwrap();
        let generation = actor.start("revision").unwrap();
        actor
            .require_authentication(generation, AuthenticationRequirement::VaultLocked)
            .unwrap();
        assert_eq!(
            actor.authentication_requirement_satisfied(
                generation,
                AuthenticationRequirement::CredentialUnavailable,
            ),
            Err(SftpRuntimeError::ChallengeMismatch)
        );
        actor
            .authentication_requirement_satisfied(
                generation,
                AuthenticationRequirement::VaultLocked,
            )
            .unwrap();
        assert_eq!(actor.summary.state, SftpSessionState::Authenticating);
    }

    #[test]
    fn listing_and_mutations_require_a_ready_subsystem_and_delete_confirmation() {
        let (actor, generation) = actor();
        assert!(
            actor
                .directory_listing(
                    operation("list"),
                    generation,
                    RemotePath::parse(b"/srv".to_vec()).unwrap(),
                    None,
                    100,
                )
                .is_ok()
        );
        assert_eq!(
            actor.delete(
                operation("delete"),
                generation,
                RemotePath::parse(b"/srv/file".to_vec()).unwrap(),
                DeleteKind::File,
                RemoteObjectPrecondition {
                    kind: RemoteEntryKind::File,
                    size: Some(1),
                    modified_at_unix_ms: Some(1_000),
                },
                false,
            ),
            Err(SftpRuntimeError::InvalidState)
        );
    }

    #[test]
    fn default_conflict_policy_never_overwrites_existing_target() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::default());
        actor.begin_transfer(&id, generation).unwrap();
        assert_eq!(
            actor.transfer_prepared(&id, generation, remote_temp(), true, true),
            Err(SftpRuntimeError::Conflict)
        );
        assert_eq!(actor.transfers[&id].state, TransferState::Failed);
        assert_eq!(
            actor.transfers[&id].failure_code,
            Some(TransferFailureCode::TargetExists)
        );
    }

    #[test]
    fn confirmed_replace_still_requires_proven_safe_commit_support() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::ReplaceSafely);
        actor.begin_transfer(&id, generation).unwrap();
        assert_eq!(
            actor.transfer_prepared(&id, generation, remote_temp(), true, false),
            Err(SftpRuntimeError::UnsafeReplaceUnsupported)
        );
        assert_eq!(
            actor.transfers[&id].failure_code,
            Some(TransferFailureCode::UnsafeReplaceUnsupported)
        );
    }

    #[test]
    fn progress_length_and_atomic_commit_are_all_protocol_facts() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::ReplaceSafely);
        actor.begin_transfer(&id, generation).unwrap();
        actor
            .transfer_prepared(&id, generation, remote_temp(), true, true)
            .unwrap();
        actor.record_progress(&id, generation, 8).unwrap();
        actor.begin_verification(&id, generation, 8).unwrap();
        actor.begin_commit(&id, generation).unwrap();
        actor
            .complete_transfer(
                &id,
                generation,
                CommitFacts {
                    final_length: 8,
                    atomic_no_replace: false,
                    atomic_replace: true,
                },
            )
            .unwrap();
        assert_eq!(actor.transfers[&id].state, TransferState::Completed);
        assert!(actor.transfers[&id].temporary_target.is_none());
        assert_eq!(
            actor.complete_transfer(
                &id,
                generation,
                CommitFacts {
                    final_length: 8,
                    atomic_no_replace: false,
                    atomic_replace: true,
                },
            ),
            Err(SftpRuntimeError::InvalidState)
        );
        assert_eq!(actor.transfers[&id].state, TransferState::Completed);
    }

    #[test]
    fn cancellation_is_not_safe_until_cleanup_is_proven() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&id, generation).unwrap();
        actor
            .transfer_prepared(&id, generation, remote_temp(), false, true)
            .unwrap();
        actor.begin_cancel(&id, generation).unwrap();
        assert_eq!(
            actor.finish_cancel(
                &id,
                generation,
                CleanupOutcome::Residual {
                    opaque_location: "/srv/.app.bin.norishell-part".to_owned(),
                },
            ),
            Err(SftpRuntimeError::CleanupIncomplete)
        );
        assert_eq!(actor.transfers[&id].state, TransferState::Failed);
        assert!(actor.transfers[&id].cleanup_residual.is_some());
    }

    #[test]
    fn failed_transfer_reports_cleanup_residual_instead_of_claiming_safe_failure() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&id, generation).unwrap();
        actor
            .transfer_prepared(&id, generation, remote_temp(), false, true)
            .unwrap();
        assert_eq!(
            actor.fail_transfer_after_cleanup(
                &id,
                generation,
                TransferFailureCode::Protocol,
                CleanupOutcome::Residual {
                    opaque_location: "/srv/.app.bin.norishell-part".to_owned(),
                },
            ),
            Ok(())
        );
        let transfer = actor
            .transfers()
            .find(|transfer| transfer.transfer_id == id)
            .unwrap();
        assert_eq!(transfer.state, TransferState::Failed);
        assert_eq!(
            transfer.failure_code,
            Some(TransferFailureCode::CleanupIncomplete)
        );
        assert_eq!(
            transfer.cleanup_residual.as_deref(),
            Some("/srv/.app.bin.norishell-part")
        );
    }

    #[test]
    fn disconnect_pauses_transfer_and_resume_requires_exact_verified_evidence() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&id, generation).unwrap();
        let temp = remote_temp();
        actor
            .transfer_prepared(&id, generation, temp.clone(), false, true)
            .unwrap();
        actor.record_progress(&id, generation, 4).unwrap();
        actor.transport_lost(generation).unwrap();
        assert_eq!(
            actor.transfers[&id].state,
            TransferState::PausedByDisconnect
        );

        actor.start("revision-2").unwrap();
        let new_generation = actor.summary.generation.unwrap();
        actor
            .authenticated_transport_ready(new_generation, "transport-2")
            .unwrap();
        actor
            .subsystem_opened(new_generation, "subsystem-2")
            .unwrap();
        actor
            .resume_transfer(
                &id,
                new_generation,
                ResumeEvidence {
                    temporary_target: temp,
                    verified_length: 4,
                    prefix_checksum_verified: true,
                },
            )
            .unwrap();
        assert_eq!(actor.transfers[&id].generation, new_generation);
        assert_eq!(actor.transfers[&id].state, TransferState::Transferring);
        assert_eq!(
            actor.record_progress(&id, generation, 5),
            Err(SftpRuntimeError::StaleGeneration)
        );
    }

    #[test]
    fn session_queue_keeps_the_second_transfer_queued_until_the_first_finishes() {
        let (mut actor, generation) = actor();
        let first = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        let second = upload(&mut actor, generation, ConflictPolicy::FailIfExists);

        actor.begin_transfer(&first, generation).unwrap();
        assert_eq!(actor.transfers[&first].state, TransferState::Preparing);
        assert_eq!(actor.transfers[&second].state, TransferState::Queued);
        actor
            .transfer_prepared(&first, generation, remote_temp(), false, true)
            .unwrap();
        actor.record_progress(&first, generation, 8).unwrap();
        actor.begin_verification(&first, generation, 8).unwrap();
        actor.begin_commit(&first, generation).unwrap();
        actor
            .complete_transfer(
                &first,
                generation,
                CommitFacts {
                    final_length: 8,
                    atomic_no_replace: true,
                    atomic_replace: false,
                },
            )
            .unwrap();

        actor.begin_transfer(&second, generation).unwrap();
        assert_eq!(actor.transfers[&first].state, TransferState::Completed);
        assert_eq!(actor.transfers[&second].state, TransferState::Preparing);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scheduler_start_failure_never_leaves_a_transfer_preparing_or_loses_its_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bin");
        std::fs::write(&source, b"12345678").unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let (mut actor, generation) = actor();
        let transfer_id = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        let boundary = LocalBoundary {
            kind: wire::SftpLocalBoundaryKind::UploadSource,
            display_name: "source.bin".to_owned(),
            size: Some(8),
            capability: Arc::new(LocalBoundaryCapability::Native(
                NativeLocalBoundary::register(&source, wire::SftpLocalBoundaryKind::UploadSource)
                    .unwrap(),
            )),
        };
        service
            .transfer_boundaries
            .lock()
            .await
            .insert(transfer_id.as_str().to_owned(), boundary);
        service.records.lock().await.insert(
            "session-1".to_owned(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::from([transfer_id.clone()]),
                active_transfer_id: None,
                open_operation_id: "test-open".to_owned(),
                open_idempotency_key: "test-open-key".to_owned(),
                open_fingerprint: b"test-open".to_vec(),
            },
        );

        service.start_next_queued_transfer("session-1").await;

        let records = service.records.lock().await;
        let record = &records["session-1"];
        let transfer = record.actor.transfers().next().unwrap();
        assert_eq!(transfer.state, TransferState::Failed);
        assert_eq!(transfer.failure_code, Some(TransferFailureCode::Protocol));
        assert!(record.active_transfer_id.is_none());
        assert!(record.pending_transfers.is_empty());
        drop(records);
        assert!(
            service
                .transfer_boundaries
                .lock()
                .await
                .contains_key(transfer_id.as_str())
        );
    }

    #[test]
    fn reconnect_rebinds_a_queued_transfer_but_rejects_old_generation_facts() {
        let (mut actor, first_generation) = actor();
        let queued = upload(&mut actor, first_generation, ConflictPolicy::FailIfExists);
        actor.transport_lost(first_generation).unwrap();
        let second_generation = actor.start("revision-2").unwrap();
        actor
            .authenticated_transport_ready(second_generation, "transport-2")
            .unwrap();
        actor
            .subsystem_opened(second_generation, "subsystem-2")
            .unwrap();

        actor
            .rebind_queued_transfer(&queued, second_generation)
            .unwrap();
        assert_eq!(actor.transfers[&queued].generation, second_generation);
        assert_eq!(
            actor.begin_transfer(&queued, first_generation),
            Err(SftpRuntimeError::StaleGeneration)
        );
        actor.begin_transfer(&queued, second_generation).unwrap();
        assert_eq!(actor.transfers[&queued].state, TransferState::Preparing);
    }

    #[test]
    fn unsupported_handle_stable_commit_rejects_resume_without_committing_a_path_substitute() {
        assert!(!sftp_v3_handle_stable_resume_commit_supported());
        let (mut actor, first_generation) = actor();
        let id = upload(&mut actor, first_generation, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&id, first_generation).unwrap();
        actor
            .transfer_prepared(&id, first_generation, remote_temp(), false, true)
            .unwrap();
        actor.record_progress(&id, first_generation, 4).unwrap();
        actor.transport_lost(first_generation).unwrap();
        let second_generation = actor.start("revision-2").unwrap();
        actor
            .authenticated_transport_ready(second_generation, "transport-2")
            .unwrap();
        actor
            .subsystem_opened(second_generation, "subsystem-2")
            .unwrap();

        actor
            .resume_verification_failed(
                &id,
                CleanupOutcome::Residual {
                    opaque_location: "/srv/.app.bin.norishell-part".to_owned(),
                },
            )
            .unwrap();

        let transfer = &actor.transfers[&id];
        assert_eq!(transfer.state, TransferState::Failed);
        assert_eq!(
            transfer.failure_code,
            Some(TransferFailureCode::CleanupIncomplete)
        );
        assert_eq!(transfer.temporary_target.as_ref(), Some(&remote_temp()));
        assert_eq!(
            transfer.cleanup_residual.as_deref(),
            Some("/srv/.app.bin.norishell-part")
        );
        assert_eq!(
            actor.begin_verification(&id, first_generation, 8),
            Err(SftpRuntimeError::InvalidState)
        );
        assert_eq!(
            actor.begin_verification(&id, second_generation, 8),
            Err(SftpRuntimeError::StaleGeneration)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_a_paused_remote_upload_preserves_the_residual_and_local_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bin");
        std::fs::write(&source, b"12345678").unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let session_id = wire::SftpSessionId::new();
        let transfer_id = wire::TransferId::new();
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse(session_id.to_string()).unwrap(),
            "host-1",
        )
        .unwrap();
        let generation = actor.start("base-revision-1").unwrap();
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .unwrap();
        actor.subsystem_opened(generation, "subsystem-1").unwrap();
        let transfer_id_internal = TransferId::parse(transfer_id.to_string()).unwrap();
        actor
            .enqueue_transfer(
                transfer_id_internal.clone(),
                generation,
                TransferDirection::Upload,
                TransferEndpoint::LocalBoundaryToken("picker-source".to_owned()),
                TransferEndpoint::Remote(RemotePath::parse(b"/srv/app.bin".to_vec()).unwrap()),
                8,
                ConflictPolicy::FailIfExists,
            )
            .unwrap();
        actor
            .begin_transfer(&transfer_id_internal, generation)
            .unwrap();
        actor
            .transfer_prepared(
                &transfer_id_internal,
                generation,
                remote_temp(),
                false,
                true,
            )
            .unwrap();
        actor
            .record_progress(&transfer_id_internal, generation, 4)
            .unwrap();
        actor.transport_lost(generation).unwrap();
        let state_revision = actor.transfers[&transfer_id_internal].state_revision;
        let boundary = LocalBoundary {
            kind: wire::SftpLocalBoundaryKind::UploadSource,
            display_name: "source.bin".to_owned(),
            size: Some(8),
            capability: Arc::new(LocalBoundaryCapability::Native(
                NativeLocalBoundary::register(&source, wire::SftpLocalBoundaryKind::UploadSource)
                    .unwrap(),
            )),
        };
        service
            .transfer_boundaries
            .lock()
            .await
            .insert(transfer_id_internal.as_str().to_owned(), boundary);
        service.records.lock().await.insert(
            session_id.to_string(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: None,
                open_operation_id: "test-open".to_owned(),
                open_idempotency_key: "test-open-key".to_owned(),
                open_fingerprint: b"test-open".to_vec(),
            },
        );

        let summary = service
            .cancel_transfer(wire::SftpTransferActionRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                operation_id: wire::OperationId::new(),
                idempotency_key: "cancel-paused-upload".to_owned(),
                transfer_id: transfer_id.clone(),
                expected_generation: WireSequence::new(generation.get()),
                expected_state_revision: WireSequence::new(state_revision),
            })
            .await
            .unwrap();

        assert_eq!(summary.state, wire::SftpTransferState::Failed);
        assert_eq!(
            summary.failure_code,
            Some(wire::SftpTransferFailureCode::CleanupIncomplete)
        );
        assert!(matches!(
            summary.cleanup_residual,
            Some(wire::SftpCleanupResidual::RemoteTemporaryTarget {
                display_path,
                ..
            }) if display_path == "/srv/.app.bin.norishell-part"
        ));
        assert!(
            service
                .transfer_boundaries
                .lock()
                .await
                .contains_key(transfer_id_internal.as_str())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_a_paused_local_download_removes_the_partial_before_releasing_boundary() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("download.bin");
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let session_id = wire::SftpSessionId::new();
        let transfer_id = wire::TransferId::new();
        let transfer_id_internal = TransferId::parse(transfer_id.to_string()).unwrap();
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse(session_id.to_string()).unwrap(),
            "host-1",
        )
        .unwrap();
        let generation = actor.start("base-revision-1").unwrap();
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .unwrap();
        actor.subsystem_opened(generation, "subsystem-1").unwrap();
        actor
            .enqueue_transfer(
                transfer_id_internal.clone(),
                generation,
                TransferDirection::Download,
                TransferEndpoint::Remote(RemotePath::parse(b"/srv/app.bin".to_vec()).unwrap()),
                TransferEndpoint::LocalBoundaryToken("picker-target".to_owned()),
                8,
                ConflictPolicy::FailIfExists,
            )
            .unwrap();
        actor
            .begin_transfer(&transfer_id_internal, generation)
            .unwrap();
        actor
            .transfer_prepared(
                &transfer_id_internal,
                generation,
                TemporaryTarget::LocalBoundaryToken(format!(
                    "local-temp:{}",
                    transfer_id_internal.as_str()
                )),
                false,
                true,
            )
            .unwrap();
        actor
            .record_progress(&transfer_id_internal, generation, 4)
            .unwrap();
        actor.transport_lost(generation).unwrap();
        let state_revision = actor.transfers[&transfer_id_internal].state_revision;

        let unix =
            NativeLocalBoundary::register(&target, wire::SftpLocalBoundaryKind::DownloadTarget)
                .unwrap();
        let temporary_name = unix.temporary_name(&transfer_id_internal).unwrap();
        let temporary_path = directory
            .path()
            .join(std::ffi::OsStr::from_bytes(temporary_name.to_bytes()));
        let mut temporary = unix.create_temporary(&temporary_name).unwrap();
        temporary.write_all(b"part").unwrap();
        drop(temporary);
        assert!(temporary_path.exists());
        service.transfer_boundaries.lock().await.insert(
            transfer_id_internal.as_str().to_owned(),
            LocalBoundary {
                kind: wire::SftpLocalBoundaryKind::DownloadTarget,
                display_name: "download.bin".to_owned(),
                size: None,
                capability: Arc::new(LocalBoundaryCapability::Native(unix)),
            },
        );
        service.records.lock().await.insert(
            session_id.to_string(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: None,
                open_operation_id: "test-open".to_owned(),
                open_idempotency_key: "test-open-key".to_owned(),
                open_fingerprint: b"test-open".to_vec(),
            },
        );

        let summary = service
            .cancel_transfer(wire::SftpTransferActionRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                operation_id: wire::OperationId::new(),
                idempotency_key: "cancel-paused-download".to_owned(),
                transfer_id,
                expected_generation: WireSequence::new(generation.get()),
                expected_state_revision: WireSequence::new(state_revision),
            })
            .await
            .unwrap();

        assert_eq!(summary.state, wire::SftpTransferState::Cancelled);
        assert!(summary.failure_code.is_none());
        assert!(summary.cleanup_residual.is_none());
        assert!(!temporary_path.exists());
        assert!(
            !service
                .transfer_boundaries
                .lock()
                .await
                .contains_key(transfer_id_internal.as_str())
        );
    }

    #[tokio::test]
    async fn cancelling_a_paused_local_download_without_boundary_preserves_a_local_residual() {
        let directory = tempfile::tempdir().unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let session_id = wire::SftpSessionId::new();
        let transfer_id = wire::TransferId::new();
        let transfer_id_internal = TransferId::parse(transfer_id.to_string()).unwrap();
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse(session_id.to_string()).unwrap(),
            "host-1",
        )
        .unwrap();
        let generation = actor.start("base-revision-1").unwrap();
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .unwrap();
        actor.subsystem_opened(generation, "subsystem-1").unwrap();
        actor
            .enqueue_transfer(
                transfer_id_internal.clone(),
                generation,
                TransferDirection::Download,
                TransferEndpoint::Remote(RemotePath::parse(b"/srv/app.bin".to_vec()).unwrap()),
                TransferEndpoint::LocalBoundaryToken("picker-target".to_owned()),
                8,
                ConflictPolicy::FailIfExists,
            )
            .unwrap();
        actor
            .begin_transfer(&transfer_id_internal, generation)
            .unwrap();
        actor
            .transfer_prepared(
                &transfer_id_internal,
                generation,
                TemporaryTarget::LocalBoundaryToken(format!(
                    "local-temp:{}",
                    transfer_id_internal.as_str()
                )),
                false,
                true,
            )
            .unwrap();
        actor.transport_lost(generation).unwrap();
        let state_revision = actor.transfers[&transfer_id_internal].state_revision;
        service.records.lock().await.insert(
            session_id.to_string(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: None,
                open_operation_id: "test-open".to_owned(),
                open_idempotency_key: "test-open-key".to_owned(),
                open_fingerprint: b"test-open".to_vec(),
            },
        );

        let summary = service
            .cancel_transfer(wire::SftpTransferActionRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                operation_id: wire::OperationId::new(),
                idempotency_key: "cancel-paused-download-missing-boundary".to_owned(),
                transfer_id,
                expected_generation: WireSequence::new(generation.get()),
                expected_state_revision: WireSequence::new(state_revision),
            })
            .await
            .unwrap();

        assert_eq!(summary.state, wire::SftpTransferState::Failed);
        assert_eq!(
            summary.failure_code,
            Some(wire::SftpTransferFailureCode::CleanupIncomplete)
        );
        assert!(matches!(
            summary.cleanup_residual,
            Some(wire::SftpCleanupResidual::LocalTemporaryTarget { .. })
        ));
    }

    #[test]
    fn failed_resume_verification_requires_a_real_restart_and_preserves_residual_facts() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&id, generation).unwrap();
        actor
            .transfer_prepared(&id, generation, remote_temp(), false, true)
            .unwrap();
        actor.record_progress(&id, generation, 4).unwrap();
        actor.transport_lost(generation).unwrap();
        let second = actor.start("revision-2").unwrap();
        actor
            .authenticated_transport_ready(second, "transport-2")
            .unwrap();
        actor.subsystem_opened(second, "subsystem-2").unwrap();

        actor
            .resume_verification_failed(&id, CleanupOutcome::Cleaned)
            .unwrap();
        assert_eq!(actor.transfers[&id].state, TransferState::Failed);
        assert_eq!(actor.transfers[&id].transferred_bytes, 4);
        actor
            .restart_transfer(&id, second, CleanupOutcome::Cleaned)
            .unwrap();
        assert_eq!(actor.transfers[&id].state, TransferState::Queued);
        assert_eq!(actor.transfers[&id].transferred_bytes, 0);

        actor.begin_transfer(&id, second).unwrap();
        actor
            .transfer_prepared(&id, second, remote_temp(), false, true)
            .unwrap();
        actor
            .fail_transfer(&id, second, TransferFailureCode::PermissionDenied)
            .unwrap();
        assert_eq!(
            actor.restart_transfer(
                &id,
                second,
                CleanupOutcome::Residual {
                    opaque_location: "/srv/.app.bin.norishell-part".to_owned(),
                },
            ),
            Err(SftpRuntimeError::CleanupIncomplete)
        );
        assert_eq!(actor.transfers[&id].state, TransferState::Failed);
        assert!(actor.transfers[&id].cleanup_residual.is_some());
    }

    #[test]
    fn remote_temporary_file_must_share_the_final_parent_directory() {
        let (mut actor, generation) = actor();
        let id = upload(&mut actor, generation, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&id, generation).unwrap();
        let wrong_parent = TemporaryTarget::Remote(
            RemotePath::parse(b"/tmp/.app.bin.norishell-part".to_vec()).unwrap(),
        );
        assert_eq!(
            actor.transfer_prepared(&id, generation, wrong_parent, false, true),
            Err(SftpRuntimeError::InvalidInput)
        );
    }

    #[test]
    fn failed_transfer_restart_requires_cleanup_and_rebinds_to_current_generation() {
        let (mut actor, first) = actor();
        let id = upload(&mut actor, first, ConflictPolicy::FailIfExists);
        actor.begin_transfer(&id, first).unwrap();
        actor
            .transfer_prepared(&id, first, remote_temp(), false, true)
            .unwrap();
        actor
            .fail_transfer(&id, first, TransferFailureCode::PermissionDenied)
            .unwrap();
        actor.transport_lost(first).unwrap();
        let second = actor.start("revision-2").unwrap();
        actor
            .authenticated_transport_ready(second, "transport-2")
            .unwrap();
        actor.subsystem_opened(second, "subsystem-2").unwrap();
        assert_eq!(
            actor.restart_transfer(
                &id,
                second,
                CleanupOutcome::Residual {
                    opaque_location: "/srv/.app.bin.norishell-part".to_owned(),
                },
            ),
            Err(SftpRuntimeError::CleanupIncomplete)
        );
        actor
            .restart_transfer(&id, second, CleanupOutcome::Cleaned)
            .unwrap();
        assert_eq!(actor.transfers[&id].generation, second);
        assert_eq!(actor.transfers[&id].state, TransferState::Queued);
        assert_eq!(actor.transfers[&id].transferred_bytes, 0);
    }

    #[test]
    fn non_utf8_remote_path_is_rejected_before_the_string_only_adapter() {
        let path = RemotePath::parse(vec![b'/', 0xff]).expect("opaque remote path");
        assert!(matches!(
            remote_path_utf8(&path),
            Err(SftpProductionError::NonUtf8RemotePath)
        ));
    }

    #[test]
    fn remote_directory_entry_must_be_a_direct_child_of_the_requested_path() {
        let parent = RemotePath::parse(b"/srv/listed".to_vec()).unwrap();
        assert_eq!(
            remote_directory_child_path(&parent, "report.txt").unwrap(),
            RemotePath::parse(b"/srv/listed/report.txt".to_vec()).unwrap()
        );
        for untrusted_name in ["", ".", "..", "../secret", "nested/file"] {
            assert!(remote_directory_child_path(&parent, untrusted_name).is_err());
        }
    }

    #[tokio::test]
    async fn remote_directory_cursor_reuses_authority_for_page_two_intent_entry() {
        let now = std::time::Instant::now();
        let directory_ref = "directory-authority-1".to_owned();
        let mut cursor = RemoteDirectoryCursor {
            directory_ref: directory_ref.clone(),
            session_id: "session-1".to_owned(),
            generation: 7,
            path: RemotePath::parse(b"/srv".to_vec()).unwrap(),
            stream: RemoteDirectoryCursorStream::Fixture(VecDeque::from([
                RemoteDirectoryEntry {
                    path: RemotePath::parse(b"/srv/page-two.bin".to_vec()).unwrap(),
                    kind: RemoteEntryKind::File,
                    size: Some(8),
                    modified_at_unix_ms: Some(1_000),
                    permission_bits: Some(0o600),
                },
                RemoteDirectoryEntry {
                    path: RemotePath::parse(b"/srv/page-three.bin".to_vec()).unwrap(),
                    kind: RemoteEntryKind::File,
                    size: Some(9),
                    modified_at_unix_ms: Some(2_000),
                    permission_bits: Some(0o600),
                },
            ])),
            expires_at: now + DIRECTORY_REFERENCE_TTL,
            pages_read: 1,
            entries_read: 1,
        };

        let page_two = cursor.take_page(1).await.unwrap();
        assert_eq!(page_two[0].path.as_bytes(), b"/srv/page-two.bin");
        assert_eq!(
            (!cursor.is_complete()).then_some(cursor.directory_ref.as_str()),
            Some(directory_ref.as_str())
        );
        let entry = RemoteEntryReference {
            directory_ref: directory_ref.clone(),
            session_id: "session-1".to_owned(),
            generation: 7,
            path: page_two[0].path.clone(),
            name: b"page-two.bin".to_vec(),
            precondition: RemoteObjectPrecondition {
                kind: page_two[0].kind.clone(),
                size: page_two[0].size,
                modified_at_unix_ms: page_two[0].modified_at_unix_ms,
            },
            display_name: "page-two.bin".to_owned(),
            expires_at: now + DIRECTORY_REFERENCE_TTL,
        };
        assert_eq!(
            validate_remote_intent_entry(&entry, &directory_ref, "session-1", 7, 8, now,),
            Ok(())
        );
    }

    #[tokio::test]
    async fn remote_directory_cancel_is_fenced_closes_once_and_replays_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let session_id = wire::SftpSessionId::new();
        service.remote_directory_cursors.lock().await.insert(
            "cursor-1".to_owned(),
            RemoteDirectoryCursor {
                directory_ref: "directory-1".to_owned(),
                session_id: session_id.to_string(),
                generation: 7,
                path: RemotePath::parse(b"/srv".to_vec()).unwrap(),
                stream: RemoteDirectoryCursorStream::Fixture(VecDeque::new()),
                expires_at: std::time::Instant::now() + DIRECTORY_REFERENCE_TTL,
                pages_read: 1,
                entries_read: 0,
            },
        );
        let request = wire::SftpDirectoryListCancelRequest {
            meta: wire::RequestMeta {
                request_id: wire::RequestId::new(),
            },
            operation_id: wire::OperationId::new(),
            idempotency_key: "cancel-directory-1".to_owned(),
            session_id,
            expected_generation: WireSequence::new(7),
            path: wire::SftpRemotePath {
                bytes: b"/srv".to_vec(),
            },
            cursor: b"cursor-1".to_vec(),
        };
        let mut stale = request.clone();
        stale.operation_id = wire::OperationId::new();
        stale.expected_generation = WireSequence::new(8);
        assert!(matches!(
            service.cancel_directory_listing(stale).await,
            Err(SftpProductionError::Runtime(SftpRuntimeError::Conflict))
        ));
        assert!(
            service
                .remote_directory_cursors
                .lock()
                .await
                .contains_key("cursor-1")
        );
        service
            .cancel_directory_listing(request.clone())
            .await
            .unwrap();
        service
            .cancel_directory_listing(request.clone())
            .await
            .unwrap();
        let mut conflicting = request;
        conflicting.idempotency_key = "different".to_owned();
        assert!(matches!(
            service.cancel_directory_listing(conflicting).await,
            Err(SftpProductionError::Runtime(SftpRuntimeError::Conflict))
        ));
    }

    #[tokio::test]
    async fn remote_directory_cancel_interrupts_an_in_flight_page_and_waits_for_close_fact() {
        let directory = tempfile::tempdir().unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let session_id = wire::SftpSessionId::new();
        let (cancel_requested, mut cancel_receiver) = watch::channel(false);
        let (close_result, close_result_receiver) = watch::channel(None);
        service.active_remote_directory_cursors.lock().await.insert(
            "active-cursor".to_owned(),
            ActiveRemoteDirectoryCursor {
                session_id: session_id.to_string(),
                generation: 7,
                path: RemotePath::parse(b"/srv".to_vec()).unwrap(),
                cancel_requested,
                close_result: close_result_receiver,
            },
        );
        let request = wire::SftpDirectoryListCancelRequest {
            meta: wire::RequestMeta {
                request_id: wire::RequestId::new(),
            },
            operation_id: wire::OperationId::new(),
            idempotency_key: "cancel-active-directory".to_owned(),
            session_id,
            expected_generation: WireSequence::new(7),
            path: wire::SftpRemotePath {
                bytes: b"/srv".to_vec(),
            },
            cursor: b"active-cursor".to_vec(),
        };
        let task = tokio::spawn({
            let service = service.clone();
            async move { service.cancel_directory_listing(request).await }
        });
        wait_for_true(&mut cancel_receiver).await;
        assert!(!task.is_finished());
        {
            let _scheduler = service.remote_directory_cursor_scheduler.lock().await;
            service
                .active_remote_directory_cursors
                .lock()
                .await
                .remove("active-cursor");
        }
        close_result.send_replace(Some(true));
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn intent_owned_actor_transfer_has_one_public_projection() {
        let directory = tempfile::tempdir().unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let session_id = wire::SftpSessionId::new();
        let transfer_id = wire::TransferId::new();
        let host_id = wire::HostId::new();
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse(session_id.to_string()).unwrap(),
            host_id.to_string(),
        )
        .unwrap();
        let generation = actor.start("base-revision-1").unwrap();
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .unwrap();
        actor.subsystem_opened(generation, "subsystem-1").unwrap();
        actor
            .enqueue_transfer(
                TransferId::parse(transfer_id.to_string()).unwrap(),
                generation,
                TransferDirection::Upload,
                TransferEndpoint::LocalBoundaryToken("intent-local".to_owned()),
                TransferEndpoint::Remote(RemotePath::parse(b"/srv/file.bin".to_vec()).unwrap()),
                8,
                ConflictPolicy::FailIfExists,
            )
            .unwrap();
        service.records.lock().await.insert(
            session_id.to_string(),
            SftpServiceRecord {
                actor,
                live: None,
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: None,
                open_operation_id: "open-operation".to_owned(),
                open_idempotency_key: "open-key".to_owned(),
                open_fingerprint: b"open".to_vec(),
            },
        );
        service
            .public_intent_transfer_ids
            .lock()
            .await
            .insert(transfer_id.to_string());

        assert!(service.snapshot().await.unwrap().transfers.is_empty());
        service
            .public_intent_transfer_ids
            .lock()
            .await
            .remove(&transfer_id.to_string());
        assert_eq!(service.snapshot().await.unwrap().transfers.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_prepare_replays_one_intent_token() {
        let directory = tempfile::tempdir().unwrap();
        let source_directory = directory.path().join("source");
        let target_directory = directory.path().join("target");
        std::fs::create_dir_all(&source_directory).unwrap();
        std::fs::create_dir_all(&target_directory).unwrap();
        std::fs::write(source_directory.join("source.bin"), b"data").unwrap();
        let service = SftpSessionService::production(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let source = service
            .register_local_directory(wire::SftpLocalDirectoryRegisterRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                selected_path: source_directory.to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        let target = service
            .register_local_directory(wire::SftpLocalDirectoryRegisterRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                selected_path: target_directory.to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        let source_listing = service
            .list_local_directory(wire::SftpLocalDirectoryListRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                operation_id: wire::OperationId::new(),
                idempotency_key: "list-source".to_owned(),
                directory_ref: source.directory_ref.clone(),
                expected_revision: source.revision,
                cursor: None,
                page_size: 8,
            })
            .await
            .unwrap();
        let source_entry = source_listing
            .entries
            .into_iter()
            .find(|entry| entry.display_name == "source.bin")
            .unwrap();
        let request = wire::SftpTransferIntentPrepareRequest {
            meta: wire::RequestMeta {
                request_id: wire::RequestId::new(),
            },
            operation_id: wire::OperationId::new(),
            idempotency_key: "prepare-once".to_owned(),
            source_pane_id: "source-pane".to_owned(),
            target_pane_id: "target-pane".to_owned(),
            source_endpoint_revision: WireSequence::new(1),
            target_endpoint_revision: WireSequence::new(1),
            source: wire::SftpTransferIntentSource::LocalDirectoryEntry {
                directory_ref: source.directory_ref,
                entry_ref: source_entry.entry_ref,
            },
            target: wire::SftpTransferIntentTarget::LocalDirectory {
                directory_ref: target.directory_ref,
            },
            expected_bytes: 4,
            conflict_policy: wire::SftpConflictPolicy::FailIfExists,
        };
        let (first, second) = tokio::join!(
            service.prepare_transfer_intent(request.clone()),
            service.prepare_transfer_intent(request),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.intent_token, second.intent_token);
        assert_eq!(service.prepared_transfer_intents.lock().await.len(), 1);
    }

    #[test]
    fn subsystem_failure_clears_transport_identity_and_fails_the_generation() {
        let mut actor =
            SftpSessionActor::new(SftpSessionId::parse("session-1").unwrap(), "host-1").unwrap();
        let generation = actor.start("revision").unwrap();
        actor
            .authenticated_transport_ready(generation, "transport-1")
            .unwrap();
        actor
            .connection_failed(generation, SftpFailureCode::SubsystemRejected)
            .unwrap();
        assert_eq!(actor.summary.state, SftpSessionState::Failed);
        assert_eq!(
            actor.summary.failure_code,
            Some(SftpFailureCode::SubsystemRejected)
        );
        assert!(actor.summary.transport_id.is_none());
        assert!(actor.summary.subsystem_id.is_none());
    }

    #[test]
    fn transport_failures_map_to_resource_specific_states() {
        assert_eq!(
            map_transport_failure(&TransportError::SftpSubsystemRejected),
            SftpFailureCode::SubsystemRejected
        );
        assert_eq!(
            map_transport_failure(&TransportError::ConnectionLost),
            SftpFailureCode::TransportLost
        );
        assert_eq!(
            map_transport_failure(&TransportError::AuthenticationRejected),
            SftpFailureCode::AuthenticationRejected
        );
    }

    #[test]
    fn javascript_number_file_size_fence_has_an_exact_boundary() {
        assert_eq!(validate_js_safe_transfer_size(0), Ok(()));
        assert_eq!(validate_js_safe_transfer_size(1), Ok(()));
        assert_eq!(validate_js_safe_transfer_size(MAX_JS_SAFE_INTEGER), Ok(()));
        assert_eq!(
            validate_js_safe_transfer_size(MAX_JS_SAFE_INTEGER + 1),
            Err(SftpRuntimeError::InvalidInput)
        );
    }

    #[test]
    fn preview_allowlist_keeps_executable_image_formats_out_of_image_rendering() {
        assert_eq!(preview_kind_for_name("notes.md"), Some(PreviewKind::Text));
        assert_eq!(preview_kind_for_name("Dockerfile"), Some(PreviewKind::Text));
        assert_eq!(
            preview_kind_for_name("photo.webp"),
            Some(PreviewKind::Image("image/webp"))
        );
        assert_eq!(preview_kind_for_name("vector.svg"), Some(PreviewKind::Text));
        assert_eq!(preview_kind_for_name("archive.zip"), None);
    }

    #[test]
    fn extensionless_nginx_sites_require_direct_standard_directory_entries() {
        let kind = |path: &[u8]| {
            preview_kind_for_entry(&RemotePath::parse(path.to_vec()).unwrap(), "default")
        };
        assert_eq!(
            kind(b"/etc/nginx/sites-available/default"),
            Some(PreviewKind::Text)
        );
        assert_eq!(
            kind(b"/etc/nginx/sites-enabled/example.com"),
            Some(PreviewKind::Text)
        );
        assert_eq!(kind(b"/srv/default"), None);
        assert_eq!(kind(b"/etc/nginx/sites-available/"), None);
        assert_eq!(kind(b"/etc/nginx/sites-available/."), None);
        assert_eq!(kind(b"/etc/nginx/sites-enabled/.."), None);
        assert_eq!(kind(b"/etc/nginx/sites-enabled/sub/default"), None);
        assert_eq!(kind(b"/etc/nginx/sites-enabled/def\x1fault"), None);
    }

    #[test]
    fn text_edit_plan_is_bounded_and_uses_a_same_directory_temporary_path() {
        let (actor, generation) = actor();
        let operation = OperationFence::parse("edit-operation", "edit-key").unwrap();
        let target = RemotePath::parse(b"/srv/app/config.toml".to_vec()).unwrap();
        let precondition = RemoteObjectPrecondition {
            kind: RemoteEntryKind::File,
            size: Some(4),
            modified_at_unix_ms: Some(10),
        };
        let plan = actor
            .write_text(
                operation,
                generation,
                target.clone(),
                precondition,
                "name = 'nori'\n".to_owned(),
            )
            .unwrap();
        assert!(matches!(plan, FileMutationPlan::WriteText { .. }));
        assert_eq!(
            remote_edit_temporary_path(&target, "edit-operation")
                .unwrap()
                .as_bytes(),
            b"/srv/app/.config.toml.norishell-edit-edit-operation.part"
        );
        assert!(matches!(
            actor.write_text(
                OperationFence::parse("too-large", "too-large-key").unwrap(),
                generation,
                target,
                RemoteObjectPrecondition {
                    kind: RemoteEntryKind::File,
                    size: Some(4),
                    modified_at_unix_ms: Some(10),
                },
                "x".repeat(MAX_TEXT_PREVIEW_BYTES + 1),
            ),
            Err(SftpRuntimeError::InvalidInput)
        ));
    }

    #[test]
    fn text_preview_preserves_detected_line_endings() {
        assert_eq!(
            detect_text_line_ending(b"a\r\nb\r\n"),
            wire::SftpTextLineEnding::CrLf
        );
        assert_eq!(
            detect_text_line_ending(b"a\rb\r"),
            wire::SftpTextLineEnding::Cr
        );
        assert_eq!(
            detect_text_line_ending(b"a\nb\n"),
            wire::SftpTextLineEnding::Lf
        );
    }

    #[test]
    fn tail_window_is_incremental_and_resets_near_the_end_after_truncation() {
        assert_eq!(tail_read_window(100, 80), (80, 20, false));
        assert_eq!(
            tail_read_window((MAX_TAIL_CHUNK_BYTES as u64) + 10, 0),
            (0, MAX_TAIL_CHUNK_BYTES as u64, false)
        );
        assert_eq!(tail_read_window(20, 80), (0, 20, true));
        assert_eq!(tail_read_window(20, 20), (20, 0, false));
        assert_eq!(tail_read_window_limited(100, 40, 16), (40, 16, false));
    }

    #[test]
    fn image_preview_requires_matching_magic_bytes() {
        assert_eq!(
            image_media_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(
            image_media_type(&[0xff, 0xd8, 0xff, 0xe0]),
            Some("image/jpeg")
        );
        assert_eq!(image_media_type(b"GIF89arest"), Some("image/gif"));
        assert_eq!(
            image_media_type(b"RIFF\x04\x00\x00\x00WEBPrest"),
            Some("image/webp")
        );
        assert_eq!(image_media_type(b"<svg><script/></svg>"), None);
    }

    #[derive(Default)]
    struct BackpressureWriter {
        bytes: Vec<u8>,
        maximum_offered: usize,
        pending_count: usize,
        should_pend: bool,
    }

    impl AsyncWrite for BackpressureWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<io::Result<usize>> {
            self.maximum_offered = self.maximum_offered.max(bytes.len());
            if !self.should_pend {
                self.should_pend = true;
                self.pending_count += 1;
                context.waker().wake_by_ref();
                return std::task::Poll::Pending;
            }
            self.should_pend = false;
            let accepted = bytes.len().min(17);
            self.bytes.extend_from_slice(&bytes[..accepted]);
            std::task::Poll::Ready(Ok(accepted))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn remote_copy_stream_is_fixed_buffered_and_target_backpressured() {
        let input = vec![0x5a; TRANSFER_CHUNK_BYTES * 3 + 11];
        let mut source = std::io::Cursor::new(input.clone());
        let mut target = BackpressureWriter::default();
        let mut progress = Vec::new();
        let copied = remote_copy::copy_bounded_stream(
            &mut source,
            &mut target,
            u64::try_from(input.len()).unwrap(),
            |value| {
                progress.push(value);
                std::future::ready(())
            },
        )
        .await
        .unwrap();
        assert_eq!(copied, u64::try_from(input.len()).unwrap());
        assert_eq!(target.bytes, input);
        assert!(target.maximum_offered <= TRANSFER_CHUNK_BYTES);
        assert!(target.pending_count > 1);
        assert_eq!(progress.last(), Some(&copied));
    }

    #[test]
    fn interrupted_remote_copy_retains_residual_and_never_claims_clean_cancel() {
        let transfer_id = TransferId::parse(wire::TransferId::new().to_string()).unwrap();
        let plan = RemoteCopyPlan {
            transfer_id,
            source_session_key: wire::SftpSessionId::new().to_string(),
            source_generation: SftpGeneration::new(3).unwrap(),
            source_path: RemotePath::parse(b"/source.bin".to_vec()).unwrap(),
            source_precondition: RemoteObjectPrecondition {
                kind: RemoteEntryKind::File,
                size: Some(8),
                modified_at_unix_ms: Some(11),
            },
            target_session_key: wire::SftpSessionId::new().to_string(),
            target_generation: SftpGeneration::new(7).unwrap(),
            target_path: RemotePath::parse(b"/target.bin".to_vec()).unwrap(),
            expected_bytes: 8,
            conflict_policy: ConflictPolicy::FailIfExists,
        };

        let (transferring_outcome, transferring_residual) =
            remote_copy_interruption_resolution(&plan, Some(wire::SftpTransferState::Transferring));
        assert_eq!(
            transferring_outcome,
            wire::SftpTransferCommitOutcome::NotCommitted
        );
        assert!(transferring_residual.is_some());

        let (committing_outcome, committing_residual) =
            remote_copy_interruption_resolution(&plan, Some(wire::SftpTransferState::Committing));
        assert_eq!(
            committing_outcome,
            wire::SftpTransferCommitOutcome::Uncertain
        );
        assert!(committing_residual.is_some());
    }
}
