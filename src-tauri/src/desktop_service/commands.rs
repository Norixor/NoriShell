//! Tauri adapters validate callers, lifecycle, and input permissions; they contain no protocol implementation.
use super::DesktopService;
use norishell_core_api::*;
use norishell_desktop_protocol::{
    DesktopCursor, DesktopFrame, DesktopInput, DesktopRect, DirtyRegion, EngineCommand,
    EngineError, MAX_DIRTY_RECTS,
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

    fn rect(x: u16, y: u16, width: u16, height: u16) -> DesktopRect {
        DesktopRect {
            x,
            y,
            width,
            height,
        }
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }

    #[test]
    fn patch_requires_matching_base_and_small_bounded_dirty_region() {
        let mut dirty = DirtyRegion::default();
        dirty.add(rect(1, 1, 2, 2));
        dirty.add(rect(6, 6, 1, 1));
        let full = vec![rect(0, 0, 8, 8)];
        assert_eq!(
            frame_plan(8, 8, 5, 5, &dirty, false),
            (5, vec![rect(1, 1, 2, 2), rect(6, 6, 1, 1)])
        );
        assert_eq!(frame_plan(8, 8, 4, 5, &dirty, false), (0, full.clone()));
        assert_eq!(frame_plan(8, 8, 5, 5, &dirty, true), (0, full.clone()));
        assert_eq!(frame_plan(8, 8, 0, 0, &dirty, false), (0, full.clone()));
        assert_eq!(
            frame_plan(8, 8, 5, 5, &DirtyRegion::default(), false),
            (0, full.clone())
        );
        let mut large = DirtyRegion::default();
        large.add(rect(0, 0, 8, 4));
        assert_eq!(frame_plan(8, 8, 5, 5, &large, false), (0, full.clone()));
        let mut outside = DirtyRegion::default();
        outside.add(rect(7, 7, 2, 1));
        assert_eq!(frame_plan(8, 8, 5, 5, &outside, false), (0, full));
    }

    #[test]
    fn v2_encoder_writes_header_rects_cursor_and_payload_in_order() {
        let mut frame = DesktopFrame::new(4, 3).unwrap();
        for (index, byte) in frame.rgba.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let rects = [rect(1, 0, 2, 1), rect(0, 2, 1, 1)];
        let bitmap = std::sync::Arc::new(
            norishell_desktop_protocol::CursorBitmap::new(1, 2, 0, 5, vec![9; 8]).unwrap(),
        );
        let cursor = DesktopCursor::Bitmap(bitmap);
        let bytes = encode_frame_response(7, 3, Some((&frame, 6, &rects)), Some(&cursor));

        assert_eq!(u32_at(&bytes, 0), 2);
        assert_eq!(u32_at(&bytes, 4), 3);
        assert_eq!(u64_at(&bytes, 8), 7);
        assert_eq!(u64_at(&bytes, 16), 3);
        assert_eq!(u64_at(&bytes, 24), 6);
        assert_eq!((u32_at(&bytes, 32), u32_at(&bytes, 36)), (4, 3));
        assert_eq!(u32_at(&bytes, 40), 2);
        let rect_words: Vec<u32> = (0..8).map(|i| u32_at(&bytes, 44 + i * 4)).collect();
        assert_eq!(rect_words, [1, 0, 2, 1, 0, 2, 1, 1]);
        // Cursor header: bitmap kind, size, hotspot clamped to the bitmap.
        let cursor_words: Vec<u32> = (0..5).map(|i| u32_at(&bytes, 76 + i * 4)).collect();
        assert_eq!(cursor_words, [2, 1, 2, 0, 1]);
        let payload = &bytes[96..];
        let first: Vec<u8> = (4..12).collect();
        let second: Vec<u8> = (32..36).collect();
        assert_eq!(&payload[..8], first.as_slice());
        assert_eq!(&payload[8..12], second.as_slice());
        assert_eq!(&payload[12..], &[9; 8]);
    }

    #[test]
    fn v2_encoder_omits_absent_blocks() {
        let cursor_only = encode_frame_response(4, 2, None, Some(&DesktopCursor::Hidden));
        assert_eq!(cursor_only.len(), 44);
        assert_eq!(u32_at(&cursor_only, 4), 2);
        assert_eq!(u64_at(&cursor_only, 8), 4);
        assert_eq!(u32_at(&cursor_only, 24), 1);

        let frame = DesktopFrame::new(2, 1).unwrap();
        let full = [rect(0, 0, 2, 1)];
        let frame_only = encode_frame_response(1, 0, Some((&frame, 0, &full)), None);
        assert_eq!(u32_at(&frame_only, 4), 1);
        assert_eq!(frame_only.len(), 24 + 20 + 16 + 8);
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
/// Chooses what one frame delivery carries: `(base, rects)` where base 0 means a full frame (one rect covering
/// everything) and a non-zero base is the client's `after` sequence that the patch rectangles apply to.
fn frame_plan(
    width: u16,
    height: u16,
    after: u64,
    base: u64,
    dirty: &DirtyRegion,
    requires_full: bool,
) -> (u64, Vec<DesktopRect>) {
    let patch = !requires_full
        && after == base
        && after != 0
        && !dirty.is_empty()
        && dirty.rects().len() <= MAX_DIRTY_RECTS
        && dirty.fits(width, height)
        && dirty.area() * 2 < u64::from(width) * u64::from(height);
    if patch {
        (after, dirty.rects().to_vec())
    } else {
        (
            0,
            vec![DesktopRect {
                x: 0,
                y: 0,
                width,
                height,
            }],
        )
    }
}

const FRAME_WIRE_VERSION: u32 = 2;
const FRAME_FLAG_FRAME: u32 = 1;
const FRAME_FLAG_CURSOR: u32 = 2;

/// Encodes a v2 frame response (little-endian; see the remote desktop frame wire contract). Rectangles must
/// already fit `frame`; pixels are copied straight from the Core buffer, so callers hold the projection lock.
fn encode_frame_response(
    frame_sequence: u64,
    cursor_sequence: u64,
    frame: Option<(&DesktopFrame, u64, &[DesktopRect])>,
    cursor: Option<&DesktopCursor>,
) -> Vec<u8> {
    let pixel_bytes = frame.map_or(0, |(_, _, rects)| {
        rects
            .iter()
            .map(|rect| rect.area() as usize * 4)
            .sum::<usize>()
    });
    let cursor_bytes = match cursor {
        Some(DesktopCursor::Bitmap(bitmap)) => bitmap.rgba.len(),
        _ => 0,
    };
    let header =
        24 + frame.map_or(0, |(_, _, rects)| 20 + rects.len() * 16) + cursor.map_or(0, |_| 20);
    let mut bytes = Vec::with_capacity(header + pixel_bytes + cursor_bytes);
    let flags = frame.map_or(0, |_| FRAME_FLAG_FRAME) | cursor.map_or(0, |_| FRAME_FLAG_CURSOR);
    bytes.extend_from_slice(&FRAME_WIRE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&frame_sequence.to_le_bytes());
    bytes.extend_from_slice(&cursor_sequence.to_le_bytes());
    if let Some((frame, base, rects)) = frame {
        bytes.extend_from_slice(&base.to_le_bytes());
        bytes.extend_from_slice(&u32::from(frame.width).to_le_bytes());
        bytes.extend_from_slice(&u32::from(frame.height).to_le_bytes());
        bytes.extend_from_slice(&(rects.len() as u32).to_le_bytes());
        for rect in rects {
            for value in [rect.x, rect.y, rect.width, rect.height] {
                bytes.extend_from_slice(&u32::from(value).to_le_bytes());
            }
        }
    }
    if let Some(cursor) = cursor {
        let (kind, width, height, hotspot_x, hotspot_y) = match cursor {
            DesktopCursor::Default => (0u32, 0, 0, 0, 0),
            DesktopCursor::Hidden => (1, 0, 0, 0, 0),
            DesktopCursor::Bitmap(bitmap) => (
                2,
                bitmap.width,
                bitmap.height,
                bitmap.hotspot_x,
                bitmap.hotspot_y,
            ),
        };
        bytes.extend_from_slice(&kind.to_le_bytes());
        for value in [width, height, hotspot_x, hotspot_y] {
            bytes.extend_from_slice(&u32::from(value).to_le_bytes());
        }
    }
    if let Some((frame, _, rects)) = frame {
        let stride = usize::from(frame.width);
        for rect in rects {
            let row_bytes = usize::from(rect.width) * 4;
            for row in 0..usize::from(rect.height) {
                let start = ((usize::from(rect.y) + row) * stride + usize::from(rect.x)) * 4;
                bytes.extend_from_slice(&frame.rgba[start..start + row_bytes]);
            }
        }
    }
    if let Some(DesktopCursor::Bitmap(bitmap)) = cursor {
        bytes.extend_from_slice(&bitmap.rgba);
    }
    bytes
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
    require_owned_session(
        &request.meta,
        &request.session_id,
        request.generation.get(),
        &webview,
        &workspaces,
        &views,
    )?;
    service
        .disconnect(&request.session_id, request.generation.get())
        .await
        .map_err(|error| map_error(&request.meta, error))
}
/// Only the Tab WebView whose Core record holds this session handle may end it.
fn require_owned_session(
    meta: &RequestMeta,
    session_id: &str,
    generation: u64,
    webview: &Webview,
    workspaces: &crate::workspace_windows::WorkspaceWindows,
    views: &crate::workspace_tab_views::WorkspaceTabViews,
) -> CoreResult<()> {
    let (id, kind, owner) = views
        .projection_identity(webview)
        .map_err(|error| map_tab_error(meta, error))?;
    if kind != "desktop"
        || !workspaces
            .owns_desktop_session(&owner, &id, session_id, generation)
            .map_err(|error| map_tab_error(meta, error))?
    {
        return Err(map_tab_error(meta, "workspace_tab.wrong_owner".into()));
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
    require_owned_session(
        &request.meta,
        &request.session_id,
        request.generation.get(),
        &webview,
        &workspaces,
        &views,
    )?;
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

/// Longest time one frame request waits for new frame or cursor state before returning an empty body.
const FRAME_LONG_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// Longest wait for a Clipboard or Resize input acknowledgement; expiry reports a timeout only.
const INPUT_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Engine queue slots that droppable pointer and wheel events may not consume.
const INPUT_RESERVED_SLOTS: usize = 16;

#[tauri::command]
pub(crate) async fn desktop_frame_get(
    request: DesktopFrameRequest,
    webview: Webview,
    service: State<'_, DesktopService>,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
) -> CoreResult<Response> {
    require_owned_session(
        &request.meta,
        &request.session_id,
        request.generation.get(),
        &webview,
        &workspaces,
        &views,
    )?;
    let session = service
        .session(&request.session_id, request.generation.get())
        .map_err(|error| map_error(&request.meta, error))?;
    // Subscribe before the first check: any change after this point wakes `changed()`, so no update is lost.
    let mut changes = session.changes.subscribe();
    let deadline = tokio::time::Instant::now() + FRAME_LONG_POLL;
    loop {
        if *session.stop.borrow() || *session.done.borrow() {
            return Ok(Response::new(Vec::<u8>::new()));
        }
        if let Some(bytes) = take_frame_update(&session, &request)
            .map_err(|error| map_error(&request.meta, error))?
        {
            return Ok(Response::new(bytes));
        }
        match tokio::time::timeout_at(deadline, changes.changed()).await {
            // The Tab may have moved to another WebView while this request waited.
            Ok(Ok(())) => require_owned_session(
                &request.meta,
                &request.session_id,
                request.generation.get(),
                &webview,
                &workspaces,
                &views,
            )?,
            _ => return Ok(Response::new(Vec::<u8>::new())),
        }
    }
}

/// Builds the response for anything newer than the client's sequences and marks it delivered.
fn take_frame_update(
    session: &super::Session,
    request: &DesktopFrameRequest,
) -> Result<Option<Vec<u8>>, EngineError> {
    let mut projection = session
        .projection
        .lock()
        .map_err(|_| EngineError::Protocol)?;
    let frame_sequence = projection.summary.frame_sequence.get();
    let cursor_sequence = projection.cursor_sequence;
    let after = request.after_sequence.get();
    let send_frame = after < frame_sequence && projection.frame.is_some();
    let send_cursor = request.after_cursor_sequence.get() < cursor_sequence;
    if !send_frame && !send_cursor {
        return Ok(None);
    }
    let plan = match (&projection.frame, send_frame) {
        (Some(frame), true) => Some(frame_plan(
            frame.width,
            frame.height,
            after,
            projection.frame_base_sequence,
            &projection.frame_dirty,
            projection.frame_requires_full,
        )),
        _ => None,
    };
    let bytes = encode_frame_response(
        frame_sequence,
        cursor_sequence,
        projection
            .frame
            .as_ref()
            .zip(plan.as_ref())
            .map(|(frame, (base, rects))| (frame, *base, rects.as_slice())),
        send_cursor.then_some(&projection.cursor),
    );
    if plan.is_some() {
        projection.frame_base_sequence = frame_sequence;
        projection.frame_dirty.clear();
        projection.frame_requires_full = false;
    }
    Ok(Some(bytes))
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
    let acknowledgement = broker
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
                // Core revokes input (prompts, window blur, terminal focus) by bumping the session epoch
                // without touching the lease; without this check fire-and-forget input would be silently
                // dropped by the engine while the view still believes it holds control.
                || *session.focus_epoch.borrow() != focus.epoch
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
                    // Input-driven resizes keep the standard remote scale; only the resolution command sets it.
                    DesktopInput::Resize {
                        width,
                        height,
                        scale_percent: 100,
                    }
                }
                DesktopInputEvent::ReleaseAll => DesktopInput::ReleaseAll,
            };
            input
                .validate()
                .map_err(|error| map_error(&request.meta, error))?;
            // Key, pointer, wheel, text and release are fire-and-forget: the engine still rejects them if the
            // focus epoch moves before the write, and the view never replays them. Clipboard and resize report
            // their outcome, but the caller waits outside the focus lock so slow writes cannot block focus changes.
            let acknowledged = matches!(
                input,
                DesktopInput::Clipboard(_) | DesktopInput::Resize { .. }
            );
            // Pointer and wheel events may be dropped by the view, so they leave headroom in the queue for keys,
            // releases and clipboard writes that must not be refused during a congested link.
            if matches!(
                input,
                DesktopInput::Pointer { .. } | DesktopInput::Wheel { .. }
            ) && session.commands.capacity() <= INPUT_RESERVED_SLOTS
            {
                return Err(map_error(&request.meta, EngineError::ResourceLimit));
            }
            let (completion, response) = oneshot::channel();
            session
                .commands
                .try_send(EngineCommand {
                    input,
                    focus_epoch: Some(focus.epoch),
                    completion,
                })
                .map_err(|_| map_error(&request.meta, EngineError::ResourceLimit))?;
            focus.sequence = request.sequence.get();
            Ok(acknowledged.then_some(response))
        })
        .await?;
    let Some(response) = acknowledgement else {
        return Ok(());
    };
    // An expired wait leaves the outcome unknown but the stream intact; engines fail the session themselves
    // when a partial write breaks it, so the timeout never stops the session here.
    match tokio::time::timeout(INPUT_ACK_TIMEOUT, response).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(map_error(&request.meta, error)),
        Ok(Err(_)) => Err(map_error(&request.meta, EngineError::ConnectionLost)),
        Err(_) => Err(map_error(&request.meta, EngineError::Timeout)),
    }
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
    if !(100..=500).contains(&request.scale_percent) {
        return Err(map_error(&request.meta, EngineError::InvalidConfiguration));
    }
    let input = DesktopInput::Resize {
        width: request.width,
        height: request.height,
        scale_percent: request.scale_percent,
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
    webview: Webview,
    service: State<'_, DesktopService>,
    workspaces: State<'_, crate::workspace_windows::WorkspaceWindows>,
    views: State<'_, crate::workspace_tab_views::WorkspaceTabViews>,
) -> CoreResult<()> {
    require_owned_session(
        &request.meta,
        &request.session_id,
        request.generation.get(),
        &webview,
        &workspaces,
        &views,
    )?;
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
