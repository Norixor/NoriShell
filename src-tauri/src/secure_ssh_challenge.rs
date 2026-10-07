//! Isolated SSH identity and keyboard-interactive decisions, fenced by the owning actors.
use crate::{metrics_session_service as metrics, ssh_session_service as ssh};
use norishell_core_api::*;
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::ObservedHostKey;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChallengeTarget {
    SshHostKey {
        request: SshHostKeyDecisionRequest,
    },
    SshKeyboard {
        request: SshKeyboardInteractiveResponseRequest,
    },
    MetricsKeyboard {
        request: MetricsKeyboardInteractiveRespondRequest,
    },
}
#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "challenge", rename_all = "camelCase")]
pub enum ChallengeContent {
    SshHostKey(SshHostKeyChallenge),
    SftpHostKey(SftpHostKeyChallenge),
    SshKeyboard(SshKeyboardInteractiveChallenge),
    MetricsKeyboard(MetricsKeyboardInteractiveChallenge),
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpHostKeyChallenge {
    endpoint: String,
    algorithm: String,
    fingerprint_sha256: String,
}
#[derive(Clone, Serialize)]
pub struct ChallengePrompt {
    id: String,
    content: ChallengeContent,
}
/// Error code of a host-key acceptance whose trust write failed. Core kept the
/// challenge, so the same protected window offers the decision again.
const TRUST_FAILED: &str = "secureChallengeTrustFailed";

type Verdict = oneshot::Sender<Result<(), String>>;

struct Answer {
    approved: bool,
    answers: Vec<Zeroizing<String>>,
    /// Resolves the window's submit: `Ok` when applied, a retryable error
    /// while the same window stays open, or the final error.
    verdict: Option<Verdict>,
}

fn reply(verdict: Option<Verdict>, result: Result<(), String>) {
    if let Some(verdict) = verdict {
        let _ = verdict.send(result);
    }
}
struct Pending {
    prompt: ChallengePrompt,
    key: String,
    sender: Option<oneshot::Sender<Answer>>,
}
#[derive(Default)]
pub struct SecureSshChallengeService {
    pending: Mutex<BTreeMap<String, Pending>>,
}
fn meta() -> RequestMeta {
    RequestMeta {
        request_id: RequestId::new(),
    }
}
fn label(id: &str) -> String {
    format!("secure-ssh-challenge-{id}")
}
fn require_label(surface_label: &str, id: &str) -> Result<(), String> {
    if surface_label != label(id) {
        return Err("secureChallengeDenied".into());
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
impl ChallengeTarget {
    fn key(&self) -> String {
        match self {
            Self::SshHostKey { request } => format!("ssh-host:{}", request.challenge_id),
            Self::SshKeyboard { request } => format!("ssh-kbi:{}", request.challenge_id),
            Self::MetricsKeyboard { request } => format!("metrics-kbi:{}", request.challenge_id),
        }
    }
}
async fn content(app: &AppHandle, target: &ChallengeTarget) -> Result<ChallengeContent, String> {
    let expired = || "secureChallengeExpired".to_owned();
    match target {
        ChallengeTarget::SshHostKey { request } => {
            let details = ssh::ssh_terminal_get(
                SshSessionGetRequest {
                    meta: meta(),
                    session_id: request.session_id.clone(),
                },
                app.state(),
            )
            .await
            .map_err(|_| expired())?;
            if !details.attachments.iter().any(|a| {
                a.attachment_id == request.attachment_id
                    && a.view_id == request.view_id
                    && a.generation == request.expected_generation
            }) {
                return Err(expired());
            }
            let challenge = details.active_host_key_challenge.ok_or_else(expired)?;
            if challenge.challenge_id != request.challenge_id
                || challenge.generation != request.expected_generation
                || challenge.state_revision != request.expected_state_revision
            {
                return Err(expired());
            }
            Ok(ChallengeContent::SshHostKey(challenge))
        }
        ChallengeTarget::SshKeyboard { request } => {
            if !request.answers.is_empty() {
                return Err("secureChallengeDenied".into());
            }
            let details = ssh::ssh_terminal_get(
                SshSessionGetRequest {
                    meta: meta(),
                    session_id: request.session_id.clone(),
                },
                app.state(),
            )
            .await
            .map_err(|_| expired())?;
            if !details.attachments.iter().any(|a| {
                a.attachment_id == request.attachment_id
                    && a.view_id == request.view_id
                    && a.generation == request.expected_generation
            }) {
                return Err(expired());
            }
            let challenge = details
                .active_keyboard_interactive_challenge
                .ok_or_else(expired)?;
            if challenge.challenge_id != request.challenge_id
                || challenge.generation != request.expected_generation
                || challenge.state_revision != request.expected_state_revision
                || challenge.round_index != request.round_index
                || challenge.expires_at_unix_ms <= crate::time::unix_time_ms()
            {
                return Err(expired());
            }
            Ok(ChallengeContent::SshKeyboard(challenge))
        }
        ChallengeTarget::MetricsKeyboard { request } => {
            if !request.answer_ref_ids.is_empty() {
                return Err("secureChallengeDenied".into());
            }
            let sessions = app
                .state::<metrics::MetricsSessionService>()
                .summaries(RequestId::new())
                .await
                .map_err(|_| expired())?;
            let challenge = sessions
                .into_iter()
                .find(|s| {
                    s.metrics_session_id == request.metrics_session_id
                        && s.host_id == request.host_id
                        && s.generation == request.expected_generation
                })
                .and_then(|s| s.keyboard_interactive_challenge)
                .ok_or_else(expired)?;
            if challenge.challenge_id != request.challenge_id
                || challenge.round_index != request.round_index
                || challenge.expires_at_unix_ms <= crate::time::unix_time_ms()
            {
                return Err(expired());
            }
            Ok(ChallengeContent::MetricsKeyboard(challenge))
        }
    }
}
async fn apply(app: &AppHandle, target: ChallengeTarget, mut answer: Answer) -> Result<(), String> {
    let current = content(app, &target).await?;
    let failed = |_| "secureChallengeFailed".to_owned();
    match target {
        ChallengeTarget::SshHostKey { mut request } => {
            request.decision = if answer.approved {
                SshHostKeyDecision::AcceptAndStore
            } else {
                SshHostKeyDecision::Reject
            };
            ssh::ssh_terminal_host_key_decide(request, app.state())
                .await
                .map_err(|error| {
                    if error.code == "ssh_terminal.host_key_trust_failed" {
                        TRUST_FAILED.to_owned()
                    } else {
                        "secureChallengeFailed".to_owned()
                    }
                })?;
        }
        ChallengeTarget::SshKeyboard { mut request } => {
            let ChallengeContent::SshKeyboard(challenge) = current else {
                return Err("secureChallengeDenied".into());
            };
            if !answer.approved || answer.answers.len() != challenge.prompts.len() {
                return Err("secureChallengeDenied".into());
            }
            for (prompt, value) in challenge.prompts.iter().zip(answer.answers.iter_mut()) {
                let prepared = ssh::ssh_terminal_keyboard_interactive_answer_prepare(
                    SshKeyboardInteractiveAnswerPrepareRequest {
                        meta: meta(),
                        session_id: request.session_id.clone(),
                        expected_generation: request.expected_generation,
                        challenge_id: request.challenge_id.clone(),
                        expected_state_revision: request.expected_state_revision,
                        round_index: request.round_index,
                        prompt_index: prompt.prompt_index,
                        attachment_id: request.attachment_id.clone(),
                        view_id: request.view_id.clone(),
                        answer: std::mem::take(&mut **value),
                    },
                    app.state(),
                )
                .await
                .map_err(failed)?;
                request
                    .answers
                    .push(SshKeyboardInteractiveAnswerInput::OneTimeAnswerRef {
                        prompt_index: prompt.prompt_index,
                        answer_ref_id: prepared.answer_ref_id,
                    });
            }
            ssh::ssh_terminal_keyboard_interactive_respond(request, app.state())
                .await
                .map_err(failed)?;
        }
        ChallengeTarget::MetricsKeyboard { mut request } => {
            let ChallengeContent::MetricsKeyboard(challenge) = current else {
                return Err("secureChallengeDenied".into());
            };
            if !answer.approved || answer.answers.len() != challenge.prompts.len() {
                return Err("secureChallengeDenied".into());
            }
            for (prompt, value) in challenge.prompts.iter().zip(answer.answers.iter_mut()) {
                let prepared = metrics::metrics_keyboard_interactive_answer_prepare(
                    MetricsKeyboardInteractiveAnswerPrepareRequest {
                        meta: meta(),
                        answer_ref_id: MetricsKeyboardInteractiveAnswerRefId::new(),
                        metrics_session_id: request.metrics_session_id.clone(),
                        expected_generation: request.expected_generation,
                        challenge_id: request.challenge_id.clone(),
                        round_index: request.round_index,
                        prompt_index: prompt.prompt_index,
                        value: std::mem::take(&mut **value),
                    },
                    app.state(),
                )
                .await
                .map_err(failed)?;
                request.answer_ref_ids.push(prepared.answer_ref_id);
            }
            metrics::metrics_keyboard_interactive_respond(request, app.state())
                .await
                .map_err(failed)?;
        }
    }
    Ok(())
}
async fn cancel_target(app: &AppHandle, target: ChallengeTarget) -> Result<(), String> {
    // Attachment teardown must not orphan authentication, but must never stop a replacement challenge.
    let ssh_target = match &target {
        ChallengeTarget::SshHostKey { request } => {
            Some((request.session_id.clone(), request.expected_generation))
        }
        ChallengeTarget::SshKeyboard { request } => {
            Some((request.session_id.clone(), request.expected_generation))
        }
        _ => None,
    };
    if let Some((session_id, generation)) = ssh_target {
        let details = ssh::ssh_terminal_get(
            SshSessionGetRequest {
                meta: meta(),
                session_id: session_id.clone(),
            },
            app.state(),
        )
        .await
        .map_err(|_| "secureChallengeCancelFailed".to_owned())?;
        let exact = details.session.generation == generation
            && match &target {
                ChallengeTarget::SshHostKey { request } => {
                    details.active_host_key_challenge.as_ref().is_some_and(|c| {
                        c.challenge_id == request.challenge_id
                            && c.generation == generation
                            && c.state_revision == request.expected_state_revision
                    })
                }
                ChallengeTarget::SshKeyboard { request } => details
                    .active_keyboard_interactive_challenge
                    .as_ref()
                    .is_some_and(|c| {
                        c.challenge_id == request.challenge_id
                            && c.generation == generation
                            && c.state_revision == request.expected_state_revision
                            && c.round_index == request.round_index
                    }),
                _ => false,
            };
        if !exact {
            return Ok(());
        }
        if matches!(target, ChallengeTarget::SshHostKey { .. })
            && content(app, &target).await.is_ok()
        {
            return apply(
                app,
                target,
                Answer {
                    approved: false,
                    answers: vec![],
                    verdict: None,
                },
            )
            .await;
        }
        ssh::ssh_terminal_disconnect(
            SshSessionDisconnectRequest {
                meta: meta(),
                operation_id: OperationId::new(),
                idempotency_key: uuid::Uuid::now_v7().to_string(),
                session_id,
                expected_generation: generation,
                expected_state_revision: details.session.state_revision,
            },
            app.state(),
        )
        .await
        .map_err(|_| "secureChallengeCancelFailed".to_owned())?;
        return Ok(());
    }
    if content(app, &target).await.is_err() {
        return Ok(());
    }
    match target {
        ChallengeTarget::MetricsKeyboard { request } => {
            app.state::<metrics::MetricsSessionService>()
                .cancel_keyboard_challenge(request)
                .await
                .map_err(|_| "secureChallengeCancelFailed".to_owned())?;
        }
        _ => {
            apply(
                app,
                target,
                Answer {
                    approved: false,
                    answers: vec![],
                    verdict: None,
                },
            )
            .await?;
        }
    }
    Ok(())
}

struct Guard {
    app: AppHandle,
    id: String,
    target: Option<ChallengeTarget>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.app.state::<SecureSshChallengeService>().pending.lock() {
            pending.remove(&self.id);
        }
        crate::secure_surface::destroy(&self.app, &label(&self.id));
        if let Some(target) = self.target.take() {
            let app = self.app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = cancel_target(&app, target).await;
            });
        }
    }
}

