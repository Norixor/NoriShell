//! Core-owned shell integration, command-completion facts, and command-history
//! wire types. The renderer only supplies an exact terminal input fence; the
//! Core generates the install script and never accepts its bytes from IPC.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    HostId, LocalAttachmentId, LocalInputLeaseId, LocalPtyId, LocalSessionId, LocalViewId,
    NativeTerminalEventId, NativeTerminalHistoryEntryId, RequestMeta, SshAttachmentId,
    SshChannelId, SshInputLeaseId, SshSessionId, SshViewId, WireSequence,
};

pub const NATIVE_TERMINAL_EVENT_SCHEMA_VERSION: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTerminalShellKind {
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTerminalCaptureState {
    Disabled,
    Pending,
    Ready,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTerminalActivity {
    Prompt,
    Executing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTerminalCaptureFailureCode {
    PromptNotConfirmed,
    ShellUnsupported,
    WriterRejected,
    EnableTimedOut,
    ProtocolViolation,
}

/// Non-secret Core settings. Command text is deliberately absent: it is only
/// returned by the separately bounded history query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSettings {
    pub history_enabled: bool,
    pub persist_encrypted: bool,
    pub history_max_entries: u16,
    pub history_retention_days: u16,
    pub history_paused: bool,
    pub notifications_enabled: bool,
    pub notification_threshold_seconds: u32,
}

impl Default for NativeTerminalSettings {
    fn default() -> Self {
        Self {
            history_enabled: false,
            persist_encrypted: false,
            history_max_entries: 500,
            history_retention_days: 30,
            history_paused: false,
            notifications_enabled: false,
            notification_threshold_seconds: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSettingsGetRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSettingsReplaceRequest {
    pub meta: RequestMeta,
    pub expected_settings_revision: WireSequence,
    pub settings: NativeTerminalSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSettingsSnapshot {
    pub settings: NativeTerminalSettings,
    pub settings_revision: WireSequence,
    /// False means encrypted persistence was selected but the Vault is locked
    /// or requires reload. Pausing or disabling capture does not change this
    /// storage-readability fact, so the user can resume without data loss.
    pub history_available: bool,
    /// A previous encrypted write failed. Core keeps a bounded latest snapshot
    /// and retries on a later mutation; settings can also trim or clear the
    /// history to recover. Command text is never placed in this status.
    pub history_persistence_failed: bool,
}

/// The exact SSH input fence, intentionally without `bytes`. Core constructs
/// the script payload after it has accepted every input-ownership fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSshInputFence {
    pub session_id: SshSessionId,
    pub expected_generation: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: SshViewId,
    pub focus_epoch: WireSequence,
    pub lease_id: SshInputLeaseId,
    pub input_epoch: WireSequence,
    pub client_seq: WireSequence,
}

/// The exact local PTY input fence, intentionally without `bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalLocalInputFence {
    pub session_id: LocalSessionId,
    pub expected_generation: WireSequence,
    pub expected_state_revision: WireSequence,
    pub pty_id: LocalPtyId,
    pub attachment_id: LocalAttachmentId,
    pub view_id: LocalViewId,
    pub lease_id: LocalInputLeaseId,
    pub focus_epoch: WireSequence,
    pub input_epoch: WireSequence,
    pub client_seq: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
pub enum NativeTerminalInputFence {
    Ssh(NativeTerminalSshInputFence),
    Local(NativeTerminalLocalInputFence),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalEnableRequest {
    pub meta: RequestMeta,
    pub input_fence: NativeTerminalInputFence,
    pub shell_kind: NativeTerminalShellKind,
    /// Shell hooks can only be safely installed after the user explicitly
    /// confirms that the current terminal is at an empty interactive prompt.
    pub confirmed_empty_prompt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeTerminalSessionScope {
    Ssh {
        session_id: SshSessionId,
        generation: WireSequence,
        channel_id: SshChannelId,
        pane_id: SshViewId,
    },
    Local {
        session_id: LocalSessionId,
        generation: WireSequence,
        pty_id: LocalPtyId,
        pane_id: LocalViewId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSessionStatus {
    pub session: NativeTerminalSessionScope,
    pub shell_kind: NativeTerminalShellKind,
    pub capture_state: NativeTerminalCaptureState,
    pub failure_code: Option<NativeTerminalCaptureFailureCode>,
    pub activity: NativeTerminalActivity,
    /// True when the Core-owned script installed for this session sends
    /// command text in its private OSC start frame. This mode is snapshotted
    /// at enable time so notification-only sessions never emit command text.
    pub captures_command: bool,
    pub history_paused: bool,
    pub prompt_observed: bool,
    /// Advances for every valid shell prompt hook, including a prompt reached
    /// after an empty line or cancelled edit. It is independent of completion
    /// delivery so the renderer can safely reset append-only suggestions.
    pub prompt_sequence: WireSequence,
    /// The session's writer sequence at the exact prompt hook. A delayed
    /// snapshot must not be mistaken for an empty current input line.
    pub prompt_input_sequence: WireSequence,
    /// The focused lease input epoch at that prompt, if the exact pane still
    /// held a lease. The renderer compares it before offering an auto-append.
    pub prompt_input_epoch: Option<WireSequence>,
    pub completion_cursor: WireSequence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalCommandCompletion {
    pub cursor: WireSequence,
    pub event_id: NativeTerminalEventId,
    pub session: NativeTerminalSessionScope,
    #[ts(type = "number")]
    pub elapsed_millis: u64,
    pub exit_code: Option<i32>,
    #[ts(type = "number")]
    pub completed_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSnapshotRequest {
    pub meta: RequestMeta,
    pub after_completion_cursor: Option<WireSequence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalSnapshot {
    pub schema_version: u16,
    pub snapshot_revision: WireSequence,
    pub history_paused: bool,
    pub history_persistence_failed: bool,
    pub completion_cursor: WireSequence,
    pub sessions: Vec<NativeTerminalSessionStatus>,
    /// Only completions strictly after `afterCompletionCursor` are included.
    /// The command text never appears in this projection.
    pub completions: Vec<NativeTerminalCommandCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeTerminalHistoryScope {
    Host { host_id: HostId },
    Local,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalHistoryListRequest {
    pub meta: RequestMeta,
    pub scope: Option<NativeTerminalHistoryScope>,
    #[serde(default)]
    pub query: String,
    pub limit: u16,
}

impl std::fmt::Debug for NativeTerminalHistoryListRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeTerminalHistoryListRequest")
            .field("meta", &self.meta)
            .field("scope", &self.scope)
            .field("query", &"[REDACTED]")
            .field("limit", &self.limit)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalHistoryEntry {
    pub entry_id: NativeTerminalHistoryEntryId,
    pub scope: NativeTerminalHistoryScope,
    pub command: String,
    #[ts(type = "number")]
    pub completed_at_unix_ms: i64,
    #[ts(type = "number")]
    pub elapsed_millis: u64,
    pub exit_code: Option<i32>,
}

impl std::fmt::Debug for NativeTerminalHistoryEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeTerminalHistoryEntry")
            .field("entry_id", &self.entry_id)
            .field("scope", &self.scope)
            .field("command", &"[REDACTED]")
            .field("completed_at_unix_ms", &self.completed_at_unix_ms)
            .field("elapsed_millis", &self.elapsed_millis)
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalHistoryDeleteRequest {
    pub meta: RequestMeta,
    pub entry_id: NativeTerminalHistoryEntryId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalHistoryClearRequest {
    pub meta: RequestMeta,
    pub scope: Option<NativeTerminalHistoryScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalHistoryPauseRequest {
    pub meta: RequestMeta,
    pub paused: bool,
}
