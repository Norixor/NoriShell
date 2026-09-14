//! The native tray projects Core snapshots; menu resource identifiers are never rebound to newer snapshots.
pub(crate) mod panel;
mod projection;

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use norishell_core_api::*;
use tauri::{
    App, AppHandle, Emitter, Manager, Runtime, State, WebviewWindow,
    menu::{Menu, MenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{lifecycle, native_notification_service::NativeNotificationService};
use projection::{Projection, Row};

const TRAY_ID: &str = "norishell-main";
const EVENT: &str = "native-tray-action";
const MENU_TTL: Duration = Duration::from_secs(120);
const CLICK_TTL: Duration = Duration::from_secs(60);
const MAX_PENDING: usize = 32;
const MAX_BINDINGS: usize = 16_384;
type CoreResult<T> = Result<T, Box<CoreApiError>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Frontend(NativeTrayAction),
    Show,
    Quit,
    LockVault,
    Pause(u64),
    Resume,
}

struct Binding {
    action: Action,
    created: Instant,
}
#[derive(Debug, PartialEq, Eq)]
struct Pending {
    token: String,
    action: NativeTrayAction,
    created: Instant,
}

#[derive(Default)]
struct TrayState {
    locale: NativeTrayLocale,
    ready: bool,
    pending_quit: bool,
    stopped: bool,
    bindings: HashMap<String, Binding>,
    pending: VecDeque<Pending>,
    panel: panel::PanelState,
    rendered: Option<Projection>,
    rendered_at: Option<Instant>,
}

impl TrayState {
    // Bound outstanding clicks even if the frontend stops consuming them.
    fn queue(&mut self, action: NativeTrayAction, now: Instant) -> Option<String> {
        if self.stopped || self.pending.len() >= MAX_PENDING {
            return None;
        }
        let token = Uuid::new_v4().to_string();
        self.pending.push_back(Pending {
            token: token.clone(),
            action,
            created: now,
        });
        Some(token)
    }

    fn take(&mut self, token: &str, now: Instant) -> Option<Pending> {
        let index = self.pending.iter().position(|entry| entry.token == token)?;
        // Remove before validating: expired or invalid clicks are consumed as well and can never be replayed.
        let entry = self.pending.remove(index)?;
        (!self.stopped && now.duration_since(entry.created) < CLICK_TTL).then_some(entry)
    }
}

#[derive(Clone)]
pub(crate) struct NativeTrayService {
    inner: Arc<Mutex<TrayState>>,
    stop: watch::Sender<bool>,
}

impl NativeTrayService {
    pub(crate) fn stop(&self, app: &AppHandle) {
        panel::close(app);
        self.stop.send_replace(true);
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stopped = true;
        state.pending.clear();
        state.bindings.clear();
        state.panel.bindings.clear();
    }
}

fn failure(request_id: RequestId) -> Box<CoreApiError> {
    crate::core_api_error::core_error(
        request_id,
        "tray.action_unavailable",
        ErrorCategory::Unavailable,
        RetryStrategy::WaitForUser,
        "errors.tray.actionUnavailable",
    )
}

fn emit_failure<R: Runtime>(app: &AppHandle<R>) {
    let _ = lifecycle::show_main_window(app);
    let _ = app.emit_to(
        "main",
        "native-tray-error",
        serde_json::json!({"messageKey": "errors.tray.actionUnavailable"}),
    );
}

#[tauri::command]
pub(crate) fn tray_actions_ready<R: Runtime>(
    window: WebviewWindow<R>,
    request: NativeTrayActionsReadyRequest,
    service: State<'_, NativeTrayService>,
) -> CoreResult<Vec<String>> {
    if window.label() != "main" {
        return Err(failure(request.meta.request_id));
    }
    let mut state = service
        .inner
        .lock()
        .map_err(|_| failure(request.meta.request_id.clone()))?;
    if state.stopped {
        return Err(failure(request.meta.request_id));
    }
    let locale_changed = state.locale != request.locale;
    state.locale = request.locale;
    state.ready = true;
    let pending_quit = std::mem::take(&mut state.pending_quit);
    let tokens = state
        .pending
        .iter()
        .map(|entry| entry.token.clone())
        .collect();
    drop(state);
    if locale_changed {
        panel::hide(window.app_handle());
    }
    if pending_quit {
        lifecycle::request_application_exit(window.app_handle());
    }
    Ok(tokens)
}

#[tauri::command]
pub(crate) async fn tray_action_take<R: Runtime>(
    window: WebviewWindow<R>,
    request: NativeTrayActionTakeRequest,
    service: State<'_, NativeTrayService>,
) -> CoreResult<NativeTrayAction> {
    if window.label() != "main" || request.token.len() > 64 {
        return Err(failure(request.meta.request_id));
    }
    let action = service
        .inner
        .lock()
        .map_err(|_| failure(request.meta.request_id.clone()))?
        .take(&request.token, Instant::now())
        .ok_or_else(|| failure(request.meta.request_id.clone()))?;
    match tokio::time::timeout(
        Duration::from_secs(3),
        validate(window.app_handle(), &action.action),
    )
    .await
    {
        Ok(true) => Ok(action.action),
        _ => Err(failure(request.meta.request_id)),
    }
}

async fn validate<R: Runtime>(app: &AppHandle<R>, action: &NativeTrayAction) -> bool {
    use crate::{sftp_session_service as sftp, ssh_session_service as ssh};
    match action {
        NativeTrayAction::OpenHost { host_id } => app
            .state::<crate::host_service::HostService>()
            .get_connection_snapshot(host_id)
            .is_ok(),
        NativeTrayAction::FocusTerminal { scope } => match scope {
            NativeTerminalSessionScope::Ssh {
                session_id,
                generation,
                channel_id,
                pane_id,
            } => ssh::ssh_terminal_get(
                SshSessionGetRequest {
                    meta: meta(),
                    session_id: session_id.clone(),
                },
                app.state(),
            )
            .await
            .is_ok_and(|details| {
                details.session.generation == *generation
                    && details.session.channel_id.as_ref() == Some(channel_id)
                    && details.attachments.iter().any(|attachment| {
                        attachment.view_id == *pane_id
                            && attachment.generation == *generation
                            && attachment.channel_id.as_ref() == Some(channel_id)
                    })
            }),
            NativeTerminalSessionScope::Local {
                session_id,
                generation,
                pty_id,
                pane_id,
            } => ssh::local_terminal_get(
                LocalSessionGetRequest {
                    meta: meta(),
                    session_id: session_id.clone(),
                },
                app.state(),
            )
            .await
            .is_ok_and(|details| {
                details.session.generation == *generation
                    && details.session.pty_id.as_ref() == Some(pty_id)
                    && details.attachments.iter().any(|attachment| {
                        attachment.view_id == *pane_id
                            && attachment.generation == *generation
                            && attachment.pty_id.as_ref() == Some(pty_id)
                    })
            }),
        },
        NativeTrayAction::FocusTelnet {
            session_id,
            generation,
            socket_id,
        } => app
            .state::<crate::telnet_session_service::TelnetSessionService>()
            .snapshot()
            .await
            .is_ok_and(|sessions| {
                sessions.iter().any(|session| {
                    session.session_id == *session_id
                        && session.generation == generation.get()
                        && session.socket_id.as_ref() == socket_id.as_ref()
                })
            }),
        NativeTrayAction::FocusSshSession {
            session_id,
            generation,
        } => app
            .state::<ssh::SshSessionService>()
            .snapshot(RequestId::new())
            .await
            .is_ok_and(|snapshot| {
                snapshot.sessions.iter().any(|session| {
                    session.session_id == *session_id && session.generation == *generation
                })
            }),
        NativeTrayAction::FocusLocalSession {
            session_id,
            generation,
        } => app
            .state::<ssh::SshSessionService>()
            .local_snapshot(RequestId::new())
            .await
            .is_ok_and(|snapshot| {
                snapshot.sessions.iter().any(|session| {
                    session.session_id == *session_id && session.generation == *generation
                })
            }),
        NativeTrayAction::FocusDesktop {
            session_id,
            generation,
        } => app
            .state::<crate::desktop_service::DesktopService>()
            .snapshot()
            .iter()
            .any(|session| session.id == *session_id && session.generation == *generation),
        NativeTrayAction::OpenTunnels {
            session_id: Some(id),
            generation: Some(generation),
        } => app
            .state::<crate::forward_session_service::ForwardSessionService>()
            .wire_summaries()
            .is_ok_and(|sessions| {
                sessions
                    .iter()
                    .any(|session| session.session_id == *id && session.generation == *generation)
            }),
        NativeTrayAction::OpenTransfers {
            transfer_id: Some(id),
            state_revision: Some(revision),
            source_fence: Some(source),
            target_fence: Some(target),
        } => sftp::sftp_transfer_intent_snapshot(
            SftpTransferIntentSnapshotRequest { meta: meta() },
            app.state(),
        )
        .await
        .is_ok_and(|snapshot| {
            snapshot.transfers.iter().any(|transfer| {
                transfer.transfer_id == *id
                    && transfer_fence_matches(
                        *revision,
                        source,
                        target,
                        transfer.state_revision,
                        &transfer.source_fence,
                        &transfer.target_fence,
                    )
            })
        }),
        NativeTrayAction::OpenSftp {
            session_id,
            generation,
        } => sftp::sftp_session_snapshot(SftpSessionSnapshotRequest { meta: meta() }, app.state())
            .await
            .is_ok_and(|snapshot| {
                snapshot.sessions.iter().any(|session| {
                    session.session_id == *session_id && session.generation == *generation
                })
            }),
        NativeTrayAction::Vault { expected_state } => {
            app.state::<crate::vault_service::VaultService>()
                .status()
                .state
                == *expected_state
        }
        NativeTrayAction::OpenTunnels {
            session_id,
            generation,
        } => session_id.is_none() && generation.is_none(),
        NativeTrayAction::OpenTransfers {
            transfer_id,
            state_revision,
            source_fence,
            target_fence,
        } => {
            transfer_id.is_none()
                && state_revision.is_none()
                && source_fence.is_none()
                && target_fence.is_none()
        }
        NativeTrayAction::NewTerminal
        | NativeTrayAction::NewLocalTerminal
        | NativeTrayAction::QuickConnect
        | NativeTrayAction::Settings => true,
    }
}

fn transfer_fence_matches(
    minimum_revision: WireSequence,
    source: &SftpTransferEndpointFence,
    target: &SftpTransferEndpointFence,
    current_revision: WireSequence,
    current_source: &SftpTransferEndpointFence,
    current_target: &SftpTransferEndpointFence,
) -> bool {
    current_revision >= minimum_revision && current_source == source && current_target == target
}

fn meta() -> RequestMeta {
    RequestMeta {
        request_id: RequestId::new(),
    }
}

fn handle_click(app: &AppHandle, id: &str) {
    let Some(service) = app.try_state::<NativeTrayService>() else {
        return;
    };
    let action = {
        let state = service
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stopped {
            return;
        }
        state
            .bindings
            .get(id)
            .filter(|entry| entry.created.elapsed() < MENU_TTL)
            .map(|entry| entry.action.clone())
    };
    let Some(action) = action else {
        // Global menu events also reach this callback; handle only IDs generated by this tray.
        if id.starts_with("norishell-tray-") {
            emit_failure(app);
        }
        return;
    };
    execute_action(app, &service, action);
}

fn execute_action(app: &AppHandle, service: &NativeTrayService, action: Action) {
    match action {
        Action::Show => {
            if lifecycle::show_main_window(app).is_err() {
                emit_failure(app);
            }
        }
        Action::Quit => {
            let ready = {
                let mut state = service
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.ready {
                    state.pending_quit = true;
                }
                state.ready
            };
            if ready {
                lifecycle::request_application_exit(app);
            } else if lifecycle::show_main_window(app).is_err() {
                emit_failure(app);
            }
        }
        Action::LockVault => {
            if app
                .state::<crate::vault_service::VaultService>()
                .status()
                .state
                != VaultState::Unlocked
                || crate::vault_service::vault_lock(
                    VaultLockRequest { meta: meta() },
                    app.state(),
                    app.state(),
                )
                .is_err()
            {
                emit_failure(app);
            } else {
                let _ = app.emit_to("main", "native-tray-vault-changed", ());
            }
        }
        Action::Pause(seconds) => {
            if app
                .state::<NativeNotificationService>()
                .pause_for(Duration::from_secs(seconds))
                .is_err()
            {
                emit_failure(app);
            }
        }
        Action::Resume => {
            if app.state::<NativeNotificationService>().resume().is_err() {
                emit_failure(app);
            }
        }
        Action::Frontend(action) => dispatch_frontend(app, service, action),
    }
}

fn dispatch_frontend(app: &AppHandle, service: &NativeTrayService, action: NativeTrayAction) {
    let (token, ready) = {
        let mut state = service
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let token = state.queue(action, Instant::now());
        (token, state.ready)
    };
    if let Some(token) = token {
        if lifecycle::show_main_window(app).is_err() {
            emit_failure(app);
        }
        if ready
            && app
                .emit_to("main", EVENT, NativeTrayActionEvent { token })
                .is_err()
        {
            emit_failure(app);
        }
    } else {
        emit_failure(app);
    }
}

fn item<R: Runtime>(
    app: &AppHandle<R>,
    row: &Row,
    bindings: &mut HashMap<String, Binding>,
) -> tauri::Result<MenuItem<R>> {
    let id = format!("norishell-tray-{}", Uuid::new_v4());
    if let Some(action) = &row.action {
        bindings.insert(
            id.clone(),
            Binding {
                action: action.clone(),
                created: Instant::now(),
            },
        );
    }
    MenuItem::with_id(
        app,
        id,
        projection::safe_label(&row.label),
        row.action.is_some(),
        None::<&str>,
    )
}

fn render<R: Runtime>(app: &AppHandle<R>, mut projection: Projection) -> tauri::Result<()> {
    let service = app.state::<NativeTrayService>();
    let mut state = service
        .inner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Settings or locale may change while collecting the snapshot. Recheck at the final native write boundary;
    // stale projections may only become a safe menu without host information, never restore labels that were disabled.
    if projection.locale != state.locale
        || projection.preferences_revision.is_some_and(|revision| {
            app.state::<crate::desktop_preferences::DesktopPreferencesService>()
                .snapshot()
                .ok()
                .map(|snapshot| snapshot.revision)
                != Some(revision)
        })
    {
        projection = Projection::initial(state.locale);
    }
    if state.stopped
        || (state.rendered.as_ref() == Some(&projection)
            && state
                .rendered_at
                .is_some_and(|time| time.elapsed() < Duration::from_secs(60)))
    {
        return Ok(());
    }
    state
        .bindings
        .retain(|_, entry| entry.created.elapsed() < MENU_TTL);
    if state.bindings.len() + projection.item_count() > MAX_BINDINGS {
        return Ok(());
    }
    let mut bindings = HashMap::new();
    let menu = Menu::new(app)?;
    for row in &projection.rows {
        if row.children.is_empty() {
            menu.append(&item(app, row, &mut bindings)?)?;
        } else {
            let submenu = Submenu::new(app, projection::safe_label(&row.label), true)?;
            for child in &row.children {
                submenu.append(&item(app, child, &mut bindings)?)?;
            }
            menu.append(&submenu)?;
        }
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(menu))?;
        tray.set_tooltip(Some(&projection.tooltip))?;
        state.bindings.extend(bindings);
        state.rendered = Some(projection);
        state.rendered_at = Some(Instant::now());
    }
    Ok(())
}

