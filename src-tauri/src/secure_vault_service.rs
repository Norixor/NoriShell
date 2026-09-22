//! Core-owned Vault prompts. Only the exact isolated window can submit secrets.
use crate::{
    connection_profile::{
        connection_has_vault_credentials, connection_requires_vault,
        resolve_long_lived_connection_profile,
    },
    host_service::HostService,
    ssh_sync_exchange_local::NoriShellSshSyncLocalAdapter,
    vault_service::VaultService,
};
use norishell_core_api::{
    HostId, RequestMeta, VaultAutoUnlockEnableRequest, VaultCreateRequest, VaultState, VaultStatus,
    VaultUnlockRequest, WireSequence,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecureVaultMode {
    EnsureUnlocked,
    EnableAutoUnlock,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecureVaultKind {
    Create,
    Unlock,
    EnableAutoUnlock,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecureVaultPrompt {
    id: String,
    kind: SecureVaultKind,
}
struct Answer {
    password: Zeroizing<String>,
    confirmation: Zeroizing<String>,
    confirmed: bool,
}
struct Pending {
    prompt: SecureVaultPrompt,
    sender: oneshot::Sender<Answer>,
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
        SecureVaultMode::EnsureUnlocked => Ok(match status.state {
            VaultState::Missing => Some(SecureVaultKind::Create),
            VaultState::Locked | VaultState::RequiresReload => Some(SecureVaultKind::Unlock),
            VaultState::Unlocked => None,
        }),
        SecureVaultMode::EnableAutoUnlock if status.state == VaultState::Unlocked => {
            Ok(Some(SecureVaultKind::EnableAutoUnlock))
        }
        SecureVaultMode::EnableAutoUnlock => Err("secureVaultStateChanged".into()),
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
) -> Result<bool, String> {
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
) -> Result<bool, String> {
    if !allowed_caller(&app, window.label()) {
        return Err("secureVaultDenied".into());
    }
    let initial = vault.status();
    let Some(kind) = kind_for(mode, &initial)? else {
        return Ok(true);
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
                },
                sender,
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
        SecureVaultKind::Unlock => 320.0,
        SecureVaultKind::Create => 400.0,
        SecureVaultKind::EnableAutoUnlock => 420.0,
    };
    let child = crate::secure_window_frame::apply_secure_window_frame(
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
    let _ = child.set_focus();
    // Expiry and owner teardown cancel only pending interaction, never an accepted mutation.
    let mut receiver = receiver;
    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);
    let mut owner_check = tokio::time::interval(Duration::from_millis(250));
    let owner_valid =
        || app.get_webview_window(window.label()).is_some() && allowed_caller(&app, window.label());
    let mut answer = loop {
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
        SecureVaultKind::EnableAutoUnlock => {
            if !answer.confirmed {
                let mut password = password;
                password.zeroize();
                return Err("secureVaultDenied".into());
            }
            crate::vault_service::vault_auto_unlock_enable(
                VaultAutoUnlockEnableRequest { meta, password },
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
) -> Result<SecureVaultPrompt, String> {
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
) -> Result<(), String> {
    let answer = Answer {
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
        .send(answer)
        .map_err(|_| "secureVaultExpired".into())
}
#[tauri::command]
pub fn secure_vault_cancel(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureVaultService>,
) -> Result<(), String> {
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
    fn automatic_unlock_confirmation_requires_an_unlocked_vault() {
        for state in [
            VaultState::Missing,
            VaultState::Locked,
            VaultState::RequiresReload,
        ] {
            assert!(kind_for(SecureVaultMode::EnableAutoUnlock, &status(state)).is_err());
        }
        assert!(matches!(
            kind_for(
                SecureVaultMode::EnableAutoUnlock,
                &status(VaultState::Unlocked)
            ),
            Ok(Some(SecureVaultKind::EnableAutoUnlock))
        ));
    }
}
