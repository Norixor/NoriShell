//! Core-owned Vault prompts. Only the exact isolated window can submit secrets.
use crate::{
    connection_profile::{
        connection_has_vault_credentials, connection_requires_vault,
        resolve_long_lived_connection_profile,
    },
    host_service::HostService,
    ssh_sync_exchange_local::NoriShellSshSyncLocalAdapter,
    vault_service::{VaultService, VaultServiceError},
};
use norishell_core_api::{
    CoreApiError, ErrorCategory, HostId, RequestId, RequestMeta, RetryStrategy,
    VaultAutoUnlockEnableRequest, VaultCreateRequest, VaultState, VaultStatus, VaultUnlockPolicy,
    VaultUnlockRequest, WireSequence,
};
use norishell_secret_vault::VaultError;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State, Webview, WebviewWindow};
use tokio::sync::oneshot;
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecureVaultMode {
    EnsureUnlocked,
    UnlockSavedLocal,
    EnableAutoUnlock,
    EnableLocalAutoUnlock,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecureVaultKind {
    Create,
    Unlock,
    EnableAutoUnlock,
    EnableLocalAutoUnlock,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureVaultPrompt {
    id: String,
    kind: SecureVaultKind,
    can_reset: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct SecureVaultCommandError(Box<CoreApiError>);

impl From<&str> for SecureVaultCommandError {
    fn from(value: &str) -> Self {
        let (category, retry, message_key) = match value {
            "secureVaultDenied" => (
                ErrorCategory::Permission,
                RetryStrategy::Never,
                "secureWindow.vault.denied",
            ),
            "secureVaultStateChanged" | "secureVaultResetChanged" => (
                ErrorCategory::Conflict,
                RetryStrategy::RefreshSnapshot,
                "secureWindow.vault.changed",
            ),
            "secureVaultRestartRequired" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "secureWindow.vault.restartRequired",
            ),
            "secureVaultExpired" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "secureWindow.vault.expired",
            ),
            "secureVaultUnavailable" => (
                ErrorCategory::Unavailable,
                RetryStrategy::RefreshSnapshot,
                "secureWindow.vault.unavailable",
            ),
            "secureVaultResetUncertain" => (
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "sshHosts.vault.reset.uncertain",
            ),
            "secureVaultResetFailed" => (
                ErrorCategory::Internal,
                RetryStrategy::Never,
                "sshHosts.vault.reset.failed",
            ),
            _ => {
                let diagnostic_id = uuid::Uuid::new_v4().to_string();
                eprintln!("secure Vault command failed: diagnostic_id={diagnostic_id}");
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
        if value == "secureVaultResetFailed" {
            let diagnostic_id = uuid::Uuid::new_v4().to_string();
            eprintln!("secure Vault operation failed: diagnostic_id={diagnostic_id} code={value}");
            error.diagnostic_id = Some(diagnostic_id);
        }
        Self(error)
    }
}

/// How the protected prompt reacts to a Vault mutation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptOutcome {
    /// The user can correct the input in the same prompt (wrong password,
    /// weak/mismatched new password, transient cross-process lock).
    Retry,
    /// The prompt closes and the caller receives the same stable error.
    Close,
}

/// Keeps the stable machine code produced by `vault_service::map_service_error`
/// and only swaps message keys that have no translated text for the protected
/// window, so distinct causes never collapse into one generic failure.
fn classify_vault_failure(
    mut error: Box<CoreApiError>,
) -> (SecureVaultCommandError, PromptOutcome) {
    let (message_key, outcome) = match error.code.as_str() {
        "vault.authentication_failed" => {
            ("secureWindow.vault.incorrectPassword", PromptOutcome::Retry)
        }
        "vault.password_confirmation_mismatch" => (
            "secureWindow.vault.confirmationMismatch",
            PromptOutcome::Retry,
        ),
        "vault.weak_password" => ("secureWindow.vault.weakPassword", PromptOutcome::Retry),
        "vault.in_use" => ("secureWindow.vault.inUse", PromptOutcome::Retry),
        "vault.already_exists" | "vault.already_unlocked" | "vault.availability_changed" => {
            ("secureWindow.vault.changed", PromptOutcome::Close)
        }
        "vault.requires_reload" => ("secureWindow.vault.requiresReload", PromptOutcome::Close),
        "vault.insecure_permissions" => (
            "secureWindow.vault.insecurePermissions",
            PromptOutcome::Close,
        ),
        "vault.invalid_auto_unlock_policy" => {
            ("secureWindow.vault.policyInvalid", PromptOutcome::Close)
        }
        // Secure-storage codes already carry translated keys; internal failures
        // keep their diagnostic identifier and generic text.
        _ => (error.message_key.as_str(), PromptOutcome::Close),
    };
    error.message_key = message_key.to_owned();
    (SecureVaultCommandError(error), outcome)
}

impl From<String> for SecureVaultCommandError {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}
struct PasswordAnswer {
    password: Zeroizing<String>,
    confirmation: Zeroizing<String>,
    confirmed: bool,
    /// The protected window waits on this verdict. A retryable rejection keeps
    /// the prompt open for another attempt; any other outcome ends it.
    verdict: oneshot::Sender<Result<(), SecureVaultCommandError>>,
}
enum Answer {
    Password(PasswordAnswer),
    Reset,
}
struct Pending {
    prompt: SecureVaultPrompt,
    sender: oneshot::Sender<Answer>,
    owner_label: String,
    reset_fingerprint: Option<[u8; 32]>,
    expires_at: Instant,
}
#[derive(Default)]
pub struct SecureVaultService {
    pending: Mutex<BTreeMap<String, Pending>>,
}
fn label(id: &str) -> String {
    format!("secure-vault-{id}")
}
fn require_label(surface_label: &str, id: &str) -> Result<(), String> {
    if surface_label != label(id) {
        return Err("secureVaultDenied".into());
    }
    Ok(())
}

fn allowed_caller(app: &AppHandle, label: &str) -> bool {
    label == "main"
        || app
            .state::<crate::workspace_windows::WorkspaceWindows>()
            .contains(label)
        || app
            .state::<crate::tool_windows::ToolWindows>()
            .is_editor(label)
        || app
            .state::<crate::secure_credential_service::SecureCredentialService>()
            .is_prompt_window(label)
}

fn vault_caller_owner(app: &AppHandle, webview: &Webview) -> Option<String> {
    let window = webview.window();
    if webview.label() == window.label()
        && app.get_webview_window(window.label()).is_some()
        && allowed_caller(app, window.label())
    {
        Some(window.label().into())
    } else {
        crate::workspace_tab_views::ordinary_owner(app, webview).ok()
    }
}

fn ordinary_owner_alive(app: &AppHandle, label: &str) -> bool {
    app.get_webview_window(label).is_some()
        && (label == "main"
            || app
                .state::<crate::workspace_windows::WorkspaceWindows>()
                .contains(label))
}
struct PromptGuard {
    app: AppHandle,
    id: String,
}
impl Drop for PromptGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.app.state::<SecureVaultService>().pending.lock() {
            pending.remove(&self.id);
        }
        crate::secure_surface::destroy(&self.app, &label(&self.id));
    }
}
fn kind_for(
    mode: SecureVaultMode,
    status: &VaultStatus,
) -> Result<Option<SecureVaultKind>, String> {
    match mode {
        SecureVaultMode::EnsureUnlocked | SecureVaultMode::UnlockSavedLocal => {
            Ok(match status.state {
                VaultState::Missing => Some(SecureVaultKind::Create),
                VaultState::Locked | VaultState::RequiresReload => Some(SecureVaultKind::Unlock),
                VaultState::Unlocked => None,
            })
        }
        SecureVaultMode::EnableAutoUnlock if status.state != VaultState::Missing => {
            Ok(Some(SecureVaultKind::EnableAutoUnlock))
        }
        SecureVaultMode::EnableAutoUnlock => Err("secureVaultStateChanged".into()),
        SecureVaultMode::EnableLocalAutoUnlock if status.state != VaultState::Missing => {
            Ok(Some(SecureVaultKind::EnableLocalAutoUnlock))
        }
        SecureVaultMode::EnableLocalAutoUnlock => Err("secureVaultStateChanged".into()),
    }
}

