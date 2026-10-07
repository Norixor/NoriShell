//! Core-owned SSH credential prompts. The main window receives an opaque ref only.
use crate::{
    host_service::HostService, metrics_session_service::MetricsSessionService,
    transient_credential_service::TransientCredentialService, vault_service::VaultService,
};
use norishell_core_api::{
    CoreApiError, CredentialImportRequest, CredentialKind, CredentialRefId, ErrorCategory,
    IdentityId, OperationId, RequestId, RequestMeta, RetryStrategy,
    TransientCredentialPrepareRequest, WireSequence,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tauri::{AppHandle, Manager, State, WebviewWindow};
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
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct SecureCredentialCommandError(Box<CoreApiError>);

impl From<&str> for SecureCredentialCommandError {
    fn from(value: &str) -> Self {
        let (category, retry, message_key) = match value {
            "secureCredentialDenied" => (
                ErrorCategory::Permission,
                RetryStrategy::Never,
                "secureWindow.credential.denied",
            ),
            "secureCredentialInvalidInput" => (
                ErrorCategory::Validation,
                RetryStrategy::Never,
                "secureWindow.credential.invalidInput",
            ),
            "secureCredentialExpired" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "secureWindow.credential.expired",
            ),
            "secureCredentialUnavailable" => (
                ErrorCategory::Unavailable,
                RetryStrategy::RefreshSnapshot,
                "secureWindow.credential.unavailable",
            ),
            "secureCredentialFailed" => (
                ErrorCategory::Internal,
                RetryStrategy::Never,
                "secureWindow.credential.failed",
            ),
            _ => {
                let diagnostic_id = uuid::Uuid::new_v4().to_string();
                eprintln!("secure credential command failed: diagnostic_id={diagnostic_id}");
                return Self(Box::new(CoreApiError::safe_internal(
                    RequestId::new(),
                    diagnostic_id,
                )));
            }
        };
        let mut error = crate::core_api_error::core_error(
            RequestId::new(),
            value,
            category,
            retry,
            message_key,
        );
        if value == "secureCredentialFailed" {
            let diagnostic_id = uuid::Uuid::new_v4().to_string();
            eprintln!("secure credential operation failed: diagnostic_id={diagnostic_id}");
            error.diagnostic_id = Some(diagnostic_id);
        }
        Self(error)
    }
}

impl From<String> for SecureCredentialCommandError {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

/// How the protected prompt reacts to a failed credential operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptOutcome {
    /// The user can correct the input (or unlock the Vault) in the same prompt.
    Retry,
    /// The prompt closes and its caller receives the same stable error.
    Close,
}

/// Keeps the stable machine code of the underlying Core error (credential,
/// Vault or persistence) and only swaps in message keys the protected window
/// translates, so distinct causes never collapse into one generic failure.
/// Unexpected failures keep (or receive) a diagnostic identifier.
fn classify_credential_failure(
    mut error: Box<CoreApiError>,
) -> (SecureCredentialCommandError, PromptOutcome) {
    let (message_key, outcome) = match error.code.as_str() {
        "credential.invalid_material" | "credential_transient.invalid_request" => (
            "secureWindow.credential.invalidMaterial",
            PromptOutcome::Retry,
        ),
        "credential.passphrase_required" | "credential_transient.passphrase_required" => (
            "secureWindow.credential.passphraseRequired",
            PromptOutcome::Retry,
        ),
        "credential.incorrect_passphrase" | "credential_transient.incorrect_passphrase" => (
            "secureWindow.credential.incorrectPassphrase",
            PromptOutcome::Retry,
        ),
        "vault.locked" => ("secureWindow.credential.vaultLocked", PromptOutcome::Retry),
        "vault.in_use" => ("secureWindow.vault.inUse", PromptOutcome::Retry),
        "vault.requires_reload" => ("secureWindow.vault.requiresReload", PromptOutcome::Close),
        "ssh_metadata.conflict"
        | "ssh_metadata.not_found"
        | "credential.invalid_replacement"
        | "credential_transient.idempotency_conflict" => {
            ("secureWindow.credential.changed", PromptOutcome::Close)
        }
        _ => {
            if error.diagnostic_id.is_none() {
                let diagnostic_id = uuid::Uuid::new_v4().to_string();
                eprintln!(
                    "secure credential operation failed: diagnostic_id={diagnostic_id} code={}",
                    error.code
                );
                error.diagnostic_id = Some(diagnostic_id);
            }
            ("secureWindow.credential.failed", PromptOutcome::Close)
        }
    };
    error.message_key = message_key.to_owned();
    (SecureCredentialCommandError(error), outcome)
}

type Verdict = oneshot::Sender<Result<(), SecureCredentialCommandError>>;