fn open_challenge_window(
    app: &AppHandle,
    id: &str,
) -> Result<crate::secure_surface::SurfaceHandle, String> {
    let child = crate::secure_surface::open(
        app,
        crate::secure_surface::SurfaceSpec::new(
            crate::secure_surface::SurfaceKind::SshChallenge,
            label(id),
        )
        .query(format!("prompt={id}"))
        .navigation_guard(),
    )
    .map_err(|_| "secureChallengeUnavailable".to_owned())?;
    let cancel_app = app.clone();
    let cancel_id = id.to_owned();
    child.on_destroyed(move || {
        if let Ok(mut pending) = cancel_app
            .state::<SecureSshChallengeService>()
            .pending
            .lock()
        {
            pending.remove(&cancel_id);
        }
    });
    let _ = child.focus();
    Ok(child)
}

pub(crate) async fn prompt_sftp_host_key(
    app: &AppHandle,
    owner_label: &str,
    operation_id: &str,
    endpoint: &Endpoint,
    observed: &ObservedHostKey,
) -> bool {
    if !ordinary_owner_alive(app, owner_label) {
        return false;
    }
    let id = uuid::Uuid::now_v7().to_string();
    let (sender, mut receiver) = oneshot::channel();
    let prompt = ChallengePrompt {
        id: id.clone(),
        content: ChallengeContent::SftpHostKey(SftpHostKeyChallenge {
            endpoint: format!("{}:{}", endpoint.normalized_address(), endpoint.port()),
            algorithm: observed.algorithm.clone(),
            fingerprint_sha256: observed.fingerprint_sha256.clone(),
        }),
    };
    {
        let service = app.state::<SecureSshChallengeService>();
        let Ok(mut pending) = service.pending.lock() else {
            return false;
        };
        let key = format!("sftp-host:{operation_id}");
        if pending.values().any(|entry| entry.key == key) {
            return false;
        }
        pending.insert(
            id.clone(),
            Pending {
                prompt,
                key,
                sender: Some(sender),
            },
        );
    }
    let _guard = Guard {
        app: app.clone(),
        id: id.clone(),
        target: None,
    };
    if open_challenge_window(app, &id).is_err() {
        return false;
    }
    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            value = &mut receiver => break value.is_ok_and(|answer| answer.approved && answer.answers.is_empty() && ordinary_owner_alive(app, owner_label)),
            _ = &mut deadline => break false,
            _ = tick.tick() => {
                if !ordinary_owner_alive(app, owner_label) || !crate::secure_surface::exists(app, &label(&id)) {
                    break false;
                }
            }
        }
    }
}

