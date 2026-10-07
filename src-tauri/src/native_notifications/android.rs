//! Android adapter: `NotificationsBridge.kt` through the norishell-android bridge.
//!
//! Kotlin posts only the localized, non-secret text built by the service and
//! reports taps by the exact host-owned event id (`nativeNotificationEvent`).
//! Every bridge call blocks on the Android UI thread's command queue, so it runs
//! on a blocking worker under a timeout and never on the UI thread.
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri_plugin_norishell_android as bridge;

use super::*;

const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const POST_TIMEOUT: Duration = Duration::from_secs(5);
// Kotlin resolves the request after the user answers the system dialog (or
// after 60 s); this outer bound only protects against a lost resolve.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(75);

#[derive(Deserialize)]
struct PermissionResponse {
    permission: String,
}

#[derive(Serialize)]
struct PostRequest<'a> {
    id: &'a str,
    title: &'a str,
    body: &'a str,
    channel: &'static str,
}

pub struct Adapter;

impl Adapter {
    pub fn new(_app: AppHandle, callback: ClickHandler) -> Self {
        // The service and its adapter live until process exit; the first
        // registration wins and is the only one ever made.
        if bridge::registry::register_notification_click(move |id| {
            // The event id is opaque; it is not logged.
            bridge::log::info("resource notification tapped");
            deliver_click(&callback, id.to_owned());
        })
        .is_err()
        {
            bridge::log::warn("notification click callback already registered");
        }
        Self
    }

    pub async fn permission(&self) -> Result<NotificationPermission, NotificationError> {
        query_permission("notificationsPermission", QUERY_TIMEOUT).await
    }

    pub async fn request_permission(&self) -> Result<NotificationPermission, NotificationError> {
        query_permission("notificationsRequestPermission", REQUEST_TIMEOUT).await
    }

    pub async fn send(
        &self,
        id: &str,
        category: NotificationCategory,
        title: &str,
        body: &str,
        guard: DispatchGuard,
    ) -> Result<DispatchResult, NotificationError> {
        let (id, title, body) = (id.to_owned(), title.to_owned(), body.to_owned());
        let channel = match category {
            NotificationCategory::Alerts => "alerts",
            NotificationCategory::Transfers => "transfers",
        };
        let task = tokio::task::spawn_blocking(move || {
            // Last check (pause, stop, preferences, Activity foreground) right
            // before Kotlin receives the post.
            if !guard() {
                return Ok(DispatchResult::Suppressed);
            }
            bridge::run_command::<serde_json::Value>(
                "notificationsPost",
                PostRequest {
                    id: &id,
                    title: &title,
                    body: &body,
                    channel,
                },
            )
            .map(|_| DispatchResult::Accepted)
            .map_err(|error| post_error(error.code()))
        });
        match tokio::time::timeout(POST_TIMEOUT, task).await {
            Ok(joined) => joined.map_err(|_| NotificationError::Unavailable)?,
            // The UI thread may still post it; delivery is unknown.
            Err(_) => Err(NotificationError::Timeout),
        }
    }
}

async fn query_permission(
    command: &'static str,
    timeout: Duration,
) -> Result<NotificationPermission, NotificationError> {
    if !bridge::is_ready() {
        return Ok(NotificationPermission::Unavailable);
    }
    let task = tokio::task::spawn_blocking(move || {
        bridge::run_command::<PermissionResponse>(command, serde_json::json!({}))
    });
    let response = match tokio::time::timeout(timeout, task).await {
        Ok(Ok(Ok(response))) => response,
        Ok(_) => return Err(NotificationError::Unavailable),
        Err(_) => return Err(NotificationError::Timeout),
    };
    Ok(permission_from_kotlin(&response.permission))
}

fn permission_from_kotlin(value: &str) -> NotificationPermission {
    match value {
        "granted" => NotificationPermission::Granted,
        "denied" => NotificationPermission::Denied,
        "notDetermined" => NotificationPermission::NotDetermined,
        _ => NotificationPermission::Unavailable,
    }
}

fn post_error(code: &str) -> NotificationError {
    match code {
        // A blocked channel is a user permission decision for that group.
        "notifications.permission_denied" | "notifications.channel_disabled" => {
            NotificationError::PermissionDenied
        }
        "notifications.invalid_content" => NotificationError::InvalidContent,
        "android_bridge.not_ready" => NotificationError::Unavailable,
        _ => NotificationError::DispatchFailed,
    }
}
