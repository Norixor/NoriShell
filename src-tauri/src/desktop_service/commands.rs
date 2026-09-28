//! Tauri adapters validate callers, lifecycle, and input permissions; they contain no protocol implementation.
use super::DesktopService;
use norishell_core_api::*;
use norishell_desktop_protocol::{
    DesktopFrame, DesktopInput, DesktopRect, EngineCommand, EngineError,
};
use tauri::{AppHandle, State, Webview, WebviewWindow, ipc::Response};
use tokio::sync::oneshot;

type CoreResult<T> = Result<T, Box<CoreApiError>>;

fn ordinary_window(
    webview: &Webview,
    workspaces: &State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: &State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    expected_owner: &str,
) -> bool {
    views.ordinary_owner(webview, workspaces).as_deref() == Ok(expected_owner)
}
#[tauri::command]
pub(crate) fn desktop_availability() -> Vec<DesktopAvailability> {
    vec![
        DesktopAvailability {
            protocol: DesktopProtocol::Rdp,
            available: true,
            reason_key: None,
            presentation: "embeddedCanvas".into(),
        },
        DesktopAvailability {
            protocol: DesktopProtocol::Vnc,
            available: true,
            reason_key: None,
            presentation: "embeddedCanvas".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_requires_matching_base_and_small_bounded_dirty_region() {
        let frame = DesktopFrame::new(8, 8).unwrap();
        let dirty = DesktopRect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        assert_eq!(frame_region(&frame, 5, 5, Some(dirty), false), (5, dirty));
        assert_eq!(frame_region(&frame, 4, 5, Some(dirty), false).0, 0);
        assert_eq!(frame_region(&frame, 5, 5, Some(dirty), true).0, 0);
        assert_eq!(
            frame_region(
                &frame,
                5,
                5,
                Some(DesktopRect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 8
                }),
                false
            )
            .0,
            0
        );
    }

    #[test]
    fn desktop_availability_is_limited_to_embedded_rdp_and_vnc() {
        let availability = desktop_availability();

        assert_eq!(availability.len(), 2);
        assert!(matches!(availability[0].protocol, DesktopProtocol::Rdp));
        assert!(matches!(availability[1].protocol, DesktopProtocol::Vnc));
        assert!(availability.iter().all(|entry| {
            entry.available && entry.reason_key.is_none() && entry.presentation == "embeddedCanvas"
        }));
    }

    #[test]
    fn profile_conflict_and_missing_profile_keep_separate_codes() {
        let meta = RequestMeta {
            request_id: RequestId::new(),
        };
        let conflict = map_profile_error(
            &meta,
            norishell_app_persistence::AppPersistenceError::Conflict,
        );
        let missing = map_profile_error(
            &meta,
            norishell_app_persistence::AppPersistenceError::NotFound,
        );
        assert_eq!(conflict.code, "desktop.profile_conflict");
        assert_eq!(missing.code, "desktop.profile_not_found");
        assert_eq!(conflict.message_key, "desktop.profileErrors.conflict");
    }
}
fn frame_region(
    frame: &DesktopFrame,
    after: u64,
    base: u64,
    dirty: Option<DesktopRect>,
    requires_full: bool,
) -> (u64, DesktopRect) {
    let patch = if !requires_full && after == base && after != 0 {
        dirty.filter(|rect| {
            rect.fits(frame.width, frame.height)
                && u64::from(rect.width) * u64::from(rect.height) * 2
                    < u64::from(frame.width) * u64::from(frame.height)
        })
    } else {
        None
    };
    match patch {
        Some(rect) => (after, rect),
        None => (
            0,
            DesktopRect {
                x: 0,
                y: 0,
                width: frame.width,
                height: frame.height,
            },
        ),
    }
}
fn map_error(meta: &RequestMeta, error: EngineError) -> Box<CoreApiError> {
    let (category, retry_strategy) = match error {
        EngineError::InvalidConfiguration
        | EngineError::RdpResolutionModeDisabled
        | EngineError::VncResolutionModeDisabled => {
            (ErrorCategory::Validation, RetryStrategy::Never)
        }
        EngineError::AuthenticationRejected | EngineError::CertificateRejected => {
            (ErrorCategory::Permission, RetryStrategy::WaitForUser)
        }
        EngineError::UnsupportedAuthentication
        | EngineError::UnsupportedOperation
        | EngineError::RdpUdpUnavailable
        | EngineError::RdpGraphicsUnavailable
        | EngineError::RdpResolutionUnavailable
        | EngineError::VncResolutionUnavailable
        | EngineError::VncResolutionRejected => (ErrorCategory::Incompatible, RetryStrategy::Never),
        EngineError::Protocol => (ErrorCategory::Internal, RetryStrategy::Never),
        EngineError::ResourceLimit | EngineError::ConnectionLost => {
            (ErrorCategory::Unavailable, RetryStrategy::RefreshSnapshot)
        }
        EngineError::Timeout
        | EngineError::RdpResolutionNotApplied
        | EngineError::VncResolutionNotApplied => {
            (ErrorCategory::Timeout, RetryStrategy::RefreshSnapshot)
        }
        EngineError::Cancelled => (ErrorCategory::Unavailable, RetryStrategy::Never),
        EngineError::StaleInput => (ErrorCategory::Conflict, RetryStrategy::RefreshSnapshot),
    };
    let diagnostic_id = matches!(error, EngineError::Protocol).then(|| {
        let id = uuid::Uuid::new_v4().to_string();
        eprintln!("desktop protocol failed: diagnostic_id={id}");
        id
    });
    Box::new(CoreApiError {
        code: format!("desktop.{error}"),
        category,
        message_key: format!("desktop.errors.{error}"),
        retry_strategy,
        request_id: Some(meta.request_id.clone()),
        diagnostic_id,
        params: Default::default(),
        conflict: None,
    })
}

fn map_tab_error(meta: &RequestMeta, code: String) -> Box<CoreApiError> {
    Box::new(CoreApiError {
        code,
        category: ErrorCategory::Conflict,
        retry_strategy: RetryStrategy::RefreshSnapshot,
        message_key: "desktop.errors.StaleInput".into(),
        request_id: Some(meta.request_id.clone()),
        diagnostic_id: None,
        params: Default::default(),
        conflict: None,
    })
}

fn map_profile_error(
    meta: &RequestMeta,
    error: norishell_app_persistence::AppPersistenceError,
) -> Box<CoreApiError> {
    use norishell_app_persistence::AppPersistenceError;
    let (code, category, retry_strategy, message_key) = match error {
        AppPersistenceError::Conflict | AppPersistenceError::IdempotencyConflict => (
            "desktop.profile_conflict",
            ErrorCategory::Conflict,
            RetryStrategy::RefreshSnapshot,
            "desktop.profileErrors.conflict",
        ),
        AppPersistenceError::NotFound => (
            "desktop.profile_not_found",
            ErrorCategory::Unavailable,
            RetryStrategy::RefreshSnapshot,
            "desktop.profileErrors.notFound",
        ),
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::Endpoint(_) => (
            "desktop.profile_invalid",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "desktop.errors.invalidConfiguration",
        ),
        _ => {
            let diagnostic_id = uuid::Uuid::new_v4().to_string();
            eprintln!("desktop profile operation failed: diagnostic_id={diagnostic_id}");
            return Box::new(CoreApiError::safe_internal(
                meta.request_id.clone(),
                diagnostic_id,
            ));
        }
    };
    Box::new(CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: message_key.to_owned(),
        request_id: Some(meta.request_id.clone()),
        diagnostic_id: None,
        params: Default::default(),
        conflict: None,
    })
}

