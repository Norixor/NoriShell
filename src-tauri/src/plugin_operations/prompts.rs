//! Protected, one-time decisions for operations on an existing SSH connection.

use std::time::Duration;
use zeroize::Zeroizing;

use norishell_core_api::{
    CoreApiError, ErrorCategory, PluginApprovalDecision, PluginApprovalId,
    PluginRemoteApprovalContent, PluginRemoteApprovalDecisionRequest,
    PluginRemoteApprovalGetRequest, PluginRemoteApprovalPrompt, RequestId, RetryStrategy,
    WireSequence,
};

use tauri::{Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tokio::sync::{oneshot, watch};

use super::{OperationFence, PluginOperationsService, RemoteInvocation, wait_cancelled};
use crate::plugin_operation_policy::{ApprovedOperationPolicy, PreparedOperationPolicy};
use crate::{core_api_error::core_error, time::unix_time_ms};

type CoreResult<T> = Result<T, Box<CoreApiError>>;
const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);
type Approval = Result<Option<ApprovedOperationPolicy>, ()>;
enum PromptReply {
    Approved(Box<Option<ApprovedOperationPolicy>>),
    CredentialInput(Zeroizing<Vec<u8>>),
    VaultInput {
        password: Zeroizing<Vec<u8>>,
        confirmation: Option<Zeroizing<Vec<u8>>>,
    },
}
type PromptOutcome = Result<PromptReply, ()>;

fn ordinary_approval(reply: PromptReply) -> Approval {
    match reply {
        PromptReply::Approved(approval) => Ok(*approval),
        PromptReply::CredentialInput(_) | PromptReply::VaultInput { .. } => Err(()),
    }
}

pub(crate) struct IndependentApproval {
    pub plugin_id: norishell_core_api::PluginId,
    pub instance_generation: WireSequence,
    pub plugin_name: String,
    pub locale: norishell_core_api::PluginLocale,
    pub host_label: String,
    pub endpoint: String,
    pub fence: OperationFence,
    pub remembered_policy: Option<PreparedOperationPolicy>,
    pub unavailable_policy: norishell_core_api::PluginRememberPolicy,
}

pub(super) struct PendingPrompt {
    pub(super) prompt: PluginRemoteApprovalPrompt,
    pub(super) instance_generation: WireSequence,
    fence: OperationFence,
    sender: oneshot::Sender<PromptOutcome>,
    remembered_policy: Option<PreparedOperationPolicy>,
}

struct PromptGuard {
    service: PluginOperationsService,
    id: PluginApprovalId,
}

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.service
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prompts
            .remove(self.id.as_str());
        if let Some(app) = &self.service.app
            && let Some(window) = app.get_webview_window(&window_label(&self.id))
        {
            let _ = window.close();
        }
    }
}

impl PluginOperationsService {
    pub(super) async fn prompt(
        &self,
        invocation: &RemoteInvocation,
        content: PluginRemoteApprovalContent,
        stop: &mut watch::Receiver<bool>,
    ) -> Approval {
        let lease = invocation.lease.clone();
        let parent_fence = invocation.fence.clone();
        let approval = IndependentApproval {
            plugin_id: invocation.plugin_id.clone(),
            instance_generation: invocation.instance_generation,
            plugin_name: invocation.plugin_name.clone(),
            locale: invocation.locale.clone(),
            host_label: invocation.host_label.clone(),
            endpoint: invocation.endpoint.clone(),
            fence: std::sync::Arc::new(move || parent_fence() && lease.is_current()),
            remembered_policy: invocation.remembered_policy.clone(),
            unavailable_policy: if matches!(
                invocation.lease.target,
                norishell_core_api::SshSessionTarget::QuickConnect { .. }
            ) {
                norishell_core_api::PluginRememberPolicy::UnstableTarget
            } else {
                norishell_core_api::PluginRememberPolicy::StorageUnavailable
            },
        };
        self.request_approval(&approval, content, stop)
            .await
            .and_then(ordinary_approval)
    }

