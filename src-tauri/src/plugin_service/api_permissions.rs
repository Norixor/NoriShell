//! Plugins can inspect their own grants and ask for an independent protected decision.
use super::api_invocation::ApiInvocation;
use super::*;
use norishell_core_api::{PluginApiErrorCode, PluginApiValue};

impl PluginService {
    pub(super) fn api_permissions(
        &self,
        installed: &PluginInstalledRecord,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        let grants = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_capability_grants(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    u64::from(PLUGIN_PROTOCOL_MAJOR),
                )
            })
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        let grants = installed
            .capabilities
            .iter()
            .map(|capability| PluginCapabilityGrant {
                capability: *capability,
                granted: grants.iter().any(|grant| {
                    grant.capability == *capability && effective_plugin_grant(installed, grant)
                }),
            })
            .collect();
        let operation_permissions = self
            .operation_policies
            .list_current(installed)
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        Ok(PluginApiValue::Permissions {
            grants,
            policy_revision: operation_permissions.policy_revision,
            operation_permissions: operation_permissions.permissions,
        })
    }

    pub(super) fn request_api_permission(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        capability: PluginCapability,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        if !matches!(invocation, ApiInvocation::Declarative { .. }) {
            // The special-permission window is currently tied to a document action lifecycle.
            // It cannot safely outlive an isolated surface without a separate cancellation
            // contract, so the isolated bridge reports the capability as unsupported.
            return Err(PluginApiErrorCode::Unsupported);
        }
        if !invocation.explicit_user_action() {
            return Err(PluginApiErrorCode::InteractionRequired);
        }
        let owner = invocation.owner(installed);
        let current = self.api_invocation_fence(&owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if !installed.capabilities.contains(&capability) {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        if !special_plugin_capability(capability) {
            return Err(PluginApiErrorCode::Unsupported);
        }
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(PluginApiErrorCode::Unavailable)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let request = match invocation {
            ApiInvocation::Declarative { request, .. } => *request,
            ApiInvocation::Isolated { .. }
            | ApiInvocation::Provider { .. }
            | ApiInvocation::Task { .. } => {
                unreachable!("isolated permission request is rejected")
            }
        };
        let approval_id = self
            .open_special_permission_window(
                PluginSpecialPermissionOpenRequest {
                    meta: request.meta.clone(),
                    target: PluginSpecialPermissionTarget::Installed {
                        plugin_id: installed.plugin_id.clone(),
                        expected_plugin_state_version: installed.state_version,
                    },
                    requested_capability: Some(capability),
                },
                app,
            )
            .map_err(|_| PluginApiErrorCode::Conflict)?;
        Ok(PluginApiValue::PermissionRequested {
            approval_id: approval_id.to_string(),
        })
    }

    pub(super) fn forget_api_permissions(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        let owner = invocation.owner(installed);
        let current = self.api_invocation_fence(&owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let policies = self
            .operation_policies
            .list_current(installed)
            .map_err(|_| PluginApiErrorCode::Unavailable)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if policies.policy_revision.is_some() {
            self.operation_policies
                .clear_current(installed, policies.policy_revision)
                .map_err(|_| PluginApiErrorCode::Conflict)?;
        }
        Ok(PluginApiValue::PermissionsForgotten {})
    }

    /// A guest may revoke only an exact-operation approval belonging to its
    /// current Core-owned installation. Capability grants remain managed by
    /// the host's install/settings flow and are deliberately not exposed here.
    pub(super) fn revoke_api_operation_permission(
        &self,
        invocation: &ApiInvocation<'_>,
        installed: &PluginInstalledRecord,
        permission_id: &str,
        expected_policy_revision: WireSequence,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        let owner = invocation.owner(installed);
        let current = self.api_invocation_fence(&owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let policy_revision = self
            .operation_policies
            .revoke_current(installed, permission_id, expected_policy_revision)
            .map_err(|error| match error {
                AppPersistenceError::NotFound => PluginApiErrorCode::NotFound,
                AppPersistenceError::Conflict => PluginApiErrorCode::Conflict,
                AppPersistenceError::InvalidInput(_) => PluginApiErrorCode::InvalidRequest,
                _ => PluginApiErrorCode::Unavailable,
            })?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(PluginApiValue::PermissionRevoked { policy_revision })
    }

    pub(super) fn open_special_permission_window(
        &self,
        request: PluginSpecialPermissionOpenRequest,
        app: AppHandle,
    ) -> CoreResult<PluginApprovalId> {
        let snapshot = self.prepare_special_permission(request.clone())?;
        let label = secure_special_permission_window_label(&snapshot.approval_id);
        let opened = crate::secure_window_frame::apply_secure_window_frame(
            &app,
            &label,
            WebviewWindowBuilder::new(
                &app,
                label.clone(),
                WebviewUrl::App(
                    format!(
                        "secure-plugin-permission.html?approvalId={}",
                        snapshot.approval_id.as_str()
                    )
                    .into(),
                ),
            ),
        )
        .title("NoriShell")
        .inner_size(760.0, 760.0)
        .min_inner_size(640.0, 620.0)
        .resizable(true)
        .center()
        .build();
        let window = match opened {
            Ok(window) => window,
            Err(_) => {
                if let Some(outcome) = self.cancel_special_permission(&snapshot.approval_id) {
                    let _ = app.emit_to("main", "plugin-special-permission-changed", outcome);
                }
                return Err(plugin_runtime_error(request.meta.request_id, None));
            }
        };
        let service = self.clone();
        let approval_id = snapshot.approval_id;
        let result_id = approval_id.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed)
                && let Some(outcome) = service.cancel_special_permission(&approval_id)
            {
                let _ = app.emit_to("main", "plugin-special-permission-changed", outcome);
            }
        });
        Ok(result_id)
    }
}
