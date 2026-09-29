//! Core-owned ordinary windows and in-process Tab ownership. Payloads are non-secret UI
//! projections; terminal output, input, and credentials remain with their Core owners.

use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, atomic::Ordering},
};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, State, Webview, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const PAGE: &str = "workspace-window.html";
const CLOSE_REQUESTED: &str = "workspace-window-close-requested";
const TAB_CHANGED: &str = "workspace-tab-state-changed";
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabRecord {
    id: String,
    kind: String,
    owner: String,
    payload: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabSnapshot {
    owned: Vec<TabRecord>,
    others: Vec<TabOwner>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabOwner {
    id: String,
    kind: String,
    owner: String,
    terminal_panes: Vec<TerminalPaneOwner>,
    file_sessions: Vec<FileSessionOwner>,
    desktop_sessions: Vec<DesktopSessionOwner>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalPaneOwner {
    pane_id: String,
    kind: String,
    session_id: Option<String>,
    generation: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileSessionOwner {
    session_id: String,
    generation: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopSessionOwner {
    session_id: String,
    generation: String,
}

fn desktop_session_owners(tab: &TabRecord) -> Vec<DesktopSessionOwner> {
    if tab.kind != "desktop"
        || tab.payload.get("schemaVersion") != Some(&Value::from(1))
        || tab.payload.get("tabId").and_then(Value::as_str) != Some(tab.id.as_str())
        || tab
            .payload
            .as_object()
            .is_none_or(|payload| payload.len() != 4)
    {
        return Vec::new();
    }
    let Some(session_id) = tab.payload.get("sessionId").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(generation) = tab.payload.get("generation").and_then(Value::as_str) else {
        return Vec::new();
    };
    if session_id.is_empty()
        || session_id.len() > 128
        || session_id.chars().any(char::is_control)
        || generation.is_empty()
        || generation.len() > 20
        || !generation.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Vec::new();
    }
    vec![DesktopSessionOwner {
        session_id: session_id.into(),
        generation: generation.into(),
    }]
}

fn file_session_owners(tab: &TabRecord) -> Vec<FileSessionOwner> {
    if tab.kind != "file"
        || tab.payload.get("version") != Some(&Value::from(1))
        || tab.payload.pointer("/tab/groupId").and_then(Value::as_str) != Some(tab.id.as_str())
    {
        return Vec::new();
    }
    let Some(panes) = tab.payload.get("panes").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    panes
        .iter()
        .filter_map(|pane| {
            let endpoint = pane.get("endpoint")?;
            if endpoint.get("kind")?.as_str()? != "remote" {
                return None;
            }
            let session_id = endpoint.get("sessionId")?.as_str()?;
            let generation = endpoint.get("generation")?.as_str()?;
            if session_id.is_empty()
                || session_id.len() > 128
                || session_id.chars().any(char::is_control)
                || generation.is_empty()
                || generation.len() > 128
                || generation.chars().any(char::is_control)
                || !seen.insert(session_id.to_owned())
            {
                return None;
            }
            Some(FileSessionOwner {
                session_id: session_id.into(),
                generation: generation.into(),
            })
        })
        .collect()
}

fn terminal_pane_owners(tab: &TabRecord) -> Vec<TerminalPaneOwner> {
    if tab.kind != "terminal" || tab.payload.get("schemaVersion") != Some(&Value::from(1)) {
        return Vec::new();
    }
    let Some(panes) = tab.payload.get("panes").and_then(Value::as_array) else {
        return Vec::new();
    };
    panes
        .iter()
        .filter_map(|pane| {
            let pane_id = pane.get("paneId")?.as_str()?;
            let kind = pane.get("kind")?.as_str()?;
            if pane_id.is_empty()
                || pane_id.len() > 128
                || !matches!(kind, "ssh" | "local" | "telnet" | "plugin")
            {
                return None;
            }
            let session_id = pane.get("sessionId")?.as_str()?;
            let generation = pane.get("generation")?.as_str()?;
            if session_id.is_empty()
                || session_id.len() > 128
                || generation.is_empty()
                || generation.len() > 128
            {
                return None;
            }
            Some(TerminalPaneOwner {
                pane_id: pane_id.into(),
                kind: kind.into(),
                session_id: Some(session_id.into()),
                generation: Some(generation.into()),
            })
        })
        .collect()
}

#[derive(Default)]
struct Registry {
    windows: BTreeSet<String>,
    closing: BTreeSet<String>,
    tabs: BTreeMap<String, TabRecord>,
    native_moves: BTreeMap<String, (String, String)>,
    native_creates: BTreeMap<String, String>,
    native_closes: BTreeMap<String, String>,
}

impl Registry {
    fn allows(&self, label: &str) -> bool {
        label == "main" || (self.windows.contains(label) && !self.closing.contains(label))
    }

    fn pending(&self, id: &str) -> bool {
        self.native_creates.contains_key(id)
            || self.native_closes.contains_key(id)
            || self.native_moves.contains_key(id)
    }

    /// Returns whether the stored projection changed.
    fn update_child_projection(
        &mut self,
        owner: &str,
        id: &str,
        kind: &str,
        payload: Value,
    ) -> Result<bool, String> {
        if !self.allows(owner) {
            return Err("workspace_tab.denied".into());
        }
        validate_identity(id, kind)?;
        validate_payload(&payload)?;
        if self.pending(id) {
            return Err("workspace_tab.transfer_pending".into());
        }
        let record = self.tabs.get_mut(id).ok_or("workspace_tab.missing")?;
        if record.owner != owner {
            return Err("workspace_tab.wrong_owner".into());
        }
        if record.kind != kind {
            return Err("workspace_tab.invalid_identity".into());
        }
        if record.payload == payload {
            return Ok(false);
        }
        record.payload = payload;
        Ok(true)
    }

    fn swap_child_projection(
        &mut self,
        owner: &str,
        id: &str,
        kind: &str,
        expected: &Value,
        payload: Value,
    ) -> Result<bool, String> {
        let current = self.tabs.get(id).ok_or("workspace_tab.missing")?;
        if current.payload != *expected {
            return Err("workspace_tab.projection_changed".into());
        }
        self.update_child_projection(owner, id, kind, payload)
    }

    fn snapshot(&self, caller: &str) -> Result<TabSnapshot, String> {
        if !self.allows(caller) {
            return Err("workspace_tab.denied".into());
        }
        Ok(TabSnapshot {
            owned: self
                .tabs
                .values()
                .filter(|tab| tab.owner == caller)
                .cloned()
                .collect(),
            others: self
                .tabs
                .values()
                .filter(|tab| tab.owner != caller)
                .map(|tab| TabOwner {
                    id: tab.id.clone(),
                    kind: tab.kind.clone(),
                    owner: tab.owner.clone(),
                    terminal_panes: terminal_pane_owners(tab),
                    file_sessions: file_session_owners(tab),
                    desktop_sessions: desktop_session_owners(tab),
                })
                .collect(),
        })
    }

    fn reserve_close(&mut self, label: &str) -> Result<(), String> {
        if !self.windows.contains(label) {
            return Err("workspace_window.denied".into());
        }
        if self.closing.contains(label) {
            return Err("workspace_window.close_in_progress".into());
        }
        self.closing.insert(label.into());
        Ok(())
    }

    fn confirm_close(
        &mut self,
        label: &str,
        native_views: &BTreeSet<String>,
    ) -> Result<(), String> {
        if !self.windows.contains(label) || !self.closing.contains(label) {
            return Err("workspace_window.denied".into());
        }
        if self
            .tabs
            .values()
            .any(|tab| tab.owner == label && native_views.contains(&tab.id))
        {
            return Err("workspace_window.tabs_owned".into());
        }
        if self
            .native_moves
            .values()
            .any(|(source, target)| source == label || target == label)
            || self.native_creates.values().any(|owner| owner == label)
            || self.native_closes.values().any(|owner| owner == label)
        {
            return Err("workspace_window.transfer_pending".into());
        }
        Ok(())
    }

    /// A destroyed workspace loses no Tab metadata. Main becomes the recovery owner.
    fn recover_destroyed(&mut self, label: &str) -> Vec<String> {
        if !self.windows.remove(label) {
            return Vec::new();
        }
        self.closing.remove(label);
        let mut changed = Vec::new();
        for tab in self.tabs.values_mut().filter(|tab| tab.owner == label) {
            tab.owner = "main".into();
            changed.push(tab.id.clone());
        }
        changed.sort();
        changed.dedup();
        changed
    }
}

fn validate_identity(id: &str, kind: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 512
        || id.chars().any(char::is_control)
        || !matches!(kind, "terminal" | "file" | "desktop" | "page")
    {
        return Err("workspace_tab.invalid_identity".into());
    }
    Ok(())
}

fn validate_payload(payload: &Value) -> Result<(), String> {
    if serde_json::to_vec(payload)
        .map_err(|_| "workspace_tab.invalid_payload")?
        .len()
        > MAX_PAYLOAD_BYTES
    {
        return Err("workspace_tab.payload_too_large".into());
    }
    Ok(())
}

#[derive(Clone, Default)]
pub(crate) struct WorkspaceWindows(Arc<Mutex<Registry>>);

impl WorkspaceWindows {
    pub(crate) fn swap_child_projection(
        &self,
        app: &AppHandle,
        owner: &str,
        id: &str,
        kind: &str,
        expected: &Value,
        payload: Value,
    ) -> Result<(), String> {
        let changed = with_registry(self, |registry| {
            registry.swap_child_projection(owner, id, kind, expected, payload)
        })?;
        if changed {
            notify_tab(app, self, id);
        }
        Ok(())
    }

    pub(crate) fn has_live_desktop_session(
        &self,
        owner: &str,
        id: &str,
        service: &crate::desktop_service::DesktopService,
    ) -> Result<bool, String> {
        let registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        let tab = registry.tabs.get(id).ok_or("workspace_tab.missing")?;
        if tab.owner != owner {
            return Err("workspace_tab.wrong_owner".into());
        }
        let sessions = desktop_session_owners(tab);
        drop(registry);
        Ok(service.snapshot().iter().any(|session| {
            sessions.iter().any(|owned| {
                owned.session_id == session.id
                    && owned.generation == session.generation.get().to_string()
            })
        }))
    }

    pub(crate) fn owns_desktop_session(
        &self,
        owner: &str,
        id: &str,
        session_id: &str,
        generation: u64,
    ) -> Result<bool, String> {
        let registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        let tab = registry.tabs.get(id).ok_or("workspace_tab.missing")?;
        if tab.owner != owner || tab.kind != "desktop" || registry.pending(id) {
            return Err("workspace_tab.wrong_owner".into());
        }
        Ok(desktop_session_owners(tab).iter().any(|owned| {
            owned.session_id == session_id && owned.generation == generation.to_string()
        }))
    }

    pub(crate) fn begin_close_with_views(
        &self,
        label: &str,
        views: &crate::workspace_tab_views::WorkspaceTabViews,
    ) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "workspace_window.unavailable")?
            .reserve_close(label)?;
        let native_views = match views.owned_ids(label) {
            Ok(ids) => ids,
            Err(error) => {
                if let Ok(mut registry) = self.0.lock() {
                    registry.closing.remove(label);
                }
                return Err(error);
            }
        };
        let mut registry = self.0.lock().map_err(|_| "workspace_window.unavailable")?;
        let result = registry.confirm_close(label, &native_views);
        if result.is_err() {
            registry.closing.remove(label);
        }
        result
    }

    pub(crate) fn is_main_or_page<R: Runtime>(&self, webview: &Webview<R>) -> bool {
        let window = webview.window();
        if webview.label() == "main" && window.label() == "main" {
            return true;
        }
        let Some(id) = crate::workspace_tab_views::tab_id_from_view_label(webview.label()) else {
            return false;
        };
        self.0.lock().is_ok_and(|registry| {
            registry.allows(window.label())
                && !registry.pending(&id)
                && registry
                    .tabs
                    .get(&id)
                    .is_some_and(|tab| tab.kind == "page" && tab.owner == window.label())
        })
    }

    pub(crate) fn begin_native_create(
        &self,
        owner: &str,
        id: &str,
        kind: &str,
        payload: &Value,
    ) -> Result<bool, String> {
        validate_identity(id, kind)?;
        validate_payload(payload)?;
        let mut registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        if !registry.allows(owner) {
            return Err("workspace_tab.denied".into());
        }
        if registry.pending(id) {
            return Err("workspace_tab.transfer_pending".into());
        }
        let existing = match registry.tabs.get(id) {
            Some(tab) if tab.owner != owner || tab.kind != kind => {
                return Err("workspace_tab.conflict".into());
            }
            Some(_) => true,
            None => false,
        };
        registry.native_creates.insert(id.into(), owner.into());
        Ok(existing)
    }

    pub(crate) fn finish_native_create(
        &self,
        owner: &str,
        id: &str,
        kind: &str,
        payload: Value,
    ) -> Result<(), String> {
        let mut registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        if registry.native_creates.remove(id).as_deref() != Some(owner) {
            return Err("workspace_tab.create_not_pending".into());
        }
        if !registry.allows(owner) {
            return Err("workspace_tab.denied".into());
        }
        if let Some(tab) = registry.tabs.get(id) {
            return if tab.owner == owner && tab.kind == kind {
                Ok(())
            } else {
                Err("workspace_tab.conflict".into())
            };
        }
        registry.tabs.insert(
            id.into(),
            TabRecord {
                id: id.into(),
                kind: kind.into(),
                owner: owner.into(),
                payload,
            },
        );
        Ok(())
    }

    pub(crate) fn abort_native_create(&self, id: &str) {
        if let Ok(mut registry) = self.0.lock() {
            registry.native_creates.remove(id);
        }
    }

    /// Verifies that `caller` still owns a settled native view of `kind`.
    pub(crate) fn check_native_view(
        &self,
        caller: &str,
        id: &str,
        kind: &str,
    ) -> Result<(), String> {
        let registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        if !registry.allows(caller) {
            return Err("workspace_tab.denied".into());
        }
        let tab = registry.tabs.get(id).ok_or("workspace_tab.missing")?;
        if tab.owner != caller {
            return Err("workspace_tab.wrong_owner".into());
        }
        if tab.kind != kind {
            return Err("workspace_tab.invalid_identity".into());
        }
        if registry.pending(id) {
            return Err("workspace_tab.transfer_pending".into());
        }
        Ok(())
    }

    pub(crate) fn move_native_view(
        &self,
        source: &str,
        target: &str,
        id: &str,
        reparent: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        {
            let mut registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
            if !registry.allows(source) || !registry.allows(target) {
                return Err("workspace_tab.target_missing".into());
            }
            if registry.pending(id) {
                return Err("workspace_tab.transfer_pending".into());
            }
            let tab = registry.tabs.get(id).ok_or("workspace_tab.missing")?;
            if tab.owner != source {
                return Err("workspace_tab.wrong_owner".into());
            }
            if source == target {
                return Ok(());
            }
            registry
                .native_moves
                .insert(id.into(), (source.into(), target.into()));
        }
        // Native dispatch waits for the main thread. Never hold the registry
        // mutex here: a concurrent WindowEvent may need it on that thread.
        let result = reparent();
        let mut registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        registry.native_moves.remove(id);
        result?;
        if !registry.allows(target) {
            if let Some(tab) = registry.tabs.get_mut(id) {
                tab.owner = "main".into();
            }
            return Err("workspace_tab.target_lost".into());
        }
        let tab = registry.tabs.get_mut(id).ok_or("workspace_tab.missing")?;
        tab.owner = target.into();
        Ok(())
    }

    pub(crate) fn close_native_view(
        &self,
        owner: &str,
        id: &str,
        close: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        {
            let mut registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
            if !registry.allows(owner) {
                return Err("workspace_tab.denied".into());
            }
            if registry.pending(id) {
                return Err("workspace_tab.transfer_pending".into());
            }
            let tab = registry.tabs.get(id).ok_or("workspace_tab.missing")?;
            if tab.owner != owner {
                return Err("workspace_tab.wrong_owner".into());
            }
            registry.native_closes.insert(id.into(), owner.into());
        }
        let result = close();
        let mut registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        registry.native_closes.remove(id);
        result?;
        registry.tabs.remove(id);
        Ok(())
    }

    pub(crate) fn owns_tab(&self, label: &str, id: &str) -> Result<(), String> {
        let registry = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        if !registry.allows(label) {
            return Err("workspace_tab.denied".into());
        }
        if registry.tabs.get(id).is_none_or(|tab| tab.owner != label) {
            return Err("workspace_tab.wrong_owner".into());
        }
        if registry.pending(id) {
            return Err("workspace_tab.transfer_pending".into());
        }
        Ok(())
    }

    pub(crate) fn contains(&self, label: &str) -> bool {
        self.0
            .lock()
            .is_ok_and(|registry| registry.windows.contains(label))
    }

    pub(crate) fn native_move_pending(&self, id: &str) -> bool {
        self.0
            .lock()
            .is_ok_and(|registry| registry.native_moves.contains_key(id))
    }

    pub(crate) fn window_labels(&self) -> Result<Vec<String>, String> {
        self.0
            .lock()
            .map(|registry| registry.windows.iter().cloned().collect())
            .map_err(|_| "workspace_window.unavailable".to_owned())
    }

    fn forget(&self, label: &str) -> Vec<String> {
        self.0
            .lock()
            .map(|mut registry| registry.recover_destroyed(label))
            .unwrap_or_default()
    }
}

fn notify_tab<R: Runtime>(app: &AppHandle<R>, state: &WorkspaceWindows, id: &str) {
    let mut recipients = state.window_labels().unwrap_or_default();
    recipients.push("main".into());
    for recipient in recipients {
        let _ = app.emit_to(&recipient, TAB_CHANGED, id);
    }
}

fn caller_allowed(state: &WorkspaceWindows, label: &str) -> bool {
    state.0.lock().is_ok_and(|registry| registry.allows(label))
}

// Async keeps window creation off the Windows main thread, where a sync
// command that builds a WebView deadlocks (wry#583).
#[tauri::command]
pub(crate) async fn workspace_window_open(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<String, String> {
    if !caller_allowed(&state, window.label()) {
        return Err("workspace_window.denied".into());
    }
    let release_position = match (x, y) {
        (None, None) => None,
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some((x, y)),
        _ => return Err("workspace_window.invalid_position".into()),
    };
    if app
        .state::<crate::tool_window_exit::ToolWindowExit>()
        .is_preparing()
    {
        return Err("workspace_window.exit_in_progress".into());
    }
    let label = format!("workspace-window-{}", uuid::Uuid::now_v7());
    state
        .0
        .lock()
        .map_err(|_| "workspace_window.unavailable")?
        .windows
        .insert(label.clone());
    let expected = app
        .config()
        .build
        .dev_url
        .as_ref()
        .and_then(|url| url.join(PAGE).ok());
    let child = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(PAGE.into()))
        .title("NoriShell")
        .on_navigation(move |url| {
            (cfg!(debug_assertions) && expected.as_ref().is_some_and(|expected| expected == url))
                || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                    || (matches!(url.scheme(), "http" | "https")
                        && url.host_str() == Some("tauri.localhost")
                        && url.port().is_none()))
                    && url.path() == "/workspace-window.html"
                    && url.query().is_none())
        })
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    let default_size =
        crate::secure_window_frame::default_window_size(&app, (1100.0, 740.0), (800.0, 560.0));
    let result = crate::secure_window_frame::apply_secure_window_frame(&app, &label, child)
        .inner_size(default_size.0, default_size.1)
        .min_inner_size(800.0, 560.0)
        .build();
    let child = match result {
        Ok(child) => child,
        Err(_) => {
            for id in state.forget(&label) {
                notify_tab(&app, &state, &id);
            }
            #[cfg(any(windows, target_os = "macos"))]
            crate::window_first_show::forget(&app, &label);
            return Err("workspace_window.create_failed".into());
        }
    };
    if let Some((x, y)) = release_position {
        let scale = child.scale_factor().unwrap_or(1.0);
        let size = child.outer_size().ok();
        let monitors = child.available_monitors().unwrap_or_default();
        let monitor = monitors.iter().find(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            x >= f64::from(position.x)
                && x < f64::from(position.x) + f64::from(size.width)
                && y >= f64::from(position.y)
                && y < f64::from(position.y) + f64::from(size.height)
        });
        let (mut left, mut top) = (x - 110.0 * scale, y - 28.0 * scale);
        if let (Some(monitor), Some(size)) = (monitor, size) {
            let position = monitor.position();
            let bounds = monitor.size();
            left = left.clamp(
                f64::from(position.x),
                (f64::from(position.x) + f64::from(bounds.width) - f64::from(size.width))
                    .max(f64::from(position.x)),
            );
            top = top.clamp(
                f64::from(position.y),
                (f64::from(position.y) + f64::from(bounds.height) - f64::from(size.height))
                    .max(f64::from(position.y)),
            );
        }
        if child
            .set_position(tauri::PhysicalPosition::new(
                left.round() as i32,
                top.round() as i32,
            ))
            .is_err()
        {
            for id in state.forget(&label) {
                notify_tab(&app, &state, &id);
            }
            #[cfg(any(windows, target_os = "macos"))]
            crate::window_first_show::forget(&app, &label);
            let _ = child.destroy();
            return Err("workspace_window.position_failed".into());
        }
    }
    if app
        .state::<crate::tool_window_exit::ToolWindowExit>()
        .is_preparing()
    {
        for id in state.forget(&label) {
            notify_tab(&app, &state, &id);
        }
        let _ = child.destroy();
        return Err("workspace_window.exit_in_progress".into());
    }
    Ok(label)
}

