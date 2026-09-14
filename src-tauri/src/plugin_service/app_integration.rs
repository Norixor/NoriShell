//! Application contributions remain owned by the exact running plugin generation.
use super::{api_invocation::ApiInvocation, *};
use crate::plugin_api::{ResourceFence, ResourceOwner};
use norishell_core_api::*;
use std::time::{Duration, Instant};
use tauri::Emitter;

#[derive(Default)]
pub(crate) struct PluginAppIntegrationState {
    entries: BTreeMap<String, Entry>,
}
struct Entry {
    snapshot: PluginAppIntegrationSnapshot,
    current: ResourceFence,
    notices: Vec<Instant>,
}

fn safe_text(value: &str, max: usize) -> bool {
    let lower = value.to_ascii_lowercase();
    !value.trim().is_empty()
        && value.len() <= max
        && !value.chars().any(char::is_control)
        && ![
            "-----begin",
            "password=",
            "password:",
            "token=",
            "secret=",
            "authorization:",
            "bearer ",
        ]
        .iter()
        .any(|pattern| lower.contains(pattern))
}
fn valid_shortcut(value: &str) -> bool {
    value
        .strip_prefix("Alt+Shift+")
        .is_some_and(|key| key.len() == 1 && key.as_bytes()[0].is_ascii_uppercase())
}
fn validate_registration(
    registration: &PluginAppRegistration,
    instance: &ActivePluginInstance,
) -> bool {
    if registration.commands.len() > 24 || registration.statuses.len() > 8 {
        return false;
    }
    let mut ids = std::collections::BTreeSet::new();
    registration.commands.iter().all(|command| {
        let document = if command.target_id.as_str() == "app.page" {
            command.page_id.as_ref().and_then(|id| instance.pages.get(id)).map(|page| &page.document)
        } else if command.page_id.is_none() && plugin_extension_registry::find(&command.target_id).is_some_and(|definition| !definition.contextual) {
            instance.ui_templates.get(command.target_id.as_str()).map(|template| &template.document)
        } else { None };
        safe_text(&command.id, 64) && ids.insert(command.id.clone()) && safe_text(&command.label, 100)
            && command.shortcut.as_deref().is_none_or(valid_shortcut)
            && command.file_extensions.len() <= 12
            && command.file_extensions.iter().all(|extension| !extension.is_empty() && extension.len() <= 12 && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            && document.is_some_and(|document| document.nodes.iter().any(|node| matches!(node, PluginUiNode::Button { action_id, disabled: false, .. } if action_id == &command.action_id)))
    }) && registration.statuses.iter().all(|status| safe_text(&status.id, 64) && ids.insert(status.id.clone()) && safe_text(&status.text, 160))
}
impl PluginService {
    pub(super) fn invoke_app_integration(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &ResourceOwner,
        operation: &PluginApiOperation,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        if !invocation.explicit_user_action() {
            return Err(PluginApiErrorCode::InteractionRequired);
        }
        let current = self.api_invocation_fence(owner, invocation);
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let instance = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get(owner.plugin_id.as_str())
            .cloned()
            .ok_or(PluginApiErrorCode::Revoked)?;
        let lifetime = self.api_runtime_fence(owner.clone());
        let mut state = self
            .app_integrations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.entries.retain(|_, entry| (entry.current)());
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        match operation {
            PluginApiOperation::AppRegister { registration } => {
                if !validate_registration(registration, &instance) {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                let entry = state
                    .entries
                    .entry(owner.plugin_id.as_str().to_owned())
                    .or_insert_with(|| Entry {
                        snapshot: PluginAppIntegrationSnapshot {
                            plugin_id: owner.plugin_id.clone(),
                            plugin_name: instance.plugin_name.clone(),
                            package_sha256: owner.package.clone(),
                            instance_generation: owner.generation,
                            registration: registration.clone(),
                            notifications: vec![],
                        },
                        current: lifetime,
                        notices: vec![],
                    });
                entry.snapshot.registration = registration.clone();
            }
            PluginApiOperation::AppNotify { notification } => {
                if !safe_text(&notification.id, 64) || !safe_text(&notification.text, 240) {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                let entry = state
                    .entries
                    .entry(owner.plugin_id.as_str().to_owned())
                    .or_insert_with(|| Entry {
                        snapshot: PluginAppIntegrationSnapshot {
                            plugin_id: owner.plugin_id.clone(),
                            plugin_name: instance.plugin_name.clone(),
                            package_sha256: owner.package.clone(),
                            instance_generation: owner.generation,
                            registration: PluginAppRegistration {
                                commands: vec![],
                                statuses: vec![],
                            },
                            notifications: vec![],
                        },
                        current: lifetime,
                        notices: vec![],
                    });
                entry
                    .notices
                    .retain(|time| time.elapsed() < Duration::from_secs(60));
                if entry.notices.len() >= 5 {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                entry.notices.push(Instant::now());
                entry
                    .snapshot
                    .notifications
                    .retain(|item| item.id != notification.id);
                entry.snapshot.notifications.push(notification.clone());
                if entry.snapshot.notifications.len() > 5 {
                    entry.snapshot.notifications.remove(0);
                }
                if let Some(app) = self
                    .app_handle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                {
                    app.emit_to(
                        "main",
                        "norishell://plugin-app-notification",
                        &entry.snapshot,
                    )
                    .map_err(|_| PluginApiErrorCode::Unavailable)?;
                }
            }
            PluginApiOperation::AppNavigate { destination } => {
                if !state.entries.contains_key(owner.plugin_id.as_str()) {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                let path = match destination {
                    PluginAppNavigation::App { path }
                        if [
                            "/terminal",
                            "/overview",
                            "/hosts",
                            "/sftp",
                            "/tunnels",
                            "/plugins",
                            "/settings",
                        ]
                        .contains(&path.as_str()) =>
                    {
                        path.clone()
                    }
                    PluginAppNavigation::PluginPage { page_id }
                        if instance.pages.contains_key(page_id) =>
                    {
                        format!("/plugin/{}/{}", owner.plugin_id.as_str(), page_id)
                    }
                    _ => return Err(PluginApiErrorCode::InvalidRequest),
                };
                let app = self
                    .app_handle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .ok_or(PluginApiErrorCode::Unavailable)?;
                if !current() {
                    return Err(PluginApiErrorCode::Revoked);
                }
                app.emit_to("main", "norishell://plugin-app-navigation", serde_json::json!({"pluginId": owner.plugin_id, "packageSha256": owner.package, "instanceGeneration": owner.generation, "path": path})).map_err(|_| PluginApiErrorCode::Unavailable)?;
            }
            _ => return Err(PluginApiErrorCode::InvalidRequest),
        }
        Ok(PluginApiValue::AppAccepted {})
    }
}

#[tauri::command]
pub(crate) fn plugin_app_integration_list<R: tauri::Runtime>(
    request: PluginAppIntegrationListRequest,
    window: tauri::WebviewWindow<R>,
    service: tauri::State<'_, PluginService>,
) -> CoreResult<Vec<PluginAppIntegrationSnapshot>> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    service.require_ready(request.meta.request_id)?;
    let mut state = service
        .app_integrations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.entries.retain(|_, entry| (entry.current)());
    Ok(state
        .entries
        .values()
        .map(|entry| entry.snapshot.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        transient_credential_service::TransientCredentialService, vault_service::VaultService,
    };

    #[tokio::test]
    async fn background_app_operations_fail_before_mutating_registrations_or_notices() {
        let directory = tempfile::tempdir().unwrap();
        let hosts = HostService::start(directory.path()).unwrap();
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).unwrap();
        let owner = ResourceOwner {
            plugin_id: PluginId::parse("com.norishell.background-test").unwrap(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        };
        let invocation = ApiInvocation::isolated(
            RequestId::new(),
            owner.clone(),
            "onOpen".into(),
            Arc::new(|| true),
            Arc::new(|| true),
            false,
        );
        for operation in [
            PluginApiOperation::AppRegister {
                registration: PluginAppRegistration {
                    commands: vec![],
                    statuses: vec![],
                },
            },
            PluginApiOperation::AppNotify {
                notification: PluginAppNotification {
                    id: "notice".into(),
                    text: "Ready".into(),
                },
            },
            PluginApiOperation::AppNavigate {
                destination: PluginAppNavigation::App {
                    path: "/plugins".into(),
                },
            },
        ] {
            assert_eq!(
                service.invoke_app_integration(&invocation, &owner, &operation),
                Err(PluginApiErrorCode::InteractionRequired),
            );
        }
        assert!(service.app_integrations.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn bounded_non_secret_text_and_reserved_shortcuts() {
        assert!(safe_text("Ready: 3 items", 160));
        assert!(!safe_text("Bearer abc", 160));
        assert!(!safe_text("line\nline", 160));
        assert!(valid_shortcut("Alt+Shift+P"));
        for shortcut in ["Meta+1", "Ctrl+W", "Alt+F4", "Alt+Shift+1", "Alt+Shift+PP"] {
            assert!(!valid_shortcut(shortcut));
        }
    }
}
