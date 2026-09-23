//! Window-bound bridge authority. The sandbox receives a port, never a Core nonce.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use norishell_core_api::{
    PluginApiCall, PluginApiErrorCode, PluginApiOperation, PluginApiOutcome,
    PluginIsolatedBridgeAction, PluginIsolatedBridgeRequest, PluginIsolatedBridgeResponse,
    PluginIsolatedPendingAction,
};

use super::*;
use crate::plugin_api::{ResourceFence, ResourceOwner};

const PENDING_TTL: Duration = Duration::from_secs(120);
const RATE_WINDOW: Duration = Duration::from_secs(10);
const MAX_WINDOW_CALLS: u16 = 60;
const MAX_REPLY_BYTES: usize = 1024 * 1024;

pub(super) struct ActivePluginIsolatedSurface {
    pub(super) plugin_id: PluginId,
    owner: ResourceOwner,
    surface_id: String,
    html: String,
    channel_nonce: String,
    document_token: String,
    authority: Arc<AtomicBool>,
    grant_epoch: WireSequence,
    loaded: bool,
    sequence: u32,
    busy: bool,
    pending: Option<PendingAction>,
    rate_started: Instant,
    rate_count: u16,
}

struct PendingAction {
    id: String,
    call: PluginApiCall,
    expires: Instant,
}

impl Drop for ActivePluginIsolatedSurface {
    fn drop(&mut self) {
        self.authority.store(false, Ordering::Release);
    }
}

struct BridgeCallGuard {
    service: PluginService,
    label: String,
    nonce: String,
}

impl Drop for BridgeCallGuard {
    fn drop(&mut self) {
        if let Some(surface) = self
            .service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .isolated_surfaces
            .get_mut(&self.label)
            .filter(|surface| surface.channel_nonce == self.nonce)
        {
            surface.busy = false;
        }
    }
}

impl ActivePluginIsolatedSurface {
    fn claim_pending(&mut self, id: &str) -> Option<PluginApiCall> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.expires <= Instant::now())
        {
            self.pending = None;
        }
        if self.pending.as_ref().is_none_or(|pending| pending.id != id) {
            return None;
        }
        self.pending.take().map(|pending| pending.call)
    }

    fn admit(&mut self, request: &PluginIsolatedBridgeRequest) -> Result<(), ()> {
        if !self.loaded
            || !self.authority.load(Ordering::Acquire)
            || request.surface_id != self.surface_id
            || request.channel_nonce != self.channel_nonce
            || request.sequence != self.sequence
        {
            return Err(());
        }
        let now = Instant::now();
        if now.duration_since(self.rate_started) >= RATE_WINDOW {
            self.rate_started = now;
            self.rate_count = 0;
        }
        if self.rate_count >= MAX_WINDOW_CALLS {
            return Err(());
        }
        self.rate_count += 1;
        self.sequence = self.sequence.checked_add(1).ok_or(())?;
        Ok(())
    }
}

