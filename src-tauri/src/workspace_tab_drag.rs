//! Native, content-free Workspace Tab drag preview and global mouse release tracking.

use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, Runtime, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::workspace_windows::WorkspaceWindows;

const PAGE: &str = "workspace-tab-drag-preview.html";
const RELEASED: &str = "workspace-tab-drag-released";
const HOVER: &str = "workspace-tab-drag-hover";
const HEADER_HEIGHT: f64 = 60.0;
const POLL_INTERVAL: Duration = Duration::from_millis(16);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(15);
const FADE_DURATION: Duration = Duration::from_millis(120);

#[derive(Clone, Default)]
pub(crate) struct WorkspaceTabDrag(Arc<Mutex<Option<ActiveDrag>>>);

struct ActiveDrag {
    id: String,
    nonce: String,
    source: String,
    preview: String,
    released_at: Option<Instant>,
    hover_target: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    id: String,
    nonce: String,
    target: Option<String>,
    x: f64,
    y: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Hover {
    id: String,
    nonce: String,
    active: bool,
}

fn emit_hover<R: Runtime>(app: &AppHandle<R>, drag: &ActiveDrag, target: &str, active: bool) {
    let _ = app.emit_to(
        target,
        HOVER,
        Hover {
            id: drag.id.clone(),
            nonce: drag.nonce.clone(),
            active,
        },
    );
}

fn update_hover<R: Runtime>(
    app: &AppHandle<R>,
    state: &WorkspaceTabDrag,
    source: &str,
    nonce: &str,
    target: Option<String>,
) {
    let Ok(mut active) = state.0.lock() else {
        return;
    };
    let Some(drag) = active.as_mut() else { return };
    if !active_matches(drag, source, nonce) || drag.released_at.is_some() {
        return;
    }
    let target = target.filter(|target| target != source);
    if drag.hover_target == target {
        return;
    }
    // Emit transitions while holding the drag lock so cancellation cannot precede a late enter.
    if let Some(previous) = drag.hover_target.as_deref() {
        emit_hover(app, drag, previous, false);
    }
    drag.hover_target = target;
    if let Some(next) = drag.hover_target.as_deref() {
        emit_hover(app, drag, next, true);
    }
}

#[cfg(target_os = "macos")]
fn left_button_down() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
    }
    // Combined-session state observes release even after the pointer leaves our WebView.
    unsafe { CGEventSourceButtonState(0, 0) }
}

#[cfg(windows)]
fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) < 0 }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn left_button_down() -> bool {
    false
}

fn active_matches(active: &ActiveDrag, source: &str, nonce: &str) -> bool {
    active.source == source && active.nonce == nonce
}

fn finish_drag<R: Runtime>(
    app: &AppHandle<R>,
    state: &WorkspaceTabDrag,
    source: &str,
    nonce: &str,
) -> Result<(), String> {
    let preview_label = {
        let mut active = state
            .0
            .lock()
            .map_err(|_| "workspace_tab.drag_unavailable")?;
        let Some(current) = active.as_ref() else {
            return Ok(());
        };
        if !active_matches(current, source, nonce) {
            return Err("workspace_tab.drag_wrong_owner".into());
        }
        active.take()
    };
    if let Some(drag) = preview_label {
        if let Some(target) = drag.hover_target.as_deref() {
            emit_hover(app, &drag, target, false);
        }
        fade_and_destroy(app.clone(), drag.preview);
    }
    Ok(())
}

fn fade_and_destroy<R: Runtime>(app: AppHandle<R>, label: String) {
    if let Some(preview) = app.get_webview_window(&label) {
        let _ = preview.eval("document.documentElement.classList.add('fade')");
        thread::spawn(move || {
            thread::sleep(FADE_DURATION);
            let _ = preview.destroy();
        });
    }
}

