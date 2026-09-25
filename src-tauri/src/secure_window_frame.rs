//! Shared native frame for Core-owned standalone secure windows.

use tauri::{Manager, Runtime, WebviewWindowBuilder};

#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;

#[cfg(windows)]
pub(crate) const WINDOWS_INITIAL_CANVAS: tauri::webview::Color =
    tauri::webview::Color(247, 248, 250, 255);

/// Standalone windows share the main header, retaining AppKit controls on macOS.
pub(crate) fn apply_secure_window_frame<'a, R, M>(
    manager: &M,
    label: &str,
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M>
where
    R: Runtime,
    M: Manager<R>,
{
    let builder = if label.starts_with("secure-") {
        builder.content_protected(true)
    } else {
        builder
    };
    let builder = builder
        .inner_size(600.0, 480.0)
        .min_inner_size(480.0, 360.0);

    #[cfg(target_os = "macos")]
    {
        crate::window_first_show::schedule_fallback(manager.app_handle(), label, true);
        builder
            .hidden_title(true)
            .title_bar_style(TitleBarStyle::Overlay)
            .traffic_light_position(tauri::LogicalPosition::new(16.0, 20.0))
            .visible(false)
    }

    #[cfg(target_os = "windows")]
    {
        crate::window_first_show::schedule_fallback(manager.app_handle(), label, true);
        // Match the light canvas before WebView2 renders its first document frame.
        builder
            .decorations(false)
            .center()
            .background_color(WINDOWS_INITIAL_CANVAS)
            .visible(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (manager, label);
        builder
    }
}
