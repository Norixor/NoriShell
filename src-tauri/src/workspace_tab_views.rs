//! One native child WebView per ordinary Workspace Tab.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use serde_json::Value;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Rect, Size, State,
    Webview, WebviewBuilder, WebviewUrl, WebviewWindow,
};

use crate::workspace_windows::WorkspaceWindows;

const PAGE: &str = "workspace-tab.html";
const CONTEXT_CHANGED: &str = "workspace-tab-context-changed";
const TAB_CHANGED: &str = "workspace-tab-state-changed";
const MAX_BOOTSTRAP_BYTES: usize = 256 * 1024;

pub(crate) fn tab_view_label(id: &str) -> String {
    format!("workspace-tab-{}", URL_SAFE_NO_PAD.encode(id.as_bytes()))
}

pub(crate) fn tab_id_from_view_label(label: &str) -> Option<String> {
    let encoded = label.strip_prefix("workspace-tab-")?;
    let id = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()?;
    (valid_id(&id) && tab_view_label(&id) == label).then_some(id)
}

#[derive(Clone)]
struct TabView {
    webview: Webview,
    kind: String,
    route: String,
    operation: Arc<Mutex<()>>,
    live: Arc<AtomicBool>,
    /// Whether the owner shell currently shows this view; hidden views may not take input focus.
    shown: Arc<AtomicBool>,
    /// The first bootstrap, taken once by the new view itself so it starts without a round trip.
    bootstrap: Arc<Mutex<Option<Value>>>,
}

#[derive(Default)]
pub(crate) struct WorkspaceTabViews(Mutex<BTreeMap<String, TabView>>);

impl WorkspaceTabViews {
    /// A Tab WebView may take terminal input only while its shell shows it. Other
    /// WebViews (window shells, tests) are not Tab views and keep their own checks.
    pub(crate) fn may_take_input(&self, label: &str) -> bool {
        let Some(id) = tab_id_from_view_label(label) else {
            return true;
        };
        self.0.lock().is_ok_and(|views| {
            views.get(&id).is_some_and(|view| {
                view.webview.label() == label && view.shown.load(Ordering::Acquire)
            })
        })
    }