fn preview_position<R: Runtime>(preview: &WebviewWindow<R>, x: f64, y: f64) {
    let scale = preview.scale_factor().unwrap_or(1.0);
    let size = preview.outer_size().ok();
    let monitor = preview.available_monitors().ok().and_then(|monitors| {
        monitors.into_iter().find(|monitor| {
            let origin = monitor.position();
            let extent = monitor.size();
            x >= f64::from(origin.x)
                && x < f64::from(origin.x) + f64::from(extent.width)
                && y >= f64::from(origin.y)
                && y < f64::from(origin.y) + f64::from(extent.height)
        })
    });
    let (mut left, mut top) = (x + 12.0 * scale, y + 12.0 * scale);
    if let (Some(monitor), Some(size)) = (monitor, size) {
        let origin = monitor.position();
        let extent = monitor.size();
        left = left.clamp(
            f64::from(origin.x),
            (f64::from(origin.x) + f64::from(extent.width) - f64::from(size.width))
                .max(f64::from(origin.x)),
        );
        top = top.clamp(
            f64::from(origin.y),
            (f64::from(origin.y) + f64::from(extent.height) - f64::from(size.height))
                .max(f64::from(origin.y)),
        );
    }
    let _ = preview.set_position(PhysicalPosition::new(
        left.round() as i32,
        top.round() as i32,
    ));
}

#[derive(Clone, Copy)]
struct WindowHit<'a> {
    label: &'a str,
    focused: bool,
    header: bool,
}

fn target_from_hits(source: &str, hits: &[WindowHit<'_>]) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    // Only a foreign Header accepts a transfer. Any ordinary window body cancels it.
    let top = hits.iter().find(|hit| hit.focused).unwrap_or(&hits[0]);
    if top.label != source && top.header {
        Some(top.label.to_owned())
    } else {
        Some(source.to_owned())
    }
}

fn release_target<R: Runtime>(app: &AppHandle<R>, source: &str, x: f64, y: f64) -> Option<String> {
    let state = app.state::<WorkspaceWindows>();
    let Ok(mut labels) = state.window_labels() else {
        return Some(source.to_owned());
    };
    labels.push("main".into());
    let hits: Vec<_> = labels
        .iter()
        .filter_map(|label| {
            let window = app.get_webview_window(label)?;
            if window.is_visible().ok() != Some(true) || window.is_minimized().ok() == Some(true) {
                return None;
            }
            let origin = window.outer_position().ok()?;
            let size = window.outer_size().ok()?;
            if x < f64::from(origin.x)
                || x >= f64::from(origin.x) + f64::from(size.width)
                || y < f64::from(origin.y)
                || y >= f64::from(origin.y) + f64::from(size.height)
            {
                return None;
            }
            Some(WindowHit {
                label,
                focused: window.is_focused().ok() == Some(true),
                header: y < f64::from(origin.y)
                    + HEADER_HEIGHT * window.scale_factor().unwrap_or(1.0),
            })
        })
        .collect();
    target_from_hits(source, &hits)
}

fn poll_drag(app: AppHandle, state: WorkspaceTabDrag, nonce: String) {
    loop {
        thread::sleep(POLL_INTERVAL);
        let current = {
            let Ok(active) = state.0.lock() else { return };
            let Some(drag) = active.as_ref() else { return };
            if drag.nonce != nonce {
                return;
            }
            (
                drag.id.clone(),
                drag.source.clone(),
                drag.preview.clone(),
                drag.released_at,
            )
        };
        let (id, source_label, preview_label, released_at) = current;
        if let Some(released_at) = released_at {
            if released_at.elapsed() >= RELEASE_TIMEOUT {
                let _ = finish_drag(&app, &state, &source_label, &nonce);
                return;
            }
            continue;
        }
        let Some(source) = app.get_webview_window(&source_label) else {
            let _ = finish_drag(&app, &state, &source_label, &nonce);
            return;
        };
        let Ok(cursor) = source.cursor_position() else {
            let _ = finish_drag(&app, &state, &source_label, &nonce);
            return;
        };
        let target = release_target(&app, &source_label, cursor.x, cursor.y);
        update_hover(&app, &state, &source_label, &nonce, target.clone());
        if left_button_down() {
            if let Some(preview) = app.get_webview_window(&preview_label) {
                preview_position(&preview, cursor.x, cursor.y);
            }
            continue;
        }
        let should_emit = state.0.lock().is_ok_and(|mut active| {
            if let Some(drag) = active.as_mut()
                && active_matches(drag, &source_label, &nonce)
                && drag.released_at.is_none()
            {
                drag.released_at = Some(Instant::now());
                return true;
            }
            false
        });
        if should_emit
            && app
                .emit_to(
                    &source_label,
                    RELEASED,
                    Release {
                        id,
                        nonce: nonce.clone(),
                        target,
                        x: cursor.x,
                        y: cursor.y,
                    },
                )
                .is_err()
        {
            let _ = finish_drag(&app, &state, &source_label, &nonce);
            return;
        }
        // Keep the native preview at the release point until the transfer settles.
        continue;
    }
}

// Async for the same reason as `workspace_window_open`: building the preview
// window from a sync command deadlocks the Windows main thread.
#[tauri::command]
pub(crate) async fn workspace_tab_drag_begin(
    app: AppHandle,
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    drag: State<'_, WorkspaceTabDrag>,
    id: String,
    nonce: String,
) -> Result<(), String> {
    tabs.owns_tab(window.label(), &id)?;
    if nonce.is_empty() || nonce.len() > 128 || !nonce.is_ascii() {
        return Err("workspace_tab.drag_invalid_nonce".into());
    }
    let mut active = drag
        .0
        .lock()
        .map_err(|_| "workspace_tab.drag_unavailable")?;
    if active.is_some() {
        return Err("workspace_tab.drag_in_progress".into());
    }
    // Keep this label outside the trusted `workspace-*` capability wildcard.
    let label = format!("tab-drag-preview-{}", uuid::Uuid::now_v7());
    let expected = app
        .config()
        .build
        .dev_url
        .as_ref()
        .and_then(|url| url.join(PAGE).ok());
    let preview = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(PAGE.into()))
        .title("NoriShell")
        .on_navigation(move |url| {
            (cfg!(debug_assertions) && expected.as_ref().is_some_and(|expected| expected == url))
                || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                    || (matches!(url.scheme(), "http" | "https")
                        && url.host_str() == Some("tauri.localhost")
                        && url.port().is_none()))
                    && url.path() == "/workspace-tab-drag-preview.html"
                    && url.query().is_none())
        })
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .inner_size(204.0, 116.0)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|_| "workspace_tab.drag_preview_failed")?;
    if preview.set_ignore_cursor_events(true).is_err() {
        let _ = preview.destroy();
        return Err("workspace_tab.drag_preview_failed".into());
    }
    if let Ok(cursor) = window.cursor_position() {
        preview_position(&preview, cursor.x, cursor.y);
    }
    if preview.show().is_err() {
        let _ = preview.destroy();
        return Err("workspace_tab.drag_preview_failed".into());
    }
    *active = Some(ActiveDrag {
        id,
        nonce: nonce.clone(),
        source: window.label().to_owned(),
        preview: label,
        released_at: None,
        hover_target: None,
    });
    drop(active);
    let drag_state = drag.inner().clone();
    thread::spawn(move || poll_drag(app, drag_state, nonce));
    Ok(())
}