    pub(crate) async fn approve_independent(
        &self,
        invocation: IndependentApproval,
        content: PluginRemoteApprovalContent,
        allow_interaction: bool,
    ) -> Result<Option<ApprovedOperationPolicy>, norishell_core_api::PluginApiErrorCode> {
        use norishell_core_api::PluginApiErrorCode;
        if matches!(
            content,
            PluginRemoteApprovalContent::Credential { .. }
                | PluginRemoteApprovalContent::VaultAccess { .. }
        ) {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        if !(invocation.fence)() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if let Some(prepared) = &invocation.remembered_policy
            && let Some(approved) = prepared
                .find()
                .map_err(|_| PluginApiErrorCode::Unavailable)?
        {
            return Ok(Some(approved));
        }
        if !allow_interaction {
            return Err(PluginApiErrorCode::InteractionRequired);
        }
        let (_cancel, mut stop) = watch::channel(false);
        self.request_approval(&invocation, content, &mut stop)
            .await
            .and_then(ordinary_approval)
            .map_err(|_| PluginApiErrorCode::PermissionDenied)
    }

    pub(crate) async fn request_credential_input(
        &self,
        mut invocation: IndependentApproval,
        label: String,
        target: norishell_core_api::PluginCredentialTarget,
        allow_interaction: bool,
    ) -> Result<Zeroizing<Vec<u8>>, norishell_core_api::PluginApiErrorCode> {
        use norishell_core_api::PluginApiErrorCode;
        if !allow_interaction {
            return Err(PluginApiErrorCode::InteractionRequired);
        }
        if !(invocation.fence)() {
            return Err(PluginApiErrorCode::Revoked);
        }
        invocation.remembered_policy = None;
        invocation.unavailable_policy = norishell_core_api::PluginRememberPolicy::Unavailable;
        let (_cancel, mut stop) = watch::channel(false);
        match self
            .request_approval(
                &invocation,
                PluginRemoteApprovalContent::Credential { label, target },
                &mut stop,
            )
            .await
        {
            Ok(PromptReply::CredentialInput(secret)) if (invocation.fence)() => Ok(secret),
            _ => Err(PluginApiErrorCode::Cancelled),
        }
    }

    pub(crate) async fn ensure_credential_vault(
        &self,
        mut invocation: IndependentApproval,
        vault: crate::vault_service::VaultService,
        allow_interaction: bool,
    ) -> Result<(), norishell_core_api::PluginApiErrorCode> {
        use norishell_core_api::{PluginApiErrorCode, VaultState};
        let state = vault.status().state;
        if !invocation.fence.as_ref()() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if state == VaultState::Unlocked {
            return Ok(());
        }
        if !allow_interaction {
            return Err(match state {
                VaultState::Missing => PluginApiErrorCode::VaultMissing,
                VaultState::Locked => PluginApiErrorCode::VaultLocked,
                VaultState::RequiresReload => PluginApiErrorCode::VaultRequiresReload,
                VaultState::Unlocked => unreachable!(),
            });
        }
        invocation.remembered_policy = None;
        invocation.unavailable_policy = norishell_core_api::PluginRememberPolicy::Unavailable;
        let create = state == VaultState::Missing;
        let (_cancel, mut stop) = watch::channel(false);
        let response = self
            .request_approval(
                &invocation,
                PluginRemoteApprovalContent::VaultAccess { create },
                &mut stop,
            )
            .await;
        let Ok(PromptReply::VaultInput {
            password,
            confirmation,
        }) = response
        else {
            return Err(PluginApiErrorCode::Cancelled);
        };
        let fence = invocation.fence;
        let continued = fence.clone();
        tokio::task::spawn_blocking(move || {
            if !fence() {
                return Err(PluginApiErrorCode::Revoked);
            }
            if create {
                let confirmation = confirmation.ok_or(PluginApiErrorCode::InvalidRequest)?;
                vault.create_for_protected_operation(&password, &confirmation)
            } else {
                vault.unlock_for_protected_operation(&password)
            }
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
            if !fence() {
                return Err(PluginApiErrorCode::Revoked);
            }
            Ok(())
        })
        .await
        .map_err(|_| PluginApiErrorCode::Unavailable)??;
        if !continued() {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(())
    }

    async fn request_approval(
        &self,
        invocation: &IndependentApproval,
        content: PluginRemoteApprovalContent,
        stop: &mut watch::Receiver<bool>,
    ) -> PromptOutcome {
        if matches!(
            content,
            PluginRemoteApprovalContent::Credential { .. }
                | PluginRemoteApprovalContent::VaultAccess { .. }
        ) && invocation.remembered_policy.is_some()
        {
            return Err(());
        }
        if *stop.borrow() || !(invocation.fence)() {
            return Err(());
        }
        if let Some(prepared) = &invocation.remembered_policy
            && let Some(approval) = prepared.find().map_err(|_| ())?
        {
            return Ok(PromptReply::Approved(Box::new(Some(approval))));
        }
        let app = self.app.as_ref().ok_or(())?;
        let id = PluginApprovalId::new();
        let prompt = PluginRemoteApprovalPrompt {
            approval_id: id.clone(),
            plugin_id: invocation.plugin_id.clone(),
            plugin_name: invocation.plugin_name.clone(),
            locale: invocation.locale.clone(),
            host_label: invocation.host_label.clone(),
            endpoint: invocation.endpoint.clone(),
            content,
            state_version: WireSequence::new(1),
            expires_at_unix_ms: unix_time_ms().saturating_add(300_000),
            remember_policy: if invocation.remembered_policy.is_some() {
                norishell_core_api::PluginRememberPolicy::ExactOperation
            } else {
                invocation.unavailable_policy
            },
        };
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.prompts.len() >= 32
                || state
                    .prompts
                    .values()
                    .any(|pending| pending.prompt.plugin_id == invocation.plugin_id)
            {
                return Err(());
            }
            state.prompts.insert(
                id.as_str().to_owned(),
                PendingPrompt {
                    prompt,
                    instance_generation: invocation.instance_generation,
                    fence: invocation.fence.clone(),
                    sender,
                    remembered_policy: invocation.remembered_policy.clone(),
                },
            );
        }
        let guard = PromptGuard {
            service: self.clone(),
            id: id.clone(),
        };
        let window =
            crate::secure_window_frame::apply_secure_window_frame(WebviewWindowBuilder::new(
                app,
                window_label(&id),
                WebviewUrl::App(
                    format!("secure-plugin-remote-approval.html?approvalId={id}").into(),
                ),
            ))
            .title("NoriShell")
            .resizable(true)
            .center()
            .build()
            .map_err(|_| ())?;
        let service = self.clone();
        let closed_id = id.clone();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                service
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .prompts
                    .remove(closed_id.as_str());
            }
        });
        let response = tokio::select! {
            biased;
            () = wait_cancelled(stop, &invocation.fence) => Err(()),
            result = tokio::time::timeout(PROMPT_TIMEOUT, receiver) => result.ok().and_then(Result::ok).unwrap_or(Err(())),
        };
        drop(guard);
        response
    }

    fn prompt_snapshot(
        &self,
        request: &PluginRemoteApprovalGetRequest,
    ) -> CoreResult<PluginRemoteApprovalPrompt> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = state
            .prompts
            .get(request.approval_id.as_str())
            .ok_or_else(|| rejected(request.meta.request_id.clone()))?;
        if pending.prompt.expires_at_unix_ms <= unix_time_ms() || !(pending.fence)() {
            return Err(rejected(request.meta.request_id.clone()));
        }
        Ok(pending.prompt.clone())
    }

    fn submit_credential_input(
        &self,
        mut request: norishell_core_api::PluginCredentialInputDecisionRequest,
    ) -> CoreResult<()> {
        let secret = Zeroizing::new(std::mem::take(&mut request.secret).into_bytes());
        let confirmation = request
            .confirmation
            .take()
            .map(|value| Zeroizing::new(value.into_bytes()));
        let pending = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prompts
            .remove(request.approval_id.as_str())
            .ok_or_else(|| rejected(request.meta.request_id.clone()))?;
        if secret.is_empty()
            || secret.len() > 4096
            || confirmation
                .as_ref()
                .is_some_and(|value| value.len() > 4096)
            || pending.prompt.state_version != request.expected_state_version
            || pending.prompt.expires_at_unix_ms <= unix_time_ms()
            || pending.remembered_policy.is_some()
            || !(pending.fence)()
        {
            return Err(rejected(request.meta.request_id));
        }
        let reply = match pending.prompt.content {
            PluginRemoteApprovalContent::Credential { .. } if confirmation.is_none() => {
                PromptReply::CredentialInput(secret)
            }
            PluginRemoteApprovalContent::VaultAccess { create }
                if if create {
                    confirmation.as_ref().is_some_and(|value| {
                        crate::vault_service::confirmations_match(&secret, value)
                    })
                } else {
                    confirmation.is_none()
                } =>
            {
                PromptReply::VaultInput {
                    password: secret,
                    confirmation,
                }
            }
            _ => return Err(rejected(request.meta.request_id)),
        };
        pending
            .sender
            .send(Ok(reply))
            .map_err(|_| rejected(request.meta.request_id))
    }

    fn decide_prompt(&self, request: PluginRemoteApprovalDecisionRequest) -> CoreResult<()> {
        let pending = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prompts
            .remove(request.approval_id.as_str())
            .ok_or_else(|| rejected(request.meta.request_id.clone()))?;
        if pending.prompt.state_version != request.expected_state_version
            || pending.prompt.expires_at_unix_ms <= unix_time_ms()
            || !(pending.fence)()
        {
            return Err(rejected(request.meta.request_id));
        }
        if request.decision == PluginApprovalDecision::Reject {
            let _ = pending.sender.send(Err(()));
            return Ok(());
        }
        if matches!(
            pending.prompt.content,
            PluginRemoteApprovalContent::Credential { .. }
                | PluginRemoteApprovalContent::VaultAccess { .. }
        ) {
            return Err(rejected(request.meta.request_id));
        }
        let approval = match request.policy {
            norishell_core_api::PluginApprovalPolicy::Once => None,
            norishell_core_api::PluginApprovalPolicy::Always => Some(
                pending
                    .remembered_policy
                    .as_ref()
                    .ok_or_else(|| rejected(request.meta.request_id.clone()))?
                    .remember(request.expiry)
                    .map_err(|_| rejected(request.meta.request_id.clone()))?,
            ),
        };
        pending
            .sender
            .send(Ok(PromptReply::Approved(Box::new(approval))))
            .map_err(|_| rejected(request.meta.request_id))
    }
}

