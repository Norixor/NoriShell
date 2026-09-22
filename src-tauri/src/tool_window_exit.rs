//! Application exit waits for child editors to finish their own draft cleanup.
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use tauri::{Emitter, Manager, State, WebviewWindow};
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

    pub(crate) async fn prepare(
        &self,
        app: &tauri::AppHandle,
        windows: &ToolWindows,
    ) -> Result<ToolWindowExitPermit, ()> {
        // Starting the attempt fences window creation before taking the live inventory.
        let (permit, _) = self.start(BTreeSet::new())?;
        let labels = app
            .webview_windows()
            .into_keys()
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
            let Some(window) = app.get_webview_window(&label) else {
                self.destroyed(&label);
                continue;
            };
            // An unresponsive renderer or failed event delivery cancels the exit, never the draft.
            window.show().map_err(|_| ())?;
            window.set_focus().map_err(|_| ())?;
            window
                .emit(
                    "tool-window-exit-requested",
                    ExitRequest {
                        attempt_id: permit.attempt_id.clone(),
                    },
                )
                .map_err(|_| ())?;
        }
        match tokio::time::timeout(Duration::from_secs(120), receiver).await {
            Ok(Ok(true)) => Ok(permit),
            _ => Err(()),
        }
    }
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