#[tauri::command]
pub(crate) fn workspace_tab_drag_cancel(
    app: AppHandle,
    window: WebviewWindow,
    drag: State<'_, WorkspaceTabDrag>,
    nonce: String,
) -> Result<(), String> {
    finish_drag(&app, &drag, window.label(), &nonce)
}

#[tauri::command]
pub(crate) fn workspace_tab_drag_finish(
    app: AppHandle,
    window: WebviewWindow,
    drag: State<'_, WorkspaceTabDrag>,
    nonce: String,
) -> Result<(), String> {
    finish_drag(&app, &drag, window.label(), &nonce)
}

pub(crate) fn on_window_destroyed<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let Some(state) = app.try_state::<WorkspaceTabDrag>() else {
        return;
    };
    let preview = state.0.lock().ok().and_then(|mut active| {
        if active.as_ref().is_some_and(|drag| drag.source == label) {
            active.take()
        } else {
            None
        }
    });
    if let Some(drag) = preview {
        if let Some(target) = drag.hover_target.as_deref() {
            emit_hover(app, &drag, target, false);
        }
        fade_and_destroy(app.clone(), drag.preview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_another_workspace_header_accepts_a_drop() {
        assert_eq!(target_from_hits("main", &[]), None);
        assert_eq!(
            target_from_hits(
                "main",
                &[WindowHit {
                    label: "main",
                    focused: true,
                    header: true
                }]
            ),
            Some("main".into())
        );
        assert_eq!(
            target_from_hits(
                "main",
                &[WindowHit {
                    label: "workspace-a",
                    focused: true,
                    header: false
                }]
            ),
            Some("main".into())
        );
        assert_eq!(
            target_from_hits(
                "main",
                &[WindowHit {
                    label: "workspace-a",
                    focused: true,
                    header: true
                }]
            ),
            Some("workspace-a".into())
        );
    }
}
