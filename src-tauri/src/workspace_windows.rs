//! Core-owned ordinary windows and in-process Tab ownership. Payloads are non-secret UI
//! projections; terminal output, input, and credentials remain with their Core owners.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

const PAGE: &str = "workspace-window.html";
const CLOSE_REQUESTED: &str = "workspace-window-close-requested";
const TAB_CHANGED: &str = "workspace-tab-state-changed";
const TAB_OFFER: &str = "workspace-tab-offer";
const TARGET_READY: &str = "workspace-tab-target-ready";
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabRecord {
    id: String,
    kind: String,
    owner: String,
    revision: u64,
    payload: Value,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NewTab {
    id: String,
    kind: String,
    payload: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum TransferPhase {
    Prepared,
    Offered,
    Ready,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingTransfer {
    ticket: String,
    tab: TabRecord,
    source: String,
    target: String,
    phase: TransferPhase,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabSnapshot {
    owned: Vec<TabRecord>,
    incoming: Vec<PendingTransfer>,
    outgoing: Vec<PendingTransfer>,
    others: Vec<TabOwner>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabOwner {
    id: String,
    kind: String,
    owner: String,
    terminal_panes: Vec<TerminalPaneOwner>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalPaneOwner {
    pane_id: String,
    kind: String,
    session_id: Option<String>,
    generation: Option<String>,
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
    transfers: BTreeMap<String, PendingTransfer>,
}

impl Registry {
    fn allows(&self, label: &str) -> bool {
        label == "main" || (self.windows.contains(label) && !self.closing.contains(label))
    }

    fn pending(&self, id: &str) -> bool {
        self.transfers
            .values()
            .any(|transfer| transfer.tab.id == id)
    }

    fn register(&mut self, caller: &str, tab: NewTab) -> Result<TabRecord, String> {
        if !self.allows(caller) {
            return Err("workspace_tab.denied".into());
        }
        validate_identity(&tab.id, &tab.kind)?;
        validate_payload(&tab.payload)?;
        if let Some(current) = self.tabs.get(&tab.id) {
            if current.owner == caller
                && current.kind == tab.kind
                && current.payload == tab.payload
                && !self.pending(&tab.id)
            {
                return Ok(current.clone());
            }
            return Err("workspace_tab.conflict".into());
        }
        let record = TabRecord {
            id: tab.id,
            kind: tab.kind,
            owner: caller.into(),
            revision: 1,
            payload: tab.payload,
        };
        self.tabs.insert(record.id.clone(), record.clone());
        Ok(record)
    }

    fn owned(&self, caller: &str, id: &str, expected_revision: u64) -> Result<&TabRecord, String> {
        if !self.allows(caller) {
            return Err("workspace_tab.denied".into());
        }
        let record = self.tabs.get(id).ok_or("workspace_tab.missing")?;
        if record.owner != caller {
            return Err("workspace_tab.wrong_owner".into());
        }
        if record.revision != expected_revision {
            return Err("workspace_tab.stale_revision".into());
        }
        Ok(record)
    }

    fn update(
        &mut self,
        caller: &str,
        id: &str,
        expected_revision: u64,
        payload: Value,
    ) -> Result<TabRecord, String> {
        self.owned(caller, id, expected_revision)?;
        validate_payload(&payload)?;
        if self.pending(id) {
            return Err("workspace_tab.transfer_pending".into());
        }
        let record = self.tabs.get_mut(id).ok_or("workspace_tab.missing")?;
        record.revision = next_revision(record.revision)?;
        record.payload = payload;
        Ok(record.clone())
    }

    fn unregister(&mut self, caller: &str, id: &str, expected_revision: u64) -> Result<(), String> {
        self.owned(caller, id, expected_revision)?;
        if self.pending(id) {
            return Err("workspace_tab.transfer_pending".into());
        }
        self.tabs.remove(id);
        Ok(())
    }

    fn prepare(
        &mut self,
        caller: &str,
        id: &str,
        target: &str,
        expected_revision: u64,
        payload: Value,
    ) -> Result<String, String> {
        self.owned(caller, id, expected_revision)?;
        if !self.allows(target) {
            return Err("workspace_tab.target_missing".into());
        }
        if caller == target {
            return Err("workspace_tab.same_owner".into());
        }
        validate_payload(&payload)?;
        if self.pending(id) {
            return Err("workspace_tab.transfer_pending".into());
        }
        // The prepared view is also the crash-recovery checkpoint. A source window can
        // disappear before the target commits, so keeping it only in the ticket would
        // leave Core with an older Tab payload when ownership returns to main.
        let record = self.tabs.get_mut(id).ok_or("workspace_tab.missing")?;
        record.revision = next_revision(record.revision)?;
        record.payload = payload;
        let tab = record.clone();
        let ticket = uuid::Uuid::now_v7().to_string();
        self.transfers.insert(
            ticket.clone(),
            PendingTransfer {
                ticket: ticket.clone(),
                tab,
                source: caller.into(),
                target: target.into(),
                phase: TransferPhase::Prepared,
            },
        );
        Ok(ticket)
    }

    fn transfer(
        &self,
        caller: &str,
        ticket: &str,
        source: bool,
    ) -> Result<&PendingTransfer, String> {
        if !self.allows(caller) {
            return Err("workspace_tab.denied".into());
        }
        let transfer = self
            .transfers
            .get(ticket)
            .ok_or("workspace_tab.ticket_missing")?;
        if (source && transfer.source != caller) || (!source && transfer.target != caller) {
            return Err("workspace_tab.wrong_owner".into());
        }
        Ok(transfer)
    }

    fn transfer_mut(
        &mut self,
        caller: &str,
        ticket: &str,
        source: bool,
    ) -> Result<&mut PendingTransfer, String> {
        if !self.allows(caller) {
            return Err("workspace_tab.denied".into());
        }
        let transfer = self
            .transfers
            .get_mut(ticket)
            .ok_or("workspace_tab.ticket_missing")?;
        if (source && transfer.source != caller) || (!source && transfer.target != caller) {
            return Err("workspace_tab.wrong_owner".into());
        }
        Ok(transfer)
    }

    fn source_frozen(&mut self, caller: &str, ticket: &str) -> Result<PendingTransfer, String> {
        let transfer = self.transfer_mut(caller, ticket, true)?;
        if transfer.phase == TransferPhase::Prepared {
            transfer.phase = TransferPhase::Offered;
        }
        Ok(transfer.clone())
    }

    fn target_ready(&mut self, caller: &str, ticket: &str) -> Result<PendingTransfer, String> {
        let transfer = self.transfer_mut(caller, ticket, false)?;
        if transfer.phase == TransferPhase::Prepared {
            return Err("workspace_tab.not_offered".into());
        }
        transfer.phase = TransferPhase::Ready;
        Ok(transfer.clone())
    }

    fn commit(&mut self, caller: &str, ticket: &str) -> Result<TabRecord, String> {
        let transfer = self.transfer(caller, ticket, true)?.clone();
        if transfer.phase != TransferPhase::Ready {
            return Err("workspace_tab.target_not_ready".into());
        }
        if !self.allows(&transfer.target) {
            return Err("workspace_tab.target_missing".into());
        }
        let record = self
            .tabs
            .get_mut(&transfer.tab.id)
            .ok_or("workspace_tab.missing")?;
        if record.owner != caller || record.revision != transfer.tab.revision {
            return Err("workspace_tab.stale_revision".into());
        }
        record.revision = next_revision(record.revision)?;
        record.owner = transfer.target;
        record.payload = transfer.tab.payload;
        let committed = record.clone();
        self.transfers.remove(ticket);
        Ok(committed)
    }

    fn abort(&mut self, caller: &str, ticket: &str) -> Result<String, String> {
        let id = self.transfer(caller, ticket, true)?.tab.id.clone();
        self.transfers.remove(ticket);
        Ok(id)
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
            incoming: self
                .transfers
                .values()
                .filter(|transfer| transfer.target == caller)
                .cloned()
                .collect(),
            outgoing: self
                .transfers
                .values()
                .filter(|transfer| transfer.source == caller)
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
                })
                .collect(),
        })
    }

    fn begin_close(&mut self, label: &str) -> Result<(), String> {
        if !self.windows.contains(label) {
            return Err("workspace_window.denied".into());
        }
        if self.closing.contains(label) {
            return Err("workspace_window.close_in_progress".into());
        }
        if self.tabs.values().any(|tab| tab.owner == label) {
            return Err("workspace_window.tabs_owned".into());
        }
        if self
            .transfers
            .values()
            .any(|transfer| transfer.source == label || transfer.target == label)
        {
            return Err("workspace_window.transfer_pending".into());
        }
        self.closing.insert(label.into());
        Ok(())
    }

    /// A destroyed workspace loses no Tab metadata. Main becomes the recovery owner.
    fn recover_destroyed(&mut self, label: &str) -> Vec<String> {
        if !self.windows.remove(label) {
            return Vec::new();
        }
        self.closing.remove(label);
        let mut changed: Vec<String> = self
            .transfers
            .values()
            .filter(|transfer| transfer.source == label || transfer.target == label)
            .map(|transfer| transfer.tab.id.clone())
            .collect();
        self.transfers
            .retain(|_, transfer| transfer.source != label && transfer.target != label);
        for tab in self.tabs.values_mut().filter(|tab| tab.owner == label) {
            tab.owner = "main".into();
            tab.revision = tab.revision.saturating_add(1);
            changed.push(tab.id.clone());
        }
        changed.sort();
        changed.dedup();
        changed
    }
}

fn validate_identity(id: &str, kind: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
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

fn next_revision(revision: u64) -> Result<u64, String> {
    revision
        .checked_add(1)
        .ok_or_else(|| "workspace_tab.revision_exhausted".into())
}

#[derive(Clone, Default)]
pub(crate) struct WorkspaceWindows(Arc<Mutex<Registry>>);

impl WorkspaceWindows {
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

    fn labels(&self) -> Result<Vec<String>, String> {
        self.0
            .lock()
            .map(|registry| registry.windows.iter().cloned().collect())
            .map_err(|_| "workspace_window.unavailable".to_owned())
    }

    pub(crate) fn window_labels(&self) -> Result<Vec<String>, String> {
        self.labels()
    }

    fn forget(&self, label: &str) -> Vec<String> {
        self.0
            .lock()
            .map(|mut registry| registry.recover_destroyed(label))
            .unwrap_or_default()
    }
}

fn notify_tab<R: Runtime>(app: &AppHandle<R>, state: &WorkspaceWindows, id: &str) {
    let mut recipients = state.labels().unwrap_or_default();
    recipients.push("main".into());
    for recipient in recipients {
        let _ = app.emit_to(&recipient, TAB_CHANGED, id);
    }
}

fn caller_allowed(state: &WorkspaceWindows, label: &str) -> bool {
    state.0.lock().is_ok_and(|registry| registry.allows(label))
}

#[tauri::command]
pub(crate) fn workspace_window_open(
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
    let label = format!("workspace-{}", uuid::Uuid::now_v7());
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
    let result = crate::secure_window_frame::apply_secure_window_frame(&app, &label, child)
        .inner_size(1100.0, 740.0)
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
pub(crate) fn workspace_window_list(
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
) -> Result<Vec<String>, String> {
    if !caller_allowed(&state, window.label()) {
        return Err("workspace_window.denied".into());
    }
    state.labels()
}

#[tauri::command]
pub(crate) fn workspace_window_at_cursor(
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
) -> Result<Option<String>, String> {
    if !caller_allowed(&state, window.label()) {
        return Err("workspace_window.denied".into());
    }
    let cursor = window
        .cursor_position()
        .map_err(|_| "workspace_window.cursor_unavailable")?;
    let app = window.app_handle();
    let mut labels = state.labels()?;
    labels.push("main".into());
    let mut hits = Vec::new();
    for label in labels {
        let Some(candidate) = app.get_webview_window(&label) else {
            continue;
        };
        if candidate.is_visible().ok() != Some(true) || candidate.is_minimized().ok() == Some(true)
        {
            continue;
        }
        let (Ok(position), Ok(size)) = (candidate.outer_position(), candidate.outer_size()) else {
            continue;
        };
        let left = f64::from(position.x);
        let top = f64::from(position.y);
        if cursor.x >= left
            && cursor.x < left + f64::from(size.width)
            && cursor.y >= top
            && cursor.y < top + f64::from(size.height)
        {
            let strip_bottom = top + 64.0 * candidate.scale_factor().unwrap_or(1.0);
            hits.push((
                label,
                candidate.is_focused().ok() == Some(true),
                cursor.y < strip_bottom,
            ));
        }
    }
    Ok(hits
        .iter()
        .find(|(label, focused, strip)| label != window.label() && *focused && *strip)
        .or_else(|| {
            hits.iter()
                .find(|(label, _, strip)| label != window.label() && *strip)
        })
        .or_else(|| hits.iter().find(|(label, _, _)| label == window.label()))
        .map(|(label, _, _)| label.clone()))
}

#[tauri::command]
pub(crate) fn workspace_window_focus(
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    label: Option<String>,
) -> Result<(), String> {
    if !caller_allowed(&state, window.label()) {
        return Err("workspace_window.denied".into());
    }
    let target_label = label.as_deref().unwrap_or(window.label());
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
) -> Result<(), String> {
    state
        .0
        .lock()
        .map_err(|_| "workspace_window.unavailable")?
        .begin_close(window.label())?;
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
    label: String,
) -> Result<(), String> {
    if !caller_allowed(&state, window.label()) {
        return Err("workspace_window.denied".into());
    }
    let target = app
        .get_webview_window(&label)
        .ok_or("workspace_window.missing")?;
    state
        .0
        .lock()
        .map_err(|_| "workspace_window.unavailable")?
        .begin_close(&label)?;
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
pub(crate) fn workspace_tab_register(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    tab: NewTab,
) -> Result<TabRecord, String> {
    let record = with_registry(&state, |registry| registry.register(window.label(), tab))?;
    notify_tab(&app, &state, &record.id);
    Ok(record)
}

#[tauri::command]
pub(crate) fn workspace_tab_update(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    id: String,
    expected_revision: u64,
    payload: Value,
) -> Result<TabRecord, String> {
    let record = with_registry(&state, |registry| {
        registry.update(window.label(), &id, expected_revision, payload)
    })?;
    notify_tab(&app, &state, &id);
    Ok(record)
}

#[tauri::command]
pub(crate) fn workspace_tab_unregister(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    id: String,
    expected_revision: u64,
) -> Result<(), String> {
    with_registry(&state, |registry| {
        registry.unregister(window.label(), &id, expected_revision)
    })?;
    notify_tab(&app, &state, &id);
    Ok(())
}

#[tauri::command]
pub(crate) fn workspace_tab_prepare(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    id: String,
    target: String,
    expected_revision: u64,
    payload: Value,
) -> Result<String, String> {
    let ticket = with_registry(&state, |registry| {
        registry.prepare(window.label(), &id, &target, expected_revision, payload)
    })?;
    notify_tab(&app, &state, &id);
    Ok(ticket)
}

#[tauri::command]
pub(crate) fn workspace_tab_source_frozen(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    ticket: String,
) -> Result<(), String> {
    let transfer = with_registry(&state, |registry| {
        registry.source_frozen(window.label(), &ticket)
    })?;
    let _ = app.emit_to(&transfer.target, TAB_OFFER, &transfer);
    notify_tab(&app, &state, &transfer.tab.id);
    Ok(())
}

#[tauri::command]
pub(crate) fn workspace_tab_target_ready(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    ticket: String,
) -> Result<(), String> {
    let transfer = with_registry(&state, |registry| {
        registry.target_ready(window.label(), &ticket)
    })?;
    let _ = app.emit_to(&transfer.source, TARGET_READY, &ticket);
    notify_tab(&app, &state, &transfer.tab.id);
    Ok(())
}

#[tauri::command]
pub(crate) fn workspace_tab_commit(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    ticket: String,
) -> Result<TabRecord, String> {
    let record = with_registry(&state, |registry| registry.commit(window.label(), &ticket))?;
    notify_tab(&app, &state, &record.id);
    Ok(record)
}

#[tauri::command]
pub(crate) fn workspace_tab_abort(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
    ticket: String,
) -> Result<(), String> {
    let id = with_registry(&state, |registry| registry.abort(window.label(), &ticket))?;
    notify_tab(&app, &state, &id);
    Ok(())
}

#[tauri::command]
pub(crate) fn workspace_tab_snapshot(
    window: WebviewWindow,
    state: State<'_, WorkspaceWindows>,
) -> Result<TabSnapshot, String> {
    with_registry(&state, |registry| registry.snapshot(window.label()))
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

    fn tab(id: &str) -> NewTab {
        NewTab {
            id: id.into(),
            kind: "terminal".into(),
            payload: json!({"paneIds":["one"]}),
        }
    }

    #[test]
    fn transfer_requires_both_handoffs_and_blocks_racing_mutations() {
        let mut registry = Registry::default();
        registry.windows.insert("workspace-a".into());
        let initial = registry.register("main", tab("one")).unwrap();
        let ticket = registry
            .prepare(
                "main",
                "one",
                "workspace-a",
                initial.revision,
                json!({"paneIds":["two"]}),
            )
            .unwrap();
        assert_eq!(
            registry.update("main", "one", 2, json!({})),
            Err("workspace_tab.transfer_pending".into())
        );
        assert_eq!(
            registry.unregister("main", "one", 2),
            Err("workspace_tab.transfer_pending".into())
        );
        assert_eq!(
            registry.commit("main", &ticket),
            Err("workspace_tab.target_not_ready".into())
        );
        registry.source_frozen("main", &ticket).unwrap();
        registry.target_ready("workspace-a", &ticket).unwrap();
        let moved = registry.commit("main", &ticket).unwrap();
        assert_eq!(moved.owner, "workspace-a");
        assert_eq!(moved.revision, 3);
        assert_eq!(moved.payload, json!({"paneIds":["two"]}));
        let main_snapshot = registry.snapshot("main").unwrap();
        assert!(main_snapshot.owned.is_empty());
        assert_eq!(
            serde_json::to_value(main_snapshot.others).unwrap(),
            json!([{"id":"one","kind":"terminal","owner":"workspace-a","terminalPanes":[]}])
        );
        assert_eq!(registry.snapshot("workspace-a").unwrap().owned, vec![moved]);
    }

    #[test]
    fn invalid_caller_and_abort_preserve_original_owner() {
        let mut registry = Registry::default();
        registry.windows.insert("workspace-a".into());
        let initial = registry.register("main", tab("one")).unwrap();
        assert_eq!(
            registry.prepare("secure-vault-x", "one", "workspace-a", 1, json!({})),
            Err("workspace_tab.denied".into())
        );
        assert_eq!(
            registry.prepare("main", "one", "workspace-forged", 1, json!({})),
            Err("workspace_tab.target_missing".into())
        );
        let ticket = registry
            .prepare("main", "one", "workspace-a", 1, json!({}))
            .unwrap();
        assert_eq!(
            registry.target_ready("workspace-a", &ticket).map(|_| ()),
            Err("workspace_tab.not_offered".into())
        );
        assert_eq!(
            registry.source_frozen("workspace-a", &ticket).map(|_| ()),
            Err("workspace_tab.wrong_owner".into())
        );
        registry.source_frozen("main", &ticket).unwrap();
        registry.abort("main", &ticket).unwrap();
        let after_abort = registry.snapshot("main").unwrap().owned[0].clone();
        assert_eq!(after_abort.owner, initial.owner);
        assert_eq!(after_abort.revision, initial.revision + 1);
        assert_eq!(after_abort.payload, json!({}));
        assert!(
            registry
                .snapshot("workspace-a")
                .unwrap()
                .incoming
                .is_empty()
        );
        registry
            .update(
                "main",
                "one",
                after_abort.revision,
                json!({"restored":true}),
            )
            .unwrap();
    }

    #[test]
    fn foreign_terminal_projection_exposes_only_live_session_identity() {
        let mut registry = Registry::default();
        registry.windows.insert("workspace-a".into());
        registry
            .register(
                "workspace-a",
                NewTab {
                    id: "terminal-one".into(),
                    kind: "terminal".into(),
                    payload: json!({
                        "schemaVersion": 1,
                        "tabId": "terminal-one",
                        "secret": "must-not-leak",
                        "panes": [
                            {"kind":"ssh","paneId":"pane-one","sessionId":"session-one","generation":"7","label":"Host"},
                            {"kind":"launcher","paneId":"pane-two","label":"New"}
                        ]
                    }),
                },
            )
            .unwrap();
        let snapshot = serde_json::to_value(registry.snapshot("main").unwrap().others).unwrap();
        assert_eq!(
            snapshot,
            json!([{
                "id":"terminal-one", "kind":"terminal", "owner":"workspace-a",
                "terminalPanes":[{"paneId":"pane-one","kind":"ssh","sessionId":"session-one","generation":"7"}]
            }])
        );
    }

    #[test]
    fn close_blocks_owned_or_pending_tabs_and_destroy_recovers_to_main() {
        let mut registry = Registry::default();
        registry.windows.insert("workspace-a".into());
        registry.windows.insert("workspace-b".into());
        let initial = registry.register("workspace-a", tab("one")).unwrap();
        let ticket = registry
            .prepare("workspace-a", "one", "workspace-b", 1, json!({}))
            .unwrap();
        assert_eq!(
            registry.begin_close("workspace-a"),
            Err("workspace_window.tabs_owned".into())
        );
        assert_eq!(registry.recover_destroyed("workspace-a"), vec!["one"]);
        let recovered = &registry.snapshot("main").unwrap().owned[0];
        assert_eq!(recovered.owner, "main");
        assert_eq!(recovered.revision, initial.revision + 2);
        assert_eq!(recovered.payload, json!({}));
        assert!(registry.transfers.is_empty());
        assert_eq!(
            registry.commit("main", &ticket),
            Err("workspace_tab.ticket_missing".into())
        );
        registry.begin_close("workspace-b").unwrap();
        assert_eq!(
            registry.register("workspace-b", tab("two")).map(|_| ()),
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
}
