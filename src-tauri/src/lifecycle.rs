use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
};

use norishell_core_api::{
    ApplicationExitRequest, CoreApiError, DesktopWindowCloseBehavior, ExitBlocker, ExitReadiness,
    RequestId, WindowCloseRequest,
};
#[cfg(target_os = "macos")]
use tauri::menu::{MenuItemKind, PredefinedMenuItem};
use tauri::{
    App, AppHandle, Emitter, Manager, Runtime, State, WebviewWindow, Window, WindowEvent,
    menu::{Menu, MenuItem},
};
use uuid::Uuid;

use crate::{
    forward_session_service::ForwardSessionService, host_service::HostService,
    metrics_session_service::MetricsSessionService, plugin_service::PluginService,
    sftp_session_service::SftpSessionService, ssh_session_service::SshSessionService,
    telnet_session_service::TelnetSessionService, tool_window_exit::ToolWindowExitPermit,
};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

const MAIN_WINDOW_LABEL: &str = "main";
const TRAY_ID: &str = "norishell-main";
const APPLICATION_QUIT_ID: &str = "norishell-application-quit";

const LIFECYCLE_IDLE: u8 = 0;
const LIFECYCLE_CLEANING: u8 = 1;
const LIFECYCLE_AUTHORIZED: u8 = 2;

#[derive(Default)]
struct LifecycleGateInner {
    phase: AtomicU8,
    active_resource_creators: AtomicUsize,
    creators_drained: tokio::sync::Notify,
}

#[derive(Clone, Default)]
pub(crate) struct LifecycleState {
    gate: Arc<LifecycleGateInner>,
}

pub(crate) struct ResourceCreationPermit {
    gate: Arc<LifecycleGateInner>,
}

impl Drop for ResourceCreationPermit {
    fn drop(&mut self) {
        if self
            .gate
            .active_resource_creators
            .fetch_sub(1, Ordering::AcqRel)
            == 1
        {
            self.gate.creators_drained.notify_waiters();
        }
    }
}

impl LifecycleState {
    pub fn authorize_exit(&self) {
        self.gate
            .phase
            .store(LIFECYCLE_AUTHORIZED, Ordering::Release);
    }

    pub fn is_exit_authorized(&self) -> bool {
        self.gate.phase.load(Ordering::Acquire) == LIFECYCLE_AUTHORIZED
    }

    pub(crate) fn acquire_resource_creation(
        &self,
        request_id: norishell_core_api::RequestId,
    ) -> CoreResult<ResourceCreationPermit> {
        if self.gate.phase.load(Ordering::Acquire) != LIFECYCLE_IDLE {
            return Err(Box::new(CoreApiError::safe_internal(
                request_id,
                Uuid::new_v4().to_string(),
            )));
        }
        self.gate
            .active_resource_creators
            .fetch_add(1, Ordering::AcqRel);
        if self.gate.phase.load(Ordering::Acquire) != LIFECYCLE_IDLE {
            if self
                .gate
                .active_resource_creators
                .fetch_sub(1, Ordering::AcqRel)
                == 1
            {
                self.gate.creators_drained.notify_waiters();
            }
            return Err(Box::new(CoreApiError::safe_internal(
                request_id,
                Uuid::new_v4().to_string(),
            )));
        }
        Ok(ResourceCreationPermit {
            gate: Arc::clone(&self.gate),
        })
    }