/// Remove the old menu immediately after settings commit; the next Core snapshot repopulates it.
pub(crate) fn preferences_changed<R: Runtime>(app: &AppHandle<R>) {
    panel::hide(app);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(service) = handle.try_state::<NativeTrayService>() else {
            return;
        };
        let locale = service
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .locale;
        if render(&handle, Projection::initial(locale)).is_err() {
            emit_failure(&handle);
        }
    });
}

pub(crate) fn install(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let (stop, mut stopped) = watch::channel(false);
    app.manage(NativeTrayService {
        inner: Arc::default(),
        stop,
    });
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .show_menu_on_left_click(false)
        .tooltip("NoriShell")
        .on_menu_event(|app, event| handle_click(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } => {
                panel::toggle(tray.app_handle(), rect);
            }
            TrayIconEvent::Click {
                button: MouseButton::Right,
                ..
            } => panel::hide(tray.app_handle()),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    render(
        app.handle(),
        Projection::initial(NativeTrayLocale::default()),
    )?;
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let mut cache = projection::Cache::default();
        loop {
            let locale = handle
                .state::<NativeTrayService>()
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .locale;
            let projected = tokio::select! {
                _ = stopped.changed() => break,
                projection = cache.refresh(&handle, locale) => projection,
            };
            let main = handle.clone();
            // Update menus and TrayIcon only on the main thread; collect snapshots outside the native UI thread.
            if handle
                .run_on_main_thread(move || {
                    if render(&main, projected).is_err() {
                        emit_failure(&main);
                    }
                })
                .is_err()
            {
                break;
            }
            tokio::select! { _ = stopped.changed() => break, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clicks_are_one_shot_expire_and_keep_exact_generations() {
        let mut state = TrayState::default();
        let now = Instant::now();
        let action = NativeTrayAction::FocusDesktop {
            session_id: "session".into(),
            generation: WireSequence::new(9007199254740993),
        };
        let token = state.queue(action.clone(), now).unwrap();
        assert_eq!(
            state.take(&token, now).map(|pending| pending.action),
            Some(action)
        );
        assert!(state.take(&token, now).is_none());
        let token = state.queue(NativeTrayAction::Settings, now).unwrap();
        assert!(state.take(&token, now + CLICK_TTL).is_none());
    }
    #[test]
    fn startup_queue_is_bounded_without_evicting_earlier_clicks() {
        let mut state = TrayState::default();
        let now = Instant::now();
        let first = state.queue(NativeTrayAction::QuickConnect, now).unwrap();
        for _ in 1..MAX_PENDING {
            assert!(state.queue(NativeTrayAction::Settings, now).is_some());
        }
        assert!(state.queue(NativeTrayAction::NewTerminal, now).is_none());
        assert_eq!(
            state.take(&first, now).map(|pending| pending.action),
            Some(NativeTrayAction::QuickConnect)
        );
    }

    #[test]
    fn newer_menu_does_not_rebind_pending_resource_action() {
        let mut state = TrayState::default();
        let now = Instant::now();
        let old = NativeTrayAction::FocusDesktop {
            session_id: "desktop".into(),
            generation: WireSequence::new(4),
        };
        let token = state.queue(old.clone(), now).unwrap();
        state.bindings.insert(
            "new-menu".into(),
            Binding {
                action: Action::Frontend(NativeTrayAction::FocusDesktop {
                    session_id: "desktop".into(),
                    generation: WireSequence::new(5),
                }),
                created: now,
            },
        );
        assert_eq!(
            state.take(&token, now).map(|pending| pending.action),
            Some(old)
        );
    }

    #[test]
    fn transfer_click_keeps_both_exact_endpoint_fences() {
        let mut state = TrayState::default();
        let now = Instant::now();
        let source = SftpTransferEndpointFence::LocalCapability {
            directory_ref: "opaque".into(),
            revision: WireSequence::new(9),
        };
        let target = SftpTransferEndpointFence::RemoteSession {
            session_id: SftpSessionId::new(),
            generation: WireSequence::new(12),
        };
        let action = NativeTrayAction::OpenTransfers {
            transfer_id: Some(TransferId::new()),
            state_revision: Some(WireSequence::new(8)),
            source_fence: Some(source),
            target_fence: Some(target),
        };
        let token = state.queue(action.clone(), now).unwrap();
        assert_eq!(state.take(&token, now).unwrap().action, action);
    }

    #[test]
    fn transfer_progress_advances_without_rebinding_either_endpoint() {
        let source = SftpTransferEndpointFence::LocalCapability {
            directory_ref: "opaque".into(),
            revision: WireSequence::new(9),
        };
        let target = SftpTransferEndpointFence::RemoteSession {
            session_id: SftpSessionId::new(),
            generation: WireSequence::new(12),
        };
        assert!(transfer_fence_matches(
            WireSequence::new(8),
            &source,
            &target,
            WireSequence::new(10),
            &source,
            &target
        ));
        assert!(!transfer_fence_matches(
            WireSequence::new(8),
            &source,
            &target,
            WireSequence::new(7),
            &source,
            &target
        ));
        let replaced_source = SftpTransferEndpointFence::LocalCapability {
            directory_ref: "opaque".into(),
            revision: WireSequence::new(10),
        };
        assert!(!transfer_fence_matches(
            WireSequence::new(8),
            &source,
            &target,
            WireSequence::new(10),
            &replaced_source,
            &target
        ));
        let mut replaced_target = target.clone();
        if let SftpTransferEndpointFence::RemoteSession { generation, .. } = &mut replaced_target {
            *generation = WireSequence::new(13);
        }
        assert!(!transfer_fence_matches(
            WireSequence::new(8),
            &source,
            &target,
            WireSequence::new(10),
            &source,
            &replaced_target
        ));
    }

    #[test]
    fn non_main_windows_cannot_read_or_consume_pending_actions() {
        let (stop, _) = watch::channel(false);
        let service = NativeTrayService {
            inner: Arc::default(),
            stop,
        };
        let token = service
            .inner
            .lock()
            .unwrap()
            .queue(NativeTrayAction::Settings, Instant::now())
            .unwrap();
        let app = tauri::test::mock_builder()
            .manage(service)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let window = tauri::WebviewWindowBuilder::new(&app, "secure-fixture", Default::default())
            .build()
            .unwrap();
        let response = tray_actions_ready(
            window,
            NativeTrayActionsReadyRequest {
                meta: meta(),
                locale: NativeTrayLocale::En,
            },
            app.state(),
        );
        assert_eq!(response.unwrap_err().code, "tray.action_unavailable");
        assert!(
            app.state::<NativeTrayService>()
                .inner
                .lock()
                .unwrap()
                .pending
                .iter()
                .any(|entry| entry.token == token)
        );
    }
}
