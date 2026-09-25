//! Each authentication decision is bound to a one-shot Core-created secure window; the main WebView cannot submit it.
use norishell_core_api::{DesktopPrompt, DesktopPromptDecision, DesktopPromptKind};
use norishell_desktop_protocol::{EngineError, Result};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::oneshot;
use uuid::Uuid;
use zeroize::Zeroize;

struct Pending {
    prompt: DesktopPrompt,
    response: oneshot::Sender<DesktopPromptDecision>,
}
#[derive(Clone, Default)]
pub(super) struct Prompts(Arc<Mutex<BTreeMap<String, Pending>>>, Arc<AtomicUsize>);

pub(super) struct InteractionGuard(Arc<AtomicUsize>);
impl Drop for InteractionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Prompts {
    pub fn has_pending(&self) -> bool {
        self.1.load(Ordering::SeqCst) > 0
    }
    pub fn block_input(&self) -> InteractionGuard {
        self.1.fetch_add(1, Ordering::SeqCst);
        InteractionGuard(self.1.clone())
    }
    pub fn get(&self, window: &WebviewWindow, id: &str) -> Result<DesktopPrompt> {
        if window.label() != label(id) {
            return Err(EngineError::InvalidConfiguration);
        }
        self.0
            .lock()
            .map_err(|_| EngineError::Protocol)?
            .get(id)
            .map(|pending| pending.prompt.clone())
            .ok_or(EngineError::Cancelled)
    }

    pub fn decide(
        &self,
        window: &WebviewWindow,
        mut decision: DesktopPromptDecision,
    ) -> Result<()> {
        let result = (|| {
            if window.label() != label(&decision.id)
                || decision
                    .password
                    .as_ref()
                    .is_some_and(|value| value.len() > 65_536)
                || decision
                    .password_confirmation
                    .as_ref()
                    .is_some_and(|value| value.len() > 65_536)
                || decision
                    .username
                    .as_ref()
                    .is_some_and(|value| value.len() > 256)
                || decision
                    .domain
                    .as_ref()
                    .is_some_and(|value| value.len() > 256)
                || decision.answers.len() > 32
                || decision.answers.iter().map(String::len).sum::<usize>() > 65_536
            {
                return Err(EngineError::InvalidConfiguration);
            }
            let pending = self
                .0
                .lock()
                .map_err(|_| EngineError::Protocol)?
                .remove(&decision.id)
                .ok_or(EngineError::Cancelled)?;
            validate_vault_decision(&pending.prompt.prompt, &decision)?;
            let reply = DesktopPromptDecision {
                id: decision.id.clone(),
                approved: decision.approved,
                username: decision.username.take(),
                domain: decision.domain.take(),
                password: decision.password.take(),
                password_confirmation: decision.password_confirmation.take(),
                answers: std::mem::take(&mut decision.answers),
            };
            if let Err(mut rejected) = pending.response.send(reply) {
                clear_decision(&mut rejected);
                return Err(EngineError::Cancelled);
            }
            Ok(())
        })();
        clear_decision(&mut decision);
        result
    }

