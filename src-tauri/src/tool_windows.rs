//! Core-owned, nonmodal editors. Window identity is never supplied by the child.
use norishell_core_api as wire;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State, WebviewWindow};

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ToolTarget {
    HostEditor {
        host_id: Option<String>,
        title: String,
        initial_section: Option<String>,
    },
    DesktopEditor {
        profile_id: Option<String>,
        title: String,
    },
    SftpFile {
        title: String,
        request: wire::SftpFilePreviewRequest,
        path: wire::SftpRemotePath,
        precondition: wire::SftpRemoteObjectPrecondition,
        tail: bool,
    },
}
impl ToolTarget {
    fn kind(&self) -> &'static str {
        match self {
            Self::HostEditor { .. } => "hostEditor",
            Self::DesktopEditor { .. } => "desktopEditor",
            Self::SftpFile { .. } => "sftpFile",
        }
    }
    fn surface_kind(&self) -> crate::secure_surface::SurfaceKind {
        match self {
            Self::HostEditor { .. } => crate::secure_surface::SurfaceKind::ToolHostEditor,
            Self::DesktopEditor { .. } => crate::secure_surface::SurfaceKind::ToolDesktopEditor,
            Self::SftpFile { .. } => crate::secure_surface::SurfaceKind::ToolSftpFile,
        }
    }
    fn title(&self) -> &str {
        match self {
            Self::HostEditor { title, .. }
            | Self::DesktopEditor { title, .. }
            | Self::SftpFile { title, .. } => title,
        }
    }
    fn same_target(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::HostEditor { host_id: a, .. }, Self::HostEditor { host_id: b, .. }) => a == b,
            (
                Self::DesktopEditor { profile_id: a, .. },
                Self::DesktopEditor { profile_id: b, .. },
            ) => a == b,
            (
                Self::SftpFile {
                    request: a,
                    path: ap,
                    tail: at,
                    ..
                },
                Self::SftpFile {
                    request: b,
                    path: bp,
                    tail: bt,
                    ..
                },
            ) => {
                a.session_id == b.session_id
                    && a.expected_generation == b.expected_generation
                    && ap == bp
                    && at == bt
            }
            _ => false,
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct ToolWindows(Arc<Mutex<BTreeMap<String, ToolTarget>>>);
impl ToolWindows {
    pub(crate) fn contains(&self, label: &str) -> bool {
        self.0.lock().is_ok_and(|items| items.contains_key(label))
    }
    pub(crate) fn is_editor(&self, label: &str) -> bool {
        self.0
            .lock()
            .ok()
            .and_then(|items| items.get(label).cloned())
            .is_some_and(|target| {
                matches!(
                    target,
                    ToolTarget::HostEditor { .. } | ToolTarget::DesktopEditor { .. }
                )
            })
    }
    fn target(&self, label: &str) -> Result<ToolTarget, String> {
        self.0
            .lock()
            .map_err(|_| "unavailable")?
            .get(label)
            .cloned()
            .ok_or_else(|| "unavailable".into())
    }
}

fn ordinary_owner_alive(app: &tauri::AppHandle, label: &str) -> bool {
    app.get_webview_window(label).is_some()
        && (label == "main"
            || app
                .state::<crate::workspace_windows::WorkspaceWindows>()
                .contains(label))
}

