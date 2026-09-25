//! Vault-backed command history and fenced local terminal observations.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    HostId, LocalAttachmentId, LocalInputLeaseId, LocalPtyId, LocalSessionId, LocalViewId,
    NativeTerminalHistoryEntryId, RequestMeta, SshAttachmentId, SshChannelId, SshInputLeaseId,
    SshSessionId, SshViewId, WireSequence,
};

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
    /// Retained for existing preference export and sync data; completion notifications are removed.
    pub notifications_enabled: bool,
    /// Retained for existing preference export and sync data; completion notifications are removed.
    pub notification_threshold_seconds: u32,
}

impl Default for NativeTerminalSettings {
    fn default() -> Self {
        Self {
            history_enabled: true,
            persist_encrypted: true,
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

/// The exact SSH input fence for one acknowledged input.
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

/// The exact local PTY input fence for one acknowledged input.
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

/// A renderer observation made immediately before an acknowledged Enter.
/// Core validates that this exact input sequence still owns the session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTerminalHistoryRecordRequest {
    pub meta: RequestMeta,
    pub input_fence: NativeTerminalInputFence,
    pub command: String,
}

impl std::fmt::Debug for NativeTerminalHistoryRecordRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeTerminalHistoryRecordRequest")
            .field("meta", &self.meta)
            .field("input_fence", &self.input_fence)
            .field("command", &"[REDACTED]")
            .finish()
    }
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