    pub async fn request(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_label: &str,
        prompt: DesktopPromptKind,
    ) -> Result<DesktopPromptDecision> {
        let id = Uuid::now_v7().to_string();
        let (response, receiver) = oneshot::channel();
        self.0.lock().map_err(|_| EngineError::Protocol)?.insert(
            id.clone(),
            Pending {
                prompt: DesktopPrompt {
                    id: id.clone(),
                    session_id: session_id.to_owned(),
                    label: session_label.to_owned(),
                    prompt,
                },
                response,
            },
        );
        let _guard = PromptGuard {
            prompts: self.clone(),
            app: app.clone(),
            id: id.clone(),
        };
        let url = format!("secure-desktop.html?prompt={id}");
        let builder = WebviewWindowBuilder::new(app, label(&id), WebviewUrl::App(url.into()))
            .title("NoriShell");
        let window =
            crate::secure_window_frame::apply_secure_window_frame(app, &label(&id), builder)
                .build()
                .map_err(|_| EngineError::Protocol)?;
        let pending = self.clone();
        let pending_id = id.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed)
                && let Ok(mut prompts) = pending.0.lock()
            {
                prompts.remove(&pending_id);
            }
        });
        let _ = crate::window_first_show::focus_if_revealed(&window);
        let mut decision = tokio::time::timeout(Duration::from_secs(180), receiver)
            .await
            .map_err(|_| EngineError::Timeout)?
            .map_err(|_| EngineError::Cancelled)?;
        if !decision.approved {
            clear_decision(&mut decision);
            return Err(EngineError::Cancelled);
        }
        Ok(decision)
    }

    pub fn cancel_session(&self, session_id: &str) {
        if let Ok(mut prompts) = self.0.lock() {
            prompts.retain(|_, pending| pending.prompt.session_id != session_id);
        }
    }
}

pub(super) fn clear_decision(decision: &mut DesktopPromptDecision) {
    if let Some(password) = &mut decision.password {
        password.zeroize();
    }
    if let Some(confirmation) = &mut decision.password_confirmation {
        confirmation.zeroize();
    }
    for answer in &mut decision.answers {
        answer.zeroize();
    }
}

fn validate_vault_decision(
    kind: &DesktopPromptKind,
    decision: &DesktopPromptDecision,
) -> Result<()> {
    if !decision.approved {
        return Ok(());
    }
    if matches!(kind, DesktopPromptKind::VaultCreate) {
        let valid = decision
            .password
            .as_ref()
            .zip(decision.password_confirmation.as_ref())
            .is_some_and(|(password, confirmation)| {
                password.chars().count() >= 8
                    && password.len() <= 65_536
                    && crate::vault_service::confirmations_match(
                        password.as_bytes(),
                        confirmation.as_bytes(),
                    )
            });
        if !valid {
            return Err(EngineError::InvalidConfiguration);
        }
    } else if decision.password_confirmation.is_some() {
        return Err(EngineError::InvalidConfiguration);
    }
    Ok(())
}

fn label(id: &str) -> String {
    format!("desktop-auth-{id}")
}
struct PromptGuard {
    prompts: Prompts,
    app: AppHandle,
    id: String,
}
impl Drop for PromptGuard {
    fn drop(&mut self) {
        if let Ok(mut prompts) = self.prompts.0.lock() {
            prompts.remove(&self.id);
        }
        if let Some(window) = self.app.get_webview_window(&label(&self.id)) {
            let _ = window.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_creation_requires_matching_confirmation_and_clears_both_secrets() {
        let mut decision = DesktopPromptDecision {
            id: "test".into(),
            approved: true,
            username: None,
            domain: None,
            password: Some("long-vault-password".into()),
            password_confirmation: None,
            answers: vec![],
        };
        assert!(validate_vault_decision(&DesktopPromptKind::VaultCreate, &decision).is_err());
        decision.password_confirmation = Some("wrong-vault-password".into());
        assert!(validate_vault_decision(&DesktopPromptKind::VaultCreate, &decision).is_err());
        decision.password_confirmation = decision.password.clone();
        assert!(validate_vault_decision(&DesktopPromptKind::VaultCreate, &decision).is_ok());
        assert!(validate_vault_decision(&DesktopPromptKind::VaultUnlock, &decision).is_err());
        clear_decision(&mut decision);
        assert_eq!(decision.password.as_deref(), Some(""));
        assert_eq!(decision.password_confirmation.as_deref(), Some(""));
    }

    #[test]
    fn overlapping_interactions_keep_input_blocked_until_every_guard_ends() {
        let prompts = Prompts::default();
        let first = prompts.block_input();
        let second = prompts.block_input();
        assert!(prompts.has_pending());
        drop(first);
        assert!(prompts.has_pending());
        drop(second);
        assert!(!prompts.has_pending());
    }
}