#[tauri::command]
pub(crate) async fn tool_window_open(
    app: tauri::AppHandle,
    webview: tauri::Webview,
    state: State<'_, ToolWindows>,
    target: ToolTarget,
) -> Result<(), String> {
    let owner =
        crate::workspace_tab_views::ordinary_owner(&app, &webview).map_err(|_| "unavailable")?;
    if !ordinary_owner_alive(&app, &owner)
        || target.title().len() > 1024
        || app
            .state::<crate::tool_window_exit::ToolWindowExit>()
            .is_preparing()
    {
        return Err("unavailable".into());
    }
    // Reserve before building so simultaneous clicks cannot create duplicate drafts.
    let (label, existing) = {
        let mut items = state.0.lock().map_err(|_| "unavailable")?;
        if let Some((label, _)) = items.iter().find(|(_, value)| value.same_target(&target)) {
            (label.clone(), true)
        } else {
            let label = format!("tool-{}-{}", target.kind(), uuid::Uuid::now_v7());
            items.insert(label.clone(), target.clone());
            (label, false)
        }
    };
    if existing {
        if !ordinary_owner_alive(&app, &owner) {
            return Err("unavailable".into());
        }
        if let Some(shown) = crate::secure_surface::show_existing(&app, &label) {
            shown.map_err(|_| "unavailable")?;
        }
        return Ok(());
    }
    let default_size =
        crate::secure_window_frame::default_window_size(&app, (1000.0, 760.0), (640.0, 480.0));
    let child = crate::secure_surface::open(
        &app,
        crate::secure_surface::SurfaceSpec::new(target.surface_kind(), label.clone())
            .title(target.title())
            .size(default_size.0, default_size.1)
            .min_size(640.0, 480.0)
            .navigation_guard(),
    );
    let child = match child {
        Ok(child) => child,
        Err(_) => {
            state.0.lock().map_err(|_| "unavailable")?.remove(&label);
            return Err("unavailable".into());
        }
    };
    if !ordinary_owner_alive(&app, &owner)
        || app
            .state::<crate::tool_window_exit::ToolWindowExit>()
            .is_preparing()
    {
        state.0.lock().map_err(|_| "unavailable")?.remove(&label);
        child.destroy();
        return Err("unavailable".into());
    }
    let exit = app
        .state::<crate::tool_window_exit::ToolWindowExit>()
        .inner()
        .clone();
    let registry = state.inner().clone();
    child.on_destroyed(move || {
        if let Ok(mut items) = registry.0.lock() {
            items.remove(&label);
            exit.destroyed(&label);
        }
    });
    child.focus().map_err(|_| "unavailable".into())
}
#[tauri::command]
pub(crate) fn tool_window_get(
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
) -> Result<ToolTarget, String> {
    tool_window_get_for_label(window.label(), &state)
}
pub(crate) fn tool_window_get_for_label(
    surface_label: &str,
    state: &ToolWindows,
) -> Result<ToolTarget, String> {
    state.target(surface_label)
}
#[tauri::command]
pub(crate) fn tool_window_changed(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
) -> Result<(), String> {
    tool_window_changed_for_label(&app, window.label(), &state)
}
pub(crate) fn tool_window_changed_for_label(
    app: &tauri::AppHandle,
    surface_label: &str,
    state: &ToolWindows,
) -> Result<(), String> {
    let target = state.target(surface_label)?;
    app.emit_to("main", "tool-window-changed", target.kind())
        .map_err(|_| "unavailable".into())
}
/// Android: the Kotlin-bound surface asks to close itself.
#[cfg(target_os = "android")]
pub(crate) fn tool_window_close_for_label(
    app: &tauri::AppHandle,
    surface_label: &str,
    state: &ToolWindows,
) -> Result<(), String> {
    state.target(surface_label)?;
    crate::secure_surface::destroy(app, surface_label);
    Ok(())
}
#[tauri::command]
pub(crate) fn tool_window_close(
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
) -> Result<(), String> {
    state.target(window.label())?;
    window.destroy().map_err(|_| "unavailable".into())
}