pub(super) fn window_label(id: &PluginApprovalId) -> String {
    format!("secure-plugin-remote-{id}")
}

fn require_window(
    window: &WebviewWindow,
    id: &PluginApprovalId,
    request_id: RequestId,
) -> CoreResult<()> {
    if window.label() != window_label(id) {
        return Err(rejected(request_id));
    }
    Ok(())
}

fn rejected(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "plugin.remote_approval_rejected",
        ErrorCategory::Permission,
        RetryStrategy::Never,
        "plugins.remoteApproval.failed",
    )
}

#[tauri::command]
pub(crate) fn plugin_remote_approval_get(
    request: PluginRemoteApprovalGetRequest,
    window: WebviewWindow,
    service: State<'_, PluginOperationsService>,
) -> CoreResult<PluginRemoteApprovalPrompt> {
    require_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    service.prompt_snapshot(&request)
}

#[tauri::command]
pub(crate) fn plugin_remote_approval_decide(
    request: PluginRemoteApprovalDecisionRequest,
    window: WebviewWindow,
    service: State<'_, PluginOperationsService>,
) -> CoreResult<()> {
    require_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    service.decide_prompt(request)
}

#[tauri::command]
pub(crate) fn plugin_credential_input_submit(
    mut request: norishell_core_api::PluginCredentialInputDecisionRequest,
    window: WebviewWindow,
    service: State<'_, PluginOperationsService>,
) -> CoreResult<()> {
    if let Err(error) = require_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    ) {
        use zeroize::Zeroize;
        request.secret.zeroize();
        request.confirmation.zeroize();
        return Err(error);
    }
    service.submit_credential_input(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::{PluginId, PluginLocale, RequestMeta};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn insert_prompt(
        service: &PluginOperationsService,
        fence: OperationFence,
    ) -> (
        PluginRemoteApprovalDecisionRequest,
        oneshot::Receiver<PromptOutcome>,
    ) {
        let id = PluginApprovalId::new();
        let (sender, receiver) = oneshot::channel();
        service.state.lock().unwrap().prompts.insert(
            id.to_string(),
            PendingPrompt {
                prompt: PluginRemoteApprovalPrompt {
                    approval_id: id.clone(),
                    plugin_id: PluginId::parse("test.ops.approval").unwrap(),
                    plugin_name: "Test operations".to_owned(),
                    locale: PluginLocale::parse("en").unwrap(),
                    host_label: "Current session".to_owned(),
                    endpoint: "test@localhost:22".to_owned(),
                    content: PluginRemoteApprovalContent::Execute {
                        command: "true".to_owned(),
                        stdin: None,
                        reason: "Test admission".to_owned(),
                    },
                    state_version: WireSequence::new(1),
                    expires_at_unix_ms: unix_time_ms() + 60_000,
                    remember_policy: norishell_core_api::PluginRememberPolicy::Unavailable,
                },
                instance_generation: WireSequence::new(1),
                fence,
                sender,
                remembered_policy: None,
            },
        );
        (
            PluginRemoteApprovalDecisionRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                approval_id: id,
                expected_state_version: WireSequence::new(1),
                decision: PluginApprovalDecision::Approve,
                policy: norishell_core_api::PluginApprovalPolicy::Once,
                expiry: norishell_core_api::PluginApprovalExpiry::Unlimited,
            },
            receiver,
        )
    }

    #[tokio::test]
    async fn protected_approval_is_one_use_and_stale_fences_never_approve() {
        let (_directory, service) = super::super::tests::fixture();
        let (request, receiver) = insert_prompt(&service, Arc::new(|| true));
        service.decide_prompt(request.clone()).unwrap();
        assert!(receiver.await.unwrap().is_ok());
        assert!(service.decide_prompt(request).is_err());
        let current = Arc::new(AtomicBool::new(true));
        let fence_current = current.clone();
        let (request, receiver) = insert_prompt(
            &service,
            Arc::new(move || fence_current.load(Ordering::Acquire)),
        );
        current.store(false, Ordering::Release);
        assert!(service.decide_prompt(request).is_err());
        assert!(receiver.await.is_err());
    }

    #[tokio::test]
    async fn stale_versions_consume_the_prompt_and_reject_cannot_be_replayed() {
        let (_directory, service) = super::super::tests::fixture();
        let (mut request, receiver) = insert_prompt(&service, Arc::new(|| true));
        request.expected_state_version = WireSequence::new(2);
        assert!(service.decide_prompt(request).is_err());
        assert!(receiver.await.is_err());
        let (mut request, receiver) = insert_prompt(&service, Arc::new(|| true));
        request.decision = PluginApprovalDecision::Reject;
        service.decide_prompt(request.clone()).unwrap();
        assert!(receiver.await.unwrap().is_err());
        assert!(service.decide_prompt(request).is_err());
    }
    #[tokio::test]
    async fn credential_input_is_private_one_use_and_cannot_be_an_ordinary_approval() {
        let (_directory, service) = super::super::tests::fixture();
        let (request, receiver) = insert_prompt(&service, Arc::new(|| true));
        service
            .state
            .lock()
            .unwrap()
            .prompts
            .get_mut(request.approval_id.as_str())
            .unwrap()
            .prompt
            .content = PluginRemoteApprovalContent::Credential {
            label: "Test credential".into(),
            target: norishell_core_api::PluginCredentialTarget {
                origin: "https://example.test".into(),
                injection: norishell_core_api::PluginCredentialInjection::Bearer {},
            },
        };
        assert!(service.decide_prompt(request).is_err());
        assert!(receiver.await.is_err());

        let (request, receiver) = insert_prompt(&service, Arc::new(|| true));
        service
            .state
            .lock()
            .unwrap()
            .prompts
            .get_mut(request.approval_id.as_str())
            .unwrap()
            .prompt
            .content = PluginRemoteApprovalContent::Credential {
            label: "Test credential".into(),
            target: norishell_core_api::PluginCredentialTarget {
                origin: "https://example.test".into(),
                injection: norishell_core_api::PluginCredentialInjection::Bearer {},
            },
        };
        service
            .submit_credential_input(norishell_core_api::PluginCredentialInputDecisionRequest {
                meta: request.meta,
                approval_id: request.approval_id,
                expected_state_version: request.expected_state_version,
                secret: "test-only-token".into(),
                confirmation: None,
            })
            .unwrap();
        let PromptReply::CredentialInput(secret) = receiver.await.unwrap().unwrap() else {
            panic!("wrong private reply");
        };
        assert_eq!(secret.as_slice(), b"test-only-token");
    }

    #[tokio::test]
    async fn secret_submission_cannot_target_an_operation_or_unconfirmed_vault_creation() {
        let (_directory, service) = super::super::tests::fixture();
        for vault in [false, true] {
            let (request, receiver) = insert_prompt(&service, Arc::new(|| true));
            if vault {
                service
                    .state
                    .lock()
                    .unwrap()
                    .prompts
                    .get_mut(request.approval_id.as_str())
                    .unwrap()
                    .prompt
                    .content = PluginRemoteApprovalContent::VaultAccess { create: true };
            }
            assert!(
                service
                    .submit_credential_input(
                        norishell_core_api::PluginCredentialInputDecisionRequest {
                            meta: request.meta,
                            approval_id: request.approval_id,
                            expected_state_version: request.expected_state_version,
                            secret: "test-vault-password".into(),
                            confirmation: None,
                        }
                    )
                    .is_err()
            );
            assert!(receiver.await.is_err());
        }
    }

    #[tokio::test]
    async fn background_credentials_and_missing_vault_never_open_a_prompt() {
        let (directory, service) = super::super::tests::fixture();
        let invocation = || IndependentApproval {
            plugin_id: PluginId::parse("test.ops.approval").unwrap(),
            instance_generation: WireSequence::new(1),
            plugin_name: "Test".into(),
            locale: PluginLocale::parse("en").unwrap(),
            host_label: "Test service".into(),
            endpoint: "https://example.test".into(),
            fence: Arc::new(|| true),
            remembered_policy: None,
            unavailable_policy: norishell_core_api::PluginRememberPolicy::Unavailable,
        };
        assert_eq!(
            service
                .request_credential_input(
                    invocation(),
                    "test".into(),
                    norishell_core_api::PluginCredentialTarget {
                        origin: "https://example.test".into(),
                        injection: norishell_core_api::PluginCredentialInjection::Bearer {},
                    },
                    false
                )
                .await
                .err(),
            Some(norishell_core_api::PluginApiErrorCode::InteractionRequired)
        );
        let vault =
            crate::vault_service::VaultService::start(directory.path().join("credential-vault"));
        assert_eq!(
            service
                .ensure_credential_vault(invocation(), vault, false)
                .await
                .err(),
            Some(norishell_core_api::PluginApiErrorCode::VaultMissing)
        );
        assert!(service.state.lock().unwrap().prompts.is_empty());
    }

    #[tokio::test]
    async fn unavailable_remember_policy_cannot_be_forged_by_a_decision() {
        let (_directory, service) = super::super::tests::fixture();
        let (mut request, receiver) = insert_prompt(&service, Arc::new(|| true));
        request.policy = norishell_core_api::PluginApprovalPolicy::Always;
        assert!(service.decide_prompt(request).is_err());
        assert!(receiver.await.is_err());
    }
}
