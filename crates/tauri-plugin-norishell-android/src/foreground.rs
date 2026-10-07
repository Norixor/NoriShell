//! Session foreground service control.

use serde::Serialize;

use crate::handle::{BridgeError, run_command};

/// Live resource counts shown in the foreground notification. Kotlin renders
/// the localized text; Rust sends only numbers and the language (no host names).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundCounts {
    pub sessions: u32,
    pub tunnels: u32,
    pub sftp: u32,
    pub transfers: u32,
}

impl ForegroundCounts {
    pub fn total(&self) -> u32 {
        self.sessions + self.tunnels + self.sftp + self.transfers
    }
}

/// Starts or updates `SessionForegroundService` with the given counts.
///
/// `locale` is the app language preference (`en`, `zh-CN` or `system`); Kotlin
/// renders the notification in it, `system` follows the device language.
pub fn foreground_update(counts: ForegroundCounts, locale: &str) -> Result<(), BridgeError> {
    #[derive(Serialize)]
    struct Payload<'a> {
        #[serde(flatten)]
        counts: ForegroundCounts,
        locale: &'a str,
    }
    run_command::<serde_json::Value>("foregroundUpdate", Payload { counts, locale }).map(|_| ())
}

/// Stops `SessionForegroundService` (no-op when not running).
pub fn foreground_stop() -> Result<(), BridgeError> {
    run_command::<serde_json::Value>("foregroundStop", serde_json::json!({})).map(|_| ())
}

/// Requests POST_NOTIFICATIONS on Android 13+. Returns whether it is granted
/// right now; the system dialog result is not awaited.
pub fn request_notification_permission() -> Result<bool, BridgeError> {
    #[derive(serde::Deserialize)]
    struct Response {
        granted: bool,
    }
    run_command::<Response>("notificationPermissionRequest", serde_json::json!({}))
        .map(|response| response.granted)
}
