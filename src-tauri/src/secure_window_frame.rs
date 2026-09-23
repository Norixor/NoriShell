//! Shared native frame for Core-owned standalone secure windows.

use tauri::{Manager, Runtime, WebviewWindowBuilder};

#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;

/// Standalone windows share the main header, retaining AppKit controls on macOS.
pub(crate) fn apply_secure_window_frame<'a, R, M>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M>
where
    R: Runtime,
    M: Manager<R>,
{
    let builder = builder
        .inner_size(600.0, 420.0)
        .min_inner_size(480.0, 360.0);

    #[cfg(target_os = "macos")]
    {
        builder
            .hidden_title(true)
            .title_bar_style(TitleBarStyle::Overlay)
            .traffic_light_position(tauri::LogicalPosition::new(16.0, 20.0))
    }

    #[cfg(target_os = "windows")]
    {
        builder.decorations(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        builder
    }
}