impl PluginService {
    pub(super) fn open_isolated_surface(
        &self,
        request: &PluginUiActionRequest,
        surface: PluginIsolatedSurfaceOpenRequest,
        html: String,
    ) -> CoreResult<()> {
        let installed = self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::UiWebviewIsolated,
        )?;
        let epoch = self.capability_grant_epoch_for_record(
            request.meta.request_id.clone(),
            &installed,
            PluginCapability::UiWebviewIsolated,
        )?;
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| plugin_runtime_error(request.meta.request_id.clone(), None))?;
        let label = format!("plugin-isolated-{}", Uuid::new_v4());
        let channel_nonce = Uuid::new_v4().to_string();
        {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            runtime
                .isolated_surfaces
                .retain(|label, _| app.get_webview_window(label).is_some());
            let count = runtime
                .isolated_surfaces
                .values()
                .filter(|record| record.plugin_id == request.plugin_id)
                .count();
            if runtime.isolated_surfaces.len() >= PLUGIN_ISOLATED_SURFACE_GLOBAL_LIMIT
                || count >= PLUGIN_ISOLATED_SURFACE_PER_PLUGIN_LIMIT
            {
                return Err(plugin_error(
                    request.meta.request_id.clone(),
                    "plugin.isolated_surface_quota",
                    ErrorCategory::Unavailable,
                    RetryStrategy::Never,
                    "errors.plugin.isolatedSurfaceQuota",
                    None,
                ));
            }
            runtime.isolated_surfaces.insert(
                label.clone(),
                ActivePluginIsolatedSurface {
                    plugin_id: request.plugin_id.clone(),
                    owner: ResourceOwner {
                        plugin_id: request.plugin_id.clone(),
                        signer: request.signer_fingerprint_sha256.clone(),
                        package: request.expected_package_sha256.clone(),
                        generation: request.instance_generation,
                    },
                    surface_id: surface.surface_id.clone(),
                    html,
                    channel_nonce: channel_nonce.clone(),
                    document_token: Uuid::new_v4().to_string(),
                    authority: Arc::new(AtomicBool::new(true)),
                    grant_epoch: epoch,
                    loaded: false,
                    sequence: 1,
                    busy: false,
                    pending: None,
                    rate_started: Instant::now(),
                    rate_count: 0,
                },
            );
        }
        let result =
            crate::secure_window_frame::apply_secure_window_frame(WebviewWindowBuilder::new(
                &app,
                &label,
                WebviewUrl::App(
                    format!(
                        "plugin-isolated.html?surfaceId={}&channelNonce={channel_nonce}",
                        surface.surface_id
                    )
                    .into(),
                ),
            ))
            .title(format!("NoriShell — {}", surface.title))
            .inner_size(f64::from(surface.width), f64::from(surface.height))
            .min_inner_size(480.0, 360.0)
            .resizable(true)
            .center()
            .build();
        let window = match result {
            Ok(window) => window,
            Err(_) => {
                self.retire_isolated_surface(&label);
                return Err(plugin_runtime_error(request.meta.request_id.clone(), None));
            }
        };
        let service = self.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                service.retire_isolated_surface(&label);
            }
        });
        Ok(())
    }

    pub(super) fn isolated_surface_content(
        &self,
        request: PluginIsolatedSurfaceContentRequest,
        window: &WebviewWindow,
    ) -> CoreResult<PluginIsolatedSurfaceContent> {
        let (owner, epoch, active) = {
            let runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let surface = runtime
                .isolated_surfaces
                .get(window.label())
                .filter(|surface| {
                    surface.surface_id == request.surface_id
                        && surface.channel_nonce == request.channel_nonce
                        && !surface.loaded
                })
                .ok_or_else(|| plugin_permission_error(request.meta.request_id.clone()))?;
            (
                surface.owner.clone(),
                surface.grant_epoch,
                surface.authority.clone(),
            )
        };
        if !active.load(Ordering::Acquire)
            || !self.api_resource_fence(owner, Some((PluginCapability::UiWebviewIsolated, epoch)))()
        {
            return Err(plugin_permission_error(request.meta.request_id));
        }
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let surface = runtime
            .isolated_surfaces
            .get_mut(window.label())
            .filter(|surface| surface.channel_nonce == request.channel_nonce && !surface.loaded)
            .ok_or_else(|| plugin_permission_error(request.meta.request_id.clone()))?;
        surface.loaded = true;
        Ok(PluginIsolatedSurfaceContent {
            surface_id: surface.surface_id.clone(),
            document_token: surface.document_token.clone(),
            next_sequence: surface.sequence,
        })
    }

    fn retire_isolated_surface(&self, label: &str) {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .isolated_surfaces
            .remove(label);
    }

    pub(super) fn close_isolated_surfaces(&self, plugin_id: &PluginId) {
        let labels = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let labels = runtime
                .isolated_surfaces
                .iter()
                .filter(|(_, surface)| surface.plugin_id == *plugin_id)
                .map(|(label, _)| label.clone())
                .collect::<Vec<_>>();
            runtime
                .isolated_surfaces
                .retain(|_, surface| surface.plugin_id != *plugin_id);
            labels
        };
        if let Some(app) = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            for label in labels {
                if let Some(window) = app.get_webview_window(&label) {
                    let _ = window.close();
                }
            }
        }
    }

    pub(crate) fn isolated_document(&self, label: &str, token: &str) -> Option<Vec<u8>> {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime
            .isolated_surfaces
            .get(label)
            .filter(|surface| {
                surface.loaded
                    && surface.document_token == token
                    && surface.authority.load(Ordering::Acquire)
            })
            .map(|surface| surface.html.as_bytes().to_vec())
    }

    async fn isolated_bridge(
        &self,
        window: &WebviewWindow,
        request: PluginIsolatedBridgeRequest,
    ) -> CoreResult<PluginIsolatedBridgeResponse> {
        if serde_json::to_vec(&request).map_or(true, |bytes| bytes.len() > 66 * 1024) {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let (owner, epoch, active, surface_id, sequence, call, explicit) = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let surface = runtime
                .isolated_surfaces
                .get_mut(window.label())
                .ok_or_else(|| plugin_permission_error(request_id.clone()))?;
            surface
                .admit(&request)
                .map_err(|()| plugin_permission_error(request_id.clone()))?;
            if matches!(request.action, PluginIsolatedBridgeAction::Deactivate {}) {
                surface.authority.store(false, Ordering::Release);
                surface.pending = None;
                return Ok(PluginIsolatedBridgeResponse {
                    next_sequence: surface.sequence,
                    reply: None,
                    pending: None,
                });
            }
            if surface.busy {
                return Err(plugin_conflict_error(request_id, None));
            }
            if surface
                .pending
                .as_ref()
                .is_some_and(|pending| pending.expires <= Instant::now())
            {
                surface.pending = None;
            }
            let (call, explicit) = match &request.action {
                PluginIsolatedBridgeAction::Call { call } => {
                    if surface.pending.is_some() {
                        return Err(plugin_conflict_error(request_id, None));
                    }
                    crate::plugin_api::validate_call(call)
                        .map_err(|_| plugin_validation_error(request_id.clone()))?;
                    (call.clone(), false)
                }
                PluginIsolatedBridgeAction::Approve { pending_id } => {
                    let call = surface
                        .claim_pending(pending_id)
                        .ok_or_else(|| plugin_permission_error(request_id.clone()))?;
                    (call, true)
                }
                PluginIsolatedBridgeAction::Cancel { pending_id } => {
                    if surface
                        .pending
                        .as_ref()
                        .is_none_or(|pending| pending.id != *pending_id)
                    {
                        return Err(plugin_permission_error(request_id));
                    }
                    surface.pending = None;
                    return Ok(PluginIsolatedBridgeResponse {
                        next_sequence: surface.sequence,
                        reply: None,
                        pending: None,
                    });
                }
                PluginIsolatedBridgeAction::Deactivate {} => unreachable!(),
            };
            surface.busy = true;
            (
                surface.owner.clone(),
                surface.grant_epoch,
                surface.authority.clone(),
                surface.surface_id.clone(),
                surface.sequence,
                call,
                explicit,
            )
        };
        let _guard = BridgeCallGuard {
            service: self.clone(),
            label: window.label().to_owned(),
            nonce: request.channel_nonce.clone(),
        };
        let runtime_fence = self.api_resource_fence(
            owner.clone(),
            Some((PluginCapability::UiWebviewIsolated, epoch)),
        );
        let atomic_active = active.clone();
        let atomic_runtime = self.api_runtime_fence(owner.clone());
        let transaction_authority: ResourceFence =
            Arc::new(move || atomic_active.load(Ordering::Acquire) && atomic_runtime());
        let authority: ResourceFence =
            Arc::new(move || active.load(Ordering::Acquire) && runtime_fence());
        if !authority() {
            return Err(plugin_permission_error(request_id));
        }
        let installed = self.has_capability(
            request_id.clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::UiWebviewIsolated,
        )?;
        let operation = operation_name(&call);
        let reply = self
            .invoke_isolated_api(
                request_id.clone(),
                &installed,
                owner,
                format!("isolated:{surface_id}:{operation}"),
                authority.clone(),
                transaction_authority,
                &call,
                explicit,
            )
            .await?;
        if !authority() {
            return Err(plugin_permission_error(request_id));
        }
        let needs_action = !explicit
            && can_request_interaction(&call)
            && matches!(
                reply.outcome,
                PluginApiOutcome::Failed {
                    code: PluginApiErrorCode::InteractionRequired
                }
            );
        let pending = if needs_action {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let surface = runtime
                .isolated_surfaces
                .get_mut(window.label())
                .filter(|surface| {
                    surface.channel_nonce == request.channel_nonce
                        && surface.authority.load(Ordering::Acquire)
                })
                .ok_or_else(|| plugin_permission_error(request_id.clone()))?;
            let id = Uuid::new_v4().to_string();
            let summary = PluginIsolatedPendingAction {
                pending_id: id.clone(),
                call_id: call.call_id.clone(),
                operation: operation.clone(),
            };
            surface.pending = Some(PendingAction {
                id,
                call,
                expires: Instant::now() + PENDING_TTL,
            });
            Some(summary)
        } else {
            None
        };
        let response = PluginIsolatedBridgeResponse {
            next_sequence: sequence,
            reply: Some(reply),
            pending,
        };
        if serde_json::to_vec(&response).map_or(true, |bytes| bytes.len() > MAX_REPLY_BYTES) {
            return Err(plugin_validation_error(request_id));
        }
        Ok(response)
    }
}

