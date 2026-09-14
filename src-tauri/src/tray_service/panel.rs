//! The tray panel holds only a bounded projection; it exposes neither resource identifiers nor general commands to the WebView.
use super::*;
use tauri::{PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const LABEL: &str = "tray-panel";
const MAX_PANEL_BINDINGS: usize = 8192;
#[derive(Default)]
pub(super) struct PanelState {
    pub bindings: HashMap<String, PanelBinding>,
    blurred_at: Option<Instant>,
}
pub(super) struct PanelBinding {
    action: Action,
    created: Instant,
    locale: NativeTrayLocale,
    revision: Option<WireSequence>,
}

impl PanelState {
    fn take(&mut self, token: &str, projection: &Projection, now: Instant) -> Option<Action> {
        let entry = self.bindings.remove(token)?;
        (now.duration_since(entry.created) < CLICK_TTL
            && entry.locale == projection.locale
            && entry.revision == projection.preferences_revision
            && contains(&projection.rows, &entry.action))
        .then_some(entry.action)
    }
}

fn contains(rows: &[Row], action: &Action) -> bool {
    rows.iter()
        .any(|row| row.action.as_ref() == Some(action) || contains(&row.children, action))
}
fn current_projection<R: Runtime>(app: &AppHandle<R>, state: &TrayState) -> Projection {
    let revision = app
        .state::<crate::desktop_preferences::DesktopPreferencesService>()
        .snapshot()
        .ok()
        .map(|value| value.revision);
    state
        .rendered
        .as_ref()
        .filter(|projection| {
            projection.locale == state.locale
                && projection.preferences_revision == revision
                && state
                    .rendered_at
                    .is_some_and(|time| time.elapsed() < MENU_TTL)
        })
        .cloned()
        .unwrap_or_else(|| Projection::initial(state.locale))
}
fn rows(
    rows: &[Row],
    projection: &Projection,
    panel: &mut PanelState,
    now: Instant,
) -> Vec<NativeTrayPanelRow> {
    rows.iter()
        .map(|row| {
            use NativeTrayPanelRowKind as Kind;
            let kind = match &row.action {
                Some(Action::Show) => Kind::Show,
                Some(Action::Quit) => Kind::Quit,
                Some(Action::Frontend(NativeTrayAction::NewTerminal)) => Kind::NewTerminal,
                Some(Action::Frontend(NativeTrayAction::NewLocalTerminal)) => {
                    Kind::NewLocalTerminal
                }
                Some(Action::Frontend(NativeTrayAction::QuickConnect)) => Kind::QuickConnect,
                Some(Action::Frontend(NativeTrayAction::Settings)) => Kind::Settings,
                Some(_) => Kind::Action,
                None if !row.children.is_empty() => Kind::Group,
                None => Kind::Status,
            };
            let id = row.action.as_ref().map(|action| {
                let token = Uuid::new_v4().to_string();
                panel.bindings.insert(
                    token.clone(),
                    PanelBinding {
                        action: action.clone(),
                        created: now,
                        locale: projection.locale,
                        revision: projection.preferences_revision,
                    },
                );
                token
            });
            NativeTrayPanelRow {
                id,
                label: row.panel_label.clone().unwrap_or_else(|| row.label.clone()),
                kind,
                role: row.role,
                children: self::rows(&row.children, projection, panel, now),
            }
        })
        .collect()
}
#[tauri::command]
pub(crate) fn tray_panel_snapshot<R: Runtime>(
    window: WebviewWindow<R>,
    request: NativeTrayPanelSnapshotRequest,
    service: State<'_, NativeTrayService>,
) -> CoreResult<NativeTrayPanelSnapshot> {
    if window.label() != LABEL || !window.is_visible().unwrap_or(false) {
        return Err(failure(request.meta.request_id));
    }
    let mut state = service
        .inner
        .lock()
        .map_err(|_| failure(request.meta.request_id.clone()))?;
    if state.stopped {
        return Err(failure(request.meta.request_id));
    }
    let projection = current_projection(window.app_handle(), &state);
    state
        .panel
        .bindings
        .retain(|_, entry| entry.created.elapsed() < CLICK_TTL);
    if state.panel.bindings.len() + projection.item_count() > MAX_PANEL_BINDINGS {
        return Err(failure(request.meta.request_id));
    }
    Ok(NativeTrayPanelSnapshot {
        locale: projection.locale,
        stats: projection.stats.clone(),
        error_summary: projection.error_summary.clone(),
        notification_state: projection.notification_state,
        rows: rows(
            &projection.rows,
            &projection,
            &mut state.panel,
            Instant::now(),
        ),
    })
}
#[tauri::command]
pub(crate) async fn tray_panel_execute(
    window: WebviewWindow,
    request: NativeTrayPanelExecuteRequest,
    service: State<'_, NativeTrayService>,
) -> CoreResult<()> {
    if window.label() != LABEL || request.token.len() > 64 || !window.is_visible().unwrap_or(false)
    {
        return Err(failure(request.meta.request_id));
    }
    let action = {
        let mut state = service
            .inner
            .lock()
            .map_err(|_| failure(request.meta.request_id.clone()))?;
        let projection = current_projection(window.app_handle(), &state);
        let action = state
            .panel
            .take(&request.token, &projection, Instant::now());
        if state.stopped {
            return Err(failure(request.meta.request_id));
        }
        action.ok_or_else(|| failure(request.meta.request_id.clone()))?
    };
    // Main also validates resource generations when consuming the forwarded token; this boundary accepts no renderer-selected target.
    hide(window.app_handle());
    execute_action(window.app_handle(), &service, action);
    Ok(())
}
#[tauri::command]
pub(crate) fn tray_panel_hide<R: Runtime>(
    window: WebviewWindow<R>,
    request: NativeTrayPanelHideRequest,
) -> CoreResult<()> {
    if window.label() != LABEL {
        return Err(failure(request.meta.request_id));
    }
    hide(window.app_handle());
    Ok(())
}
pub(super) fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(service) = app.try_state::<NativeTrayService>() {
        service
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .panel
            .bindings
            .clear();
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
}
pub(super) fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.destroy();
    }
}