/// Resolves the current saved-Host authentication plan before an explicit
/// foreground Vault prompt. Only a live ordinary window may request this
/// continuation: resource factories must report VaultLocked instead of opening
/// an interactive window.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn secure_vault_ensure_for_host(
    host_id: HostId,
    expected_host_state_version: WireSequence,
    include_optional_credentials: Option<bool>,
    webview: Webview,
    app: AppHandle,
    service: State<'_, SecureVaultService>,
    vault: State<'_, VaultService>,
    ssh_sync: State<'_, NoriShellSshSyncLocalAdapter>,
    hosts: State<'_, HostService>,
) -> Result<bool, SecureVaultCommandError> {
    let owner = crate::workspace_tab_views::ordinary_owner(&app, &webview)
        .map_err(|_| "secureVaultDenied")?;
    if !ordinary_owner_alive(&app, &owner) {
        return Err("secureVaultDenied".into());
    }
    let initial =
        resolve_long_lived_connection_profile(&hosts, &host_id, expected_host_state_version)
            .map_err(|_| "secureVaultStateChanged")?;
    let should_prompt = connection_requires_vault(&initial.connection)
        || (include_optional_credentials.unwrap_or(false)
            && connection_has_vault_credentials(&initial.connection));
    if !should_prompt {
        return Ok(true);
    }
    let revision_token = initial.connection.revision_token.clone();
    if !secure_vault_open(
        SecureVaultMode::EnsureUnlocked,
        webview,
        app,
        service,
        vault,
        ssh_sync,
    )
    .await?
    {
        return Ok(false);
    }
    let current =
        resolve_long_lived_connection_profile(&hosts, &host_id, expected_host_state_version)
            .map_err(|_| "secureVaultStateChanged")?;
    if current.connection.revision_token != revision_token {
        return Err("secureVaultStateChanged".into());
    }
    Ok(true)
}