fn operation_name(call: &PluginApiCall) -> String {
    serde_json::to_value(&call.operation)
        .ok()
        .and_then(|value| {
            value
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .expect("typed API operations have a kind")
}

fn can_request_interaction(call: &PluginApiCall) -> bool {
    matches!(
        call.operation,
        PluginApiOperation::FilePick { .. }
            | PluginApiOperation::NetworkStart { .. }
            | PluginApiOperation::ProcessStart { .. }
            | PluginApiOperation::SerialOpen { .. }
            | PluginApiOperation::ProtocolOpen { .. }
            | PluginApiOperation::TaskStart { .. }
            | PluginApiOperation::TaskResume { .. }
            | PluginApiOperation::AppNavigate { .. }
            | PluginApiOperation::RemoteExecStart { .. }
            | PluginApiOperation::SftpOpen { .. }
            | PluginApiOperation::Credential {
                operation: norishell_core_api::PluginCredentialOperation::Create { .. }
            }
    )
}

#[tauri::command]
pub(crate) async fn plugin_isolated_bridge(
    request: PluginIsolatedBridgeRequest,
    service: State<'_, PluginService>,
    window: WebviewWindow,
) -> CoreResult<PluginIsolatedBridgeResponse> {
    service.isolated_bridge(&window, request).await
}

/// Guest HTML has its own response policy; the trusted wrapper keeps script-src 'self'.
pub(crate) fn document_response(
    service: Option<&PluginService>,
    label: &str,
    path: &str,
) -> tauri::http::Response<Vec<u8>> {
    let token = path
        .strip_prefix('/')
        .filter(|token| Uuid::parse_str(token).is_ok());
    let body =
        token.and_then(|token| service.and_then(|service| service.isolated_document(label, token)));
    tauri::http::Response::builder().status(if body.is_some() { 200 } else { 404 })
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'")
        .body(body.unwrap_or_default()).expect("static response headers are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ActivePluginIsolatedSurface {
        let plugin_id = PluginId::parse("test.isolated").unwrap();
        ActivePluginIsolatedSurface {
            plugin_id: plugin_id.clone(),
            owner: ResourceOwner {
                plugin_id,
                signer: "a".repeat(64),
                package: "b".repeat(64),
                generation: WireSequence::new(1),
            },
            surface_id: "main".to_owned(),
            html: "<p>fixture</p>".to_owned(),
            channel_nonce: Uuid::new_v4().to_string(),
            document_token: Uuid::new_v4().to_string(),
            authority: Arc::new(AtomicBool::new(true)),
            grant_epoch: WireSequence::new(1),
            loaded: true,
            sequence: 1,
            busy: false,
            pending: None,
            rate_started: Instant::now(),
            rate_count: 0,
        }
    }

    fn request(surface: &ActivePluginIsolatedSurface) -> PluginIsolatedBridgeRequest {
        PluginIsolatedBridgeRequest {
            meta: norishell_core_api::RequestMeta {
                request_id: RequestId::new(),
            },
            surface_id: surface.surface_id.clone(),
            channel_nonce: surface.channel_nonce.clone(),
            sequence: surface.sequence,
            action: PluginIsolatedBridgeAction::Deactivate {},
        }
    }

    #[test]
    fn channel_rejects_wrong_nonce_replay_rate_and_revocation() {
        let mut surface = fixture();
        let original = request(&surface);
        let mut forged = original.clone();
        forged.channel_nonce = "guest-claimed-authority".to_owned();
        assert!(surface.admit(&forged).is_err());
        assert_eq!(surface.sequence, 1);
        assert!(surface.admit(&original).is_ok());
        assert!(surface.admit(&original).is_err());
        surface.rate_count = MAX_WINDOW_CALLS;
        assert!(surface.admit(&request(&surface)).is_err());
        surface.rate_count = 0;
        surface.authority.store(false, Ordering::Release);
        assert!(surface.admit(&request(&surface)).is_err());
    }

    #[test]
    fn approval_atomically_consumes_original_call_and_expiry_never_restores_it() {
        let mut surface = fixture();
        let call = PluginApiCall {
            call_id: "original".to_owned(),
            operation: PluginApiOperation::Describe {},
        };
        surface.pending = Some(PendingAction {
            id: "pending".to_owned(),
            call: call.clone(),
            expires: Instant::now() + PENDING_TTL,
        });
        assert!(surface.claim_pending("forged").is_none());
        assert_eq!(surface.claim_pending("pending"), Some(call.clone()));
        assert!(surface.claim_pending("pending").is_none());
        surface.pending = Some(PendingAction {
            id: "expired".to_owned(),
            call,
            expires: Instant::now() - Duration::from_secs(1),
        });
        assert!(surface.claim_pending("expired").is_none());
        assert!(surface.pending.is_none());
        let token = surface.authority.clone();
        drop(surface);
        assert!(!token.load(Ordering::Acquire));
    }

    #[test]
    fn terminal_requests_cannot_create_an_approval_shortcut_and_document_policy_is_separate() {
        let call = PluginApiCall {
            call_id: "input".to_owned(),
            operation: PluginApiOperation::TerminalRequestInput {
                terminal_handle: "guest".to_owned(),
                payload: "text".to_owned(),
                append_enter: false,
            },
        };
        assert!(!can_request_interaction(&call));
        let response = document_response(None, "wrong-window", "/invalid");
        assert_eq!(response.status(), 404);
        let policy = response.headers()["Content-Security-Policy"]
            .to_str()
            .unwrap();
        assert!(policy.contains("connect-src 'none'"));
        assert!(policy.contains("frame-src 'none'"));
        assert!(
            !response
                .headers()
                .contains_key("Access-Control-Allow-Origin")
        );
    }
}