#[tauri::command]
pub async fn secure_ssh_challenge_open(
    target: ChallengeTarget,
    webview: tauri::Webview,
    app: AppHandle,
    service: State<'_, SecureSshChallengeService>,
) -> Result<bool, String> {
    let owner = crate::workspace_tab_views::ordinary_owner(&app, &webview)
        .map_err(|_| "secureChallengeDenied")?;
    if !ordinary_owner_alive(&app, &owner) {
        return Err("secureChallengeDenied".into());
    }
    let projection = content(&app, &target).await?;
    if !ordinary_owner_alive(&app, &owner)
        || crate::workspace_tab_views::ordinary_owner(&app, &webview).as_deref()
            != Ok(owner.as_str())
    {
        return Err("secureChallengeDenied".into());
    }
    let id = uuid::Uuid::now_v7().to_string();
    let (sender, mut receiver) = oneshot::channel();
    {
        let mut pending = service
            .pending
            .lock()
            .map_err(|_| "secureChallengeUnavailable")?;
        if pending.values().any(|entry| entry.key == target.key()) {
            return Err("secureChallengeAlreadyOpen".into());
        }
        pending.insert(
            id.clone(),
            Pending {
                prompt: ChallengePrompt {
                    id: id.clone(),
                    content: projection,
                },
                key: target.key(),
                sender: Some(sender),
            },
        );
    }
    let mut guard = Guard {
        app: app.clone(),
        id: id.clone(),
        target: Some(target.clone()),
    };
    let _child = open_challenge_window(&app, &id)?;
    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let owner_valid = || {
        ordinary_owner_alive(&app, &owner)
            && crate::workspace_tab_views::ordinary_owner(&app, &webview).as_deref()
                == Ok(owner.as_str())
    };
    loop {
        let answer = loop {
            tokio::select! {
                value = &mut receiver => break value.ok(),
                _ = &mut deadline => break None,
                _ = tick.tick() => { if !owner_valid() || content(&app, &target).await.is_err() { break None; } }
            }
        };
        let Some(mut answer) = answer.filter(|answer| answer.approved && owner_valid()) else {
            cancel_target(&app, target).await?;
            guard.target = None;
            return Ok(false);
        };
        let verdict = answer.verdict.take();
        match apply(&app, target.clone(), answer).await {
            Ok(()) => {
                reply(verdict, Ok(()));
                guard.target = None;
                return Ok(true);
            }
            // Only a failed trust write keeps the prompt: Core retained the
            // challenge, so the user can accept again or reject explicitly.
            Err(code) if code == TRUST_FAILED && rearm(&service, &id, &mut receiver) => {
                reply(verdict, Err(code));
            }
            Err(code) => {
                reply(verdict, Err(code.clone()));
                return Err(code);
            }
        }
    }
}