struct Answer {
    secret: Zeroizing<String>,
    passphrase: Zeroizing<String>,
    /// The protected window waits on this verdict. A retryable failure keeps
    /// the prompt open for another attempt; any other outcome ends it.
    verdict: Verdict,
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

fn require_label(surface_label: &str, id: &str) -> Result<(), String> {
    if surface_label != window_label(id) {
        return Err("secureCredentialDenied".into());
    }
    Ok(())
}

fn ordinary_owner_alive(app: &AppHandle, label: &str) -> bool {
    app.get_webview_window(label).is_some()
        && (label == "main"
            || app
                .state::<crate::workspace_windows::WorkspaceWindows>()
                .contains(label))
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
        crate::secure_surface::destroy(&self.app, &window_label(&self.id));
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
    webview: tauri::Webview,
    app: AppHandle,
    service: State<'_, SecureCredentialService>,
    hosts: State<'_, HostService>,
    vault: State<'_, VaultService>,
    metrics: State<'_, MetricsSessionService>,
) -> Result<Option<String>, SecureCredentialCommandError> {
    // Only registered ordinary windows can open a prompt. The prompt itself
    // returns an opaque Core-issued reference through its isolated capability.
    let owner = crate::workspace_tab_views::ordinary_owner(&app, &webview)
        .map_err(|_| "secureCredentialDenied")?;
    if !ordinary_owner_alive(&app, &owner) {
        return Err("secureCredentialDenied".into());
    }
    if !valid_open_request(&request) {
        return Err("secureCredentialInvalidInput".into());
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
    let window_height = match request.kind {
        SecureCredentialKind::Password => 400.0,
        SecureCredentialKind::PrivateKey => 480.0,
    };
    let child = crate::secure_surface::open(
        &app,
        crate::secure_surface::SurfaceSpec::new(
            crate::secure_surface::SurfaceKind::Credential,
            window_label(&id),
        )
        .query(format!("prompt={id}"))
        .size(600.0, window_height)
        .min_size(480.0, 320.0)
        .navigation_guard(),
    )
    .map_err(|_| "secureCredentialUnavailable")?;
    let cancelled_app = app.clone();
    let cancelled_id = id.clone();
    child.on_destroyed(move || {
        if let Ok(mut pending) = cancelled_app
            .state::<SecureCredentialService>()
            .pending
            .lock()
        {
            pending.remove(&cancelled_id);
        }
    });
    let _ = child.focus();

    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);
    let mut owner_check = tokio::time::interval(Duration::from_millis(250));
    let owner_valid = || {
        ordinary_owner_alive(&app, &owner)
            && crate::workspace_tab_views::ordinary_owner(&app, &webview).as_deref()
                == Ok(owner.as_str())
    };
    let prompt = SecureCredentialPrompt {
        id: id.clone(),
        kind: request.kind,
        label: request.label.clone(),
        persistent: request.identity_id.is_some(),
        replacement: request.credential_ref_id.is_some(),
    };
    let mut receiver = receiver;
    loop {
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
        let verdict = answer.verdict;
        let result = apply_credential(
            &request,
            std::mem::take(&mut *answer.secret),
            std::mem::take(&mut *answer.passphrase),
            &app,
            &hosts,
            &vault,
            &metrics,
        )
        .await;
        let error = match result {
            Ok(credential_ref_id) => {
                let _ = verdict.send(Ok(()));
                return Ok(Some(credential_ref_id));
            }
            Err(error) => error,
        };
        let (error, outcome) = classify_credential_failure(error);
        if outcome == PromptOutcome::Close || !owner_valid() {
            let _ = verdict.send(Err(error.clone()));
            return Err(error);
        }
        // Re-arm the same prompt before replying so the window's next submit
        // finds it; the original deadline still applies.
        let (sender, next_receiver) = oneshot::channel();
        service
            .pending
            .lock()
            .map_err(|_| "secureCredentialUnavailable")?
            .insert(
                id.clone(),
                Pending {
                    prompt: prompt.clone(),
                    sender,
                },
            );
        // Checked after inserting: a window destroyed before the insert is
        // detected here, one destroyed after it removes the entry itself.
        if !crate::secure_surface::exists(&app, &window_label(&id)) {
            if let Ok(mut pending) = service.pending.lock() {
                pending.remove(&id);
            }
            let _ = verdict.send(Err(error));
            return Ok(None);
        }
        receiver = next_receiver;
        let _ = verdict.send(Err(error));
    }
}

/// Priorities are unique per Identity and tried in ascending order. Material the
/// user just entered (typically after the saved one was rejected) is tried first:
/// just below the current lowest priority, or the lowest free value when 0 is taken.
fn first_attempt_priority(taken: &[u32]) -> u32 {
    match taken.iter().min() {
        None => 100,
        Some(&lowest) if lowest > 0 => lowest - 1,
        Some(_) => (0..=u32::from(u16::MAX))
            .find(|candidate| !taken.contains(candidate))
            .unwrap_or(u32::from(u16::MAX)),
    }
}

/// Imports a credential for an Identity at a free priority. A fixed priority made
/// every save for an Identity that already had a credential fail with
/// `ssh_metadata.conflict`, so re-authentication with "save" never reconnected.
/// One retry covers a priority taken concurrently between the read and the import.
async fn import_identity_credential(
    meta: RequestMeta,
    identity_id: IdentityId,
    kind: CredentialKind,
    secret: Zeroizing<String>,
    passphrase: Option<Zeroizing<String>>,
    label: String,
    app: &AppHandle,
) -> Result<String, Box<CoreApiError>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let taken: Vec<u32> = crate::host_service::credential_ref_list(
            norishell_core_api::CredentialRefListRequest {
                meta: RequestMeta {
                    request_id: meta.request_id.clone(),
                },
                identity_id: identity_id.clone(),
            },
            app.state(),
        )?
        .into_iter()
        .map(|credential| credential.priority)
        .collect();
        let result = crate::host_service::credential_import(
            CredentialImportRequest {
                meta: RequestMeta {
                    request_id: meta.request_id.clone(),
                },
                operation_id: OperationId::new(),
                idempotency_key: uuid::Uuid::now_v7().to_string(),
                identity_id: identity_id.clone(),
                kind,
                secret: (*secret).clone(),
                passphrase: passphrase.as_ref().map(|value| (**value).clone()),
                priority: first_attempt_priority(&taken),
                label: label.clone(),
            },
            app.state(),
            app.state(),
            app.state(),
        )
        .await;
        match result {
            Err(error) if error.code == "ssh_metadata.conflict" && attempts < 2 => continue,
            other => return other.map(|summary| summary.credential_ref_id.as_str().to_owned()),
        }
    }
}

