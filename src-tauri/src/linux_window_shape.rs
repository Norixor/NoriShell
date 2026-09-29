//! Keeps the page-drawn rounded window corners in step with the native window state on Linux.
//!
//! WebKit cannot tell a maximized window from a merely large one (docks and panels are not part
//! of its screen metrics), so the real state is pushed into every page of the window.

use tauri::{Runtime, Webview, Window, WindowEvent};

fn covers_screen<R: Runtime>(window: &Window<R>) -> bool {
    window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false)
}

fn script(covered: bool) -> String {
    format!("document.documentElement.toggleAttribute('data-nvx-maximized', {covered})")
}

/// Re-applies the state to the shell and every Tab view after the window changes size.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if !matches!(event, WindowEvent::Resized(_)) {
        return;
    }
    let script = script(covers_screen(window));
    for webview in window.webviews() {
        let _ = webview.eval(&script);
    }
}

/// A page that finishes loading after the last resize would otherwise start with a stale state.
pub fn on_page_load<R: Runtime>(webview: &Webview<R>) {
    let _ = webview.eval(script(covers_screen(&webview.window())));
}
