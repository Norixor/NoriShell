//! Core-owned SSH credential prompts. The main window receives an opaque ref only.
use crate::{
    host_service::HostService, metrics_session_service::MetricsSessionService,
    transient_credential_service::TransientCredentialService, vault_service::VaultService,
};
use norishell_core_api::{
    CredentialImportRequest, CredentialKind, CredentialRefId, IdentityId, OperationId, RequestId,
    RequestMeta, TransientCredentialPrepareRequest, WireSequence,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

const MAX_PASSWORD_BYTES: usize = 64 * 1024;
const MAX_PRIVATE_KEY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecureCredentialKind {
    Password,
    PrivateKey,
}

impl From<SecureCredentialKind> for CredentialKind {
    fn from(kind: SecureCredentialKind) -> Self {
        match kind {
            SecureCredentialKind::Password => Self::Password,
            SecureCredentialKind::PrivateKey => Self::PrivateKey,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureCredentialOpenRequest {
    kind: SecureCredentialKind,
    label: String,
    identity_id: Option<IdentityId>,
    credential_ref_id: Option<CredentialRefId>,
    expected_state_version: Option<WireSequence>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureCredentialPrompt {
    id: String,
    kind: SecureCredentialKind,
    label: String,
    persistent: bool,
    replacement: bool,
}

struct Answer {
    secret: Zeroizing<String>,
    passphrase: Zeroizing<String>,
}

struct Pending {
    prompt: SecureCredentialPrompt,
    sender: oneshot::Sender<Answer>,
}

#[derive(Default)]
pub struct SecureCredentialService {
    pending: Mutex<BTreeMap<String, Pending>>,
}

fn window_label(id: &str) -> String {
    format!("secure-credential-{id}")
}

fn require_window(window: &WebviewWindow, id: &str) -> Result<(), String> {
    if window.label() != window_label(id) {
        return Err("secureCredentialDenied".into());
    }
    Ok(())
}

impl SecureCredentialService {
    /// Used by the Vault gate to authorize only a live, Core-owned credential prompt.
    pub(crate) fn is_prompt_window(&self, label: &str) -> bool {
        self.pending
            .lock()
            .ok()
            .is_some_and(|pending| pending.keys().any(|id| label == window_label(id)))
    }
}

struct PromptGuard {
    app: AppHandle,
    id: String,
}

impl Drop for PromptGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.app.state::<SecureCredentialService>().pending.lock() {
            pending.remove(&self.id);
        }
        if let Some(window) = self.app.get_webview_window(&window_label(&self.id)) {
            let _ = window.destroy();
        }
    }
}

fn valid_open_request(request: &SecureCredentialOpenRequest) -> bool {
    let replacement_fields_match =
        request.credential_ref_id.is_some() == request.expected_state_version.is_some();
    let replacement_is_valid = request.credential_ref_id.is_none()
        || (matches!(request.kind, SecureCredentialKind::Password)
            && request.identity_id.is_some());
    !request.label.trim().is_empty()
        && request.label.len() <= 512
        && replacement_fields_match
        && replacement_is_valid
}

#[tauri::command]
pub async fn secure_credential_open(
    request: SecureCredentialOpenRequest,
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, SecureCredentialService>,
    hosts: State<'_, HostService>,
    vault: State<'_, VaultService>,
    metrics: State<'_, MetricsSessionService>,
) -> Result<Option<String>, String> {
    // Opening is deliberately main-window-only. The child has no command that can open another
    // credential prompt, and it can return only an opaque Core-issued reference.
    if window.label() != "main" || !valid_open_request(&request) {
        return Err("secureCredentialDenied".into());
    }
    let id = uuid::Uuid::now_v7().to_string();
    let (sender, receiver) = oneshot::channel();
    service
        .pending
        .lock()
        .map_err(|_| "secureCredentialUnavailable")?
        .insert(
            id.clone(),
            Pending {
                prompt: SecureCredentialPrompt {
                    id: id.clone(),
                    kind: request.kind,
                    label: request.label.clone(),
                    persistent: request.identity_id.is_some(),
                    replacement: request.credential_ref_id.is_some(),
                },
                sender,
            },
        );
    let _guard = PromptGuard {
        app: app.clone(),
        id: id.clone(),
    };
    let prompt_path = format!("secure-credential.html?prompt={id}");
    let expected = app
        .config()
        .build
        .dev_url
        .as_ref()
        .and_then(|url| url.join(&prompt_path).ok());
    let expected_query = format!("prompt={id}");
    let window_height = match request.kind {
        SecureCredentialKind::Password => 340.0,
        SecureCredentialKind::PrivateKey => 420.0,
    };
    let child = crate::secure_window_frame::apply_secure_window_frame(
        WebviewWindowBuilder::new(&app, window_label(&id), WebviewUrl::App(prompt_path.into()))
            .title("NoriShell"),
    )
    .inner_size(600.0, window_height)
    .min_inner_size(480.0, 320.0)
    .on_navigation(move |url| {
        (cfg!(debug_assertions) && expected.as_ref().is_some_and(|expected| expected == url))
            || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                || (matches!(url.scheme(), "http" | "https")
                    && url.host_str() == Some("tauri.localhost")
                    && url.port().is_none()))
                && url.path() == "/secure-credential.html"
                && url.query() == Some(expected_query.as_str()))
    })
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .build()
    .map_err(|_| "secureCredentialUnavailable")?;
    let cancelled_app = app.clone();
    let cancelled_id = id.clone();
    child.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed)
            && let Ok(mut pending) = cancelled_app
                .state::<SecureCredentialService>()
                .pending
                .lock()
        {
            pending.remove(&cancelled_id);
        }
    });
    let _ = child.set_focus();

    let mut receiver = receiver;
    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);
    let mut owner_check = tokio::time::interval(Duration::from_millis(250));
    let owner_valid =
        || app.get_webview_window(window.label()).is_some() && window.label() == "main";
    let mut answer = loop {
        tokio::select! {
            response = &mut receiver => {
                let Ok(answer) = response else { return Ok(None); };
                break answer;
            }
            _ = &mut deadline => return Ok(None),
            _ = owner_check.tick() => if !owner_valid() { return Ok(None); },
        }
    };
    if !owner_valid() {
        return Ok(None);
    }

    let kind: CredentialKind = request.kind.into();
    let secret = std::mem::take(&mut *answer.secret);
    let passphrase = match request.kind {
        SecureCredentialKind::Password => None,
        SecureCredentialKind::PrivateKey if answer.passphrase.is_empty() => None,
        SecureCredentialKind::PrivateKey => Some(std::mem::take(&mut *answer.passphrase)),
    };
    let meta = RequestMeta {
        request_id: RequestId::new(),
    };
    let credential_ref_id = match (
        request.identity_id,
        request.credential_ref_id,
        request.expected_state_version,
    ) {
        (Some(identity_id), Some(credential_ref_id), Some(expected_state_version)) => {
            let summary = crate::host_service::replace_password_credential(
                meta.request_id.clone(),
                &credential_ref_id,
                &identity_id,
                expected_state_version,
                Zeroizing::new(secret.into_bytes()),
                &hosts,
                &vault,
            )
            .map_err(|_| "secureCredentialFailed".to_owned())?;
            let _ = metrics
                .recheck_hosts_referencing_identity(meta.request_id, identity_id)
                .await;
            Ok(summary.credential_ref_id.as_str().to_owned())
        }
        (Some(identity_id), None, None) => crate::host_service::credential_import(
            CredentialImportRequest {
                meta,
                operation_id: OperationId::new(),
                idempotency_key: uuid::Uuid::now_v7().to_string(),
                identity_id,
                kind,
                secret,
                passphrase,
                priority: 100,
                label: request.label,
            },
            hosts,
            vault,
            metrics,
        )
        .await
        .map(|summary| summary.credential_ref_id.as_str().to_owned()),
        (None, None, None) => crate::transient_credential_service::credential_transient_prepare(
            TransientCredentialPrepareRequest {
                meta,
                operation_id: OperationId::new(),
                idempotency_key: uuid::Uuid::now_v7().to_string(),
                kind,
                secret,
                passphrase,
            },
            app.state::<TransientCredentialService>(),
        )
        .await
        .map(|reference| reference.credential_ref_id.as_str().to_owned()),
        _ => Err(Box::new(norishell_core_api::CoreApiError {
            code: "credential.invalid_replacement".to_owned(),
            category: norishell_core_api::ErrorCategory::Validation,
            retry_strategy: norishell_core_api::RetryStrategy::Never,
            message_key: "errors.credential.invalidMaterial".to_owned(),
            params: BTreeMap::new(),
            request_id: Some(meta.request_id),
            diagnostic_id: None,
            conflict: None,
        })),
    };
    credential_ref_id
        .map(Some)
        .map_err(|_| "secureCredentialFailed".to_owned())
}