#[tauri::command]
pub(crate) async fn tool_file_preview(
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
    service: State<'_, crate::sftp_session_service::SftpSessionService>,
) -> Result<wire::SftpFilePreview, Box<wire::CoreApiError>> {
    tool_file_preview_for_label(window.label(), &state, service).await
}
pub(crate) async fn tool_file_preview_for_label(
    surface_label: &str,
    state: &ToolWindows,
    service: State<'_, crate::sftp_session_service::SftpSessionService>,
) -> Result<wire::SftpFilePreview, Box<wire::CoreApiError>> {
    let target = state.target(surface_label).map_err(|_| {
        crate::core_api_error::core_error(
            wire::RequestId::new(),
            "tool_window.unavailable",
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::Never,
            "errors.core.unavailable",
        )
    })?;
    let ToolTarget::SftpFile { request, .. } = target else {
        return Err(crate::core_api_error::core_error(
            wire::RequestId::new(),
            "tool_window.invalid_target",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
            "errors.core.invalidRequest",
        ));
    };
    crate::sftp_session_service::sftp_file_preview(request, service).await
}
#[tauri::command]
pub(crate) async fn tool_file_tail(
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
    service: State<'_, crate::sftp_session_service::SftpSessionService>,
    offset: wire::WireSequence,
) -> Result<wire::SftpFileTailResult, Box<wire::CoreApiError>> {
    tool_file_tail_for_label(window.label(), &state, service, offset).await
}
pub(crate) async fn tool_file_tail_for_label(
    surface_label: &str,
    state: &ToolWindows,
    service: State<'_, crate::sftp_session_service::SftpSessionService>,
    offset: wire::WireSequence,
) -> Result<wire::SftpFileTailResult, Box<wire::CoreApiError>> {
    let target = state.target(surface_label).map_err(|_| {
        crate::core_api_error::core_error(
            wire::RequestId::new(),
            "tool_window.unavailable",
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::Never,
            "errors.core.unavailable",
        )
    })?;
    let ToolTarget::SftpFile { request, .. } = target else {
        return Err(crate::core_api_error::core_error(
            wire::RequestId::new(),
            "tool_window.invalid_target",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
            "errors.core.invalidRequest",
        ));
    };
    crate::sftp_session_service::sftp_file_tail(
        wire::SftpFileTailRequest {
            meta: request.meta,
            session_id: request.session_id,
            expected_generation: request.expected_generation,
            directory_ref: request.directory_ref,
            entry_ref: request.entry_ref,
            offset,
        },
        service,
    )
    .await
}
#[tauri::command]
pub(crate) async fn tool_file_save(
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
    service: State<'_, crate::sftp_session_service::SftpSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
    text: String,
    meta: wire::RequestMeta,
    operation_id: wire::OperationId,
) -> Result<(), Box<wire::CoreApiError>> {
    tool_file_save_for_label(
        window.label(),
        &state,
        service,
        lifecycle,
        text,
        meta,
        operation_id,
    )
    .await
}
pub(crate) async fn tool_file_save_for_label(
    surface_label: &str,
    state: &ToolWindows,
    service: State<'_, crate::sftp_session_service::SftpSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
    text: String,
    meta: wire::RequestMeta,
    operation_id: wire::OperationId,
) -> Result<(), Box<wire::CoreApiError>> {
    let target = state.target(surface_label).map_err(|_| {
        crate::core_api_error::core_error(
            meta.request_id.clone(),
            "tool_window.unavailable",
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::Never,
            "errors.core.unavailable",
        )
    })?;
    let ToolTarget::SftpFile {
        request,
        path,
        precondition,
        tail,
        ..
    } = target
    else {
        return Err(crate::core_api_error::core_error(
            meta.request_id.clone(),
            "tool_window.invalid_target",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
            "errors.core.invalidRequest",
        ));
    };
    if tail {
        return Err(crate::core_api_error::core_error(
            meta.request_id.clone(),
            "tool_window.read_only",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
            "sftpToolWindow.readOnly",
        ));
    }
    if text.len() > 1_048_576 {
        return Err(crate::core_api_error::core_error(
            meta.request_id.clone(),
            "tool_window.content_too_large",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
            "sftpToolWindow.contentTooLarge",
        ));
    }
    let idempotency_key = operation_id.to_string();
    crate::sftp_session_service::sftp_file_mutate(
        wire::SftpFileMutationRequest {
            meta,
            operation_id,
            idempotency_key,
            session_id: request.session_id,
            expected_generation: request.expected_generation,
            mutation: wire::SftpFileMutation::WriteText {
                path,
                precondition,
                text,
            },
        },
        service,
        lifecycle,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_rejects_unregistered_windows_and_distinguishes_editor_roles() {
        let state = ToolWindows::default();
        assert!(state.target("main").is_err());
        assert!(!state.is_editor("tool-hostEditor-forged"));
        let target = ToolTarget::HostEditor {
            host_id: Some("one".into()),
            title: "Host".into(),
            initial_section: None,
        };
        state
            .0
            .lock()
            .unwrap()
            .insert("exact".into(), target.clone());
        assert!(state.is_editor("exact"));
        assert!(!state.is_editor("exact-other"));
        assert!(target.same_target(&ToolTarget::HostEditor {
            host_id: Some("one".into()),
            title: "Renamed".into(),
            initial_section: Some("advanced".into())
        }));
        assert!(!target.same_target(&ToolTarget::HostEditor {
            host_id: Some("two".into()),
            title: "Host".into(),
            initial_section: None
        }));
        state.0.lock().unwrap().remove("exact");
        assert!(state.target("exact").is_err());
    }
}