    async fn begin_cleanup(&self) -> bool {
        if self
            .gate
            .phase
            .compare_exchange(
                LIFECYCLE_IDLE,
                LIFECYCLE_CLEANING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        loop {
            let drained = self.gate.creators_drained.notified();
            if self.gate.active_resource_creators.load(Ordering::Acquire) == 0 {
                return true;
            }
            drained.await;
        }
    }

    fn cancel_cleanup(&self) {
        let _ = self.gate.phase.compare_exchange(
            LIFECYCLE_CLEANING,
            LIFECYCLE_IDLE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

#[derive(Default)]
pub(crate) struct UpdateExitState {
    permit: Mutex<Option<ToolWindowExitPermit>>,
}

/// Plugin operations and resources are children of the global session owners.
/// Finish their cleanup attempt first so the global pass cannot remove the
/// exact session facts that a plugin still needs to reconcile. The second
/// phase always runs, including when plugin cleanup reports an error.
async fn cleanup_plugins_before_global_owners<
    PluginCleanup,
    PluginResult,
    GlobalCleanup,
    GlobalResult,
>(
    plugin_cleanup: PluginCleanup,
    global_cleanup: impl FnOnce() -> GlobalCleanup,
) -> (PluginResult, GlobalResult)
where
    PluginCleanup: Future<Output = PluginResult>,
    GlobalCleanup: Future<Output = GlobalResult>,
{
    let plugin_result = plugin_cleanup.await;
    let global_result = global_cleanup().await;
    (plugin_result, global_result)
}

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        crate::window_first_show::show_if_revealed(&window)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainWindowCloseAction {
    Hide,
    RequestApplicationExit,
}

fn close_action_for_behavior(behavior: DesktopWindowCloseBehavior) -> MainWindowCloseAction {
    match behavior {
        DesktopWindowCloseBehavior::Hide => MainWindowCloseAction::Hide,
        DesktopWindowCloseBehavior::Quit => MainWindowCloseAction::RequestApplicationExit,
    }
}

fn main_window_close_action(
    label: &str,
    exit_authorized: bool,
    behavior: DesktopWindowCloseBehavior,
) -> Option<MainWindowCloseAction> {
    (label == MAIN_WINDOW_LABEL && !exit_authorized).then(|| close_action_for_behavior(behavior))
}

fn tray_is_available<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

fn keep_main_window_visible<R: Runtime>(window: &Window<R>) {
    #[cfg(any(windows, target_os = "macos"))]
    if !crate::window_first_show::was_revealed(window.app_handle(), window.label()) {
        return;
    }
    if let Err(error) = window.show() {
        eprintln!("failed to keep main window visible after close refusal: {error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("failed to focus main window after close refusal: {error}");
    }
}

pub fn hide_window_on_close<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let lifecycle = window.state::<LifecycleState>();
    #[cfg(windows)]
    eprintln!(
        "NoriShell window close requested; label={}; exit_authorized={}",
        window.label(),
        lifecycle.is_exit_authorized()
    );
    if window.label() != MAIN_WINDOW_LABEL || lifecycle.is_exit_authorized() {
        // Standalone secure windows must actually be destroyed to execute denial and cancellation cleanup.
        return;
    }
    let Some(preferences) =
        window.try_state::<crate::desktop_preferences::DesktopPreferencesService>()
    else {
        api.prevent_close();
        keep_main_window_visible(window);
        eprintln!("refused main-window close because desktop preferences are unavailable");
        return;
    };
    let behavior = match preferences.window_close_behavior() {
        Ok(behavior) => behavior,
        Err(error) => {
            api.prevent_close();
            keep_main_window_visible(window);
            eprintln!(
                "refused main-window close because desktop preferences could not be read: {error}"
            );
            return;
        }
    };
    match main_window_close_action(window.label(), lifecycle.is_exit_authorized(), behavior) {
        Some(MainWindowCloseAction::Hide) => {
            api.prevent_close();
            if !tray_is_available(window.app_handle()) {
                keep_main_window_visible(window);
                eprintln!("refused main-window hide because tray {TRAY_ID} is unavailable");
                return;
            }
            if let Err(error) = window.hide() {
                keep_main_window_visible(window);
                eprintln!("failed to hide main window after close request: {error}");
            }
        }
        Some(MainWindowCloseAction::RequestApplicationExit) => {
            api.prevent_close();
            request_application_exit(window.app_handle());
        }
        None => {}
    }
}

pub fn request_application_exit<R: Runtime>(app: &AppHandle<R>) {
    if let Err(error) = show_main_window(app) {
        eprintln!("failed to show application before exit request: {error}");
    }
    // The frontend owns the latest debounced Tab/Pane projection. Native
    // menu/tray/OS exit intents first request its durable-write barrier; the
    // acknowledged application_request_exit command below then applies the
    // authoritative Core session blocker gate and exits.
    if let Err(error) = app.emit("application-exit-requested", ()) {
        eprintln!("failed to request terminal layout flush before exit: {error}");
    }
}

/// The macOS predefined Quit item invokes AppKit's `terminate:` selector
/// directly, which bypasses Tauri's preventable `ExitRequested` event. Replace
/// it with an ordinary menu item so Cmd-Q follows the same readiness gate as
/// the tray and frontend exit intents.
pub fn application_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    #[cfg(target_os = "macos")]
    {
        // Native performClose: handles Command-W before the WebView shortcut registry.
        // Keep the menu action but remove its fixed accelerator so configurable tab closing exclusively owns the shortcut.
        let close_label = PredefinedMenuItem::close_window(app, None)?.text()?;
        for item in menu.items()? {
            let MenuItemKind::Submenu(submenu) = item else {
                continue;
            };
            for child in submenu.items()? {
                if let MenuItemKind::Predefined(predefined) = &child
                    && predefined.text()? == close_label
                {
                    let index = submenu
                        .items()?
                        .iter()
                        .position(|candidate| candidate.id() == child.id());
                    if let Some(index) = index {
                        submenu.remove_at(index)?;
                        let close = MenuItem::with_id(
                            app,
                            "application-close-window",
                            &close_label,
                            true,
                            None::<&str>,
                        )?;
                        submenu.insert(&close, index)?;
                    }
                }
            }
        }
        let app_submenu = menu
            .items()?
            .into_iter()
            .next()
            .and_then(|item| match item {
                MenuItemKind::Submenu(submenu) => Some(submenu),
                _ => None,
            })
            .ok_or_else(|| {
                tauri::Error::Io(std::io::Error::other(
                    "macOS application submenu is missing",
                ))
            })?;
        let quit_position = app_submenu.items()?.len().checked_sub(1).ok_or_else(|| {
            tauri::Error::Io(std::io::Error::other(
                "macOS application quit item is missing",
            ))
        })?;
        app_submenu.remove_at(quit_position)?.ok_or_else(|| {
            tauri::Error::Io(std::io::Error::other(
                "macOS application quit item could not be removed",
            ))
        })?;
        let quit = MenuItem::with_id(
            app,
            APPLICATION_QUIT_ID,
            format!("Quit {}", app.package_info().name),
            true,
            Some("CmdOrCtrl+Q"),
        )?;
        app_submenu.append(&quit)?;
    }
    Ok(menu)
}

pub fn handle_application_menu_event<R: Runtime>(
    app: &AppHandle<R>,
    event: tauri::menu::MenuEvent,
) {
    if event.id().as_ref() == APPLICATION_QUIT_ID {
        request_application_exit(app);
    }
    #[cfg(target_os = "macos")]
    if event.id().as_ref() == "application-close-window"
        && let Some(window) = app
            .webview_windows()
            .into_values()
            .find(|window| window.is_focused().unwrap_or(false))
    {
        let _ = window.close();
    }
}

#[tauri::command]
pub fn window_request_close(
    request: WindowCloseRequest,
    window: WebviewWindow,
    preferences: State<'_, crate::desktop_preferences::DesktopPreferencesService>,
) -> CoreResult<ExitReadiness> {
    let request_id = request.meta.request_id;
    if window.label() != MAIN_WINDOW_LABEL {
        return Err(crate::desktop_preferences::main_window_required_error(
            request_id,
        ));
    }
    let behavior = preferences.window_close_behavior().map_err(|error| {
        crate::desktop_preferences::map_preferences_error(request_id.clone(), error)
    })?;
    match close_action_for_behavior(behavior) {
        MainWindowCloseAction::Hide => {
            if !tray_is_available(window.app_handle()) {
                return Err(crate::desktop_preferences::tray_unavailable_error(
                    request_id,
                ));
            }
            window
                .hide()
                .map_err(|_| crate::desktop_preferences::window_hide_error(request_id))?;
        }
        MainWindowCloseAction::RequestApplicationExit => {
            request_application_exit(window.app_handle());
        }
    }
    Ok(current_exit_readiness(window.app_handle()))
}

#[allow(clippy::too_many_arguments)]
async fn shutdown_all_resources<R: Runtime>(
    request_id: RequestId,
    app: &AppHandle<R>,
    sessions: &SshSessionService,
    metrics: &MetricsSessionService,
    telnet: &TelnetSessionService,
    forwards: &ForwardSessionService,
    sftp: &SftpSessionService,
    plugins: &PluginService,
    hosts: &HostService,
) -> CoreResult<()> {
    // Every owner receives the same explicit-exit cleanup opportunity even
    // when a sibling fails. A single residual keeps the application alive.
    let (plugin_result, (ssh_result, metrics_result, telnet_result, forward_result, sftp_result)) =
        cleanup_plugins_before_global_owners(plugins.shutdown_all(), || async {
            tokio::join!(
                sessions.shutdown_all(request_id.clone()),
                metrics.shutdown_all(request_id.clone()),
                telnet.shutdown_all(),
                forwards.shutdown_all(),
                sftp.shutdown_all(),
            )
        })
        .await;
    let cleanup_result = ssh_result.and(metrics_result).and_then(|_| {
        telnet_result.map_err(|_| {
            Box::new(CoreApiError::safe_internal(
                request_id.clone(),
                Uuid::new_v4().to_string(),
            ))
        })
    });
    let cleanup_result = cleanup_result.and_then(|_| {
        forward_result.map(|_| ()).map_err(|_| {
            Box::new(CoreApiError::safe_internal(
                request_id.clone(),
                Uuid::new_v4().to_string(),
            ))
        })
    });
    let cleanup_result = cleanup_result.and_then(|_| {
        sftp_result.map_err(|_| {
            Box::new(CoreApiError::safe_internal(
                request_id.clone(),
                Uuid::new_v4().to_string(),
            ))
        })
    });
    let cleanup_result = cleanup_result.and(plugin_result);
    let desktop_result =
        if let Some(desktops) = app.try_state::<crate::desktop_service::DesktopService>() {
            desktops.shutdown_all().await.map_err(|_| {
                Box::new(CoreApiError::safe_internal(
                    request_id.clone(),
                    Uuid::new_v4().to_string(),
                ))
            })
        } else {
            Ok(())
        };
    cleanup_result.and(desktop_result)?;
    hosts.shutdown_login_automation_secret_reconciler();
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn application_request_exit(
    request: ApplicationExitRequest,
    app: AppHandle,
    lifecycle: State<'_, LifecycleState>,
    sessions: State<'_, SshSessionService>,
    metrics: State<'_, MetricsSessionService>,
    telnet: State<'_, TelnetSessionService>,
    forwards: State<'_, ForwardSessionService>,
    sftp: State<'_, SftpSessionService>,
    plugins: State<'_, PluginService>,
    hosts: State<'_, HostService>,
) -> CoreResult<ExitReadiness> {
    let readiness = current_exit_readiness(&app);
    if readiness.can_exit || request.disconnect_active_resources {
        // Editors must approve and finish their own staged-secret/file cleanup before
        // any session owner is stopped. Cancellation leaves all resource owners intact.
        let tool_exit = app.state::<crate::tool_window_exit::ToolWindowExit>();
        let tool_windows = app.state::<crate::tool_windows::ToolWindows>();
        let tool_exit_permit = tool_exit.prepare(&app, &tool_windows).await.map_err(|()| {
            Box::new(CoreApiError {
                code: "app.tool_window_exit_cancelled".into(),
                category: norishell_core_api::ErrorCategory::Conflict,
                retry_strategy: norishell_core_api::RetryStrategy::WaitForUser,
                message_key: "toolWindows.exitCancelled".into(),
                params: Default::default(),
                request_id: Some(request.meta.request_id.clone()),
                diagnostic_id: None,
                conflict: None,
            })
        })?;
        if !lifecycle.begin_cleanup().await {
            return Err(Box::new(CoreApiError::safe_internal(
                request.meta.request_id,
                Uuid::new_v4().to_string(),
            )));
        }
        // Resource creation can finish while the user considers a draft prompt.
        // Recheck after creation permits drain; never treat the earlier empty snapshot
        // as consent to disconnect resources created during that interval.
        if !request.disconnect_active_resources {
            let current = current_exit_readiness(&app);
            if !current.can_exit {
                lifecycle.cancel_cleanup();
                return Ok(current);
            }
        }
        if let Err(error) = shutdown_all_resources(
            request.meta.request_id.clone(),
            &app,
            &sessions,
            &metrics,
            &telnet,
            &forwards,
            &sftp,
            &plugins,
            &hosts,
        )
        .await
        {
            lifecycle.cancel_cleanup();
            return Err(error);
        }
        lifecycle.authorize_exit();
        tool_exit_permit.commit();
        app.exit(0);
    } else if show_main_window(&app).is_err() {
        return Err(Box::new(CoreApiError::safe_internal(
            request.meta.request_id,
            Uuid::new_v4().to_string(),
        )));
    }
    Ok(if request.disconnect_active_resources {
        ExitReadiness {
            can_exit: true,
            blockers: Vec::new(),
        }
    } else {
        readiness
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn release_update_prepare_install<R: Runtime>(
    window: WebviewWindow<R>,
    disconnect_active_resources: bool,
    app: AppHandle<R>,
    lifecycle: State<'_, LifecycleState>,
    update_exit: State<'_, UpdateExitState>,
    sessions: State<'_, SshSessionService>,
    metrics: State<'_, MetricsSessionService>,
    telnet: State<'_, TelnetSessionService>,
    forwards: State<'_, ForwardSessionService>,
    sftp: State<'_, SftpSessionService>,
    plugins: State<'_, PluginService>,
    hosts: State<'_, HostService>,
) -> CoreResult<ExitReadiness> {
    let request_id = RequestId::new();
    if window.label() != MAIN_WINDOW_LABEL {
        return Err(Box::new(CoreApiError::safe_internal(
            request_id,
            Uuid::new_v4().to_string(),
        )));
    }
    let readiness = current_update_readiness(&app);
    if !readiness.can_exit && !disconnect_active_resources {
        return Ok(readiness);
    }
    let tool_exit = app.state::<crate::tool_window_exit::ToolWindowExit>();
    let tool_windows = app.state::<crate::tool_windows::ToolWindows>();
    let permit = tool_exit.prepare(&app, &tool_windows).await.map_err(|()| {
        Box::new(CoreApiError {
            code: "app.tool_window_exit_cancelled".into(),
            category: norishell_core_api::ErrorCategory::Conflict,
            retry_strategy: norishell_core_api::RetryStrategy::WaitForUser,
            message_key: "toolWindows.exitCancelled".into(),
            params: Default::default(),
            request_id: Some(request_id.clone()),
            diagnostic_id: None,
            conflict: None,
        })
    })?;
    if !lifecycle.begin_cleanup().await {
        return Err(Box::new(CoreApiError::safe_internal(
            request_id,
            Uuid::new_v4().to_string(),
        )));
    }
    let current = current_update_readiness(&app);
    if !current.can_exit && !disconnect_active_resources {
        lifecycle.cancel_cleanup();
        return Ok(current);
    }
    if let Err(error) = shutdown_all_resources(
        request_id.clone(),
        &app,
        &sessions,
        &metrics,
        &telnet,
        &forwards,
        &sftp,
        &plugins,
        &hosts,
    )
    .await
    {
        lifecycle.cancel_cleanup();
        return Err(error);
    }
    let mut pending = match update_exit.permit.lock() {
        Ok(pending) => pending,
        Err(_) => {
            lifecycle.cancel_cleanup();
            return Err(Box::new(CoreApiError::safe_internal(
                request_id,
                Uuid::new_v4().to_string(),
            )));
        }
    };
    if pending.is_some() {
        lifecycle.cancel_cleanup();
        return Err(Box::new(CoreApiError::safe_internal(
            request_id,
            Uuid::new_v4().to_string(),
        )));
    }
    *pending = Some(permit);
    Ok(ExitReadiness {
        can_exit: true,
        blockers: Vec::new(),
    })
}

#[tauri::command]
pub(crate) fn release_update_allow_relaunch<R: Runtime>(
    window: WebviewWindow<R>,
    lifecycle: State<'_, LifecycleState>,
    update_exit: State<'_, UpdateExitState>,
) -> Result<(), &'static str> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err("unavailable");
    }
    if update_exit
        .permit
        .lock()
        .map_err(|_| "unavailable")?
        .is_none()
    {
        return Err("unavailable");
    }
    lifecycle.authorize_exit();
    Ok(())
}

pub(crate) fn current_exit_readiness<R: Runtime>(app: &AppHandle<R>) -> ExitReadiness {
    let sessions = app.state::<SshSessionService>();
    let mut blockers = sessions
        .exit_blockers()
        .into_iter()
        .map(|session_id| ExitBlocker::SshSession { session_id })
        .collect::<Vec<_>>();
    if let Some(desktops) = app.try_state::<crate::desktop_service::DesktopService>() {
        blockers.extend(
            desktops
                .exit_blockers()
                .into_iter()
                .map(|session_id| ExitBlocker::DesktopSession { session_id }),
        );
    }
    blockers.extend(
        sessions
            .local_exit_blockers()
            .into_iter()
            .map(|session_id| ExitBlocker::LocalTerminal { session_id }),
    );
    let plugins = app.state::<PluginService>();
    blockers.extend(plugins.exit_blockers());
    let telnet = app.state::<TelnetSessionService>();
    blockers.extend(telnet.exit_blockers().into_iter().filter_map(|session_id| {
        norishell_core_api::TelnetSessionId::parse(session_id)
            .ok()
            .map(|session_id| ExitBlocker::TelnetSession { session_id })
    }));
    let forwards = app.state::<ForwardSessionService>();
    blockers.extend(
        forwards
            .exit_blockers()
            .into_iter()
            .filter_map(|session_id| {
                norishell_core_api::ForwardSessionId::parse(session_id)
                    .ok()
                    .map(|session_id| ExitBlocker::ForwardSession { session_id })
            }),
    );
    let sftp = app.state::<SftpSessionService>();
    let (sftp_sessions, sftp_transfers) = sftp.exit_blockers();
    blockers.extend(
        sftp_sessions
            .into_iter()
            .map(|session_id| ExitBlocker::SftpSession { session_id }),
    );
    blockers.extend(
        sftp_transfers
            .into_iter()
            .map(|transfer_id| ExitBlocker::SftpTransfer { transfer_id }),
    );
    ExitReadiness {
        can_exit: blockers.is_empty(),
        blockers,
    }
}

pub(crate) fn current_update_readiness<R: Runtime>(app: &AppHandle<R>) -> ExitReadiness {
    update_readiness_from_exit(current_exit_readiness(app))
}

fn update_readiness_from_exit(mut readiness: ExitReadiness) -> ExitReadiness {
    // An idle plugin host is owned by the app and will be stopped by the
    // update cleanup. Plugin resources and user sessions remain blockers.
    readiness
        .blockers
        .retain(|blocker| !matches!(blocker, ExitBlocker::PluginHost { .. }));
    readiness.can_exit = readiness.blockers.is_empty();
    readiness
}

pub fn install_tray(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    crate::tray_service::install(app)
}

#[cfg(test)]
mod tests {
    use super::{
        LifecycleState, MainWindowCloseAction, cleanup_plugins_before_global_owners,
        main_window_close_action, update_readiness_from_exit,
    };
    use crate::telnet_session_service::{
        TelnetOpenRequest, TelnetSessionService, TelnetSessionState,
    };
    use norishell_core_api::{
        DesktopWindowCloseBehavior, ExitBlocker, ExitReadiness, PluginId, RequestId, SshSessionId,
    };
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::{io::AsyncReadExt, net::TcpListener};
    use uuid::Uuid;

    #[test]
    fn only_the_main_window_uses_its_preferred_close_behavior() {
        assert_eq!(
            main_window_close_action("main", false, DesktopWindowCloseBehavior::Hide),
            Some(MainWindowCloseAction::Hide)
        );
        assert_eq!(
            main_window_close_action("main", false, DesktopWindowCloseBehavior::Quit),
            Some(MainWindowCloseAction::RequestApplicationExit)
        );
        assert_eq!(
            main_window_close_action("main", true, DesktopWindowCloseBehavior::Hide),
            None
        );
        for label in [
            "secure-plugin-permission-fixture",
            "secure-plugin-host-fixture",
            "secure-plugin-input-fixture",
            "secure-plugin-remote-fixture",
            "secure-ssh-sync-fixture",
            "plugin-isolated-fixture",
        ] {
            assert_eq!(
                main_window_close_action(label, false, DesktopWindowCloseBehavior::Hide),
                None,
                "{label}"
            );
            assert_eq!(
                main_window_close_action(label, false, DesktopWindowCloseBehavior::Quit),
                None,
                "{label}"
            );
        }
    }

    #[test]
    fn exit_requires_explicit_authorization() {
        let state = LifecycleState::default();
        assert!(!state.is_exit_authorized());
        state.authorize_exit();
        assert!(state.is_exit_authorized());
    }

    #[test]
    fn update_allows_idle_plugin_hosts_but_not_live_resources() {
        let host = ExitBlocker::PluginHost {
            plugin_id: PluginId::parse("com.norishell.self-host-sync").unwrap(),
            process_id: 42,
        };
        let ready = update_readiness_from_exit(ExitReadiness {
            can_exit: false,
            blockers: vec![host.clone()],
        });
        assert!(ready.can_exit);
        assert!(ready.blockers.is_empty());

        let ssh = ExitBlocker::SshSession {
            session_id: SshSessionId::new(),
        };
        let plugin_resource = ExitBlocker::PluginResource {
            plugin_id: PluginId::parse("com.norishell.self-host-sync").unwrap(),
            resource_kind: "plugin-workflow-task".to_owned(),
            resource_id: "task-1".to_owned(),
        };
        let blocked = update_readiness_from_exit(ExitReadiness {
            can_exit: false,
            blockers: vec![host, ssh.clone(), plugin_resource.clone()],
        });
        assert!(!blocked.can_exit);
        assert_eq!(blocked.blockers, vec![ssh, plugin_resource]);
    }

    #[tokio::test]
    async fn plugin_cleanup_failure_still_precedes_and_runs_every_global_owner() {
        async fn record_owner(
            events: Arc<Mutex<Vec<&'static str>>>,
            owner: &'static str,
            result: Result<(), &'static str>,
        ) -> Result<(), &'static str> {
            events.lock().unwrap().push(owner);
            result
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let plugin_events = Arc::clone(&events);
        let global_events = Arc::clone(&events);
        let (plugin_result, global_results) = cleanup_plugins_before_global_owners(
            async move {
                plugin_events.lock().unwrap().push("plugins");
                Err::<(), _>("plugin cleanup failed")
            },
            move || async move {
                tokio::join!(
                    record_owner(Arc::clone(&global_events), "ssh", Ok(())),
                    record_owner(
                        Arc::clone(&global_events),
                        "metrics",
                        Err("metrics cleanup failed")
                    ),
                    record_owner(Arc::clone(&global_events), "telnet", Ok(())),
                    record_owner(Arc::clone(&global_events), "forwards", Ok(())),
                    record_owner(Arc::clone(&global_events), "sftp", Ok(())),
                )
            },
        )
        .await;

        assert_eq!(plugin_result, Err("plugin cleanup failed"));
        assert_eq!(global_results.0, Ok(()));
        assert_eq!(global_results.1, Err("metrics cleanup failed"));
        assert_eq!(global_results.2, Ok(()));
        assert_eq!(global_results.3, Ok(()));
        assert_eq!(global_results.4, Ok(()));
        let events = events.lock().unwrap();
        assert_eq!(events.first(), Some(&"plugins"));
        let mut global_owners = events[1..].to_vec();
        global_owners.sort_unstable();
        assert_eq!(
            global_owners,
            ["forwards", "metrics", "sftp", "ssh", "telnet"]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cleanup_drains_inflight_creation_and_fences_new_resources_until_cancelled() {
        let state = LifecycleState::default();
        let permit = state
            .acquire_resource_creation(RequestId::new())
            .expect("idle lifecycle accepts resource creation");
        let cleanup_state = state.clone();
        let cleanup = tokio::spawn(async move { cleanup_state.begin_cleanup().await });
        tokio::task::yield_now().await;

        assert!(
            state.acquire_resource_creation(RequestId::new()).is_err(),
            "new resources are fenced while cleanup waits for an older creator"
        );
        assert!(!cleanup.is_finished());
        drop(permit);
        assert!(cleanup.await.expect("cleanup task"));
        assert!(
            state.acquire_resource_creation(RequestId::new()).is_err(),
            "cleanup reply cannot reopen the resource creation race"
        );

        state.cancel_cleanup();
        assert!(
            state.acquire_resource_creation(RequestId::new()).is_ok(),
            "failed cleanup restores the normal creation phase"
        );
        state.authorize_exit();
        assert!(
            state.acquire_resource_creation(RequestId::new()).is_err(),
            "authorized exit keeps the fence closed"
        );
    }

    #[tokio::test]
    async fn failed_sibling_cleanup_reopens_the_gate_and_telnet_cleanup_remains_repeatable() {
        let state = LifecycleState::default();
        let telnet = TelnetSessionService::start();

        for cleanup_attempt in 0..2 {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
            let port = listener.local_addr().expect("address").port();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut byte = [0_u8; 1];
                tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
                    .await
                    .expect("shutdown timeout")
                    .expect("server read")
            });
            let permit = state
                .acquire_resource_creation(RequestId::new())
                .expect("idle lifecycle accepts Telnet creation");
            let opened = telnet
                .open(TelnetOpenRequest {
                    operation_id: Uuid::new_v4().to_string(),
                    idempotency_key: format!("cleanup-attempt-{cleanup_attempt}"),
                    open_attempt_id: Uuid::new_v4().to_string(),
                    address: "127.0.0.1".to_owned(),
                    port: Some(port),
                    rows: 24,
                    cols: 80,
                    cleartext_risk_accepted: true,
                    attach_attempt_id: Uuid::new_v4().to_string(),
                    view_id: Uuid::new_v4().to_string(),
                })
                .await
                .expect("open accepted");
            drop(permit);
            let mut reached_running = false;
            for _ in 0..100 {
                reached_running = telnet
                    .snapshot()
                    .await
                    .expect("actor remains available")
                    .into_iter()
                    .any(|session| {
                        session.session_id == opened.session.session_id
                            && session.state == TelnetSessionState::Running
                    });
                if reached_running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(reached_running, "Telnet session did not reach Running");

            assert!(state.begin_cleanup().await);
            telnet
                .shutdown_all()
                .await
                .expect("repeatable Telnet cleanup");
            assert_eq!(server.await.expect("server task"), 0);

            if cleanup_attempt == 0 {
                // A sibling service failed after Telnet cleanup. The lifecycle
                // returns to Idle and the same Telnet actor accepts new work.
                state.cancel_cleanup();
                telnet.snapshot().await.expect("actor remains available");
            } else {
                state.authorize_exit();
            }
        }
    }
}