#[tauri::command]
pub fn secure_credential_get(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureCredentialService>,
) -> Result<SecureCredentialPrompt, String> {
    require_window(&window, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureCredentialUnavailable")?
        .get(&id)
        .map(|entry| entry.prompt.clone())
        .ok_or_else(|| "secureCredentialExpired".into())
}

#[tauri::command]
pub fn secure_credential_submit(
    id: String,
    secret: String,
    passphrase: String,
    window: WebviewWindow,
    service: State<'_, SecureCredentialService>,
) -> Result<(), String> {
    require_window(&window, &id)?;
    let answer = Answer {
        secret: Zeroizing::new(secret),
        passphrase: Zeroizing::new(passphrase),
    };
    let prompt = service
        .pending
        .lock()
        .map_err(|_| "secureCredentialUnavailable")?
        .get(&id)
        .map(|entry| entry.prompt.clone())
        .ok_or("secureCredentialExpired")?;
    let max_secret_bytes = match prompt.kind {
        SecureCredentialKind::Password => MAX_PASSWORD_BYTES,
        SecureCredentialKind::PrivateKey => MAX_PRIVATE_KEY_BYTES,
    };
    if answer.secret.is_empty()
        || answer.secret.len() > max_secret_bytes
        || answer.passphrase.len() > MAX_PASSWORD_BYTES
        || matches!(prompt.kind, SecureCredentialKind::Password) && !answer.passphrase.is_empty()
    {
        return Err("secureCredentialDenied".into());
    }
    let pending = service
        .pending
        .lock()
        .map_err(|_| "secureCredentialUnavailable")?
        .remove(&id)
        .ok_or("secureCredentialExpired")?;
    pending
        .sender
        .send(answer)
        .map_err(|_| "secureCredentialExpired".into())
}

#[tauri::command]
pub fn secure_credential_cancel(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureCredentialService>,
) -> Result<(), String> {
    require_window(&window, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureCredentialUnavailable")?
        .remove(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_window_requires_the_exact_prompt_label() {
        assert_eq!(window_label("id"), "secure-credential-id");
        assert!(valid_open_request(&SecureCredentialOpenRequest {
            kind: SecureCredentialKind::Password,
            label: "ops@example.test".into(),
            identity_id: None,
            credential_ref_id: None,
            expected_state_version: None,
        }));
    }

    #[test]
    fn empty_or_oversized_labels_are_rejected_before_a_window_exists() {
        for label in ["".to_owned(), " ".to_owned(), "a".repeat(513)] {
            assert!(!valid_open_request(&SecureCredentialOpenRequest {
                kind: SecureCredentialKind::PrivateKey,
                label,
                identity_id: None,
                credential_ref_id: None,
                expected_state_version: None,
            }));
        }
    }

    #[test]
    fn replacements_require_all_password_identity_fences() {
        let request = SecureCredentialOpenRequest {
            kind: SecureCredentialKind::Password,
            label: "Production password".into(),
            identity_id: Some(IdentityId::new()),
            credential_ref_id: Some(CredentialRefId::new()),
            expected_state_version: Some(WireSequence::new(2)),
        };
        assert!(valid_open_request(&request));

        assert!(!valid_open_request(&SecureCredentialOpenRequest {
            expected_state_version: None,
            ..request
        }));
    }
}
