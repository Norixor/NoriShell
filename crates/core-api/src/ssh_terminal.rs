use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AlgorithmCategory, CredentialRefId, HostId, OperationId, PluginApprovalId, RequestMeta,
    SshAttachAttemptId, SshAttachmentId, SshChannelId, SshHostKeyChallengeId, SshInputLeaseId,
    SshKeyboardInteractiveAnswerRefId, SshKeyboardInteractiveChallengeId, SshOpenAttemptId,
    SshSessionId, SshViewId, WireSequence,
};

pub const SSH_TERMINAL_EVENT_SCHEMA_VERSION: u16 = 5;
pub const SSH_TERMINAL_OUTPUT_FRAME_MAX_BYTES: usize = 64 * 1024;

/// Connection lifecycle owned exclusively by the SSH Session actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionState {
    Resolving,
    Connecting,
    VerifyingHostKey,
    AwaitingHostKeyDecision,
    Authenticating,
    OpeningChannel,
    AutomatingLogin,
    Running,
    Disconnecting,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionFailureStage {
    Resolving,
    Connecting,
    RouteIngress,
    HostKeyVerification,
    Authentication,
    ChannelOpen,
    Running,
    Disconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionFailureCode {
    ResolutionFailed,
    ConnectionRefused,
    ConnectionTimedOut,
    ProxyConnectionFailed,
    ProxyRejected,
    ProxyProtocolError,
    ProxyCredentialLocked,
    ProxyCredentialUnavailable,
    JumpChannelFailed,
    HostKeyRejected,
    HostKeyMismatch,
    HostKeyDecisionExpired,
    CredentialUnavailable,
    AuthenticationRejected,
    AuthenticationFailed,
    SshAgentKeyUnavailable,
    SshAgentUnavailable,
    AlgorithmNegotiationFailed,
    ChannelOpenFailed,
    ConnectionLost,
    ProtocolError,
    DisconnectTimedOut,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionRetryStrategy {
    Never,
    RetryOpen,
    RefreshSession,
    UnlockVault,
    ChooseCredential,
    ReviewHostKey,
}

/// User-safe failure facts. Raw transport errors and secret material never
/// cross the wire; `diagnostic_id` may be correlated with redacted Core logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionFailureReason {
    pub code: SshSessionFailureCode,
    pub stage: SshSessionFailureStage,
    /// Location within the resolved route at which the failure occurred.
    /// This never contains credentials or proxy authentication material.
    pub route_stage: Option<SshSessionRouteStage>,
    /// Present only for algorithm negotiation failures. Both lists are
    /// bounded, de-duplicated protocol names and contain no authentication material.
    #[serde(default)]
    pub algorithm_negotiation: Option<SshAlgorithmNegotiationFailure>,
    pub retry_strategy: SshSessionRetryStrategy,
    pub message_key: String,
    pub diagnostic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshAlgorithmNegotiationFailure {
    pub category: AlgorithmCategory,
    pub client_candidates: Vec<String>,
    pub server_candidates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshSessionCloseReason {
    UserRequested,
    LastAttachmentConfirmed,
    ApplicationExit,
    RemoteEof,
    RemoteExitStatus { exit_status: u32 },
    RemoteExitSignal { signal_name: String },
}

/// Canonical endpoint used for host-key display and trust lookup. The address
/// remains the user-selected DNS name or bracket-free IP literal, never a DNS
/// result substituted by Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionEndpoint {
    pub address: String,
    pub port: u16,
    pub username: Option<String>,
}

/// A connection-only SSH verification. It never opens a PTY, Shell, Terminal
/// Session, Pane, attachment, or workspace Tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectionTestRequest {
    pub meta: RequestMeta,
    pub endpoint: SshSessionEndpoint,
    pub credential_ref_id: CredentialRefId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectionTestResponse {
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshSessionRouteStage {
    Ingress,
    JumpHost {
        hop_index: u8,
        host_id: HostId,
        endpoint: SshSessionEndpoint,
    },
    Target,
}

/// Secret-free handshake facts for one exact route stage and policy snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshNegotiatedAlgorithms {
    pub route_stage: SshSessionRouteStage,
    pub policy_id: String,
    pub policy_revision: Option<WireSequence>,
    pub policy_catalog_version: String,
    pub key_exchange: String,
    pub host_key: String,
    pub cipher_client_to_server: String,
    pub cipher_server_to_client: String,
    pub mac_client_to_server: String,
    pub mac_server_to_client: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshSessionTarget {
    QuickConnect {
        endpoint: SshSessionEndpoint,
    },
    Host {
        host_id: HostId,
        expected_host_state_version: WireSequence,
    },
}

/// Secret-free, rebuildable projection of one SSH terminal session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionSummary {
    pub session_id: SshSessionId,
    pub open_attempt_id: SshOpenAttemptId,
    pub target: SshSessionTarget,
    pub credential_ref_id: Option<CredentialRefId>,
    pub endpoint: Option<SshSessionEndpoint>,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    pub event_seq: WireSequence,
    pub channel_id: Option<SshChannelId>,
    pub negotiated_algorithms: Vec<SshNegotiatedAlgorithms>,
    pub state: SshSessionState,
    pub close_reason: Option<SshSessionCloseReason>,
    pub failure_reason: Option<SshSessionFailureReason>,
    pub attachment_count: u32,
    #[ts(type = "number")]
    pub created_at_unix_ms: i64,
    #[ts(type = "number")]
    pub updated_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionAttachment {
    pub attachment_id: SshAttachmentId,
    pub attach_attempt_id: SshAttachAttemptId,
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub channel_id: Option<SshChannelId>,
    pub view_id: SshViewId,
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    #[ts(type = "number")]
    pub attached_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionInputLease {
    pub lease_id: SshInputLeaseId,
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    /// Epoch of the process-wide terminal input focus broker that issued this
    /// lease. Future LocalTerminalSession targets must join the same broker.
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

/// One exact writable SSH Pane target for the process-wide terminal input
/// focus broker. LocalTerminalSession will use the same top-level broker when
/// it is implemented rather than introducing a second ownership domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshTerminalInputFocusTarget {
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshTerminalInputFocusSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshTerminalInputFocusSnapshot {
    pub focus_epoch: WireSequence,
    pub target: Option<SshTerminalInputFocusTarget>,
    pub lease: Option<SshSessionInputLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshTerminalInputFocusChangeRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub expected_focus_epoch: WireSequence,
    /// `None` atomically clears the old target for a blank Tab, blocking
    /// Dialog, or workspace with no writable Pane.
    pub target: Option<SshTerminalInputFocusTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshTerminalInputFocusChangeResponse {
    pub focus_epoch: WireSequence,
    pub target: Option<SshTerminalInputFocusTarget>,
    pub lease: Option<SshSessionInputLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionOpenRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub open_attempt_id: SshOpenAttemptId,
    pub attach_attempt_id: SshAttachAttemptId,
    pub target: SshSessionTarget,
    /// A saved authentication choice is represented only by CredentialRefId.
    /// SecretRefId and plaintext authentication material are not accepted.
    pub credential_ref_id: Option<CredentialRefId>,
    /// One-time Core-owned authorization emitted only after a protected plugin
    /// Host session approval. Normal user launches always send `None`.
    pub plugin_authorization_token: Option<PluginApprovalId>,
    pub view_id: SshViewId,
    pub rows: u16,
    pub cols: u16,
}

/// Open is accepted asynchronously: this response only proves that Core
/// created the session and first attachment projection at `state_revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionOpenResponse {
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub open_attempt_id: SshOpenAttemptId,
    pub state_revision: WireSequence,
    pub session: SshSessionSummary,
    pub attachment: SshSessionAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionSnapshotRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionSnapshot {
    pub snapshot_revision: WireSequence,
    pub sessions: Vec<SshSessionSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshHeartbeatMode {
    Disabled,
    TransportKeepalive,
    ShellHeartbeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshShellHeartbeatSkipReason {
    UserActive,
    WriterBusy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshTransportHeartbeatStatus {
    pub route_stage: SshSessionRouteStage,
    #[ts(type = "number | null")]
    pub next_due_at_unix_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub last_sent_at_unix_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub last_ack_at_unix_ms: Option<i64>,
    pub consecutive_failures: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshShellHeartbeatStatus {
    #[ts(type = "number | null")]
    pub next_due_at_unix_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub last_sent_at_unix_ms: Option<i64>,
    pub skip_reason: Option<SshShellHeartbeatSkipReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionHeartbeatStatus {
    pub policy_revision: Option<WireSequence>,
    pub mode: SshHeartbeatMode,
    pub transports: Vec<SshTransportHeartbeatStatus>,
    pub shell: Option<SshShellHeartbeatStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionGetRequest {
    pub meta: RequestMeta,
    pub session_id: SshSessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionDetails {
    pub session: SshSessionSummary,
    pub attachments: Vec<SshSessionAttachment>,
    pub active_host_key_challenge: Option<SshHostKeyChallenge>,
    pub active_keyboard_interactive_challenge: Option<SshKeyboardInteractiveChallenge>,
    pub active_login_automation: Option<SshLoginAutomationProgress>,
    pub heartbeat: SshSessionHeartbeatStatus,
    pub input_lease: Option<SshSessionInputLease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshLoginAutomationStepKind {
    Expect,
    SendText,
    SendSecret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshLoginAutomationStatus {
    Running,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshLoginAutomationFailureCode {
    StepTimeout,
    TotalTimeout,
    MatchBufferExceeded,
    AttachmentUnavailable,
    VaultLocked,
    SecretUnavailable,
    ChannelUnavailable,
}

/// Secret-free projection of the automation actor for one exact Shell Channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshLoginAutomationProgress {
    pub policy_revision: WireSequence,
    pub current_step_index: u8,
    pub total_steps: u8,
    pub step_kind: SshLoginAutomationStepKind,
    pub secret_label: Option<String>,
    pub status: SshLoginAutomationStatus,
    pub failure_code: Option<SshLoginAutomationFailureCode>,
    #[ts(type = "number")]
    pub started_at_unix_ms: i64,
    #[ts(type = "number")]
    pub step_deadline_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionAttachRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub attach_attempt_id: SshAttachAttemptId,
    pub view_id: SshViewId,
    pub after_output_seq: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionAttachResponse {
    pub state_revision: WireSequence,
    pub attachment_revision: WireSequence,
    pub attachment: SshSessionAttachment,
    pub replay: Vec<SshSessionOutputItem>,
}

/// Refreshes the renderer binding deadline for one exact attachment. This is
/// independent from input ownership: read-only or background views must remain
/// observable even when they do not hold the session input lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionAttachmentHeartbeatRequest {
    pub meta: RequestMeta,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_attachment_revision: WireSequence,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionDetachIntent {
    /// A user-visible Pane close. The existing last-attachment confirmation
    /// contract remains mandatory for an active session.
    #[default]
    UserClose,
    /// The renderer or component binding is going away. Core removes only the
    /// attachment and its lease; it never disconnects the remote Shell.
    RendererUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionLastDetachAction {
    KeepAttached,
    DisconnectAndDetach,
}

/// A second detach call may carry this confirmation only after Core returned
/// `ConfirmationRequired`. Both revisions must still match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionLastDetachConfirmation {
    pub action: SshSessionLastDetachAction,
    pub expected_state_revision: WireSequence,
    pub expected_attachment_revision: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionDetachRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    #[serde(default)]
    pub intent: SshSessionDetachIntent,
    pub confirmation: Option<SshSessionLastDetachConfirmation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshSessionDetachResult {
    Detached {
        session: SshSessionSummary,
        remaining_attachment_count: u32,
    },
    ConfirmationRequired {
        session: SshSessionSummary,
        expected_state_revision: WireSequence,
        expected_attachment_revision: WireSequence,
    },
    Disconnecting {
        session: SshSessionSummary,
    },
    DisconnectedAndDetached {
        session: SshSessionSummary,
    },
    KeptAttached {
        session: SshSessionSummary,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshHostKeyChallenge {
    pub challenge_id: SshHostKeyChallengeId,
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub endpoint: SshSessionEndpoint,
    pub key_algorithm: String,
    pub fingerprint_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshHostKeyDecision {
    AcceptAndStore,
    Reject,
}

/// A host-key decision is fenced to the exact session view and challenge.
/// The server public key blob is retained by Core and is deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshHostKeyDecisionRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub challenge_id: SshHostKeyChallengeId,
    pub expected_state_revision: WireSequence,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub decision: SshHostKeyDecision,
}

/// One untrusted server prompt. `text` is display-only and must never be
/// interpreted as markup. Non-echo prompts always require a one-time Core ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyboardInteractivePrompt {
    pub prompt_index: u8,
    pub text: String,
    pub echo: bool,
    pub sensitive: bool,
}

/// A bounded keyboard-interactive round owned by the SSH Session actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyboardInteractiveChallenge {
    pub challenge_id: SshKeyboardInteractiveChallengeId,
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub route_stage: SshSessionRouteStage,
    pub credential_ref_id: CredentialRefId,
    pub attempt_index: u8,
    pub round_index: u8,
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<SshKeyboardInteractivePrompt>,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

/// Plain answers are accepted only for an echo prompt. Sensitive answers are
/// represented by an exact, one-time Core reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshKeyboardInteractiveAnswerInput {
    EchoText {
        prompt_index: u8,
        value: String,
    },
    OneTimeAnswerRef {
        prompt_index: u8,
        answer_ref_id: SshKeyboardInteractiveAnswerRefId,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyboardInteractiveAnswerPrepareRequest {
    pub meta: RequestMeta,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub challenge_id: SshKeyboardInteractiveChallengeId,
    pub expected_state_revision: WireSequence,
    pub round_index: u8,
    pub prompt_index: u8,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub answer: String,
}

impl fmt::Debug for SshKeyboardInteractiveAnswerPrepareRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshKeyboardInteractiveAnswerPrepareRequest")
            .field("meta", &self.meta)
            .field("session_id", &self.session_id)
            .field("expected_generation", &self.expected_generation)
            .field("challenge_id", &self.challenge_id)
            .field("expected_state_revision", &self.expected_state_revision)
            .field("round_index", &self.round_index)
            .field("prompt_index", &self.prompt_index)
            .field("attachment_id", &self.attachment_id)
            .field("view_id", &self.view_id)
            .field("answer", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyboardInteractiveAnswerPrepareResponse {
    pub answer_ref_id: SshKeyboardInteractiveAnswerRefId,
    #[ts(type = "number")]
    pub expires_at_unix_ms: i64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyboardInteractiveResponseRequest {
    pub meta: RequestMeta,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub challenge_id: SshKeyboardInteractiveChallengeId,
    pub expected_state_revision: WireSequence,
    pub round_index: u8,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub answers: Vec<SshKeyboardInteractiveAnswerInput>,
}

impl fmt::Debug for SshKeyboardInteractiveResponseRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshKeyboardInteractiveResponseRequest")
            .field("meta", &self.meta)
            .field("session_id", &self.session_id)
            .field("expected_generation", &self.expected_generation)
            .field("challenge_id", &self.challenge_id)
            .field("expected_state_revision", &self.expected_state_revision)
            .field("round_index", &self.round_index)
            .field("attachment_id", &self.attachment_id)
            .field("view_id", &self.view_id)
            .field("answer_count", &self.answers.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionInputLeaseAcquireRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionInputLeaseRenewRequest {
    pub meta: RequestMeta,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub focus_epoch: WireSequence,
    pub lease_id: SshInputLeaseId,
    pub input_epoch: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionInputRequest {
    pub meta: RequestMeta,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub focus_epoch: WireSequence,
    pub lease_id: SshInputLeaseId,
    pub input_epoch: WireSequence,
    pub client_seq: WireSequence,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionResizeRequest {
    pub meta: RequestMeta,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub focus_epoch: WireSequence,
    pub lease_id: SshInputLeaseId,
    pub input_epoch: WireSequence,
    pub resize_seq: WireSequence,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionDisconnectRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
}

/// Stops the remaining automation steps and hands the already-open Shell to
/// one exact visible attachment. It never replays or redirects queued bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshLoginAutomationTakeoverRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub expected_focus_epoch: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshLoginAutomationTakeoverResponse {
    pub details: SshSessionDetails,
    pub focus: SshTerminalInputFocusChangeResponse,
}

/// Starts a fresh SSH transport and Shell Channel for an existing Pane while
/// preserving its view and scrollback. The replacement credential is required
/// for one-time Quick Connect sessions and may be omitted for a saved Host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionReconnectRequest {
    pub meta: RequestMeta,
    pub operation_id: OperationId,
    pub idempotency_key: String,
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub credential_ref_id: Option<CredentialRefId>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionOutputGapReason {
    RingBufferOverflow,
    AttachBufferOverflow,
    SequenceUnavailable,
}

/// Raw SSH Channel bytes. Consumers feed `bytes` directly into xterm and
/// order them by generation, channel and output sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionOutputFrame {
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub channel_id: SshChannelId,
    pub output_seq: WireSequence,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionOutputGap {
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub channel_id: SshChannelId,
    pub dropped_from_output_seq: WireSequence,
    pub resumes_at_output_seq: WireSequence,
    pub reason: SshSessionOutputGapReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
pub enum SshSessionOutputItem {
    Frame(SshSessionOutputFrame),
    Gap(SshSessionOutputGap),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionAttachmentChange {
    Attached,
    Detached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SshSessionInputLeaseChange {
    Acquired,
    Renewed,
    Released,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshSessionEventPayload {
    StateChanged {
        previous_state: SshSessionState,
        state: SshSessionState,
        close_reason: Option<SshSessionCloseReason>,
        failure_reason: Option<SshSessionFailureReason>,
    },
    HostKeyChallenge {
        challenge: SshHostKeyChallenge,
    },
    KeyboardInteractiveChallengeChanged {
        challenge: Option<SshKeyboardInteractiveChallenge>,
    },
    LoginAutomationProgressChanged {
        progress: Option<SshLoginAutomationProgress>,
    },
    HeartbeatChanged {
        heartbeat: SshSessionHeartbeatStatus,
    },
    NegotiatedAlgorithmsChanged {
        algorithms: Vec<SshNegotiatedAlgorithms>,
    },
    AttachmentChanged {
        change: SshSessionAttachmentChange,
        attachment_revision: WireSequence,
        attachment: SshSessionAttachment,
    },
    InputLeaseChanged {
        change: SshSessionInputLeaseChange,
        lease: Option<SshSessionInputLease>,
    },
    OutputFrame {
        frame: SshSessionOutputFrame,
    },
    OutputGap {
        gap: SshSessionOutputGap,
    },
}

/// Ordered per-session event. Every variant carries the same session fences so
/// a delayed event can never be applied to a newer connection generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SshSessionEvent {
    pub schema_version: u16,
    pub session_id: SshSessionId,
    pub generation: WireSequence,
    pub state_revision: WireSequence,
    pub event_seq: WireSequence,
    #[ts(type = "number")]
    pub occurred_at_unix_ms: i64,
    pub payload: SshSessionEventPayload,
}

#[cfg(test)]
mod tests {
    use super::{
        SSH_TERMINAL_EVENT_SCHEMA_VERSION, SshHostKeyChallenge, SshHostKeyDecision,
        SshHostKeyDecisionRequest, SshKeyboardInteractiveAnswerInput,
        SshKeyboardInteractiveAnswerPrepareRequest, SshKeyboardInteractiveResponseRequest,
        SshSessionEndpoint, SshSessionEvent, SshSessionEventPayload, SshSessionFailureCode,
        SshSessionFailureReason, SshSessionFailureStage, SshSessionOutputFrame,
        SshSessionRetryStrategy, SshSessionRouteStage, SshSessionState,
    };
    use crate::{
        HostId, OperationId, RequestId, RequestMeta, SshAttachmentId, SshChannelId,
        SshHostKeyChallengeId, SshKeyboardInteractiveAnswerRefId,
        SshKeyboardInteractiveChallengeId, SshSessionId, SshViewId, WireSequence,
    };

    #[test]
    fn output_event_round_trips_raw_bytes_and_string_sequences() {
        let session_id = SshSessionId::new();
        let generation = WireSequence::new(u64::MAX - 1);
        let event = SshSessionEvent {
            schema_version: SSH_TERMINAL_EVENT_SCHEMA_VERSION,
            session_id: session_id.clone(),
            generation,
            state_revision: WireSequence::new(9),
            event_seq: WireSequence::new(u64::MAX),
            occurred_at_unix_ms: 1,
            payload: SshSessionEventPayload::OutputFrame {
                frame: SshSessionOutputFrame {
                    session_id,
                    generation,
                    channel_id: SshChannelId::new(),
                    output_seq: WireSequence::new(u64::MAX),
                    bytes: vec![0, 0xff, 0x1b, b'[', b'm'],
                },
            },
        };

        let encoded = serde_json::to_string(&event).expect("serialize SSH event");
        assert!(encoded.contains(&format!("\"eventSeq\":\"{}\"", u64::MAX)));
        assert!(encoded.contains("\"bytes\":[0,255,27,91,109]"));
        let decoded: SshSessionEvent =
            serde_json::from_str(&encoded).expect("deserialize SSH event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn host_key_decision_contains_only_challenge_identity_and_fences() {
        let request = SshHostKeyDecisionRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "accept-once".to_owned(),
            session_id: SshSessionId::new(),
            expected_generation: WireSequence::new(3),
            challenge_id: SshHostKeyChallengeId::new(),
            expected_state_revision: WireSequence::new(8),
            attachment_id: SshAttachmentId::new(),
            view_id: SshViewId::new(),
            decision: SshHostKeyDecision::AcceptAndStore,
        };

        let encoded = serde_json::to_value(&request).expect("serialize decision");
        assert!(encoded.get("challengeId").is_some());
        assert!(encoded.get("publicKeyBase64").is_none());
        assert!(encoded.get("keyBlob").is_none());

        let challenge = SshHostKeyChallenge {
            challenge_id: request.challenge_id,
            session_id: request.session_id,
            generation: request.expected_generation,
            state_revision: request.expected_state_revision,
            endpoint: super::SshSessionEndpoint {
                address: "example.test".to_owned(),
                port: 22,
                username: Some("user".to_owned()),
            },
            key_algorithm: "ssh-ed25519".to_owned(),
            fingerprint_sha256: "SHA256:display-only".to_owned(),
        };
        let encoded = serde_json::to_string(&challenge).expect("serialize challenge");
        assert!(encoded.contains("example.test"));
        assert!(encoded.contains("ssh-ed25519"));
        assert!(encoded.contains("SHA256:display-only"));
    }

    #[test]
    fn keyboard_interactive_request_debug_output_redacts_all_answer_values() {
        let session_id = SshSessionId::new();
        let challenge_id = SshKeyboardInteractiveChallengeId::new();
        let attachment_id = SshAttachmentId::new();
        let view_id = SshViewId::new();
        let prepare = SshKeyboardInteractiveAnswerPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: session_id.clone(),
            expected_generation: WireSequence::new(4),
            challenge_id: challenge_id.clone(),
            expected_state_revision: WireSequence::new(9),
            round_index: 2,
            prompt_index: 0,
            attachment_id: attachment_id.clone(),
            view_id: view_id.clone(),
            answer: "secret-one-time-code".to_owned(),
        };
        let debug = format!("{prepare:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-one-time-code"));

        let response = SshKeyboardInteractiveResponseRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id,
            expected_generation: WireSequence::new(4),
            challenge_id,
            expected_state_revision: WireSequence::new(9),
            round_index: 2,
            attachment_id,
            view_id,
            answers: vec![
                SshKeyboardInteractiveAnswerInput::EchoText {
                    prompt_index: 0,
                    value: "visible-account".to_owned(),
                },
                SshKeyboardInteractiveAnswerInput::OneTimeAnswerRef {
                    prompt_index: 1,
                    answer_ref_id: SshKeyboardInteractiveAnswerRefId::new(),
                },
            ],
        };
        let debug = format!("{response:?}");
        assert!(debug.contains("answer_count"));
        assert!(!debug.contains("visible-account"));
    }

    #[test]
    fn all_required_session_states_have_stable_camel_case_wire_names() {
        let states = [
            SshSessionState::Resolving,
            SshSessionState::Connecting,
            SshSessionState::VerifyingHostKey,
            SshSessionState::AwaitingHostKeyDecision,
            SshSessionState::Authenticating,
            SshSessionState::OpeningChannel,
            SshSessionState::Running,
            SshSessionState::Disconnecting,
            SshSessionState::Closed,
            SshSessionState::Failed,
        ];
        let encoded = serde_json::to_value(states).expect("serialize states");
        assert_eq!(
            encoded,
            serde_json::json!([
                "resolving",
                "connecting",
                "verifyingHostKey",
                "awaitingHostKeyDecision",
                "authenticating",
                "openingChannel",
                "running",
                "disconnecting",
                "closed",
                "failed"
            ])
        );
    }

    #[test]
    fn jump_failure_route_stage_round_trips_without_credentials() {
        let reason = SshSessionFailureReason {
            code: SshSessionFailureCode::JumpChannelFailed,
            stage: SshSessionFailureStage::RouteIngress,
            route_stage: Some(SshSessionRouteStage::JumpHost {
                hop_index: 1,
                host_id: HostId::new(),
                endpoint: SshSessionEndpoint {
                    address: "jump.example".to_owned(),
                    port: 22,
                    username: Some("deploy".to_owned()),
                },
            }),
            algorithm_negotiation: None,
            retry_strategy: SshSessionRetryStrategy::RetryOpen,
            message_key: "errors.sshSession.jumpChannelFailed".to_owned(),
            diagnostic_id: None,
        };
        let encoded = serde_json::to_string(&reason).expect("serialize jump failure");
        assert!(encoded.contains("\"kind\":\"jumpHost\""));
        assert!(encoded.contains("\"hopIndex\":1"));
        assert!(encoded.contains("jump.example"));
        assert!(!encoded.contains("password"));
        let decoded: SshSessionFailureReason =
            serde_json::from_str(&encoded).expect("deserialize jump failure");
        assert_eq!(decoded, reason);
    }
}
