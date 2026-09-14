//! The native tray returns one-shot actions without granting connection or input permissions.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    ForwardSessionId, HostId, LocalSessionId, NativeTerminalSessionScope, RequestMeta,
    SftpSessionId, SftpTransferEndpointFence, SshSessionId, TransferId, VaultState, WireSequence,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum NativeTrayLocale {
    #[default]
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeTrayAction {
    NewTerminal,
    NewLocalTerminal,
    QuickConnect,
    Settings,
    OpenHost {
        host_id: HostId,
    },
    FocusTerminal {
        scope: NativeTerminalSessionScope,
    },
    FocusSshSession {
        session_id: SshSessionId,
        generation: WireSequence,
    },
    FocusLocalSession {
        session_id: LocalSessionId,
        generation: WireSequence,
    },
    FocusTelnet {
        session_id: String,
        generation: WireSequence,
        socket_id: Option<String>,
    },
    FocusDesktop {
        session_id: String,
        generation: WireSequence,
    },
    OpenTunnels {
        session_id: Option<ForwardSessionId>,
        generation: Option<WireSequence>,
    },
    OpenTransfers {
        transfer_id: Option<TransferId>,
        /// Minimum revision within the exact two-ended fence; ordinary transfer progress does not invalidate navigation.
        state_revision: Option<WireSequence>,
        source_fence: Option<SftpTransferEndpointFence>,
        target_fence: Option<SftpTransferEndpointFence>,
    },
    OpenSftp {
        session_id: SftpSessionId,
        generation: WireSequence,
    },
    Vault {
        expected_state: VaultState,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTrayActionTakeRequest {
    pub meta: RequestMeta,
    pub token: String,
}

/// Call after installing the event listener; repeated calls update the locale and return unconsumed clicks.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTrayActionsReadyRequest {
    pub meta: RequestMeta,
    pub locale: NativeTrayLocale,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTrayActionEvent {
    pub token: String,
}

/// The restricted tray panel receives only display text and one-shot action identifiers.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTrayPanelSnapshot {
    pub locale: NativeTrayLocale,
    /// Empty when status display is disabled; null stats indicate unknown sources, never fabricated zeros.
    pub stats: Vec<NativeTrayPanelStat>,
    /// A short localized description when status display is enabled and a resource has an error.
    pub error_summary: Option<String>,
    pub notification_state: NativeTrayPanelNotificationState,
    pub rows: Vec<NativeTrayPanelRow>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTrayPanelStatKind {
    Sessions,
    Tunnels,
    Sftp,
    Transfers,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTrayPanelStat {
    pub kind: NativeTrayPanelStatKind,
    pub count: Option<usize>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTrayPanelNotificationState {
    Active,
    Paused,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NativeTrayPanelRow {
    pub id: Option<String>,
    pub label: String,
    pub kind: NativeTrayPanelRowKind,
    pub role: Option<NativeTrayPanelRole>,
    pub children: Vec<NativeTrayPanelRow>,
}
/// A panel-only structural role, not a resource identifier or an extension of executable permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTrayPanelRole {
    Summary,
    Recent,
    Sessions,
    Tunnels,
    Transfers,
    Notifications,
    Vault,
    RecentHost,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NativeTrayPanelRowKind {
    Show,
    NewTerminal,
    NewLocalTerminal,
    QuickConnect,
    Settings,
    Quit,
    Action,
    Status,
    Group,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTrayPanelSnapshotRequest {
    pub meta: RequestMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTrayPanelExecuteRequest {
    pub meta: RequestMeta,
    pub token: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeTrayPanelHideRequest {
    pub meta: RequestMeta,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_generations_remain_lossless_decimal_strings() {
        let action = NativeTrayAction::FocusTelnet {
            session_id: "session".into(),
            generation: WireSequence::new(9_007_199_254_740_993),
            socket_id: Some("socket".into()),
        };
        assert_eq!(
            serde_json::to_value(action).unwrap()["generation"],
            "9007199254740993"
        );
    }
    #[test]
    fn panel_execute_accepts_only_meta_and_opaque_token() {
        let request = serde_json::json!({"meta":{"requestId":crate::RequestId::new()},"token":"opaque","action":{"kind":"newTerminal"}});
        assert!(serde_json::from_value::<NativeTrayPanelExecuteRequest>(request).is_err());
    }
    #[test]
    fn renderer_cannot_smuggle_a_target_into_token_request() {
        let request = serde_json::json!({"meta":{"requestId":crate::RequestId::new()},"token":"opaque","hostId":"other"});
        assert!(serde_json::from_value::<NativeTrayActionTakeRequest>(request).is_err());
    }
}
