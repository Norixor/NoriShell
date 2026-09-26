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
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
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
#[derive(Debug, Serialize)]
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
            "secureVaultFailed" => (
                ErrorCategory::Internal,
                RetryStrategy::Never,
                "sshHosts.vault.failed",
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
        if matches!(value, "secureVaultFailed" | "secureVaultResetFailed") {
            let diagnostic_id = uuid::Uuid::new_v4().to_string();
            eprintln!("secure Vault operation failed: diagnostic_id={diagnostic_id} code={value}");
            error.diagnostic_id = Some(diagnostic_id);
        }
        Self(error)
    }
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
fn require_window(window: &WebviewWindow, id: &str) -> Result<(), String> {
    if window.label() != label(id) {
        return Err("secureVaultDenied".into());
    }
    Ok(())
}

fn allowed_caller(app: &AppHandle, label: &str) -> bool {
    label == "main"
        || app
            .state::<crate::tool_windows::ToolWindows>()
            .is_editor(label)
        || app
            .state::<crate::secure_credential_service::SecureCredentialService>()
            .is_prompt_window(label)
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
        if let Some(window) = self.app.get_webview_window(&label(&self.id)) {
            let _ = window.destroy();
        }
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
/// foreground Vault prompt. Only main may request this continuation: resource
/// factories must report VaultLocked instead of opening an interactive window.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn secure_vault_ensure_for_host(
    host_id: HostId,
    expected_host_state_version: WireSequence,
    include_optional_credentials: Option<bool>,
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, SecureVaultService>,
    vault: State<'_, VaultService>,
    ssh_sync: State<'_, NoriShellSshSyncLocalAdapter>,
    hosts: State<'_, HostService>,
) -> Result<bool, SecureVaultCommandError> {
    if window.label() != "main" {
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
        window,
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
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, SecureVaultService>,
    vault: State<'_, VaultService>,
    ssh_sync: State<'_, NoriShellSshSyncLocalAdapter>,
) -> Result<bool, SecureVaultCommandError> {
    if !allowed_caller(&app, window.label()) {
        return Err("secureVaultDenied".into());
    }
    let mode = if matches!(mode, SecureVaultMode::UnlockSavedLocal) {
        if window.label() != "main" {
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
                owner_label: window.label().to_owned(),
                reset_fingerprint,
                expires_at: Instant::now() + Duration::from_secs(180),
            },
        );
    let _guard = PromptGuard {
        app: app.clone(),
        id: id.clone(),
    };
    let prompt_path = format!("secure-vault.html?prompt={id}");
    let expected = app
        .config()
        .build
        .dev_url
        .as_ref()
        .and_then(|url| url.join(&prompt_path).ok());
    let expected_query = format!("prompt={id}");
    let window_height = match kind {
        SecureVaultKind::Unlock => 380.0,
        SecureVaultKind::Create => 460.0,
        SecureVaultKind::EnableAutoUnlock => 500.0,
        SecureVaultKind::EnableLocalAutoUnlock => 540.0,
    };
    let child = crate::secure_window_frame::apply_secure_window_frame(
        &app,
        &label(&id),
        WebviewWindowBuilder::new(
            &app,
            label(&id),
            WebviewUrl::App(format!("secure-vault.html?prompt={id}").into()),
        )
        .title("NoriShell"),
    )
    .inner_size(600.0, window_height)
    .min_inner_size(480.0, 300.0)
    .on_navigation(move |url| {
        (cfg!(debug_assertions) && expected.as_ref().is_some_and(|expected| expected == url))
            || (((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                || (matches!(url.scheme(), "http" | "https")
                    && url.host_str() == Some("tauri.localhost")
                    && url.port().is_none()))
                && url.path() == "/secure-vault.html"
                && url.query() == Some(expected_query.as_str()))
    })
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .build()
    .map_err(|_| "secureVaultUnavailable")?;
    let cancel_app = app.clone();
    let cancel_id = id.clone();
    child.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed)
            && let Ok(mut pending) = cancel_app.state::<SecureVaultService>().pending.lock()
        {
            pending.remove(&cancel_id);
        }
    });
    let _ = crate::window_first_show::focus_if_revealed(&child);
    // Expiry and owner teardown cancel only pending interaction, never an accepted mutation.
    let mut receiver = receiver;
    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);
    let mut owner_check = tokio::time::interval(Duration::from_millis(250));
    let owner_valid =
        || app.get_webview_window(window.label()).is_some() && allowed_caller(&app, window.label());
    let answer = loop {
        tokio::select! {
            response = &mut receiver => {
                let Ok(answer) = response else { return Ok(false); };
                break answer;
            }
            _ = &mut deadline => return Ok(false),
            _ = owner_check.tick() => {
                if !owner_valid() { return Ok(false); }
            }
        }
    };
    if !owner_valid() {
        return Ok(false);
    }
    let Answer::Password(mut answer) = answer else {
        let _ = app.emit_to("main", "native-tray-vault-changed", ());
        return Ok(false);
    };
    if vault.status() != initial {
        return Err("secureVaultStateChanged".into());
    }
    let meta = RequestMeta {
        request_id: norishell_core_api::RequestId::new(),
    };
    let password = std::mem::take(&mut *answer.password);
    let result = match kind {
        SecureVaultKind::Create => crate::vault_service::vault_create(
            VaultCreateRequest {
                meta,
                password,
                password_confirmation: std::mem::take(&mut *answer.confirmation),
            },
            vault,
        ),
        SecureVaultKind::Unlock => {
            crate::vault_service::vault_unlock(
                VaultUnlockRequest { meta, password },
                vault,
                ssh_sync,
            )
            .await
        }
        SecureVaultKind::EnableAutoUnlock | SecureVaultKind::EnableLocalAutoUnlock => {
            if !answer.confirmed {
                let mut password = password;
                password.zeroize();
                return Err("secureVaultDenied".into());
            }
            crate::vault_service::vault_auto_unlock_enable(
                VaultAutoUnlockEnableRequest {
                    meta,
                    password,
                    policy: (kind == SecureVaultKind::EnableLocalAutoUnlock)
                        .then_some(VaultUnlockPolicy::AutomaticLocal),
                },
                vault,
            )
        }
    };
    result.map_err(|_| "secureVaultFailed".to_owned())?;
    let _ = app.emit_to("main", "native-tray-vault-changed", ());
    Ok(true)
}
#[tauri::command]
pub fn secure_vault_get(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureVaultService>,
) -> Result<SecureVaultPrompt, SecureVaultCommandError> {
    require_window(&window, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureVaultUnavailable")?
        .get(&id)
        .map(|entry| entry.prompt.clone())
        .ok_or_else(|| "secureVaultExpired".into())
}
#[tauri::command]
pub fn secure_vault_submit(
    id: String,
    password: String,
    password_confirmation: String,
    confirmed: bool,
    window: WebviewWindow,
    service: State<'_, SecureVaultService>,
) -> Result<(), SecureVaultCommandError> {
    let answer = PasswordAnswer {
        password: Zeroizing::new(password),
        confirmation: Zeroizing::new(password_confirmation),
        confirmed,
    };
    require_window(&window, &id)?;
    if answer.password.is_empty()
        || answer.password.len() > 65_536
        || answer.confirmation.len() > 65_536
    {
        return Err("secureVaultDenied".into());
    }
    let pending = service
        .pending
        .lock()
        .map_err(|_| "secureVaultUnavailable")?
        .remove(&id)
        .ok_or("secureVaultExpired")?;
    pending
        .sender
        .send(Answer::Password(answer))
        .map_err(|_| "secureVaultExpired".into())
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
    require_window(&window, &id)?;
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
        || app.get_webview_window(&entry.owner_label).is_none()
        || !allowed_caller(&app, &entry.owner_label)
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
    require_window(&window, &id)?;
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