#[tauri::command]
pub(crate) fn workspace_window_focus(
    webview: Webview,
    state: State<'_, WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    label: Option<String>,
) -> Result<(), String> {
    let window = webview.window();
    if !caller_allowed(&state, window.label()) {
        return Err("workspace_window.denied".into());
    }
    let target_label = label.as_deref().unwrap_or(window.label());
    if webview.label() != window.label() {
        let id = crate::workspace_tab_views::tab_id_from_view_label(webview.label())
            .ok_or("workspace_window.denied")?;
        if target_label != window.label()
            || views.child_owner(&webview, &state, &id)? != window.label()
        {
            return Err("workspace_window.denied".into());
        }
    }
    if !caller_allowed(&state, target_label) {
        return Err("workspace_window.denied".into());
    }
    let target = window
        .app_handle()
        .get_webview_window(target_label)
        .ok_or("workspace_window.missing")?;
    target.show().map_err(|_| "workspace_window.focus_failed")?;
    target
        .set_focus()
        .map_err(|_| "workspace_window.focus_failed".into())
}

#[tauri::command]
pub(crate) fn workspace_window_close(
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
) -> Result<(), String> {
    state.begin_close_with_views(window.label(), &views)?;
    if window.destroy().is_err() {
        if let Ok(mut registry) = state.0.lock() {
            registry.closing.remove(window.label());
        }
        return Err("workspace_window.close_failed".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn workspace_window_close_empty(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    label: String,
) -> Result<(), String> {
    if !caller_allowed(&state, window.label()) {
        return Err("workspace_window.denied".into());
    }
    let target = app
        .get_webview_window(&label)
        .ok_or("workspace_window.missing")?;
    state.begin_close_with_views(&label, &views)?;
    if target.destroy().is_err() {
        if let Ok(mut registry) = state.0.lock() {
            registry.closing.remove(&label);
        }
        return Err("workspace_window.close_failed".into());
    }
    Ok(())
}

fn with_registry<T>(
    state: &WorkspaceWindows,
    work: impl FnOnce(&mut Registry) -> Result<T, String>,
) -> Result<T, String> {
    let mut registry = state.0.lock().map_err(|_| "workspace_tab.unavailable")?;
    work(&mut registry)
}

#[tauri::command]
pub(crate) async fn workspace_tab_projection_update(
    app: AppHandle,
    webview: Webview,
    state: State<'_, WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    payload: Value,
) -> Result<(), String> {
    let (id, changed) = {
        let (operation, live) = views.projection_operation(&webview)?;
        // Native reparenting holds this lock until the Core owner is committed.
        // Wait without holding the view table or the Registry mutex.
        let _operation = operation.lock().map_err(|_| "workspace_tab.unavailable")?;
        if !live.load(Ordering::Acquire) {
            return Err("workspace_tab.view_missing".into());
        }
        let (id, kind, owner) = views.projection_identity(&webview)?;
        // A Desktop Tab cannot drop the handle of a session that is still live;
        // the session must be closed first so it never becomes unreachable.
        if kind == "desktop"
            && payload.get("sessionId").is_none()
            && let Some(desktop) = app.try_state::<crate::desktop_service::DesktopService>()
            && state.has_live_desktop_session(&owner, &id, &desktop)?
        {
            return Err("workspace_tab.desktop_session_active".into());
        }
        let changed = with_registry(&state, |registry| {
            registry.update_child_projection(&owner, &id, &kind, payload)
        })?;
        (id, changed)
    };
    if changed {
        notify_tab(&app, &state, &id);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn workspace_tab_snapshot(
    webview: Webview,
    state: State<'_, WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
) -> Result<TabSnapshot, String> {
    let window = webview.window();
    if webview.label() == window.label() {
        return with_registry(&state, |registry| registry.snapshot(window.label()));
    }
    let id = crate::workspace_tab_views::tab_id_from_view_label(webview.label())
        .ok_or("workspace_tab.denied")?;
    let owner = views.child_owner(&webview, &state, &id)?;
    let mut snapshot = with_registry(&state, |registry| registry.snapshot(&owner))?;
    snapshot.owned.retain(|tab| tab.id == id);
    snapshot.others.clear();
    Ok(snapshot)
}

pub(crate) fn on_window_event<R: Runtime>(window: &tauri::Window<R>, event: &WindowEvent) {
    let Some(state) = window.try_state::<WorkspaceWindows>() else {
        return;
    };
    if !state.contains(window.label()) {
        return;
    }
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            if window
                .try_state::<crate::lifecycle::LifecycleState>()
                .is_some_and(|lifecycle| lifecycle.is_exit_authorized())
            {
                return;
            }
            api.prevent_close();
            let _ = window
                .app_handle()
                .emit_to(window.label(), CLOSE_REQUESTED, ());
        }
        WindowEvent::Destroyed => {
            for id in state.forget(window.label()) {
                notify_tab(window.app_handle(), &state, &id);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn insert(registry: &mut Registry, owner: &str, id: &str, kind: &str, payload: Value) {
        registry.tabs.insert(
            id.into(),
            TabRecord {
                id: id.into(),
                kind: kind.into(),
                owner: owner.into(),
                payload,
            },
        );
    }

    fn terminal(registry: &mut Registry, owner: &str, id: &str) {
        insert(registry, owner, id, "terminal", json!({"paneIds":["one"]}));
    }

    #[test]
    fn desktop_claim_requires_current_idle_projection_before_session_creation() {
        let mut registry = Registry::default();
        let idle = json!({"schemaVersion":1,"tabId":"desktop:one","profileId":"profile-one"});
        let owned = json!({"schemaVersion":1,"tabId":"desktop:one","sessionId":"session-one","generation":"1"});
        insert(
            &mut registry,
            "main",
            "desktop:one",
            "desktop",
            idle.clone(),
        );
        assert_eq!(
            registry.swap_child_projection(
                "main",
                "desktop:one",
                "desktop",
                &json!({"schemaVersion":1,"tabId":"desktop:one","profileId":"other"}),
                owned.clone()
            ),
            Err("workspace_tab.projection_changed".into())
        );
        assert_eq!(registry.tabs["desktop:one"].payload, idle);
        assert_eq!(
            registry.swap_child_projection("main", "desktop:one", "desktop", &idle, owned.clone()),
            Ok(true)
        );
        assert_eq!(registry.tabs["desktop:one"].payload, owned);
        assert_eq!(
            registry.swap_child_projection(
                "main",
                "desktop:one",
                "desktop",
                &idle,
                json!({"schemaVersion":1,"tabId":"desktop:one","sessionId":"second","generation":"1"})
            ),
            Err("workspace_tab.projection_changed".into())
        );
        assert_eq!(registry.tabs["desktop:one"].payload, owned);
    }

    #[test]
    fn desktop_close_handle_must_match_registry_generation() {
        let state = WorkspaceWindows::default();
        insert(
            &mut state.0.lock().unwrap(),
            "main",
            "desktop:one",
            "desktop",
            json!({"schemaVersion":1,"tabId":"desktop:one","sessionId":"session-one","generation":"7"}),
        );
        assert_eq!(
            state.owns_desktop_session("main", "desktop:one", "session-one", 7),
            Ok(true)
        );
        assert_eq!(
            state.owns_desktop_session("main", "desktop:one", "session-one", 8),
            Ok(false)
        );
        assert_eq!(
            state.owns_desktop_session("main", "desktop:one", "other", 7),
            Ok(false)
        );
        assert_eq!(
            state.owns_desktop_session("other", "desktop:one", "session-one", 7),
            Err("workspace_tab.wrong_owner".into())
        );
    }

    #[test]
    fn native_move_commits_owner_only_after_reparent_succeeds() {
        let state = WorkspaceWindows::default();
        {
            let mut registry = state.0.lock().unwrap();
            registry.windows.insert("workspace-a".into());
            terminal(&mut registry, "main", "one");
        }
        assert_eq!(
            state.move_native_view("main", "workspace-a", "one", || {
                Err("workspace_tab.reparent_failed".into())
            }),
            Err("workspace_tab.reparent_failed".into())
        );
        assert_eq!(state.check_native_view("main", "one", "terminal"), Ok(()));
        let during_reparent = state.clone();
        assert_eq!(
            state.move_native_view("main", "workspace-a", "one", || {
                assert_eq!(
                    during_reparent.check_native_view("main", "one", "terminal"),
                    Err("workspace_tab.transfer_pending".into())
                );
                Ok(())
            }),
            Ok(())
        );
        assert_eq!(
            state.check_native_view("workspace-a", "one", "terminal"),
            Ok(())
        );
        assert_eq!(
            state.check_native_view("main", "one", "terminal"),
            Err("workspace_tab.wrong_owner".into())
        );
        assert_eq!(
            state.move_native_view("main", "workspace-a", "one", || Ok(())),
            Err("workspace_tab.wrong_owner".into())
        );
    }

    #[test]
    fn lost_target_keeps_tab_record_for_main_attach() {
        let state = WorkspaceWindows::default();
        {
            let mut registry = state.0.lock().unwrap();
            registry.windows.insert("workspace-a".into());
            terminal(&mut registry, "main", "one");
        }
        let during_reparent = state.clone();
        assert_eq!(
            state.move_native_view("main", "workspace-a", "one", || {
                assert!(during_reparent.native_move_pending("one"));
                during_reparent
                    .0
                    .lock()
                    .unwrap()
                    .windows
                    .remove("workspace-a");
                Ok(())
            }),
            Err("workspace_tab.target_lost".into())
        );
        let recovered = state.0.lock().unwrap().tabs["one"].clone();
        assert_eq!(recovered.owner, "main");
        assert_eq!(recovered.payload, json!({"paneIds":["one"]}));
        assert!(!state.native_move_pending("one"));
    }

    #[test]
    fn lost_source_after_failed_reparent_keeps_record_for_main_attach() {
        let state = WorkspaceWindows::default();
        {
            let mut registry = state.0.lock().unwrap();
            registry.windows.insert("workspace-a".into());
            registry.windows.insert("workspace-b".into());
            terminal(&mut registry, "workspace-a", "one");
        }
        let during_reparent = state.clone();
        assert_eq!(
            state.move_native_view("workspace-a", "workspace-b", "one", || {
                assert!(during_reparent.native_move_pending("one"));
                assert_eq!(
                    during_reparent
                        .0
                        .lock()
                        .unwrap()
                        .recover_destroyed("workspace-a"),
                    vec!["one"]
                );
                Err("workspace_tab.reparent_failed".into())
            }),
            Err("workspace_tab.reparent_failed".into())
        );
        let recovered = state.0.lock().unwrap().tabs["one"].clone();
        assert_eq!(recovered.owner, "main");
        assert_eq!(recovered.payload, json!({"paneIds":["one"]}));
        assert!(!state.native_move_pending("one"));
    }

    #[test]
    fn native_create_and_close_keep_core_record_atomic_with_webview() {
        let state = WorkspaceWindows::default();
        let seed = json!({"id":"one"});
        assert_eq!(
            state.begin_native_create("main", "one", "terminal", &seed),
            Ok(false)
        );
        assert_eq!(
            state.check_native_view("main", "one", "terminal"),
            Err("workspace_tab.missing".into())
        );
        assert_eq!(
            state.begin_native_create("main", "one", "terminal", &seed),
            Err("workspace_tab.transfer_pending".into())
        );
        state.abort_native_create("one");
        assert_eq!(
            state.begin_native_create("main", "one", "terminal", &seed),
            Ok(false)
        );
        assert_eq!(
            state.finish_native_create("main", "one", "terminal", seed.clone()),
            Ok(())
        );
        assert_eq!(
            state.begin_native_create("main", "one", "terminal", &seed),
            Ok(true)
        );
        state.abort_native_create("one");
        assert_eq!(
            state.close_native_view("main", "one", || Err(
                "workspace_tab.view_close_failed".into()
            )),
            Err("workspace_tab.view_close_failed".into())
        );
        assert_eq!(state.check_native_view("main", "one", "terminal"), Ok(()));
        assert_eq!(state.close_native_view("main", "one", || Ok(())), Ok(()));
        assert_eq!(
            state.check_native_view("main", "one", "terminal"),
            Err("workspace_tab.missing".into())
        );
    }

    #[test]
    fn foreign_terminal_projection_exposes_only_live_session_identity() {
        let mut registry = Registry::default();
        registry.windows.insert("workspace-a".into());
        insert(
            &mut registry,
            "workspace-a",
            "terminal-one",
            "terminal",
            json!({
                "schemaVersion": 1,
                "tabId": "terminal-one",
                "secret": "must-not-leak",
                "panes": [
                    {"kind":"ssh","paneId":"pane-one","sessionId":"session-one","generation":"7","label":"Host"},
                    {"kind":"launcher","paneId":"pane-two","label":"New"}
                ]
            }),
        );
        let snapshot = serde_json::to_value(registry.snapshot("main").unwrap().others).unwrap();
        assert_eq!(
            snapshot,
            json!([{
                "id":"terminal-one", "kind":"terminal", "owner":"workspace-a",
                "terminalPanes":[{"paneId":"pane-one","kind":"ssh","sessionId":"session-one","generation":"7"}],
                "fileSessions":[], "desktopSessions":[]
            }])
        );
    }

    #[test]
    fn foreign_file_projection_exposes_only_remote_session_identity() {
        let mut registry = Registry::default();
        registry.windows.insert("workspace-a".into());
        insert(
            &mut registry,
            "workspace-a",
            "file:one",
            "file",
            json!({
                "version": 1,
                "tab": {"groupId": "file:one"},
                "secret": "must-not-leak",
                "panes": [
                    {"endpoint": {"kind":"remote","hostId":"private-host","sessionId":"session-one","generation":"7"}},
                    {"endpoint": {"kind":"local","directoryRef":"private-directory"}},
                    {"endpoint": {"kind":"remote","sessionId":"session-one","generation":"7"}}
                ]
            }),
        );
        let snapshot = serde_json::to_value(registry.snapshot("main").unwrap().others).unwrap();
        assert_eq!(
            snapshot,
            json!([{
                "id":"file:one", "kind":"file", "owner":"workspace-a",
                "terminalPanes":[], "fileSessions":[{"sessionId":"session-one","generation":"7"}],
                "desktopSessions":[]
            }])
        );
    }

    #[test]
    fn foreign_desktop_projection_exposes_only_bounded_session_handle() {
        let mut registry = Registry::default();
        registry.windows.insert("workspace-a".into());
        insert(
            &mut registry,
            "workspace-a",
            "desktop:stable-tab",
            "desktop",
            json!({
                "schemaVersion": 1,
                "tabId": "desktop:stable-tab",
                "sessionId": "reconnected-session",
                "generation": "7"
            }),
        );
        let snapshot = serde_json::to_value(registry.snapshot("main").unwrap().others).unwrap();
        assert_eq!(
            snapshot,
            json!([{
                "id":"desktop:stable-tab", "kind":"desktop", "owner":"workspace-a",
                "terminalPanes":[], "fileSessions":[],
                "desktopSessions":[{"sessionId":"reconnected-session","generation":"7"}]
            }])
        );
        registry
            .update_child_projection(
                "workspace-a",
                "desktop:stable-tab",
                "desktop",
                json!({
                    "schemaVersion": 1,
                    "tabId": "desktop:stable-tab",
                    "sessionId": "next-session",
                    "generation": "8"
                }),
            )
            .unwrap();
        let latest = serde_json::to_value(registry.snapshot("main").unwrap().others).unwrap();
        assert_eq!(
            latest[0]["desktopSessions"],
            json!([{"sessionId":"next-session","generation":"8"}])
        );
        let tab = registry.tabs.get_mut("desktop:stable-tab").unwrap();
        tab.payload["generation"] = Value::from("not-a-generation");
        assert!(desktop_session_owners(tab).is_empty());
    }

    #[test]
    fn close_blocks_native_views_but_recovers_native_less_tabs_to_main() {
        let state = WorkspaceWindows::default();
        let mut registry = state.0.lock().unwrap();
        registry.windows.insert("workspace-a".into());
        registry.windows.insert("workspace-b".into());
        terminal(&mut registry, "workspace-a", "one");
        let initial = registry.tabs["one"].clone();
        registry.reserve_close("workspace-a").unwrap();
        assert_eq!(
            registry.confirm_close("workspace-a", &BTreeSet::from(["one".into()])),
            Err("workspace_window.tabs_owned".into())
        );
        registry.closing.remove("workspace-a");
        registry.reserve_close("workspace-a").unwrap();
        assert_eq!(
            registry.confirm_close("workspace-a", &BTreeSet::new()),
            Ok(())
        );
        registry.closing.remove("workspace-a");
        assert_eq!(registry.recover_destroyed("workspace-a"), vec!["one"]);
        let recovered = &registry.snapshot("main").unwrap().owned[0];
        assert_eq!(recovered.owner, "main");
        assert_eq!(recovered.payload, initial.payload);
        registry.reserve_close("workspace-b").unwrap();
        registry
            .confirm_close("workspace-b", &BTreeSet::new())
            .unwrap();
        drop(registry);
        assert_eq!(
            state.begin_native_create("workspace-b", "two", "terminal", &json!({})),
            Err("workspace_tab.denied".into())
        );
    }

    #[test]
    fn payload_follows_existing_workspace_limit() {
        validate_payload(&json!({"value":"x".repeat(250_000)})).unwrap();
        assert_eq!(
            validate_payload(&json!({"value":"x".repeat(MAX_PAYLOAD_BYTES)})),
            Err("workspace_tab.payload_too_large".into())
        );
    }

    #[test]
    fn child_projection_reports_change_and_keeps_owner_fence() {
        let mut registry = Registry::default();
        terminal(&mut registry, "main", "one");
        registry.windows.insert("workspace-other".into());
        let initial = registry.tabs["one"].payload.clone();
        assert_eq!(
            registry.update_child_projection("main", "one", "terminal", initial),
            Ok(false)
        );

        let next = json!({"paneIds":["one","two"]});
        assert_eq!(
            registry.update_child_projection("main", "one", "terminal", next.clone()),
            Ok(true)
        );
        assert_eq!(registry.tabs["one"].payload, next);
        assert_eq!(
            registry.update_child_projection("main", "one", "file", json!({})),
            Err("workspace_tab.invalid_identity".into())
        );
        assert_eq!(
            registry.update_child_projection("workspace-other", "one", "terminal", json!({})),
            Err("workspace_tab.wrong_owner".into())
        );

        registry
            .native_moves
            .insert("one".into(), ("main".into(), "workspace-other".into()));
        assert_eq!(
            registry.update_child_projection("main", "one", "terminal", json!({})),
            Err("workspace_tab.transfer_pending".into())
        );
        registry.native_moves.remove("one");
        assert_eq!(
            registry.update_child_projection(
                "main",
                "one",
                "terminal",
                json!({"value":"x".repeat(MAX_PAYLOAD_BYTES)})
            ),
            Err("workspace_tab.payload_too_large".into())
        );
        assert_eq!(registry.tabs["one"].payload, next);
        registry.tabs.get_mut("one").unwrap().owner = "workspace-other".into();
        assert_eq!(
            registry.update_child_projection(
                "workspace-other",
                "one",
                "terminal",
                json!({"moved":true})
            ),
            Ok(true)
        );
        assert_eq!(registry.tabs["one"].payload, json!({"moved":true}));
    }
}
