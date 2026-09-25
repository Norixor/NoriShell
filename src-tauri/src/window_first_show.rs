//! One-time reveal of desktop WebView windows after the trusted renderer has mounted.

#[cfg(any(windows, target_os = "macos", test))]
use std::collections::{HashMap, HashSet};
#[cfg(any(windows, target_os = "macos"))]
use std::{sync::Mutex, time::Duration};

use tauri::WebviewWindow;
#[cfg(any(windows, target_os = "macos"))]
use tauri::{AppHandle, Manager as _, Runtime};
#[cfg(any(windows, target_os = "macos", test))]
use uuid::Uuid;

#[cfg(any(windows, target_os = "macos"))]
const REVEAL_FALLBACK: Duration = Duration::from_secs(5);

#[cfg(any(windows, target_os = "macos", test))]
struct PendingWindow {
    generation: Uuid,
    positioned: bool,
    reveal_requested: bool,
    show_failures: u8,
}

#[cfg(any(windows, target_os = "macos", test))]
#[derive(Default)]
struct WindowShowState {
    pending: HashMap<String, PendingWindow>,
    revealed: HashSet<String>,
}

#[cfg(any(windows, target_os = "macos", test))]
impl WindowShowState {
    fn register(&mut self, label: &str, generation: Uuid, positioned: bool) {
        self.revealed.remove(label);
        self.pending.insert(
            label.to_owned(),
            PendingWindow {
                generation,
                positioned,
                reveal_requested: false,
                show_failures: 0,
            },
        );
    }

    /// Returns true only for this creation after both placement and renderer readiness.
    fn request(
        &mut self,
        label: &str,
        expected: Option<Uuid>,
        position_only: bool,
    ) -> Option<PendingWindow> {
        let entry = self.pending.get_mut(label)?;
        if expected.is_some_and(|generation| generation != entry.generation) {
            return None;
        }
        if position_only {
            entry.positioned = true;
        } else {
            entry.reveal_requested = true;
        }
        if !entry.positioned || !entry.reveal_requested {
            return None;
        }
        let pending = self.pending.remove(label)?;
        self.revealed.insert(label.to_owned());
        Some(pending)
    }

    fn forget_generation(&mut self, label: &str, generation: Uuid) {
        if self
            .pending
            .get(label)
            .is_some_and(|entry| entry.generation == generation)
        {
            self.pending.remove(label);
        }
    }

    fn forget(&mut self, label: &str) {
        self.pending.remove(label);
        self.revealed.remove(label);
    }

    fn show_failed(&mut self, label: &str, mut failed: PendingWindow) -> Option<Uuid> {
        if self.pending.contains_key(label) {
            return None;
        }
        self.revealed.remove(label);
        if failed.show_failures >= 2 {
            return None;
        }
        failed.show_failures += 1;
        let generation = failed.generation;
        self.pending.insert(label.to_owned(), failed);
        Some(generation)
    }

    fn was_revealed(&self, label: &str) -> bool {
        self.revealed.contains(label)
    }
}

#[derive(Default)]
pub(crate) struct WindowFirstShow {
    #[cfg(any(windows, target_os = "macos"))]
    state: Mutex<WindowShowState>,
}

#[tauri::command]
pub(crate) fn window_renderer_ready(window: WebviewWindow) -> Result<(), String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        request_reveal(&window, None, false)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = window;
        Ok(())
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn schedule_fallback<R: Runtime>(app: &AppHandle<R>, label: &str, positioned: bool) {
    let generation = Uuid::new_v4();
    let state = app.state::<WindowFirstShow>();
    if let Ok(mut current) = state.state.lock() {
        current.register(label, generation, positioned);
    } else {
        return;
    }
    let app = app.clone();
    let label = label.to_owned();
    schedule_attempt(app, label, generation, REVEAL_FALLBACK);
}