#[tauri::command]
pub(crate) fn desktop_profile_list(
    meta: RequestMeta,
    service: State<'_, DesktopService>,
) -> CoreResult<Vec<DesktopProfile>> {
    service
        .hosts
        .with_desktop_repository(|repo| repo.list_desktop_profiles())
        .map_err(|error| map_profile_error(&meta, error))
}
#[tauri::command]
pub(crate) fn desktop_profile_save(
    request: DesktopProfileSaveRequest,
    service: State<'_, DesktopService>,
) -> CoreResult<DesktopProfile> {
    let result = match request.password_stage.as_ref() {
        Some(password_stage) => service
            .hosts
            .with_desktop_password_stage_repository(|repo| {
                repo.save_desktop_profile_with_password_stage(&request.profile, password_stage)
            }),
        None => service
            .hosts
            .with_desktop_repository(|repo| repo.save_desktop_profile(&request.profile)),
    };
    result.map_err(|error| map_profile_error(&request.meta, error))
}
#[tauri::command]
pub(crate) fn desktop_profile_delete(
    request: DesktopProfileDeleteRequest,
    service: State<'_, DesktopService>,
) -> CoreResult<()> {
    service
        .hosts
        .with_desktop_repository(|repo| {
            repo.delete_desktop_profile(&request.id, request.expected_revision)
        })
        .map_err(|error| map_profile_error(&request.meta, error))
}
#[tauri::command]
pub(crate) async fn desktop_session_open_owned(
    request: DesktopOpenRequest,
    app: AppHandle,
    webview: Webview,
    service: State<'_, DesktopService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
) -> CoreResult<DesktopSessionSummary> {
    let _permit = lifecycle.acquire_resource_creation(request.meta.request_id.clone())?;
    norishell_app_persistence::validate_desktop_profile(&request.profile)
        .map_err(|_| map_error(&request.meta, EngineError::InvalidConfiguration))?;
    uuid::Uuid::parse_str(&request.operation_id)
        .map_err(|_| map_error(&request.meta, EngineError::InvalidConfiguration))?;

    // Serialize with native move/close and other child projection writes. The
    // registry owns the handle before DesktopService can create the session.
    let (operation, live) = views
        .projection_operation(&webview)
        .map_err(|error| map_tab_error(&request.meta, error))?;
    let _operation = operation
        .lock()
        .map_err(|_| map_tab_error(&request.meta, "workspace_tab.unavailable".into()))?;
    if !live.load(std::sync::atomic::Ordering::Acquire) {
        return Err(map_tab_error(
            &request.meta,
            "workspace_tab.view_missing".into(),
        ));
    }
    let (id, kind, owner) = views
        .projection_identity(&webview)
        .map_err(|error| map_tab_error(&request.meta, error))?;
    if kind != "desktop" || workspaces.owns_tab(&owner, &id).is_err() {
        return Err(map_tab_error(
            &request.meta,
            "workspace_tab.wrong_owner".into(),
        ));
    }
    let idle = serde_json::json!({
        "schemaVersion": 1, "tabId": id, "profileId": request.profile.id,
    });
    let owned = serde_json::json!({
        "schemaVersion": 1, "tabId": id, "sessionId": request.operation_id,
        "generation": "1",
    });
    workspaces
        .swap_child_projection(&app, &owner, &id, &kind, &idle, owned.clone())
        .map_err(|error| map_tab_error(&request.meta, error))?;
    match service.open(request.clone()) {
        Ok(summary) => Ok(summary),
        Err(error) => {
            if let Err(rollback) =
                workspaces.swap_child_projection(&app, &owner, &id, &kind, &owned, idle)
            {
                eprintln!("desktop owned open rollback failed: open={error}, rollback={rollback}");
                return Err(map_tab_error(
                    &request.meta,
                    "workspace_tab.projection_rollback_failed".into(),
                ));
            }
            Err(map_error(&request.meta, error))
        }
    }
}
#[tauri::command]
pub(crate) fn desktop_session_snapshot(
    service: State<'_, DesktopService>,
) -> Vec<DesktopSessionSummary> {
    service.snapshot()
}
#[tauri::command]
pub(crate) async fn desktop_session_disconnect(
    request: DesktopSessionRequest,
    webview: Webview,
    service: State<'_, DesktopService>,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
) -> CoreResult<()> {
    require_owned_session(&request, &webview, &workspaces, &views)?;
    service
        .disconnect(&request.session_id, request.generation.get())
        .await
        .map_err(|error| map_error(&request.meta, error))
}
/// Only the Tab WebView whose Core record holds this session handle may end it.
fn require_owned_session(
    request: &DesktopSessionRequest,
    webview: &Webview,
    workspaces: &crate::workspace_windows::WorkspaceWindows,
    views: &crate::workspace_tab_views::WorkspaceTabViews,
) -> CoreResult<()> {
    let (id, kind, owner) = views
        .projection_identity(webview)
        .map_err(|error| map_tab_error(&request.meta, error))?;
    if kind != "desktop"
        || !workspaces
            .owns_desktop_session(&owner, &id, &request.session_id, request.generation.get())
            .map_err(|error| map_tab_error(&request.meta, error))?
    {
        return Err(map_tab_error(
            &request.meta,
            "workspace_tab.wrong_owner".into(),
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn desktop_session_close_owned(
    request: DesktopSessionRequest,
    webview: Webview,
    service: State<'_, DesktopService>,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
) -> CoreResult<()> {
    require_owned_session(&request, &webview, &workspaces, &views)?;
    service
        .disconnect(&request.session_id, request.generation.get())
        .await
        .map_err(|error| map_error(&request.meta, error))?;
    service
        .sessions
        .lock()
        .map_err(|_| map_error(&request.meta, EngineError::Protocol))?
        .remove(&request.session_id);
    Ok(())
}

#[tauri::command]
pub(crate) fn desktop_frame_get(
    request: DesktopFrameRequest,
    service: State<'_, DesktopService>,
) -> CoreResult<Response> {
    let session = service
        .session(&request.session_id, request.generation.get())
        .map_err(|error| map_error(&request.meta, error))?;
    let mut projection = session
        .projection
        .lock()
        .map_err(|_| map_error(&request.meta, EngineError::Protocol))?;
    let sequence = projection.summary.frame_sequence.get();
    if request.after_sequence.get() >= sequence {
        return Ok(Response::new(Vec::<u8>::new()));
    }
    let Some(frame) = projection.frame.clone() else {
        return Ok(Response::new(Vec::<u8>::new()));
    };
    let (base, rect) = frame_region(
        &frame,
        request.after_sequence.get(),
        projection.frame_base_sequence,
        projection.frame_dirty,
        projection.frame_requires_full,
    );
    projection.frame_base_sequence = sequence;
    projection.frame_dirty = None;
    projection.frame_requires_full = false;
    drop(projection);

    let row_bytes = usize::from(rect.width) * 4;
    let mut bytes = Vec::with_capacity(40 + row_bytes * usize::from(rect.height));
    bytes.extend_from_slice(&sequence.to_le_bytes());
    bytes.extend_from_slice(&base.to_le_bytes());
    bytes.extend_from_slice(&u32::from(frame.width).to_le_bytes());
    bytes.extend_from_slice(&u32::from(frame.height).to_le_bytes());
    bytes.extend_from_slice(&u32::from(rect.x).to_le_bytes());
    bytes.extend_from_slice(&u32::from(rect.y).to_le_bytes());
    bytes.extend_from_slice(&u32::from(rect.width).to_le_bytes());
    bytes.extend_from_slice(&u32::from(rect.height).to_le_bytes());
    for row in 0..usize::from(rect.height) {
        let start =
            ((usize::from(rect.y) + row) * usize::from(frame.width) + usize::from(rect.x)) * 4;
        bytes.extend_from_slice(&frame.rgba[start..start + row_bytes]);
    }
    Ok(Response::new(bytes))
}

#[tauri::command]
pub(crate) async fn desktop_focus_change(
    request: DesktopFocusRequest,
    webview: Webview,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    service: State<'_, DesktopService>,
    ssh: State<'_, crate::ssh_session_service::SshSessionService>,
) -> CoreResult<WireSequence> {
    let window = webview.window();
    if !ordinary_window(&webview, &workspaces, &views, window.label()) {
        return Err(map_error(&request.meta, EngineError::StaleInput));
    }
    let broker = ssh.focus_broker();
    broker
        .linearize(async {
            let mut focus = service.focus.lock().await;
            if !ordinary_window(&webview, &workspaces, &views, window.label()) {
                return Err(map_error(&request.meta, EngineError::StaleInput));
            }
            // A blur from a former window cannot revoke a newer window's input lease.
            if request.session_id.is_none()
                && focus.session.is_some()
                && focus.window_label.as_deref() != Some(window.label())
            {
                return Ok(WireSequence::new(focus.epoch));
            }
            if let Some(id) = &request.session_id {
                let session = service
                    .session(id, request.generation.map(|value| value.get()).unwrap_or(0))
                    .map_err(|error| map_error(&request.meta, error))?;
                let terminal = ssh
                    .terminal_focus_snapshot_unserialized(request.meta.request_id.clone())
                    .await?;
                if !window.is_focused().unwrap_or(false)
                    || terminal.target.is_some()
                    || service.prompts.has_pending()
                    || session.summary().state != DesktopSessionState::Running
                {
                    return Err(map_error(&request.meta, EngineError::StaleInput));
                }
            }
            let current = service
                .sessions
                .lock()
                .map_err(|_| map_error(&request.meta, EngineError::Protocol))?
                .values()
                .map(|session| *session.focus_epoch.borrow())
                .max()
                .unwrap_or(0);
            focus.epoch = focus.epoch.max(current).saturating_add(1);
            focus.session = None;
            focus.window_label = None;
            focus.sequence = 0;
            service.invalidate_input();
            if let Some(id) = &request.session_id {
                let session = service
                    .session(id, request.generation.map(|value| value.get()).unwrap_or(0))
                    .map_err(|error| map_error(&request.meta, error))?;
                focus.epoch = focus
                    .epoch
                    .max(*session.focus_epoch.borrow())
                    .saturating_add(1);
                session.focus_epoch.send_replace(focus.epoch);
                focus.session = Some(id.clone());
                focus.window_label = Some(window.label().to_owned());
            }
            Ok(WireSequence::new(focus.epoch))
        })
        .await
}

#[tauri::command]
pub(crate) async fn desktop_input(
    request: DesktopInputRequest,
    webview: Webview,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    service: State<'_, DesktopService>,
    ssh: State<'_, crate::ssh_session_service::SshSessionService>,
) -> CoreResult<()> {
    let window = webview.window();
    if !ordinary_window(&webview, &workspaces, &views, window.label()) {
        return Err(map_error(&request.meta, EngineError::StaleInput));
    }
    let broker = ssh.focus_broker();
    broker
        .linearize(async {
            let mut focus = service.focus.lock().await;
            if !ordinary_window(&webview, &workspaces, &views, window.label()) {
                return Err(map_error(&request.meta, EngineError::StaleInput));
            }
            let session = service
                .session(&request.session_id, request.generation.get())
                .map_err(|error| map_error(&request.meta, error))?;
            let terminal = ssh
                .terminal_focus_snapshot_unserialized(request.meta.request_id.clone())
                .await?;
            if !window.is_focused().unwrap_or(false)
                || terminal.target.is_some()
                || service.prompts.has_pending()
                || focus.session.as_deref() != Some(&request.session_id)
                || focus.window_label.as_deref() != Some(window.label())
                || focus.epoch != request.focus_epoch.get()
                || request.sequence.get() <= focus.sequence
                || session.summary().state != DesktopSessionState::Running
            {
                return Err(map_error(&request.meta, EngineError::StaleInput));
            }
            let input = match request.input {
                DesktopInputEvent::Key {
                    scan_code,
                    keysym,
                    down,
                } => DesktopInput::Key {
                    scan_code,
                    keysym,
                    down,
                },
                DesktopInputEvent::Pointer { x, y, buttons } => {
                    DesktopInput::Pointer { x, y, buttons }
                }
                DesktopInputEvent::Wheel {
                    x,
                    y,
                    delta_x,
                    delta_y,
                } => DesktopInput::Wheel {
                    x,
                    y,
                    delta_x,
                    delta_y,
                },
                DesktopInputEvent::Text { text } => DesktopInput::Text(text),
                DesktopInputEvent::Clipboard { text } => {
                    if !session.summary().profile.clipboard_enabled {
                        return Err(map_error(&request.meta, EngineError::UnsupportedOperation));
                    }
                    DesktopInput::Clipboard(text)
                }
                DesktopInputEvent::Resize { width, height } => {
                    let profile = &session.summary().profile;
                    match profile.protocol {
                        DesktopProtocol::Rdp
                            if profile.rdp_resolution_mode != RdpResolutionMode::Adaptive =>
                        {
                            return Err(map_error(
                                &request.meta,
                                EngineError::RdpResolutionModeDisabled,
                            ));
                        }
                        DesktopProtocol::Vnc
                            if profile.vnc_resolution_mode != VncResolutionMode::Adaptive =>
                        {
                            return Err(map_error(
                                &request.meta,
                                EngineError::VncResolutionModeDisabled,
                            ));
                        }
                        _ => {}
                    }
                    if width < 200
                        || height < 200
                        || profile.protocol == DesktopProtocol::Rdp && !width.is_multiple_of(2)
                    {
                        return Err(map_error(&request.meta, EngineError::InvalidConfiguration));
                    }
                    DesktopInput::Resize { width, height }
                }
                DesktopInputEvent::ReleaseAll => DesktopInput::ReleaseAll,
            };
            input
                .validate()
                .map_err(|error| map_error(&request.meta, error))?;
            let (completion, response) = oneshot::channel();
            session
                .commands
                .try_send(EngineCommand {
                    input,
                    focus_epoch: Some(focus.epoch),
                    completion,
                })
                .map_err(|_| map_error(&request.meta, EngineError::ResourceLimit))?;
            match tokio::time::timeout(std::time::Duration::from_secs(5), response).await {
                Ok(Ok(Ok(()))) => {
                    focus.sequence = request.sequence.get();
                    Ok(())
                }
                Ok(Ok(Err(error))) => Err(map_error(&request.meta, error)),
                _ => {
                    session.fail_and_stop("inputUncertain");
                    Err(map_error(&request.meta, EngineError::Timeout))
                }
            }
        })
        .await
}

#[tauri::command]
pub(crate) async fn desktop_resolution_set(
    request: DesktopResolutionRequest,
    webview: Webview,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    service: State<'_, DesktopService>,
) -> CoreResult<()> {
    let window = webview.window();
    let session = service
        .session(&request.session_id, request.generation.get())
        .map_err(|error| map_error(&request.meta, error))?;
    if !ordinary_window(&webview, &workspaces, &views, window.label()) {
        return Err(map_error(&request.meta, EngineError::StaleInput));
    }
    let summary = session.summary();
    if summary.state != DesktopSessionState::Running {
        return Err(map_error(&request.meta, EngineError::StaleInput));
    }
    match summary.profile.protocol {
        DesktopProtocol::Rdp
            if summary.profile.rdp_resolution_mode != RdpResolutionMode::Adaptive =>
        {
            return Err(map_error(
                &request.meta,
                EngineError::RdpResolutionModeDisabled,
            ));
        }
        DesktopProtocol::Vnc
            if summary.profile.vnc_resolution_mode != VncResolutionMode::Adaptive =>
        {
            return Err(map_error(
                &request.meta,
                EngineError::VncResolutionModeDisabled,
            ));
        }
        _ => {}
    }
    let input = DesktopInput::Resize {
        width: request.width,
        height: request.height,
    };
    input
        .validate()
        .map_err(|error| map_error(&request.meta, error))?;
    if request.width < 200
        || request.height < 200
        || summary.profile.protocol == DesktopProtocol::Rdp && !request.width.is_multiple_of(2)
    {
        return Err(map_error(&request.meta, EngineError::InvalidConfiguration));
    }
    let (completion, response) = oneshot::channel();
    session
        .commands
        .try_send(EngineCommand {
            input,
            focus_epoch: None,
            completion,
        })
        .map_err(|_| map_error(&request.meta, EngineError::ResourceLimit))?;
    match tokio::time::timeout(std::time::Duration::from_secs(14), response).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(map_error(&request.meta, error)),
        Ok(Err(_)) => Err(map_error(&request.meta, EngineError::ConnectionLost)),
        Err(_) => Err(map_error(&request.meta, EngineError::Timeout)),
    }
}

#[tauri::command]
pub(crate) fn desktop_clipboard_get(
    request: DesktopSessionRequest,
    webview: Webview,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
    service: State<'_, DesktopService>,
) -> CoreResult<Option<String>> {
    let window = webview.window();
    let session = service
        .session(&request.session_id, request.generation.get())
        .map_err(|error| map_error(&request.meta, error))?;
    if !ordinary_window(&webview, &workspaces, &views, window.label())
        || !window.is_focused().unwrap_or(false)
        || !session.summary().profile.clipboard_enabled
    {
        return Err(map_error(&request.meta, EngineError::StaleInput));
    }
    let result = session
        .projection
        .lock()
        .map_err(|_| map_error(&request.meta, EngineError::Protocol))?
        .clipboard
        .clone();
    Ok(result)
}

#[tauri::command]
pub(crate) fn desktop_prompt_get(
    id: String,
    window: WebviewWindow,
    service: State<'_, DesktopService>,
) -> Result<DesktopPrompt, String> {
    service
        .prompts
        .get(&window, &id)
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub(crate) fn desktop_prompt_decide(
    decision: DesktopPromptDecision,
    window: WebviewWindow,
    service: State<'_, DesktopService>,
) -> Result<(), String> {
    service
        .prompts
        .decide(&window, decision)
        .map_err(|error| error.to_string())
}

impl DesktopService {
    pub(crate) fn invalidate_input(&self) {
        if let Ok(sessions) = self.sessions.lock() {
            for session in sessions.values() {
                session
                    .focus_epoch
                    .send_modify(|epoch| *epoch = epoch.saturating_add(1));
            }
        }
    }
}

#[tauri::command]
pub(crate) fn desktop_audio_mute(
    request: DesktopAudioMuteRequest,
    service: State<'_, DesktopService>,
) -> CoreResult<()> {
    let session = service
        .session(&request.session_id, request.generation.get())
        .map_err(|error| map_error(&request.meta, error))?;
    let mut projection = session
        .projection
        .lock()
        .map_err(|_| map_error(&request.meta, EngineError::Protocol))?;
    if !projection.summary.profile.audio_playback_enabled
        || projection.summary.profile.protocol != DesktopProtocol::Rdp
        || projection.summary.state != DesktopSessionState::Running
        || *session.stop.borrow()
    {
        return Err(map_error(&request.meta, EngineError::UnsupportedOperation));
    }
    session.audio_muted.send_modify(|state| {
        if state.muted != request.muted {
            state.muted = request.muted;
            state.revision = state.revision.saturating_add(1);
        }
    });
    projection.summary.audio_muted = request.muted;
    projection.summary.revision = WireSequence::new(projection.summary.revision.get() + 1);
    Ok(())
}