    pub(crate) fn owned_ids(&self, owner: &str) -> Result<BTreeSet<String>, String> {
        self.0
            .lock()
            .map(|views| {
                views
                    .iter()
                    .filter(|(_, view)| view.webview.window().label() == owner)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .map_err(|_| "workspace_tab.unavailable".into())
    }

    pub(crate) fn ordinary_owner(
        &self,
        caller: &Webview,
        tabs: &WorkspaceWindows,
    ) -> Result<String, String> {
        let window = caller.window();
        if caller.label() == window.label() {
            if window.label() == "main" || tabs.contains(window.label()) {
                return Ok(window.label().into());
            }
            return Err("workspace_tab.denied".into());
        }
        let id = tab_id_from_view_label(caller.label()).ok_or("workspace_tab.denied")?;
        self.child_owner(caller, tabs, &id)
    }

    pub(crate) fn child_owner(
        &self,
        caller: &Webview,
        tabs: &WorkspaceWindows,
        id: &str,
    ) -> Result<String, String> {
        let views = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        let view = views.get(id).ok_or("workspace_tab.view_missing")?;
        if caller.label() != view.webview.label() || !caller.label().starts_with("workspace-tab-") {
            return Err("workspace_tab.wrong_owner".into());
        }
        let owner = caller.window().label().to_owned();
        tabs.owns_tab(&owner, id)?;
        Ok(owner)
    }

    pub(crate) fn projection_identity(
        &self,
        caller: &Webview,
    ) -> Result<(String, String, String), String> {
        let id = tab_id_from_view_label(caller.label()).ok_or("workspace_tab.denied")?;
        let views = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        let view = views.get(&id).ok_or("workspace_tab.view_missing")?;
        if view.webview.label() != caller.label() || !view.live.load(Ordering::Acquire) {
            return Err("workspace_tab.view_missing".into());
        }
        let owner = caller.window().label().to_owned();
        if view.webview.window().label() != owner {
            return Err("workspace_tab.wrong_owner".into());
        }
        Ok((id, view.kind.clone(), owner))
    }

    pub(crate) fn projection_operation(
        &self,
        caller: &Webview,
    ) -> Result<(Arc<Mutex<()>>, Arc<AtomicBool>), String> {
        let id = tab_id_from_view_label(caller.label()).ok_or("workspace_tab.denied")?;
        let views = self.0.lock().map_err(|_| "workspace_tab.unavailable")?;
        let view = views.get(&id).ok_or("workspace_tab.view_missing")?;
        if view.webview.label() != caller.label() || !view.live.load(Ordering::Acquire) {
            return Err("workspace_tab.view_missing".into());
        }
        Ok((view.operation.clone(), view.live.clone()))
    }
}

pub(crate) fn ordinary_owner(app: &AppHandle, caller: &Webview) -> Result<String, String> {
    let tabs = app.state::<WorkspaceWindows>();
    let views = app.state::<WorkspaceTabViews>();
    views.ordinary_owner(caller, &tabs)
}

pub(crate) fn child_liveness(app: &AppHandle, caller: &Webview) -> Option<Arc<AtomicBool>> {
    let id = tab_id_from_view_label(caller.label())?;
    let views = app.state::<WorkspaceTabViews>();
    views
        .0
        .lock()
        .ok()?
        .get(&id)
        .and_then(|view| (view.webview.label() == caller.label()).then(|| view.live.clone()))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabViewInfo {
    id: String,
    label: String,
    owner_window: String,
    created: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TabViewContext {
    id: String,
    owner_window: String,
}

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TabViewBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn info(id: &str, view: &TabView, owner: &str, created: bool) -> TabViewInfo {
    TabViewInfo {
        id: id.into(),
        label: view.webview.label().into(),
        owner_window: owner.into(),
        created,
    }
}

/// Releases Core resources bound to a destroyed WebView label.
pub(crate) fn release_webview_resources(app: &AppHandle, label: &str) {
    if let Some(plugins) = app.try_state::<crate::plugin_service::PluginService>() {
        plugins.close_target_contexts_for_webview(label);
    }
    if let Some(sftp) = app.try_state::<crate::sftp_session_service::SftpSessionService>() {
        sftp.on_webview_destroyed(label);
    }
}

/// Forgets a native Tab view whose WebView is closed or gone.
fn release_view(app: &AppHandle, views: &WorkspaceTabViews, id: &str, view: &TabView) {
    view.live.store(false, Ordering::Release);
    if let Ok(mut map) = views.0.lock() {
        map.remove(id);
    }
    release_webview_resources(app, view.webview.label());
}

fn encoded_id(id: &str) -> String {
    let mut encoded = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && id
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '?' | '#' | '/' | '\\' | '%'))
}

fn valid_route(route: &str) -> bool {
    route.starts_with('/')
        && route.len() <= 512
        && !route.starts_with("//")
        && !route.chars().any(char::is_control)
}

fn require_owner(
    views: &WorkspaceTabViews,
    tabs: &WorkspaceWindows,
    id: &str,
    caller: &str,
) -> Result<TabView, String> {
    let view = views
        .0
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?
        .get(id)
        .cloned()
        .ok_or("workspace_tab.view_missing")?;
    if view.webview.window().label() != caller {
        return Err("workspace_tab.wrong_owner".into());
    }
    tabs.check_native_view(caller, id, &view.kind)?;
    Ok(view)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects four context parameters beside the IPC fields.
pub(crate) async fn create_tab_view(
    app: AppHandle,
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
    id: String,
    kind: String,
    route: String,
    payload: Value,
    bootstrap: Option<Value>,
    background: Option<[u8; 3]>,
) -> Result<TabViewInfo, String> {
    if !valid_id(&id) || !valid_route(&route) {
        return Err("workspace_tab.invalid_identity".into());
    }
    if let Some(bootstrap) = &bootstrap
        && serde_json::to_vec(bootstrap).map_or(true, |bytes| bytes.len() > MAX_BOOTSTRAP_BYTES)
    {
        return Err("workspace_tab.payload_too_large".into());
    }
    let owner = window.label().to_owned();
    let label = tab_view_label(&id);
    let registered = tabs.begin_native_create(&owner, &id, &kind, &payload)?;
    let existing = match views.0.lock() {
        Ok(map) => map.get(&id).cloned(),
        Err(_) => {
            tabs.abort_native_create(&id);
            return Err("workspace_tab.unavailable".into());
        }
    };
    if let Some(existing) = existing {
        if registered
            && existing.webview.window().label() == owner
            && existing.kind == kind
            && existing.route == route
        {
            tabs.abort_native_create(&id);
            return Ok(info(&id, &existing, &owner, false));
        }
        tabs.abort_native_create(&id);
        return Err("workspace_tab.view_exists".into());
    }
    if app.get_webview(&label).is_some() {
        tabs.abort_native_create(&id);
        return Err("workspace_tab.view_exists".into());
    }
    let parent = match app.get_window(&owner) {
        Some(parent) => parent,
        None => {
            tabs.abort_native_create(&id);
            return Err("workspace_window.missing".into());
        }
    };
    let url = format!("{PAGE}?tabId={}", encoded_id(&id));
    let expected_query = format!("tabId={}", encoded_id(&id));
    let expected = app
        .config()
        .build
        .dev_url
        .as_ref()
        .and_then(|base| base.join(&url).ok());
    let builder = WebviewBuilder::new(&label, WebviewUrl::App(url.into()))
        .on_navigation(move |url| {
            (cfg!(debug_assertions)
                && expected.as_ref().is_some_and(|expected| {
                    let mut navigation = url.clone();
                    navigation.set_fragment(None);
                    expected == &navigation
                }))
                || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                    || (matches!(url.scheme(), "http" | "https")
                        && url.host_str() == Some("tauri.localhost")
                        && url.port().is_none()))
                    && url.path() == format!("/{PAGE}")
                    && url.query() == Some(expected_query.as_str()))
        })
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    // Until its first document frame a new view would paint the platform default
    // (white on WebView2) over the shell's placeholder. With a colour, WKWebView stops
    // drawing its own background, so the placeholder below stays visible until then.
    #[cfg(not(target_os = "linux"))]
    let builder = match background {
        Some([red, green, blue]) => {
            builder.background_color(tauri::webview::Color(red, green, blue, 255))
        }
        None => builder,
    };
    // An opaque colour would square off the rounded window corners the page draws itself.
    #[cfg(target_os = "linux")]
    let builder = {
        let _ = background;
        builder.transparent(true)
    };
    // The shell supplies the final content rectangle after its first layout.
    // A hidden 1x1 child avoids drawing over the Header in the meantime.
    let webview = match parent.add_child(
        builder,
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(1.0, 1.0),
    ) {
        Ok(webview) => webview,
        Err(error) => {
            eprintln!("workspace tab native view creation failed: {error}");
            tabs.abort_native_create(&id);
            return Err("workspace_tab.view_create_failed".into());
        }
    };
    if let Err(error) = webview.hide() {
        eprintln!("workspace tab native view hide failed: {error}");
        let _ = webview.close();
        tabs.abort_native_create(&id);
        return Err("workspace_tab.view_create_failed".into());
    }
    let mut map = match views.0.lock() {
        Ok(map) => map,
        Err(_) => {
            let _ = webview.close();
            tabs.abort_native_create(&id);
            return Err("workspace_tab.unavailable".into());
        }
    };
    if let Err(error) = tabs.finish_native_create(&owner, &id, &kind, payload) {
        drop(map);
        let _ = webview.close();
        return Err(error);
    }
    let view = TabView {
        webview,
        kind,
        route,
        operation: Arc::new(Mutex::new(())),
        live: Arc::new(AtomicBool::new(true)),
        shown: Arc::new(AtomicBool::new(false)),
        bootstrap: Arc::new(Mutex::new(bootstrap)),
    };
    let result = info(&id, &view, &owner, !registered);
    map.insert(id.clone(), view);
    drop(map);
    let _ = app.emit_to(&owner, TAB_CHANGED, &id);
    Ok(result)
}

#[tauri::command]
pub(crate) fn workspace_tab_view_get(
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
    id: String,
) -> Result<Option<TabViewInfo>, String> {
    let views = views.0.lock().map_err(|_| "workspace_tab.unavailable")?;
    let Some(view) = views.get(&id) else {
        return Ok(None);
    };
    if view.webview.window().label() != window.label() {
        return Err("workspace_tab.wrong_owner".into());
    }
    tabs.check_native_view(window.label(), &id, &view.kind)?;
    Ok(Some(info(&id, view, window.label(), false)))
}

/// Hands the calling Tab WebView its creation bootstrap exactly once.
#[tauri::command]
pub(crate) fn workspace_tab_bootstrap_take(
    webview: Webview,
    views: State<'_, WorkspaceTabViews>,
) -> Result<Option<Value>, String> {
    let id = tab_id_from_view_label(webview.label()).ok_or("workspace_tab.denied")?;
    let views = views.0.lock().map_err(|_| "workspace_tab.unavailable")?;
    let view = views.get(&id).ok_or("workspace_tab.view_missing")?;
    if view.webview.label() != webview.label() {
        return Err("workspace_tab.wrong_owner".into());
    }
    let mut bootstrap = view
        .bootstrap
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?;
    Ok(bootstrap.take())
}

#[tauri::command]
pub(crate) fn workspace_tab_context_get(
    webview: Webview,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
) -> Result<TabViewContext, String> {
    let id = tab_id_from_view_label(webview.label()).ok_or("workspace_tab.denied")?;
    let view = views
        .0
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?
        .get(&id)
        .cloned()
        .ok_or("workspace_tab.view_missing")?;
    let owner = webview.window().label().to_owned();
    if view.webview.label() != webview.label() || view.webview.window().label() != owner {
        return Err("workspace_tab.wrong_owner".into());
    }
    tabs.check_native_view(&owner, &id, &view.kind)?;
    Ok(TabViewContext {
        id,
        owner_window: owner,
    })
}

#[tauri::command]
pub(crate) async fn set_tab_view_bounds(
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
    id: String,
    bounds: TabViewBounds,
) -> Result<(), String> {
    if ![bounds.x, bounds.y, bounds.width, bounds.height]
        .into_iter()
        .all(f64::is_finite)
        || bounds.x < 0.0
        || bounds.y < 0.0
        || bounds.width < 1.0
        || bounds.height < 1.0
        || bounds.width > 16_384.0
        || bounds.height > 16_384.0
    {
        return Err("workspace_tab.invalid_bounds".into());
    }
    let view = require_owner(&views, &tabs, &id, window.label())?;
    let _operation = view
        .operation
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?;
    tabs.check_native_view(window.label(), &id, &view.kind)?;
    view.webview
        .set_bounds(Rect {
            position: Position::Logical(LogicalPosition::new(bounds.x, bounds.y)),
            size: Size::Logical(LogicalSize::new(bounds.width, bounds.height)),
        })
        .map_err(|_| "workspace_tab.bounds_failed".into())
}

#[tauri::command]
pub(crate) async fn set_tab_view_visible(
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
    id: String,
    visible: bool,
) -> Result<(), String> {
    let view = require_owner(&views, &tabs, &id, window.label())?;
    let _operation = view
        .operation
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?;
    tabs.check_native_view(window.label(), &id, &view.kind)?;
    // Revoke input eligibility before hiding; grant it only after the view is shown.
    if !visible {
        view.shown.store(false, Ordering::Release);
    }
    let result = if visible {
        view.webview.show()
    } else {
        view.webview.hide()
    };
    result.map_err(|_| "workspace_tab.visibility_failed")?;
    if visible {
        view.shown.store(true, Ordering::Release);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn focus_tab_view(
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
    id: String,
) -> Result<(), String> {
    let view = require_owner(&views, &tabs, &id, window.label())?;
    let _operation = view
        .operation
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?;
    tabs.check_native_view(window.label(), &id, &view.kind)?;
    view.webview
        .set_focus()
        .map_err(|_| "workspace_tab.focus_failed".into())
}

#[tauri::command]
pub(crate) async fn move_tab_view(
    app: AppHandle,
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
    id: String,
    target: String,
) -> Result<TabViewInfo, String> {
    let source = window.label();
    let view = require_owner(&views, &tabs, &id, source)?;
    let _operation = view
        .operation
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?;
    let target_window = app
        .get_window(&target)
        .ok_or("workspace_tab.target_missing")?;
    if let Err(error) = tabs.move_native_view(source, &target, &id, || {
        view.webview
            .reparent(&target_window)
            .map_err(|_| "workspace_tab.reparent_failed".into())
    }) {
        // When the target or source window disappeared mid-move, Core already
        // recovered the record to main. Release the unusable child so main can
        // attach a fresh view from the recorded resource identities.
        let recovered = error == "workspace_tab.target_lost"
            || (tabs.owns_tab("main", &id).is_ok() && view.webview.window().label() != "main");
        if recovered {
            let _ = view.webview.close();
            release_view(&app, &views, &id, &view);
            let _ = app.emit_to("main", TAB_CHANGED, &id);
        }
        return Err(error);
    }
    let result = info(&id, &view, &target, false);
    if source != target {
        let _ = app.emit_to(&result.label, CONTEXT_CHANGED, &result);
        let _ = app.emit_to(source, TAB_CHANGED, &id);
        let _ = app.emit_to(&target, TAB_CHANGED, &id);
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn close_tab_view(
    app: AppHandle,
    window: WebviewWindow,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
    id: String,
) -> Result<(), String> {
    let view = require_owner(&views, &tabs, &id, window.label())?;
    let _operation = view
        .operation
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?;
    close_locked_view(&app, &tabs, &views, &id, &view, window.label())
}

#[tauri::command]
pub(crate) async fn close_own_tab_view(
    app: AppHandle,
    caller: Webview,
    tabs: State<'_, WorkspaceWindows>,
    views: State<'_, WorkspaceTabViews>,
) -> Result<(), String> {
    let id = tab_id_from_view_label(caller.label()).ok_or("workspace_tab.denied")?;
    let view = views
        .0
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?
        .get(&id)
        .cloned()
        .ok_or("workspace_tab.view_missing")?;
    if caller.label() != view.webview.label() {
        return Err("workspace_tab.wrong_owner".into());
    }
    let _operation = view
        .operation
        .lock()
        .map_err(|_| "workspace_tab.unavailable")?;
    // Read the parent only after acquiring the move/close lock. The child may
    // have been reparented while it was finishing its resource-safe close.
    let owner = view.webview.window().label().to_owned();
    close_locked_view(&app, &tabs, &views, &id, &view, &owner)
}

fn close_locked_view(
    app: &AppHandle,
    tabs: &WorkspaceWindows,
    views: &WorkspaceTabViews,
    id: &str,
    view: &TabView,
    owner: &str,
) -> Result<(), String> {
    if view.kind == "desktop"
        && tabs.has_live_desktop_session(
            owner,
            id,
            &app.state::<crate::desktop_service::DesktopService>(),
        )?
    {
        return Err("workspace_tab.desktop_session_active".into());
    }
    tabs.close_native_view(owner, id, || {
        view.webview
            .close()
            .map_err(|_| "workspace_tab.view_close_failed".into())
    })?;
    release_view(app, views, id, view);
    let _ = app.emit_to(owner, TAB_CHANGED, id);
    Ok(())
}

pub(crate) fn on_window_destroyed(app: &AppHandle, label: &str) {
    let state = app.state::<WorkspaceTabViews>();
    let tabs = app.state::<WorkspaceWindows>();
    let Ok(mut views) = state.0.lock() else {
        return;
    };
    // A crashed ordinary child window is recovered by the existing Tab owner
    // registry; its native WebViews cannot survive destruction of the parent.
    let removed = views
        .iter()
        .filter(|(id, view)| {
            view.webview.window().label() == label && !tabs.native_move_pending(id)
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let removed = removed
        .iter()
        .filter_map(|id| views.remove(id))
        .collect::<Vec<_>>();
    drop(views);
    for view in removed {
        view.live.store(false, Ordering::Release);
        release_webview_resources(app, view.webview.label());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_encoding_keeps_tab_identity_out_of_query_syntax() {
        assert_eq!(encoded_id("terminal:abc"), "terminal%3Aabc");
        let plugin_id = "page:plugin:com.norishell.self-host-sync:sync";
        let label = tab_view_label(plugin_id);
        assert_eq!(
            label,
            "workspace-tab-cGFnZTpwbHVnaW46Y29tLm5vcmlzaGVsbC5zZWxmLWhvc3Qtc3luYzpzeW5j"
        );
        assert_eq!(tab_id_from_view_label(&label).as_deref(), Some(plugin_id));
        assert!(tab_id_from_view_label("workspace-tab-invalid.").is_none());
        assert!(!valid_id("tab?other=1"));
        assert!(valid_route("/terminal"));
        assert!(!valid_route("//example.org"));
    }
}
