//! Native notifications accept only localized, non-secret summaries; clicks return only an event ID.
//! `Accepted` means the OS accepted dispatch, not that a banner was shown; Focus modes may suppress it.
use std::sync::Arc;

use serde::Serialize;
use tauri::AppHandle;

#[cfg(target_os = "macos")]
#[path = "native_notifications/macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "native_notifications/windows.rs"]
mod platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationPermission {
    NotDetermined,
    Granted,
    Denied,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchResult {
    Accepted,
    Suppressed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NotificationError {
    #[error("native notification service is unavailable")]
    Unavailable,
    #[error("native notification permission is not granted")]
    PermissionDenied,
    #[error("native notification request is invalid")]
    InvalidContent,
    #[error("native notification dispatch failed")]
    DispatchFailed,
    #[error("native notification operation timed out; delivery is unknown")]
    Timeout,
}

type ClickHandler = Arc<dyn Fn(String) + Send + Sync>;
type DispatchGuard = Arc<dyn Fn() -> bool + Send + Sync>;

/// Create once on the Tauri setup main thread and retain as managed state until process exit.
/// Methods may run in Rust background tasks without a WebView or frontend timer.
pub struct NativeNotifications {
    #[cfg(any(target_os = "macos", windows))]
    platform: platform::Adapter,
}

impl NativeNotifications {
    pub fn new(app: AppHandle, on_click: impl Fn(String) + Send + Sync + 'static) -> Self {
        #[cfg(any(target_os = "macos", windows))]
        return Self {
            platform: platform::Adapter::new(app, Arc::new(on_click)),
        };
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (app, on_click);
            Self {}
        }
    }

    pub async fn permission(&self) -> Result<NotificationPermission, NotificationError> {
        #[cfg(any(target_os = "macos", windows))]
        return self.platform.permission().await;
        #[cfg(not(any(target_os = "macos", windows)))]
        Ok(NotificationPermission::Unavailable)
    }

    /// Call only after an explicit user action to enable notifications. Windows has no in-app prompt; return actual system settings.
    pub async fn request_permission(&self) -> Result<NotificationPermission, NotificationError> {
        #[cfg(any(target_os = "macos", windows))]
        return self.platform.request_permission().await;
        #[cfg(not(any(target_os = "macos", windows)))]
        Ok(NotificationPermission::Unavailable)
    }

    // General adapter entry point; the long-running task service uses send_if to recheck context before dispatch.
    #[allow(dead_code)]
    pub async fn send(
        &self,
        id: &str,
        title: &str,
        body: &str,
    ) -> Result<DispatchResult, NotificationError> {
        self.send_if(id, title, body, || true).await
    }

    /// Recheck host settings, foreground context, and lifecycle before the OS actually receives the request.
    pub async fn send_if(
        &self,
        id: &str,
        title: &str,
        body: &str,
        guard: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<DispatchResult, NotificationError> {
        validate_content(id, title, body)?;
        match self.permission().await? {
            NotificationPermission::Granted => {}
            NotificationPermission::Unavailable => return Err(NotificationError::Unavailable),
            _ => return Err(NotificationError::PermissionDenied),
        }
        #[cfg(any(target_os = "macos", windows))]
        return self.platform.send(id, title, body, Arc::new(guard)).await;
        #[cfg(not(any(target_os = "macos", windows)))]
        Err(NotificationError::Unavailable)
    }
}

fn validate_content(id: &str, title: &str, body: &str) -> Result<(), NotificationError> {
    // The event ID is host-owned; URLs, commands, and arbitrary activation arguments are not accepted.
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        || title.is_empty()
        || title.len() > 256
        || body.len() > 2048
        || title.chars().chain(body.chars()).any(|c| c.is_control())
    {
        return Err(NotificationError::InvalidContent);
    }
    Ok(())
}

fn deliver_click(callback: &ClickHandler, id: String) {
    // Never let a panic in a host callback unwind across the system FFI boundary.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(id)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_activation_payloads_and_control_characters() {
        for id in ["", "https://example.com", "a;b", "a\n", "$(id)"] {
            assert_eq!(
                validate_content(id, "完成", "终端任务已完成"),
                Err(NotificationError::InvalidContent)
            );
        }
        assert!(validate_content("event-123_456", "完成", "终端任务已完成").is_ok());
        assert!(validate_content("event", "完成\0", "摘要").is_err());
        assert!(validate_content("event", "完成", &"x".repeat(2049)).is_err());
    }

    #[test]
    fn click_preserves_exact_event_id() {
        let (tx, rx) = std::sync::mpsc::channel();
        let callback: ClickHandler = Arc::new(move |id| {
            tx.send(id).unwrap();
        });
        deliver_click(&callback, "event-0123".into());
        assert_eq!(rx.recv().unwrap(), "event-0123");
    }
}