#[cfg(any(windows, target_os = "macos"))]
fn schedule_attempt<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    generation: Uuid,
    delay: Duration,
) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        if let Some(window) = app.get_webview_window(&label) {
            let _ = request_reveal(&window, Some(generation), false);
        } else if let Some(state) = app.try_state::<WindowFirstShow>()
            && let Ok(mut current) = state.state.lock()
        {
            current.forget_generation(&label, generation);
        }
    });
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn positioned<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = request_reveal(window, None, true);
}

#[cfg(any(windows, target_os = "macos"))]
fn request_reveal<R: Runtime>(
    window: &WebviewWindow<R>,
    expected_generation: Option<Uuid>,
    position_only: bool,
) -> Result<(), String> {
    let label = window.label();
    let state = window.app_handle().state::<WindowFirstShow>();
    let pending = state
        .state
        .lock()
        .map_err(|_| "window unavailable")?
        .request(label, expected_generation, position_only);
    if let Some(pending) = pending {
        if let Err(error) = window.show() {
            if window.app_handle().get_webview_window(label).is_some()
                && let Ok(mut current) = state.state.lock()
                && let Some(generation) = current.show_failed(label, pending)
            {
                schedule_attempt(
                    window.app_handle().clone(),
                    label.to_owned(),
                    generation,
                    Duration::from_secs(1),
                );
            }
            return Err(error.to_string());
        }
        // The previous creation path focused each newly opened prompt and the main window.
        let _ = window.set_focus();
    }
    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn forget<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(state) = app.try_state::<WindowFirstShow>()
        && let Ok(mut current) = state.state.lock()
    {
        current.forget(label);
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) fn was_revealed<R: Runtime>(app: &AppHandle<R>, label: &str) -> bool {
    app.try_state::<WindowFirstShow>()
        .and_then(|state| {
            state
                .state
                .lock()
                .ok()
                .map(|current| current.was_revealed(label))
        })
        .unwrap_or(false)
}

pub(crate) fn focus_if_revealed<R: tauri::Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    #[cfg(any(windows, target_os = "macos"))]
    if !was_revealed(window.app_handle(), window.label()) {
        return Ok(());
    }
    window.set_focus()
}

pub(crate) fn show_if_revealed<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
) -> tauri::Result<bool> {
    #[cfg(any(windows, target_os = "macos"))]
    if !was_revealed(window.app_handle(), window.label()) {
        return Ok(false);
    }
    window.unminimize()?;
    window.show()?;
    window.set_focus()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_timeout_cannot_reveal_a_recreated_window_with_the_same_label() {
        let mut state = WindowShowState::default();
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        state.register("tray-panel", old, false);
        state.forget("tray-panel");
        state.register("tray-panel", new, false);

        assert!(state.request("tray-panel", Some(old), false).is_none());
        state.forget_generation("tray-panel", old);
        assert!(!state.was_revealed("tray-panel"));
        assert!(state.request("tray-panel", None, false).is_none());
        assert!(state.request("tray-panel", None, true).is_some());
        assert!(state.was_revealed("tray-panel"));
        assert!(state.request("tray-panel", Some(new), false).is_none());
    }

    #[test]
    fn renderer_and_position_must_both_be_ready_and_reveal_only_once() {
        let mut state = WindowShowState::default();
        let generation = Uuid::new_v4();
        state.register("tray-panel", generation, false);
        assert!(state.request("tray-panel", None, false).is_none());
        assert!(
            state
                .request("tray-panel", Some(generation), true)
                .is_some()
        );
        assert!(
            state
                .request("tray-panel", Some(generation), false)
                .is_none()
        );
        state.forget("tray-panel");
        assert!(!state.was_revealed("tray-panel"));
    }

    #[test]
    fn failed_native_show_rearms_a_bounded_retry() {
        let mut state = WindowShowState::default();
        let generation = Uuid::new_v4();
        state.register("main", generation, true);
        for _ in 0..2 {
            let pending = state.request("main", None, false).expect("reveal pending");
            assert_eq!(state.show_failed("main", pending), Some(generation));
            assert!(!state.was_revealed("main"));
        }
        let pending = state.request("main", None, false).expect("final pending");
        assert_eq!(state.show_failed("main", pending), None);
        assert!(!state.was_revealed("main"));
    }
}
