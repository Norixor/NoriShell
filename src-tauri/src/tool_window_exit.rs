//! Application exit waits for child editors to finish their own draft cleanup.
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
#[cfg(not(target_os = "android"))]
use tauri::{Emitter, Manager};
use tauri::{State, WebviewWindow};
use tokio::sync::oneshot;

use crate::tool_windows::ToolWindows;

#[derive(Default)]
struct PendingExit {
    attempt_id: Option<String>,
    windows: BTreeSet<String>,
    completion: Option<oneshot::Sender<bool>>,
}

#[derive(Clone, Default)]
pub(crate) struct ToolWindowExit(Arc<Mutex<PendingExit>>);

pub(crate) struct ToolWindowExitPermit {
    state: ToolWindowExit,
    attempt_id: String,
    committed: bool,
}
impl ToolWindowExitPermit {
    /// Keep new windows fenced while the authorized process exit is dispatched.
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for ToolWindowExitPermit {
    fn drop(&mut self) {
        if !self.committed
            && let Ok(mut pending) = self.state.0.lock()
            && pending.attempt_id.as_deref() == Some(self.attempt_id.as_str())
        {
            *pending = PendingExit::default();
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExitRequest {
    attempt_id: String,
}

impl ToolWindowExit {
    pub(crate) fn is_preparing(&self) -> bool {
        self.0
            .lock()
            .map_or(true, |pending| pending.attempt_id.is_some())
    }

    fn start(
        &self,
        windows: BTreeSet<String>,
    ) -> Result<(ToolWindowExitPermit, oneshot::Receiver<bool>), ()> {
        let mut pending = self.0.lock().map_err(|_| ())?;
        if pending.attempt_id.is_some() {
            return Err(());
        }
        let attempt_id = uuid::Uuid::new_v4().to_string();
        let (completion, receiver) = oneshot::channel();
        *pending = PendingExit {
            attempt_id: Some(attempt_id.clone()),
            windows,
            completion: Some(completion),
        };
        if pending.windows.is_empty()
            && let Some(completion) = pending.completion.take()
        {
            let _ = completion.send(true);
        }
        Ok((
            ToolWindowExitPermit {
                state: self.clone(),
                attempt_id,
                committed: false,
            },
            receiver,
        ))
    }

    pub(crate) fn destroyed(&self, label: &str) {
        if let Ok(mut pending) = self.0.lock()
            && pending.windows.remove(label)
            && pending.windows.is_empty()
            && let Some(completion) = pending.completion.take()
        {
            let _ = completion.send(true);
        }
    }

    fn reply(&self, label: &str, attempt_id: &str, approved: bool) -> Result<(), String> {
        let mut pending = self.0.lock().map_err(|_| "unavailable")?;
        if pending.attempt_id.as_deref() != Some(attempt_id) || !pending.windows.contains(label) {
            return Err("unavailable".into());
        }
        // Approval alone is insufficient: successful destruction is the cleanup barrier.
        if !approved && let Some(completion) = pending.completion.take() {
            let _ = completion.send(false);
        }
        Ok(())
    }

    pub(crate) async fn prepare<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        windows: &ToolWindows,
    ) -> Result<ToolWindowExitPermit, ()> {
        // Starting the attempt fences window creation before taking the live inventory.
        let (permit, _) = self.start(BTreeSet::new())?;
        let labels = live_tool_labels(app)
            .into_iter()
            .filter(|label| windows.contains(label))
            .collect::<BTreeSet<_>>();
        let (completion, receiver) = oneshot::channel();
        {
            let mut pending = self.0.lock().map_err(|_| ())?;
            pending.windows = labels.clone();
            pending.completion = Some(completion);
            if labels.is_empty()
                && let Some(completion) = pending.completion.take()
            {
                let _ = completion.send(true);
            }
        }
        for label in labels {
            #[cfg(not(target_os = "android"))]
            {
                let Some(window) = app.get_webview_window(&label) else {
                    self.destroyed(&label);
                    continue;
                };
                // An unresponsive renderer or failed event delivery cancels the exit, never the draft.
                if !crate::window_first_show::show_if_revealed(&window).map_err(|_| ())? {
                    return Err(());
                }
                window
                    .emit(
                        "tool-window-exit-requested",
                        ExitRequest {
                            attempt_id: permit.attempt_id.clone(),
                        },
                    )
                    .map_err(|_| ())?;
            }
            #[cfg(target_os = "android")]
            {
                use crate::secure_surface::android as surface;
                let attempt_id = permit.attempt_id.clone();
                // Bridge calls wait for the Android UI thread; keep them off the runtime.
                let delivered = tauri::async_runtime::spawn_blocking(move || {
                    // Every editor gets the request; the surface that shows a
                    // draft confirmation is brought forward.
                    surface::bring_forward(&label)
                        .and_then(|()| {
                            surface::post_event(
                                &label,
                                "tool-window-exit-requested",
                                ExitRequest { attempt_id },
                            )
                        })
                        .map_err(|_| label)
                })
                .await
                .map_err(|_| ())?;
                if let Err(label) = delivered {
                    if surface::is_alive(&label) {
                        // Failed delivery cancels the exit, never the draft.
                        return Err(());
                    }
                    self.destroyed(&label);
                }
            }
        }
        match tokio::time::timeout(Duration::from_secs(120), receiver).await {
            Ok(Ok(true)) => Ok(permit),
            _ => Err(()),
        }
    }
}

/// Labels of every live tool surface: native windows on desktop, Kotlin
/// Secure Surfaces on Android.
fn live_tool_labels<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<String> {
    #[cfg(not(target_os = "android"))]
    {
        app.webview_windows().into_keys().collect()
    }
    #[cfg(target_os = "android")]
    {
        let _ = app;
        crate::secure_surface::android::live_labels("tool-")
    }
}

/// Android: the Kotlin-bound tool surface answers the exit request.
#[cfg(target_os = "android")]
pub(crate) fn tool_window_exit_reply_for_label(
    label: &str,
    windows: &ToolWindows,
    state: &ToolWindowExit,
    attempt_id: &str,
    approved: bool,
) -> Result<(), String> {
    if !windows.contains(label) {
        return Err("unavailable".into());
    }
    state.reply(label, attempt_id, approved)
}

#[tauri::command]
pub(crate) fn tool_window_exit_reply(
    window: WebviewWindow,
    windows: State<'_, ToolWindows>,
    state: State<'_, ToolWindowExit>,
    attempt_id: String,
    approved: bool,
) -> Result<(), String> {
    if !windows.contains(window.label()) {
        return Err("unavailable".into());
    }
    state.reply(window.label(), &attempt_id, approved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approval_waits_for_exact_window_destruction() {
        let state = ToolWindowExit::default();
        let (permit, mut result) = state.start(BTreeSet::from(["tool-a".into()])).unwrap();
        assert!(state.reply("main", &permit.attempt_id, true).is_err());
        assert!(state.reply("tool-a", "stale", false).is_err());
        state.reply("tool-a", &permit.attempt_id, true).unwrap();
        assert!(result.try_recv().is_err());
        state.destroyed("main");
        assert!(result.try_recv().is_err());
        state.destroyed("tool-a");
        assert!(result.await.unwrap());
        assert!(state.is_preparing());
        drop(permit);
        assert!(!state.is_preparing());
    }

    #[tokio::test]
    async fn refusal_cancels_without_releasing_creation_fence_early() {
        let state = ToolWindowExit::default();
        let (permit, result) = state
            .start(BTreeSet::from(["tool-a".into(), "tool-b".into()]))
            .unwrap();
        state.reply("tool-b", &permit.attempt_id, false).unwrap();
        assert!(!result.await.unwrap());
        assert!(state.is_preparing());
        drop(permit);
        assert!(!state.is_preparing());
    }
}