/// Imports, replaces or transiently prepares the submitted material. Errors
/// keep the stable code of the operation that failed.
async fn apply_credential(
    request: &SecureCredentialOpenRequest,
    secret: String,
    passphrase: String,
    app: &AppHandle,
    hosts: &State<'_, HostService>,
    vault: &State<'_, VaultService>,
    metrics: &State<'_, MetricsSessionService>,
) -> Result<String, Box<CoreApiError>> {
    let kind: CredentialKind = request.kind.into();
    let mut passphrase = Zeroizing::new(passphrase);
    let passphrase = match request.kind {
        SecureCredentialKind::Password => None,
        SecureCredentialKind::PrivateKey if passphrase.is_empty() => None,
        SecureCredentialKind::PrivateKey => Some(std::mem::take(&mut *passphrase)),
    };
    let meta = RequestMeta {
        request_id: RequestId::new(),
    };
    match (
        request.identity_id.clone(),
        request.credential_ref_id.clone(),
        request.expected_state_version,
    ) {
        (Some(identity_id), Some(credential_ref_id), Some(expected_state_version)) => {
            let summary = crate::host_service::replace_password_credential(
                meta.request_id.clone(),
                &credential_ref_id,
                &identity_id,
                expected_state_version,
                Zeroizing::new(secret.into_bytes()),
                hosts,
                vault,
            )?;
            let _ = metrics
                .recheck_hosts_referencing_identity(meta.request_id, identity_id)
                .await;
            Ok(summary.credential_ref_id.as_str().to_owned())
        }
        (Some(identity_id), None, None) => {
            import_identity_credential(
                meta,
                identity_id,
                kind,
                Zeroizing::new(secret),
                passphrase.map(Zeroizing::new),
                request.label.clone(),
                app,
            )
            .await
        }
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
    }
}

#[tauri::command]
pub fn secure_credential_get(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureCredentialService>,
) -> Result<SecureCredentialPrompt, SecureCredentialCommandError> {
    secure_credential_get_for_label(window.label(), id, &service)
}

