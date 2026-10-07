//! System bar appearance for the Android windows.
//!
//! The frontend resolves the app theme (Settings light/dark/system plus the
//! active theme colors) and sends the result here; Kotlin applies the bar icon
//! contrast and the color behind the bars to MainActivity and every Secure
//! Surface (`SystemBars.kt`). Only non-secret style data crosses the bridge.

use norishell_core_api::{CoreApiError, ErrorCategory, RequestMeta, RetryStrategy};
use serde::{Deserialize, Serialize};
use tauri_plugin_norishell_android as bridge;

use crate::core_api_error::core_error;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SystemBarsRequest {
    pub meta: RequestMeta,
    /// Whether the app theme is dark (light bar icons).
    pub dark: bool,
    /// `#rrggbb` drawn behind the status and navigation bars.
    pub background: String,
}

#[derive(Serialize)]
struct Payload<'a> {
    dark: bool,
    background: &'a str,
}

fn is_hex_color(text: &str) -> bool {
    text.len() == 7
        && text.starts_with('#')
        && text[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[tauri::command]
pub(crate) async fn android_set_system_bars(
    request: SystemBarsRequest,
) -> Result<(), Box<CoreApiError>> {
    let request_id = request.meta.request_id;
    if !is_hex_color(&request.background) {
        return Err(core_error(
            request_id.clone(),
            "system_bars.invalid_payload",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.core.invalidRequest",
        ));
    }
    // Bridge calls block until the Kotlin command ran on the UI thread.
    let result = tauri::async_runtime::spawn_blocking(move || {
        bridge::run_command::<serde_json::Value>(
            "systemBarsSet",
            Payload {
                dark: request.dark,
                background: &request.background,
            },
        )
        .map(|_| ())
        .map_err(|error| error.code().to_owned())
    })
    .await;
    let unavailable = || {
        core_error(
            request_id.clone(),
            "system_bars.unavailable",
            ErrorCategory::Unavailable,
            RetryStrategy::Never,
            "errors.core.unavailable",
        )
    };
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(code)) => {
            bridge::log::warn(&format!("system bars update failed: {code}"));
            Err(unavailable())
        }
        Err(_) => Err(unavailable()),
    }
}
