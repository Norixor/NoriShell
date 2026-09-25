//! Core-owned, nonmodal editors. Window identity is never supplied by the child.
use norishell_core_api as wire;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

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
#[tauri::command]
pub(crate) async fn tool_window_open(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
    target: ToolTarget,
) -> Result<(), String> {
    if window.label() != "main"
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
        if let Some(child) = app.get_webview_window(&label) {
            crate::window_first_show::show_if_revealed(&child).map_err(|_| "unavailable")?;
        }
        return Ok(());
    }
    let expected = app
        .config()
        .build
        .dev_url
        .as_ref()
        .and_then(|url| url.join("tool-window.html").ok());
    let builder =
        WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("tool-window.html".into()))
            .title(target.title())
            .on_navigation(move |url| {
                (cfg!(debug_assertions)
                    && expected.as_ref().is_some_and(|expected| expected == url))
                    || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                        || (matches!(url.scheme(), "http" | "https")
                            && url.host_str() == Some("tauri.localhost")
                            && url.port().is_none()))
                        && url.path() == "/tool-window.html"
                        && url.query().is_none())
            })
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    let child = crate::secure_window_frame::apply_secure_window_frame(&app, &label, builder)
        .inner_size(1000.0, 760.0)
        .min_inner_size(640.0, 480.0)
        .build();
    let child = match child {
        Ok(child) => child,
        Err(_) => {
            state.0.lock().map_err(|_| "unavailable")?.remove(&label);
            return Err("unavailable".into());
        }
    };
    if app
        .state::<crate::tool_window_exit::ToolWindowExit>()
        .is_preparing()
    {
        state.0.lock().map_err(|_| "unavailable")?.remove(&label);
        let _ = child.destroy();
        return Err("unavailable".into());
    }
    let exit = app
        .state::<crate::tool_window_exit::ToolWindowExit>()
        .inner()
        .clone();
    let registry = state.inner().clone();
    child.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed)
            && let Ok(mut items) = registry.0.lock()
        {
            items.remove(&label);
            exit.destroyed(&label);
        }
    });
    crate::window_first_show::focus_if_revealed(&child).map_err(|_| "unavailable".into())
}
#[tauri::command]
pub(crate) fn tool_window_get(
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
) -> Result<ToolTarget, String> {
    state.target(window.label())
}
#[tauri::command]
pub(crate) fn tool_window_changed(
    app: tauri::AppHandle,
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
) -> Result<(), String> {
    let target = state.target(window.label())?;
    app.emit_to("main", "tool-window-changed", target.kind())
        .map_err(|_| "unavailable".into())
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
) -> Result<wire::SftpFilePreview, String> {
    let ToolTarget::SftpFile { request, .. } = state.target(window.label())? else {
        return Err("unavailable".into());
    };
    crate::sftp_session_service::sftp_file_preview(request, service)
        .await
        .map_err(|_| "unavailable".into())
}
#[tauri::command]
pub(crate) async fn tool_file_tail(
    window: WebviewWindow,
    state: State<'_, ToolWindows>,
    service: State<'_, crate::sftp_session_service::SftpSessionService>,
    offset: wire::WireSequence,
) -> Result<wire::SftpFileTailResult, String> {
    let ToolTarget::SftpFile { request, .. } = state.target(window.label())? else {
        return Err("unavailable".into());
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
    .map_err(|_| "unavailable".into())
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
) -> Result<(), String> {
    let ToolTarget::SftpFile {
        request,
        path,
        precondition,
        tail,
        ..
    } = state.target(window.label())?
    else {
        return Err("unavailable".into());
    };
    if tail || text.len() > 1_048_576 {
        return Err("unavailable".into());
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
    .await
    .map_err(|_| "unavailable".to_owned())?;
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
