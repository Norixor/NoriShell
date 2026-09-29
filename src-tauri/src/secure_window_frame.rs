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

    #[cfg(target_os = "linux")]
    {
        let _ = manager;
        let builder = builder.decorations(false);
        // Approval and secure prompts stay opaque; only windows whose page rounds its own corners are transparent.
        if label.starts_with("workspace-window-") || label.starts_with("tool-") {
            builder.transparent(true)
        } else {
            builder
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (manager, label);
        builder
    }
}

/// Shrinks a default window size so a small screen keeps a visible margin around it,
/// never going below the window's own minimum size.
#[cfg(any(target_os = "linux", test))]
fn fit_default_size(monitor: (f64, f64), requested: (f64, f64), minimum: (f64, f64)) -> (f64, f64) {
    const SCREEN_SHARE: f64 = 0.85;
    (
        requested.0.min(monitor.0 * SCREEN_SHARE).max(minimum.0),
        requested.1.min(monitor.1 * SCREEN_SHARE).max(minimum.1),
    )
}

/// Default inner size for a new window. Linux screens are often small and windows have no
/// native frame to reveal, so a full-screen-sized window would look maximized; macOS and
/// Windows keep the requested size unchanged.
pub(crate) fn default_window_size<R: Runtime, M: Manager<R>>(
    manager: &M,
    requested: (f64, f64),
    minimum: (f64, f64),
) -> (f64, f64) {
    #[cfg(target_os = "linux")]
    {
        let app = manager.app_handle();
        let monitor = app.primary_monitor().ok().flatten().or_else(|| {
            app.available_monitors()
                .ok()
                .and_then(|monitors| monitors.into_iter().next())
        });
        match monitor {
            Some(monitor) => {
                let scale = monitor.scale_factor();
                let size = monitor.size();
                fit_default_size(
                    (
                        f64::from(size.width) / scale,
                        f64::from(size.height) / scale,
                    ),
                    requested,
                    minimum,
                )
            }
            None => requested,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (manager, minimum);
        requested
    }
}

#[cfg(test)]
mod tests {
    use super::fit_default_size;

    #[test]
    fn default_size_leaves_a_margin_on_small_screens() {
        assert_eq!(
            fit_default_size((1280.0, 800.0), (1100.0, 740.0), (800.0, 560.0)),
            (1088.0, 680.0)
        );
    }

    #[test]
    fn default_size_never_drops_below_the_minimum() {
        assert_eq!(
            fit_default_size((1280.0, 800.0), (1280.0, 800.0), (1024.0, 720.0)),
            (1088.0, 720.0)
        );
    }

    #[test]
    fn default_size_keeps_the_request_on_large_screens() {
        assert_eq!(
            fit_default_size((3840.0, 2160.0), (1100.0, 740.0), (800.0, 560.0)),
            (1100.0, 740.0)
        );
    }
}