/// Convert work areas and tray rectangles to physical pixels, supporting negative coordinates and mixed DPI.
fn position(
    anchor: (f64, f64, f64, f64),
    work: (i32, i32, u32, u32),
    size: (u32, u32),
) -> (i32, i32) {
    let (ax, ay, aw, ah) = anchor;
    let (wx, wy, ww, wh) = work;
    let (width, height) = size;
    let x = ax + aw / 2.0 - f64::from(width) / 2.0;
    let y = if ay + ah / 2.0 < f64::from(wy) + f64::from(wh) / 2.0 {
        ay + ah + 6.0
    } else {
        ay - f64::from(height) - 6.0
    };
    (
        x.round().clamp(
            f64::from(wx),
            f64::from(wx) + f64::from(ww.saturating_sub(width)),
        ) as i32,
        y.round().clamp(
            f64::from(wy),
            f64::from(wy) + f64::from(wh.saturating_sub(height)),
        ) as i32,
    )
}
pub(super) fn toggle(app: &AppHandle, rect: tauri::Rect) {
    let handle = app.clone();
    // Tauri executes run_on_main_thread immediately when already on the main thread. Leave the native event
    // callback before queueing UI work, avoiding reentrant tray callbacks during Windows WebView2 creation.
    tauri::async_runtime::spawn(async move {
        let main = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let _ = show_or_hide(&main, rect);
        });
    });
}
fn show_or_hide(app: &AppHandle, rect: tauri::Rect) -> tauri::Result<()> {
    {
        let service = app.state::<NativeTrayService>();
        let mut state = service
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped {
            return Ok(());
        }
        // A tray click usually triggers blur before Up; the same click must not reopen the panel.
        if state
            .panel
            .blurred_at
            .take()
            .is_some_and(|time| time.elapsed() < Duration::from_millis(350))
        {
            return Ok(());
        }
    }
    if app
        .get_webview_window(LABEL)
        .is_some_and(|window| window.is_visible().unwrap_or(false))
    {
        hide(app);
        return Ok(());
    }
    let window = if let Some(window) = app.get_webview_window(LABEL) {
        window
    } else {
        let expected = app
            .config()
            .build
            .dev_url
            .as_ref()
            .and_then(|url| url.join("tray-panel.html").ok());
        let window =
            WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("tray-panel.html".into()))
                .title("NoriShell")
                .inner_size(380.0, 560.0)
                .resizable(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible(false)
                .focused(false)
                .on_navigation(move |url| {
                    (cfg!(debug_assertions)
                        && expected.as_ref().is_some_and(|expected| expected == url))
                        || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                            || (matches!(url.scheme(), "http" | "https")
                                && url.host_str() == Some("tauri.localhost")
                                && url.port().is_none()))
                            && url.path() == "/tray-panel.html"
                            && url.query().is_none())
                })
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
        let handle = app.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                hide(&handle);
            }
            if matches!(event, WindowEvent::Focused(false)) {
                handle
                    .state::<NativeTrayService>()
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .panel
                    .blurred_at = Some(Instant::now());
                hide(&handle);
            }
        });
        window
    };
    let initial_scale = window.scale_factor()?;
    let anchor = rect.position.to_physical::<f64>(initial_scale);
    let monitor = window
        .monitor_from_point(anchor.x, anchor.y)?
        .or(window.primary_monitor()?);
    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let anchor = rect.position.to_physical::<f64>(scale);
        let anchor_size = rect.size.to_physical::<f64>(scale);
        let work = monitor.work_area();
        let width = (380.0 * scale).round() as u32;
        let height = (560.0 * scale).round() as u32;
        let size = (width.min(work.size.width), height.min(work.size.height));
        let (x, y) = position(
            (anchor.x, anchor.y, anchor_size.width, anchor_size.height),
            (
                work.position.x,
                work.position.y,
                work.size.width,
                work.size.height,
            ),
            size,
        );
        window.set_size(PhysicalSize::new(size.0, size.1))?;
        window.set_position(PhysicalPosition::new(x, y))?;
    }
    window.show()?;
    window.set_focus()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_tokens_keep_old_snapshot_actions_and_are_one_shot() {
        let projection = Projection::initial(NativeTrayLocale::En);
        let mut panel = PanelState::default();
        let first = rows(&projection.rows, &projection, &mut panel, Instant::now());
        let _second = rows(&projection.rows, &projection, &mut panel, Instant::now());
        let token = first[0].id.as_ref().unwrap();
        assert_eq!(panel.bindings.remove(token).unwrap().action, Action::Show);
        assert!(panel.bindings.remove(token).is_none());
    }
    #[test]
    fn expiry_privacy_locale_and_removed_actions_fail_closed() {
        let projection = Projection::initial(NativeTrayLocale::En);
        let now = Instant::now();
        for case in 0..4 {
            let mut panel = PanelState::default();
            let result = rows(&projection.rows, &projection, &mut panel, now);
            let token = result[0].id.as_ref().unwrap();
            let mut changed = projection.clone();
            let mut time = now;
            match case {
                0 => time += CLICK_TTL,
                1 => changed.preferences_revision = Some(WireSequence::new(2)),
                2 => changed.locale = NativeTrayLocale::ZhCn,
                _ => changed.rows.clear(),
            }
            assert!(panel.take(token, &changed, time).is_none());
            assert!(panel.take(token, &projection, now).is_none());
        }
    }
    #[test]
    fn other_windows_cannot_hide_panel() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let error =
            tray_panel_hide(window, NativeTrayPanelHideRequest { meta: meta() }).unwrap_err();
        assert_eq!(error.code, "tray.action_unavailable");
    }
    #[test]
    fn panel_uses_structural_roles_and_separate_recent_host_label() {
        let projection = Projection {
            rows: vec![
                Row::leaf("Native host · 1", None)
                    .with_panel_label("Native host")
                    .with_role(NativeTrayPanelRole::RecentHost),
            ],
            tooltip: "NoriShell".into(),
            preferences_revision: None,
            locale: NativeTrayLocale::En,
            stats: vec![],
            error_summary: None,
            notification_state: NativeTrayPanelNotificationState::Unavailable,
        };
        let mut panel = PanelState::default();
        let result = rows(&projection.rows, &projection, &mut panel, Instant::now());
        assert_eq!(result[0].label, "Native host");
        assert_eq!(result[0].role, Some(NativeTrayPanelRole::RecentHost));
    }
    #[test]
    fn initial_unavailable_fallback_is_not_a_summary_role() {
        let projection = Projection::initial(NativeTrayLocale::En);
        let mut panel = PanelState::default();
        let result = rows(&projection.rows, &projection, &mut panel, Instant::now());
        assert!(projection.stats.is_empty());
        assert!(matches!(result[1].kind, NativeTrayPanelRowKind::Status));
        assert_eq!(result[1].role, None);
    }
    #[test]
    fn panel_position_clamps_negative_monitor_and_bottom_taskbar() {
        assert_eq!(
            position(
                (-20.0, 0.0, 20.0, 24.0),
                (-1920, 24, 1920, 1056),
                (380, 560)
            ),
            (-380, 30)
        );
        assert_eq!(
            position((1900.0, 1040.0, 20.0, 40.0), (0, 0, 1920, 1040), (380, 560)),
            (1540, 474)
        );
    }
}