pub(crate) fn secure_credential_get_for_label(
    surface_label: &str,
    id: String,
    service: &SecureCredentialService,
) -> Result<SecureCredentialPrompt, SecureCredentialCommandError> {
    require_label(surface_label, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureCredentialUnavailable")?
        .get(&id)
        .map(|entry| entry.prompt.clone())
        .ok_or_else(|| "secureCredentialExpired".into())
}

#[tauri::command]
pub async fn secure_credential_submit(
    id: String,
    secret: String,
    passphrase: String,
    window: WebviewWindow,
    service: State<'_, SecureCredentialService>,
) -> Result<(), SecureCredentialCommandError> {
    secure_credential_submit_for_label(
        window.label(),
        id,
        Zeroizing::new(secret),
        Zeroizing::new(passphrase),
        &service,
    )
    .await
}

/// Hands the answer to the waiting prompt owner and resolves with its verdict:
/// `Ok` once the credential is ready, a retryable error while the same prompt
/// stays open (invalid key material, locked Vault), or the final error before
/// the prompt closes.
pub(crate) async fn secure_credential_submit_for_label(
    surface_label: &str,
    id: String,
    secret: Zeroizing<String>,
    passphrase: Zeroizing<String>,
    service: &SecureCredentialService,
) -> Result<(), SecureCredentialCommandError> {
    require_label(surface_label, &id)?;
    let (verdict, verdict_receiver) = oneshot::channel();
    let answer = Answer {
        secret,
        passphrase,
        verdict,
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
        return Err("secureCredentialInvalidInput".into());
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
        .map_err(|_| SecureCredentialCommandError::from("secureCredentialExpired"))?;
    // A dropped verdict means the owner went away or the prompt expired.
    verdict_receiver
        .await
        .unwrap_or_else(|_| Err("secureCredentialExpired".into()))
}

#[tauri::command]
pub fn secure_credential_cancel(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureCredentialService>,
) -> Result<(), SecureCredentialCommandError> {
    secure_credential_cancel_for_label(window.label(), id, &service)
}

pub(crate) fn secure_credential_cancel_for_label(
    surface_label: &str,
    id: String,
    service: &SecureCredentialService,
) -> Result<(), SecureCredentialCommandError> {
    require_label(surface_label, &id)?;
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
    fn saved_material_takes_a_free_priority_that_is_tried_first() {
        assert_eq!(first_attempt_priority(&[]), 100);
        // The Identity already has the rejected credential at the old fixed priority.
        assert_eq!(first_attempt_priority(&[100]), 99);
        assert_eq!(first_attempt_priority(&[100, 5]), 4);
        assert_eq!(first_attempt_priority(&[0, 1, 3]), 2);
        assert_eq!(first_attempt_priority(&[0]), 1);
    }

    #[test]
    fn invalid_credential_input_is_not_a_window_permission_failure() {
        let denied = SecureCredentialCommandError::from("secureCredentialDenied");
        let invalid = SecureCredentialCommandError::from("secureCredentialInvalidInput");
        assert_eq!(denied.0.code, "secureCredentialDenied");
        assert_eq!(invalid.0.code, "secureCredentialInvalidInput");
        assert_ne!(denied.0.category, invalid.0.category);
    }

    fn core(code: &str) -> Box<CoreApiError> {
        crate::core_api_error::core_error(
            RequestId::new(),
            code,
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.original",
        )
    }

    #[test]
    fn credential_failures_keep_their_code_and_only_correctable_ones_retry() {
        for (code, key, outcome) in [
            (
                "credential.invalid_material",
                "secureWindow.credential.invalidMaterial",
                PromptOutcome::Retry,
            ),
            (
                "credential_transient.invalid_request",
                "secureWindow.credential.invalidMaterial",
                PromptOutcome::Retry,
            ),
            (
                "credential.passphrase_required",
                "secureWindow.credential.passphraseRequired",
                PromptOutcome::Retry,
            ),
            (
                "credential_transient.passphrase_required",
                "secureWindow.credential.passphraseRequired",
                PromptOutcome::Retry,
            ),
            (
                "credential.incorrect_passphrase",
                "secureWindow.credential.incorrectPassphrase",
                PromptOutcome::Retry,
            ),
            (
                "credential_transient.incorrect_passphrase",
                "secureWindow.credential.incorrectPassphrase",
                PromptOutcome::Retry,
            ),
            (
                "vault.locked",
                "secureWindow.credential.vaultLocked",
                PromptOutcome::Retry,
            ),
            (
                "vault.in_use",
                "secureWindow.vault.inUse",
                PromptOutcome::Retry,
            ),
            (
                "vault.requires_reload",
                "secureWindow.vault.requiresReload",
                PromptOutcome::Close,
            ),
            (
                "ssh_metadata.conflict",
                "secureWindow.credential.changed",
                PromptOutcome::Close,
            ),
        ] {
            let (error, actual) = classify_credential_failure(core(code));
            assert_eq!(error.0.code, code);
            assert_eq!(error.0.message_key, key);
            assert_eq!(actual, outcome, "{code}");
            assert!(error.0.diagnostic_id.is_none());
        }
        let (error, outcome) = classify_credential_failure(core("vault.storage_failed"));
        assert_eq!(error.0.code, "vault.storage_failed");
        assert_eq!(outcome, PromptOutcome::Close);
        assert!(error.0.diagnostic_id.is_some());
    }

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