#[tauri::command]
pub async fn secure_vault_open(
    mode: SecureVaultMode,
    webview: Webview,
    app: AppHandle,
    service: State<'_, SecureVaultService>,
    vault: State<'_, VaultService>,
    ssh_sync: State<'_, NoriShellSshSyncLocalAdapter>,
) -> Result<bool, SecureVaultCommandError> {
    let owner = vault_caller_owner(&app, &webview).ok_or("secureVaultDenied")?;
    let owner_valid = || vault_caller_owner(&app, &webview).as_deref() == Some(owner.as_str());
    open_for_owner(mode, &app, &owner, owner_valid, &service, vault, ssh_sync).await
}

/// Android: a Core-owned surface (credential prompt or editor) asks for the
/// nested Vault prompt. The surface label is bound by Kotlin, never the page.
#[cfg(target_os = "android")]
pub(crate) async fn secure_vault_open_for_surface(
    mode: SecureVaultMode,
    surface_label: &str,
    app: &AppHandle,
) -> Result<bool, SecureVaultCommandError> {
    let owner_valid =
        || crate::secure_surface::exists(app, surface_label) && allowed_caller(app, surface_label);
    if !owner_valid() {
        return Err("secureVaultDenied".into());
    }
    open_for_owner(
        mode,
        app,
        surface_label,
        owner_valid,
        &app.state::<SecureVaultService>(),
        app.state(),
        app.state(),
    )
    .await
}

async fn open_for_owner(
    mode: SecureVaultMode,
    app: &AppHandle,
    owner: &str,
    owner_valid: impl Fn() -> bool,
    service: &SecureVaultService,
    vault: State<'_, VaultService>,
    ssh_sync: State<'_, NoriShellSshSyncLocalAdapter>,
) -> Result<bool, SecureVaultCommandError> {
    let mode = if matches!(mode, SecureVaultMode::UnlockSavedLocal) {
        if owner != "main" {
            return Err("secureVaultDenied".into());
        }
        if vault.unlock_with_saved_local_password().is_ok() {
            if ssh_sync.reconcile_after_vault_unlock().await.is_err() {
                eprintln!(
                    "SSH sync cleanup remains pending after Vault unlock; background reconciliation will retry"
                );
            }
            let _ = app.emit_to("main", "native-tray-vault-changed", ());
            return Ok(true);
        }
        // A missing, unsafe or stale local file falls back to the ordinary
        // protected password prompt without changing the saved policy.
        SecureVaultMode::EnsureUnlocked
    } else {
        mode
    };
    let initial = vault.status();
    if vault.requires_restart_after_reset() {
        return Err("secureVaultRestartRequired".into());
    }
    let Some(kind) = kind_for(mode, &initial)? else {
        return Ok(true);
    };
    let reset_fingerprint =
        if kind == SecureVaultKind::Unlock && initial.state == VaultState::Locked {
            vault.locked_reset_fingerprint().ok()
        } else {
            None
        };
    let id = uuid::Uuid::now_v7().to_string();
    let expires_at = Instant::now() + Duration::from_secs(180);
    let (sender, receiver) = oneshot::channel();
    service
        .pending
        .lock()
        .map_err(|_| "secureVaultUnavailable")?
        .insert(
            id.clone(),
            Pending {
                prompt: SecureVaultPrompt {
                    id: id.clone(),
                    kind,
                    can_reset: reset_fingerprint.is_some(),
                },
                sender,
                owner_label: owner.to_owned(),
                reset_fingerprint,
                expires_at,
            },
        );
    let _guard = PromptGuard {
        app: app.clone(),
        id: id.clone(),
    };
    let window_height = match kind {
        SecureVaultKind::Unlock => 380.0,
        SecureVaultKind::Create => 460.0,
        SecureVaultKind::EnableAutoUnlock => 500.0,
        SecureVaultKind::EnableLocalAutoUnlock => 540.0,
    };
    let child = crate::secure_surface::open(
        app,
        crate::secure_surface::SurfaceSpec::new(
            crate::secure_surface::SurfaceKind::Vault,
            label(&id),
        )
        .query(format!("prompt={id}"))
        .size(600.0, window_height)
        .min_size(480.0, 300.0)
        .navigation_guard(),
    )
    .map_err(|_| "secureVaultUnavailable")?;
    let cancel_app = app.clone();
    let cancel_id = id.clone();
    child.on_destroyed(move || {
        if let Ok(mut pending) = cancel_app.state::<SecureVaultService>().pending.lock() {
            pending.remove(&cancel_id);
        }
    });
    let _ = child.focus();
    let template = PromptTemplate {
        id: id.clone(),
        kind,
        owner_label: owner.to_owned(),
        reset_fingerprint,
        expires_at,
    };
    let end = answer_loop(
        service,
        &template,
        receiver,
        owner_valid,
        || crate::secure_surface::exists(app, &label(&id)),
        || vault.status() == initial,
        |password, confirmation| {
            apply_password(
                kind,
                password,
                confirmation,
                vault.clone(),
                ssh_sync.clone(),
            )
        },
    )
    .await?;
    match end {
        PromptEnd::Applied => {
            let _ = app.emit_to("main", "native-tray-vault-changed", ());
            Ok(true)
        }
        PromptEnd::Reset => {
            let _ = app.emit_to("main", "native-tray-vault-changed", ());
            Ok(false)
        }
        PromptEnd::Cancelled => Ok(false),
    }
}