/// Gives the pending prompt a fresh answer channel for another attempt.
fn rearm(
    service: &SecureSshChallengeService,
    id: &str,
    receiver: &mut oneshot::Receiver<Answer>,
) -> bool {
    let Ok(mut pending) = service.pending.lock() else {
        return false;
    };
    let Some(entry) = pending.get_mut(id) else {
        return false;
    };
    let (sender, next) = oneshot::channel();
    entry.sender = Some(sender);
    *receiver = next;
    true
}
#[tauri::command]
pub fn secure_ssh_challenge_get(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureSshChallengeService>,
) -> Result<ChallengePrompt, String> {
    secure_ssh_challenge_get_for_label(window.label(), id, &service)
}

pub(crate) fn secure_ssh_challenge_get_for_label(
    surface_label: &str,
    id: String,
    service: &SecureSshChallengeService,
) -> Result<ChallengePrompt, String> {
    require_label(surface_label, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureChallengeUnavailable")?
        .get(&id)
        .map(|p| p.prompt.clone())
        .ok_or_else(|| "secureChallengeExpired".into())
}
#[tauri::command]
pub async fn secure_ssh_challenge_submit(
    id: String,
    approved: bool,
    answers: Vec<String>,
    window: WebviewWindow,
    service: State<'_, SecureSshChallengeService>,
) -> Result<(), String> {
    secure_ssh_challenge_submit_for_label(
        window.label(),
        id,
        approved,
        answers.into_iter().map(Zeroizing::new).collect(),
        &service,
    )
    .await
}

/// Hands the decision to the waiting prompt owner and resolves with its
/// verdict. A dropped verdict means the prompt ended (its window closes).
pub(crate) async fn secure_ssh_challenge_submit_for_label(
    surface_label: &str,
    id: String,
    approved: bool,
    answers: Vec<Zeroizing<String>>,
    service: &SecureSshChallengeService,
) -> Result<(), String> {
    let (verdict, verdict_receiver) = oneshot::channel();
    let answer = Answer {
        approved,
        answers,
        verdict: Some(verdict),
    };
    require_label(surface_label, &id)?;
    if answer.answers.len() > 32 || answer.answers.iter().map(|v| v.len()).sum::<usize>() > 65_536 {
        return Err("secureChallengeDenied".into());
    }
    let sender = service
        .pending
        .lock()
        .map_err(|_| "secureChallengeUnavailable")?
        .get_mut(&id)
        .and_then(|pending| pending.sender.take())
        .ok_or("secureChallengeExpired")?;
    sender
        .send(answer)
        .map_err(|_| "secureChallengeExpired".to_owned())?;
    verdict_receiver.await.unwrap_or(Ok(()))
}
#[tauri::command]
pub fn secure_ssh_challenge_cancel(
    id: String,
    window: WebviewWindow,
    service: State<'_, SecureSshChallengeService>,
) -> Result<(), String> {
    secure_ssh_challenge_cancel_for_label(window.label(), id, &service)
}

pub(crate) fn secure_ssh_challenge_cancel_for_label(
    surface_label: &str,
    id: String,
    service: &SecureSshChallengeService,
) -> Result<(), String> {
    require_label(surface_label, &id)?;
    service
        .pending
        .lock()
        .map_err(|_| "secureChallengeUnavailable")?
        .remove(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_entry() -> Pending {
        Pending {
            prompt: ChallengePrompt {
                id: "id".into(),
                content: ChallengeContent::SftpHostKey(SftpHostKeyChallenge {
                    endpoint: "host:22".into(),
                    algorithm: "ssh-ed25519".into(),
                    fingerprint_sha256: "SHA256:x".into(),
                }),
            },
            key: "k".into(),
            sender: None,
        }
    }

    #[test]
    fn a_failed_trust_write_rearms_the_same_prompt_for_another_decision() {
        let service = SecureSshChallengeService::default();
        service
            .pending
            .lock()
            .unwrap()
            .insert("id".into(), pending_entry());
        let (_, mut receiver) = oneshot::channel::<Answer>();
        assert!(rearm(&service, "id", &mut receiver));
        let sender = service
            .pending
            .lock()
            .unwrap()
            .get_mut("id")
            .and_then(|entry| entry.sender.take())
            .expect("re-armed sender");
        assert!(
            sender
                .send(Answer {
                    approved: true,
                    answers: vec![],
                    verdict: None,
                })
                .is_ok()
        );
        assert!(receiver.try_recv().is_ok_and(|answer| answer.approved));
        // A prompt that already ended is never re-armed.
        let (_, mut other) = oneshot::channel::<Answer>();
        assert!(!rearm(&service, "gone", &mut other));
    }
}
