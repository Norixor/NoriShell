//! Admission, protected input, and persistent lifecycle wiring for plugin-owned credentials.
use super::{api_invocation::ApiInvocation, *};
use crate::{
    plugin_api::{ResourceFence, ResourceOwner},
    plugin_credential_service::{
        PluginCredentialIntent, PluginCredentialPreparation, PluginCredentialService,
        PluginCredentialServiceError,
    },
    plugin_operations::prompts::IndependentApproval,
};
use norishell_core_api::{
    PluginApiErrorCode, PluginApiValue, PluginCredentialOperation, PluginCredentialResult,
};

struct CredentialIntentGuard {
    service: PluginCredentialService,
    intent: Option<PluginCredentialIntent>,
}
impl Drop for CredentialIntentGuard {
    fn drop(&mut self) {
        if let Some(intent) = self.intent.take() {
            let service = self.service.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let _ = service.cancel_create(&intent);
            });
        }
    }
}

impl PluginService {
    pub(super) async fn invoke_api_credential(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
        operation: &PluginCredentialOperation,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        let lifetime = self.api_credential_fence(invocation, installed, owner)?;
        let current = self.api_invocation_fence(owner, invocation);
        let fence: ResourceFence = Arc::new(move || lifetime() && current());
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let service = self.api_credential_service()?;
        // Capability mutation stops this instance first. The transaction check stays memory-only
        // so it cannot re-enter the repository mutex while a durable intent/CAS is committing.
        let transaction_fence = self.api_transaction_fence(owner, invocation);
        let result = match operation {
            PluginCredentialOperation::Create {
                operation_id,
                idempotency_key,
                label,
                target,
            } => {
                if !invocation.explicit_user_action() {
                    return Err(PluginApiErrorCode::InteractionRequired);
                }
                let prepare_service = service.clone();
                let prepare_owner = owner.clone();
                let operation_id = operation_id.clone();
                let idempotency_key = idempotency_key.clone();
                let prepare_label = label.clone();
                let prepare_target = target.clone();
                let prepare_fence = transaction_fence.clone();
                let prepared = tauri::async_runtime::spawn_blocking(move || {
                    prepare_service.prepare_create(
                        &prepare_owner,
                        operation_id,
                        idempotency_key,
                        prepare_label,
                        prepare_target,
                        prepare_fence,
                    )
                })
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)?
                .map_err(map_credential_error)?;
                match prepared {
                    PluginCredentialPreparation::Existing(credential) => {
                        PluginCredentialResult::Created { credential }
                    }
                    PluginCredentialPreparation::Intent(intent) => {
                        let mut guard = CredentialIntentGuard {
                            service: service.clone(),
                            intent: Some(intent.clone()),
                        };
                        let operations = self
                            .operations
                            .as_ref()
                            .ok_or(PluginApiErrorCode::Unavailable)?;
                        let locale = self
                            .active_instance(
                                invocation.request_id().clone(),
                                &owner.plugin_id,
                                &owner.signer,
                                owner.generation,
                            )
                            .map_err(|_| PluginApiErrorCode::Revoked)?
                            .locale;
                        let approval = || IndependentApproval {
                            plugin_id: owner.plugin_id.clone(),
                            instance_generation: owner.generation,
                            plugin_name: installed.name.clone(),
                            locale: locale.clone(),
                            host_label: label.clone(),
                            endpoint: target.origin.clone(),
                            fence: fence.clone(),
                            remembered_policy: None,
                            unavailable_policy:
                                norishell_core_api::PluginRememberPolicy::Unavailable,
                        };
                        operations
                            .ensure_credential_vault(approval(), self.api_vault_service()?, true)
                            .await?;
                        let secret = operations
                            .request_credential_input(
                                approval(),
                                label.clone(),
                                target.clone(),
                                true,
                            )
                            .await?;
                        let _permit = self
                            .api_creation_permit(invocation.request_id().clone())
                            .map_err(|_| PluginApiErrorCode::Revoked)?;
                        if !fence() {
                            return Err(PluginApiErrorCode::Revoked);
                        }
                        let finish_service = service.clone();
                        let finish_fence = transaction_fence.clone();
                        let credential = tauri::async_runtime::spawn_blocking(move || {
                            finish_service.finish_create(intent, secret, finish_fence)
                        })
                        .await
                        .map_err(|_| PluginApiErrorCode::Unavailable)?
                        .map_err(map_credential_error)?;
                        guard.intent = None;
                        PluginCredentialResult::Created { credential }
                    }
                }
            }
            PluginCredentialOperation::List {} => {
                let owner = owner.clone();
                let fence = transaction_fence.clone();
                let credentials =
                    tauri::async_runtime::spawn_blocking(move || service.list(&owner, fence))
                        .await
                        .map_err(|_| PluginApiErrorCode::Unavailable)?
                        .map_err(map_credential_error)?;
                PluginCredentialResult::List { credentials }
            }
            PluginCredentialOperation::Revoke {
                handle,
                expected_revision,
            } => {
                let owner = owner.clone();
                let handle = handle.clone();
                let revision = *expected_revision;
                let fence = transaction_fence.clone();
                let credential = tauri::async_runtime::spawn_blocking(move || {
                    service.revoke(&owner, &handle, revision, fence)
                })
                .await
                .map_err(|_| PluginApiErrorCode::Unavailable)?
                .map_err(map_credential_error)?;
                PluginCredentialResult::Revoked { credential }
            }
        };
        if !fence() {
            return Err(if matches!(operation, PluginCredentialOperation::List {}) {
                PluginApiErrorCode::Revoked
            } else {
                PluginApiErrorCode::OutcomeUnknown
            });
        }
        Ok(PluginApiValue::Credential { result })
    }

    pub(super) async fn delete_plugin_credential_data(
        &self,
        request_id: RequestId,
        plugin_id: &PluginId,
    ) -> CoreResult<()> {
        let service = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .and_then(|app| {
                app.try_state::<PluginCredentialService>()
                    .map(|service| service.inner().clone())
            });
        if let Some(service) = service {
            let plugin_id = plugin_id.clone();
            tauri::async_runtime::spawn_blocking(move || service.cleanup_plugin(&plugin_id))
                .await
                .map_err(|_| plugin_runtime_error(request_id.clone(), None))?
                .map_err(|_| plugin_runtime_error(request_id, None))?;
        } else {
            // Headless/reconciliation paths still revoke durably. The startup reconciler can
            // delete the encrypted material when the Vault service becomes available.
            self.hosts
                .with_plugin_repository(|repository| {
                    repository.prepare_plugin_credential_cleanup_by_plugin(plugin_id)
                })
                .map_err(|error| map_persistence_error(request_id, error))?;
        }
        Ok(())
    }

    pub(super) fn api_credential_fence(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        owner: &ResourceOwner,
    ) -> Result<ResourceFence, PluginApiErrorCode> {
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            PluginCapability::CredentialsPlugin,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        let epoch = self
            .capability_grant_epoch_for_record(
                invocation.request_id().clone(),
                installed,
                PluginCapability::CredentialsPlugin,
            )
            .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        Ok(self.api_resource_fence(
            owner.clone(),
            Some((PluginCapability::CredentialsPlugin, epoch)),
        ))
    }

    pub(super) fn api_credential_service(
        &self,
    ) -> Result<PluginCredentialService, PluginApiErrorCode> {
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        Ok(app
            .try_state::<PluginCredentialService>()
            .ok_or(PluginApiErrorCode::Unavailable)?
            .inner()
            .clone())
    }

    pub(super) fn api_vault_service(
        &self,
    ) -> Result<crate::vault_service::VaultService, PluginApiErrorCode> {
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        Ok(app
            .try_state::<crate::vault_service::VaultService>()
            .ok_or(PluginApiErrorCode::Unavailable)?
            .inner()
            .clone())
    }
}

pub(super) fn map_credential_error(error: PluginCredentialServiceError) -> PluginApiErrorCode {
    match error {
        PluginCredentialServiceError::InvalidRequest => PluginApiErrorCode::InvalidRequest,
        PluginCredentialServiceError::NotFound => PluginApiErrorCode::NotFound,
        PluginCredentialServiceError::Conflict => PluginApiErrorCode::Conflict,
        PluginCredentialServiceError::Revoked => PluginApiErrorCode::Revoked,
        PluginCredentialServiceError::VaultMissing => PluginApiErrorCode::VaultMissing,
        PluginCredentialServiceError::VaultLocked => PluginApiErrorCode::VaultLocked,
        PluginCredentialServiceError::VaultRequiresReload => {
            PluginApiErrorCode::VaultRequiresReload
        }
        PluginCredentialServiceError::Persistence | PluginCredentialServiceError::Vault => {
            PluginApiErrorCode::Unavailable
        }
    }
}
