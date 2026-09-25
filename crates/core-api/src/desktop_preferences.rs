//! Core-owned, non-secret desktop preferences.
//!
//! These values shape host UI behavior only. They deliberately do not carry
//! notification payloads, credentials, connection state, or any Vault data.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{RequestMeta, WireSequence};

/// The behavior requested when the primary application window is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DesktopWindowCloseBehavior {
    Hide,
    Quit,
}

/// Version-one, non-secret preferences owned by the desktop Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPreferences {
    pub window_close_behavior: DesktopWindowCloseBehavior,
    pub tray_show_status: bool,
    pub tray_recent_limit: u8,
    pub tray_show_host_names: bool,
    pub notification_background_only: bool,
    /// Retained for existing SQLite and sync profiles; command notifications are removed.
    pub notification_failure_only: bool,
    pub notify_transfer_completed: bool,
    pub notify_transfer_failed: bool,
    pub notify_disconnected: bool,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            window_close_behavior: DesktopWindowCloseBehavior::Hide,
            tray_show_status: true,
            tray_recent_limit: 5,
            tray_show_host_names: true,
            notification_background_only: true,
            notification_failure_only: false,
            notify_transfer_completed: false,
            notify_transfer_failed: false,
            notify_disconnected: false,
        }
    }
}

impl DesktopPreferences {
    /// The only numeric preference is intentionally bounded so untrusted IPC
    /// and persisted data cannot grow tray projections without limit.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.tray_recent_limit > 10 {
            return Err("desktop_preferences.tray_recent_limit_invalid");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPreferencesGetRequest {
    pub meta: RequestMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPreferencesReplaceRequest {
    pub meta: RequestMeta,
    pub expected_revision: WireSequence,
    pub preferences: DesktopPreferences,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPreferencesSnapshot {
    pub preferences: DesktopPreferences,
    pub revision: WireSequence,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_version_one_contract() {
        assert_eq!(
            DesktopPreferences::default(),
            DesktopPreferences {
                window_close_behavior: DesktopWindowCloseBehavior::Hide,
                tray_show_status: true,
                tray_recent_limit: 5,
                tray_show_host_names: true,
                notification_background_only: true,
                notification_failure_only: false,
                notify_transfer_completed: false,
                notify_transfer_failed: false,
                notify_disconnected: false,
            }
        );
    }

    #[test]
    fn invalid_or_unknown_preference_input_is_rejected() {
        let preferences = DesktopPreferences {
            tray_recent_limit: 11,
            ..DesktopPreferences::default()
        };
        assert!(preferences.validate().is_err());

        let request = serde_json::json!({
            "meta": { "requestId": "019d0000-0000-7000-8000-000000000701" },
            "expectedRevision": "1",
            "preferences": {
                "windowCloseBehavior": "hide",
                "trayShowStatus": true,
                "trayRecentLimit": 5,
                "trayShowHostNames": true,
                "notificationBackgroundOnly": true,
                "notificationFailureOnly": false,
                "notifyTransferCompleted": false,
                "notifyTransferFailed": false,
                "notifyDisconnected": false,
                "unexpected": true
            }
        });
        assert!(serde_json::from_value::<DesktopPreferencesReplaceRequest>(request).is_err());
    }
}