/// Everything needed to re-arm the same prompt after a retryable failure.
struct PromptTemplate {
    id: String,
    kind: SecureVaultKind,
    owner_label: String,
    reset_fingerprint: Option<[u8; 32]>,
    expires_at: Instant,
}

impl PromptTemplate {
    fn pending(&self, sender: oneshot::Sender<Answer>) -> Pending {
        Pending {
            prompt: SecureVaultPrompt {
                id: self.id.clone(),
                kind: self.kind,
                can_reset: self.reset_fingerprint.is_some(),
            },
            sender,
            owner_label: self.owner_label.clone(),
            reset_fingerprint: self.reset_fingerprint,
            expires_at: self.expires_at,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PromptEnd {
    Applied,
    Reset,
    Cancelled,
}

/// Waits for protected-window answers and applies them. Expiry and owner
/// teardown cancel only pending interaction, never an accepted mutation.
async fn answer_loop<Apply, Fut>(
    service: &SecureVaultService,
    template: &PromptTemplate,
    mut receiver: oneshot::Receiver<Answer>,
    owner_valid: impl Fn() -> bool,
    prompt_alive: impl Fn() -> bool,
    status_unchanged: impl Fn() -> bool,
    mut apply: Apply,
) -> Result<PromptEnd, SecureVaultCommandError>
where
    Apply: FnMut(String, String) -> Fut,
    Fut: std::future::Future<Output = Result<(), Box<CoreApiError>>>,
{
    let deadline = tokio::time::sleep(
        template
            .expires_at
            .saturating_duration_since(Instant::now()),
    );
    tokio::pin!(deadline);
    let mut owner_check = tokio::time::interval(Duration::from_millis(250));
    loop {
        let answer = loop {
            tokio::select! {
                response = &mut receiver => {
                    let Ok(answer) = response else { return Ok(PromptEnd::Cancelled); };
                    break answer;
                }
                _ = &mut deadline => return Ok(PromptEnd::Cancelled),
                _ = owner_check.tick() => {
                    if !owner_valid() { return Ok(PromptEnd::Cancelled); }
                }
            }
        };
        if !owner_valid() {
            return Ok(PromptEnd::Cancelled);
        }
        let Answer::Password(answer) = answer else {
            return Ok(PromptEnd::Reset);
        };
        let PasswordAnswer {
            mut password,
            mut confirmation,
            confirmed,
            verdict,
        } = answer;
        if !status_unchanged() {
            let error = SecureVaultCommandError::from("secureVaultStateChanged");
            let _ = verdict.send(Err(error.clone()));
            return Err(error);
        }
        if kind_requires_confirmation(template.kind) && !confirmed {
            let error = SecureVaultCommandError::from("secureVaultDenied");
            let _ = verdict.send(Err(error.clone()));
            return Err(error);
        }
        let result = apply(
            std::mem::take(&mut *password),
            std::mem::take(&mut *confirmation),
        )
        .await;
        let Err(error) = result else {
            let _ = verdict.send(Ok(()));
            return Ok(PromptEnd::Applied);
        };
        let (error, outcome) = classify_vault_failure(error);
        if outcome == PromptOutcome::Close || !owner_valid() {
            let _ = verdict.send(Err(error.clone()));
            return Err(error);
        }
        // Re-arm the same prompt before replying so the window's next submit
        // finds it. Only one attempt is in flight at a time because the entry
        // is removed while an answer is processed; the KDF cost bounds the
        // retry rate and the original deadline still applies.
        let (sender, next_receiver) = oneshot::channel();
        service
            .pending
            .lock()
            .map_err(|_| "secureVaultUnavailable")?
            .insert(template.id.clone(), template.pending(sender));
        // Checked after inserting: a prompt window destroyed before the insert
        // is detected here, one destroyed after it removes the entry itself.
        if !prompt_alive() {
            if let Ok(mut pending) = service.pending.lock() {
                pending.remove(&template.id);
            }
            let _ = verdict.send(Err(error));
            return Ok(PromptEnd::Cancelled);
        }
        receiver = next_receiver;
        let _ = verdict.send(Err(error));
    }
}

fn kind_requires_confirmation(kind: SecureVaultKind) -> bool {
    matches!(
        kind,
        SecureVaultKind::EnableAutoUnlock | SecureVaultKind::EnableLocalAutoUnlock
    )
}

async fn apply_password(
    kind: SecureVaultKind,
    password: String,
    confirmation: String,
    vault: State<'_, VaultService>,
    ssh_sync: State<'_, NoriShellSshSyncLocalAdapter>,
) -> Result<(), Box<CoreApiError>> {
    let meta = RequestMeta {
        request_id: norishell_core_api::RequestId::new(),
    };
    match kind {
        SecureVaultKind::Create => crate::vault_service::vault_create(
            VaultCreateRequest {
                meta,
                password,
                password_confirmation: confirmation,
            },
            vault,
        )
        .map(drop),
        SecureVaultKind::Unlock => {
            let mut confirmation = confirmation;
            confirmation.zeroize();
            crate::vault_service::vault_unlock(
                VaultUnlockRequest { meta, password },
                vault,
                ssh_sync,
            )
            .await
            .map(drop)
        }
        SecureVaultKind::EnableAutoUnlock | SecureVaultKind::EnableLocalAutoUnlock => {
            let mut confirmation = confirmation;
            confirmation.zeroize();
            crate::vault_service::vault_auto_unlock_enable(
                VaultAutoUnlockEnableRequest {
                    meta,
                    password,
                    policy: (kind == SecureVaultKind::EnableLocalAutoUnlock)
                        .then_some(VaultUnlockPolicy::AutomaticLocal),
                },
                vault,
            )
            .map(drop)
        }
    }
}
#[tauri::command]
pub fn secure_vault_get(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureVaultService>,
) -> Result<SecureVaultPrompt, SecureVaultCommandError> {
    secure_vault_get_for_label(window.label(), id, &service)
}

pub(crate) fn secure_vault_get_for_label(
    surface_label: &str,
    id: String,
    service: &SecureVaultService,
) -> Result<SecureVaultPrompt, SecureVaultCommandError> {
    require_label(surface_label, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureVaultUnavailable")?
        .get(&id)
        .map(|entry| entry.prompt.clone())
        .ok_or_else(|| "secureVaultExpired".into())
}
#[tauri::command]
pub async fn secure_vault_submit(
    id: String,
    password: String,
    password_confirmation: String,
    confirmed: bool,
    window: WebviewWindow,
    service: State<'_, SecureVaultService>,
) -> Result<(), SecureVaultCommandError> {
    secure_vault_submit_for_label(
        window.label(),
        id,
        Zeroizing::new(password),
        Zeroizing::new(password_confirmation),
        confirmed,
        &service,
    )
    .await
}

/// Hands the answer to the waiting prompt owner and resolves with its verdict:
/// `Ok` after the Vault mutation succeeded, a retryable error while the same
/// prompt stays open (for example an incorrect password), or the final error
/// before the prompt closes.
pub(crate) async fn secure_vault_submit_for_label(
    surface_label: &str,
    id: String,
    password: Zeroizing<String>,
    password_confirmation: Zeroizing<String>,
    confirmed: bool,
    service: &SecureVaultService,
) -> Result<(), SecureVaultCommandError> {
    require_label(surface_label, &id)?;
    if password.is_empty() || password.len() > 65_536 || password_confirmation.len() > 65_536 {
        return Err("secureVaultDenied".into());
    }
    let (verdict, verdict_receiver) = oneshot::channel();
    let answer = PasswordAnswer {
        password,
        confirmation: password_confirmation,
        confirmed,
        verdict,
    };
    let pending = service
        .pending
        .lock()
        .map_err(|_| "secureVaultUnavailable")?
        .remove(&id)
        .ok_or("secureVaultExpired")?;
    pending
        .sender
        .send(Answer::Password(answer))
        .map_err(|_| SecureVaultCommandError::from("secureVaultExpired"))?;
    // A dropped verdict means the owner went away or the prompt expired.
    verdict_receiver
        .await
        .unwrap_or_else(|_| Err("secureVaultExpired".into()))
}
#[tauri::command]
pub fn secure_vault_reset(
    id: String,
    phrase: String,
    confirmed: bool,
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, SecureVaultService>,
    vault: State<'_, VaultService>,
) -> Result<(), SecureVaultCommandError> {
    secure_vault_reset_for_label(
        window.label(),
        id,
        phrase,
        confirmed,
        &app,
        &service,
        &vault,
    )
}

pub(crate) fn secure_vault_reset_for_label(
    surface_label: &str,
    id: String,
    phrase: String,
    confirmed: bool,
    app: &AppHandle,
    service: &SecureVaultService,
    vault: &VaultService,
) -> Result<(), SecureVaultCommandError> {
    require_label(surface_label, &id)?;
    if phrase != "RESET" || !confirmed {
        return Err("secureVaultDenied".into());
    }
    let mut pending = service
        .pending
        .lock()
        .map_err(|_| "secureVaultUnavailable")?;
    let entry = pending.get(&id).ok_or("secureVaultExpired")?;
    if entry.prompt.kind != SecureVaultKind::Unlock
        || !entry.prompt.can_reset
        || entry.expires_at <= Instant::now()
        || entry.sender.is_closed()
        || !crate::secure_surface::exists(app, &entry.owner_label)
        || !allowed_caller(app, &entry.owner_label)
    {
        return Err("secureVaultExpired".into());
    }
    let expected = entry
        .reset_fingerprint
        .as_ref()
        .ok_or("secureVaultDenied")?;
    vault
        .reset_local_locked_vault(expected)
        .map_err(|error| match error {
            VaultServiceError::Vault(VaultError::VaultChanged) => "secureVaultResetChanged",
            VaultServiceError::Vault(VaultError::CommitStateUnknown(_)) => {
                "secureVaultResetUncertain"
            }
            _ => "secureVaultResetFailed",
        })?;
    let entry = pending.remove(&id).ok_or("secureVaultExpired")?;
    let _ = entry.sender.send(Answer::Reset);
    Ok(())
}
#[tauri::command]
pub fn secure_vault_cancel(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureVaultService>,
) -> Result<(), SecureVaultCommandError> {
    secure_vault_cancel_for_label(window.label(), id, &service)
}

pub(crate) fn secure_vault_cancel_for_label(
    surface_label: &str,
    id: String,
    service: &SecureVaultService,
) -> Result<(), SecureVaultCommandError> {
    require_label(surface_label, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureVaultUnavailable")?
        .remove(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::VaultUnlockPolicy;

    fn template(id: &str, kind: SecureVaultKind) -> PromptTemplate {
        PromptTemplate {
            id: id.to_owned(),
            kind,
            owner_label: "main".to_owned(),
            reset_fingerprint: None,
            expires_at: Instant::now() + Duration::from_secs(30),
        }
    }

    fn arm(service: &SecureVaultService, template: &PromptTemplate) -> oneshot::Receiver<Answer> {
        let (sender, receiver) = oneshot::channel();
        service
            .pending
            .lock()
            .expect("pending prompts")
            .insert(template.id.clone(), template.pending(sender));
        receiver
    }

    async fn submit(
        service: &SecureVaultService,
        id: &str,
        password: &str,
        confirmation: &str,
    ) -> Result<(), SecureVaultCommandError> {
        secure_vault_submit_for_label(
            &label(id),
            id.to_owned(),
            Zeroizing::new(password.to_owned()),
            Zeroizing::new(confirmation.to_owned()),
            false,
            service,
        )
        .await
    }

    fn vault_error(error: VaultServiceError) -> Box<CoreApiError> {
        crate::vault_service::map_service_error(RequestId::new(), error)
    }

    #[test]
    fn vault_failures_keep_distinct_codes_and_prompt_outcomes() {
        let cases = [
            (
                VaultServiceError::Vault(VaultError::AuthenticationFailed),
                "vault.authentication_failed",
                "secureWindow.vault.incorrectPassword",
                PromptOutcome::Retry,
            ),
            (
                VaultServiceError::PasswordConfirmationMismatch,
                "vault.password_confirmation_mismatch",
                "secureWindow.vault.confirmationMismatch",
                PromptOutcome::Retry,
            ),
            (
                VaultServiceError::Vault(VaultError::WeakPassword),
                "vault.weak_password",
                "secureWindow.vault.weakPassword",
                PromptOutcome::Retry,
            ),
            (
                VaultServiceError::Vault(VaultError::Locked),
                "vault.in_use",
                "secureWindow.vault.inUse",
                PromptOutcome::Retry,
            ),
            (
                VaultServiceError::Vault(VaultError::ReloadRequired),
                "vault.requires_reload",
                "secureWindow.vault.requiresReload",
                PromptOutcome::Close,
            ),
            (
                VaultServiceError::Vault(VaultError::InsecurePermissions),
                "vault.insecure_permissions",
                "secureWindow.vault.insecurePermissions",
                PromptOutcome::Close,
            ),
            (
                VaultServiceError::AvailabilityChanged,
                "vault.availability_changed",
                "secureWindow.vault.changed",
                PromptOutcome::Close,
            ),
        ];
        for (error, code, message_key, outcome) in cases {
            let (error, actual) = classify_vault_failure(vault_error(error));
            assert_eq!(error.0.code, code);
            assert_eq!(error.0.message_key, message_key);
            assert_eq!(actual, outcome, "{code}");
        }
        let (internal, outcome) = classify_vault_failure(vault_error(VaultServiceError::Vault(
            VaultError::KeyDerivation,
        )));
        assert_eq!(internal.0.code, "core.internal");
        assert!(internal.0.diagnostic_id.is_some());
        assert_eq!(outcome, PromptOutcome::Close);
    }

    /// Regression: a wrong password used to close the prompt with a generic
    /// "new passwords must be 8 characters" failure. It must stay on the same
    /// prompt with a distinct code, keep the Vault locked, and accept a retry.
    #[tokio::test(flavor = "current_thread")]
    async fn wrong_unlock_password_keeps_prompt_open_and_retry_unlocks() {
        let directory = tempfile::tempdir().expect("temporary Vault");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"correct horse battery")
            .expect("create Vault");
        vault.lock_for_tests().expect("lock Vault");
        let service = SecureVaultService::default();
        let template = template("retry", SecureVaultKind::Unlock);
        let receiver = arm(&service, &template);
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let owner = answer_loop(
            &service,
            &template,
            receiver,
            || true,
            || true,
            || true,
            |password, _confirmation| {
                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let result = vault
                    .unlock_for_protected_operation(password.as_bytes())
                    .map(drop)
                    .map_err(vault_error);
                async move { result }
            },
        );
        let user = async {
            let wrong = submit(&service, "retry", "wrong password", "")
                .await
                .expect_err("wrong password is rejected");
            assert_eq!(wrong.0.code, "vault.authentication_failed");
            assert_eq!(wrong.0.message_key, "secureWindow.vault.incorrectPassword");
            assert!(wrong.0.diagnostic_id.is_none());
            assert!(!vault.is_unlocked());
            let prompt = secure_vault_get_for_label(&label("retry"), "retry".to_owned(), &service)
                .expect("the same prompt stays available after a wrong password");
            assert_eq!(prompt.kind, SecureVaultKind::Unlock);
            submit(&service, "retry", "correct horse battery", "")
                .await
                .expect("retry with the correct password unlocks");
        };
        let (end, ()) = tokio::join!(owner, user);
        assert_eq!(end.expect("prompt completes"), PromptEnd::Applied);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(vault.is_unlocked());
        assert!(service.pending.lock().expect("pending").is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn non_retryable_failure_closes_prompt_with_the_same_stable_code() {
        let service = SecureVaultService::default();
        let template = template("close", SecureVaultKind::Unlock);
        let receiver = arm(&service, &template);
        let owner = answer_loop(
            &service,
            &template,
            receiver,
            || true,
            || true,
            || true,
            |_password, _confirmation| async {
                Err(vault_error(VaultServiceError::Vault(
                    VaultError::ReloadRequired,
                )))
            },
        );
        let user = submit(&service, "close", "any password", "");
        let (end, submitted) = tokio::join!(owner, user);
        let window_error = submitted.expect_err("window receives the final error");
        let caller_error = end.expect_err("caller receives the same final error");
        assert_eq!(window_error.0.code, "vault.requires_reload");
        assert_eq!(caller_error.0.code, "vault.requires_reload");
        assert!(service.pending.lock().expect("pending").is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn create_confirmation_mismatch_is_retryable_and_distinct() {
        let directory = tempfile::tempdir().expect("temporary Vault");
        let vault = VaultService::start(directory.path());
        let service = SecureVaultService::default();
        let template = template("create", SecureVaultKind::Create);
        let receiver = arm(&service, &template);
        let owner = answer_loop(
            &service,
            &template,
            receiver,
            || true,
            || true,
            || true,
            |password, confirmation| {
                let result = vault
                    .create_for_protected_operation(password.as_bytes(), confirmation.as_bytes())
                    .map(drop)
                    .map_err(vault_error);
                async move { result }
            },
        );
        let user = async {
            let mismatch = submit(&service, "create", "first password", "second password")
                .await
                .expect_err("mismatch rejected");
            assert_eq!(mismatch.0.code, "vault.password_confirmation_mismatch");
            let weak = submit(&service, "create", "short", "short")
                .await
                .expect_err("weak password rejected");
            assert_eq!(weak.0.code, "vault.weak_password");
            submit(
                &service,
                "create",
                "long enough password",
                "long enough password",
            )
            .await
            .expect("valid create");
        };
        let (end, ()) = tokio::join!(owner, user);
        assert_eq!(end.expect("prompt completes"), PromptEnd::Applied);
        assert!(vault.is_unlocked());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retryable_failure_after_prompt_window_closed_does_not_rearm() {
        let service = SecureVaultService::default();
        let template = template("closed", SecureVaultKind::Unlock);
        let receiver = arm(&service, &template);
        let owner = answer_loop(
            &service,
            &template,
            receiver,
            || true,
            || false,
            || true,
            |_password, _confirmation| async {
                Err(vault_error(VaultServiceError::Vault(
                    VaultError::AuthenticationFailed,
                )))
            },
        );
        let user = submit(&service, "closed", "wrong password", "");
        let (end, submitted) = tokio::join!(owner, user);
        assert_eq!(
            submitted.expect_err("rejected").0.code,
            "vault.authentication_failed"
        );
        assert_eq!(end.expect("cancelled"), PromptEnd::Cancelled);
        assert!(service.pending.lock().expect("pending").is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_while_waiting_reports_cancelled_not_failure() {
        let service = SecureVaultService::default();
        let template = template("cancel", SecureVaultKind::Unlock);
        let receiver = arm(&service, &template);
        let owner = answer_loop(
            &service,
            &template,
            receiver,
            || true,
            || true,
            || true,
            |_password, _confirmation| async { Ok(()) },
        );
        let user = async {
            secure_vault_cancel_for_label(&label("cancel"), "cancel".to_owned(), &service)
                .expect("cancel");
        };
        let (end, ()) = tokio::join!(owner, user);
        assert_eq!(end.expect("cancel is not an error"), PromptEnd::Cancelled);
    }

    #[test]
    fn reset_conflict_and_uncertain_commit_have_distinct_codes() {
        let changed = SecureVaultCommandError::from("secureVaultResetChanged");
        let uncertain = SecureVaultCommandError::from("secureVaultResetUncertain");
        assert_eq!(changed.0.code, "secureVaultResetChanged");
        assert_eq!(uncertain.0.code, "secureVaultResetUncertain");
        assert_ne!(changed.0.message_key, uncertain.0.message_key);
    }

    fn status(state: VaultState) -> VaultStatus {
        VaultStatus {
            state,
            vault_id: None,
            revision: None,
            entry_count: None,
            unlock_policy: VaultUnlockPolicy::CurrentSession,
            auto_unlock_failure: None,
        }
    }

    #[test]
    fn missing_requires_create_and_reload_never_becomes_create() {
        assert!(matches!(
            kind_for(
                SecureVaultMode::EnsureUnlocked,
                &status(VaultState::Missing)
            ),
            Ok(Some(SecureVaultKind::Create))
        ));
        for state in [VaultState::Locked, VaultState::RequiresReload] {
            assert!(matches!(
                kind_for(SecureVaultMode::EnsureUnlocked, &status(state)),
                Ok(Some(SecureVaultKind::Unlock))
            ));
        }
        assert!(matches!(
            kind_for(
                SecureVaultMode::EnsureUnlocked,
                &status(VaultState::Unlocked)
            ),
            Ok(None)
        ));
    }

    #[test]
    fn automatic_unlock_confirmation_accepts_one_password_while_locked() {
        for (mode, kind) in [
            (
                SecureVaultMode::EnableAutoUnlock,
                SecureVaultKind::EnableAutoUnlock,
            ),
            (
                SecureVaultMode::EnableLocalAutoUnlock,
                SecureVaultKind::EnableLocalAutoUnlock,
            ),
        ] {
            assert!(kind_for(mode, &status(VaultState::Missing)).is_err());
            for state in [
                VaultState::Locked,
                VaultState::RequiresReload,
                VaultState::Unlocked,
            ] {
                assert_eq!(kind_for(mode, &status(state)).unwrap(), Some(kind));
            }
        }
    }
}
