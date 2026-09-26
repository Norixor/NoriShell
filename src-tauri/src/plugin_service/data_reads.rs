//! Explicit, category-scoped reads from Core-owned durable data.

use super::api_invocation::ApiInvocation;
use super::*;
use norishell_core_api::{
    ApplicationPreferenceGroupId, NativeTerminalHistoryListRequest, NativeTerminalHistoryScope,
    PluginApiErrorCode, PluginApiValue, PluginDataCategory, PluginDataReadRequest,
    PluginDataReadResult, PluginPreferenceGroupSnapshot, PluginTerminalHistoryEntry, RequestMeta,
    VaultState,
};
use tauri::Manager;

fn required_data_capability(
    category: PluginDataCategory,
    explicit_user_action: bool,
) -> Result<PluginCapability, PluginApiErrorCode> {
    match category {
        PluginDataCategory::AppPreferences => Ok(PluginCapability::AppPreferencesRead),
        PluginDataCategory::TerminalHistory if explicit_user_action => {
            Ok(PluginCapability::TerminalHistoryRead)
        }
        PluginDataCategory::TerminalHistory => Err(PluginApiErrorCode::InteractionRequired),
        PluginDataCategory::Hosts
        | PluginDataCategory::Credentials
        | PluginDataCategory::DesktopProfiles => Err(PluginApiErrorCode::Unsupported),
    }
}

impl PluginService {
    pub(super) fn read_api_data(
        &self,
        invocation: &ApiInvocation<'_>,
        owner: &crate::plugin_api::ResourceOwner,
        request: &PluginDataReadRequest,
        current: &crate::plugin_api::ResourceFence,
    ) -> Result<PluginApiValue, PluginApiErrorCode> {
        request.validate()?;
        let capability =
            required_data_capability(request.category, invocation.explicit_user_action())?;
        self.has_capability(
            invocation.request_id().clone(),
            &owner.plugin_id,
            &owner.signer,
            capability,
        )
        .map_err(|_| PluginApiErrorCode::PermissionDenied)?;
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let result = match request.category {
            PluginDataCategory::AppPreferences => {
                let mut groups = Vec::with_capacity(8);
                let mut migration_required = Vec::new();
                for group in ApplicationPreferenceGroupId::ALL {
                    let snapshot = self
                        .hosts
                        .with_desktop_repository(|repository| {
                            repository.get_application_preferences(group)
                        })
                        .map_err(|_| PluginApiErrorCode::Unavailable)?;
                    match (snapshot.revision, snapshot.value) {
                        (Some(revision), Some(value)) => {
                            groups.push(PluginPreferenceGroupSnapshot {
                                group: group.as_str().to_owned(),
                                revision,
                                value,
                            })
                        }
                        (None, None) => migration_required.push(group.as_str().to_owned()),
                        _ => return Err(PluginApiErrorCode::Unavailable),
                    }
                }
                let desktop = self
                    .hosts
                    .with_desktop_repository(|repository| repository.get_desktop_preferences())
                    .map_err(|_| PluginApiErrorCode::Unavailable)?;
                groups.push(PluginPreferenceGroupSnapshot {
                    group: "desktop".to_owned(),
                    revision: desktop.revision,
                    value: serde_json::to_value(&desktop.preferences)
                        .map_err(|_| PluginApiErrorCode::Unavailable)?,
                });
                let app = self
                    .app_handle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .ok_or(PluginApiErrorCode::Unavailable)?;
                let native_terminal = app
                    .try_state::<crate::native_terminal::NativeTerminalService>()
                    .ok_or(PluginApiErrorCode::Unavailable)?;
                let notifications = native_terminal.settings_snapshot();
                groups.push(PluginPreferenceGroupSnapshot {
                    group: "commandNotifications".to_owned(),
                    revision: notifications.settings_revision,
                    value: serde_json::json!({
                        "notificationsEnabled": notifications.settings.notifications_enabled,
                        "notificationThresholdSeconds": notifications.settings.notification_threshold_seconds,
                    }),
                });
                PluginDataReadResult::AppPreferences {
                    groups,
                    migration_required,
                }
            }
            PluginDataCategory::TerminalHistory => {
                // The admission gate required an explicit click before any Vault access.
                let app = self
                    .app_handle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .ok_or(PluginApiErrorCode::Unavailable)?;
                let vault_service = app
                    .try_state::<crate::vault_service::VaultService>()
                    .ok_or(PluginApiErrorCode::Unavailable)?;
                let state = vault_service.status().state;
                match state {
                    VaultState::Missing => return Err(PluginApiErrorCode::VaultMissing),
                    VaultState::Locked => return Err(PluginApiErrorCode::VaultLocked),
                    VaultState::RequiresReload => {
                        return Err(PluginApiErrorCode::VaultRequiresReload);
                    }
                    VaultState::Unlocked => {}
                }
                let terminal = app
                    .try_state::<crate::native_terminal::NativeTerminalService>()
                    .ok_or(PluginApiErrorCode::Unavailable)?;
                // This service applies the user's history enabled/pause settings and
                // keeps the in-memory projection in step with Vault availability.
                let history = terminal
                    .history_list(NativeTerminalHistoryListRequest {
                        meta: RequestMeta {
                            request_id: invocation.request_id().clone(),
                        },
                        scope: None,
                        query: String::new(),
                        limit: 2_000,
                    })
                    .map_err(|_| PluginApiErrorCode::Unavailable)?;
                let post_read_state = vault_service.status().state;
                match post_read_state {
                    VaultState::Missing => return Err(PluginApiErrorCode::VaultMissing),
                    VaultState::Locked => return Err(PluginApiErrorCode::VaultLocked),
                    VaultState::RequiresReload => {
                        return Err(PluginApiErrorCode::VaultRequiresReload);
                    }
                    VaultState::Unlocked => {}
                }
                let total =
                    u16::try_from(history.len()).map_err(|_| PluginApiErrorCode::Unavailable)?;
                let end = usize::from(request.offset)
                    .saturating_add(usize::from(request.limit))
                    .min(history.len());
                let entries = history
                    .into_iter()
                    .skip(usize::from(request.offset))
                    .take(usize::from(request.limit))
                    .map(|entry| PluginTerminalHistoryEntry {
                        entry_id: entry.entry_id.as_str().to_owned(),
                        scope: match entry.scope {
                            NativeTerminalHistoryScope::Host { host_id } => {
                                format!("host:{}", host_id.as_str())
                            }
                            NativeTerminalHistoryScope::Local => "local".to_owned(),
                        },
                        command: entry.command,
                        completed_at_unix_ms: entry.completed_at_unix_ms,
                        elapsed_millis: entry.elapsed_millis,
                        exit_code: entry.exit_code,
                    })
                    .collect();
                PluginDataReadResult::TerminalHistory {
                    entries,
                    next_offset: (end < usize::from(total)).then_some(end as u16),
                    total,
                }
            }
            PluginDataCategory::Hosts
            | PluginDataCategory::Credentials
            | PluginDataCategory::DesktopProfiles => unreachable!("validated category"),
        };
        if !current() {
            return Err(PluginApiErrorCode::Revoked);
        }
        Ok(PluginApiValue::DataRead { result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_permissions_are_distinct_and_history_rejects_background_calls() {
        assert_eq!(
            required_data_capability(PluginDataCategory::AppPreferences, false),
            Ok(PluginCapability::AppPreferencesRead)
        );
        assert_eq!(
            required_data_capability(PluginDataCategory::TerminalHistory, true),
            Ok(PluginCapability::TerminalHistoryRead)
        );
        assert_eq!(
            required_data_capability(PluginDataCategory::TerminalHistory, false),
            Err(PluginApiErrorCode::InteractionRequired)
        );
    }

    #[test]
    fn bounded_category_requests_reject_broad_or_invalid_reads() {
        for request in [
            PluginDataReadRequest {
                category: PluginDataCategory::TerminalHistory,
                offset: 0,
                limit: 0,
            },
            PluginDataReadRequest {
                category: PluginDataCategory::TerminalHistory,
                offset: 0,
                limit: 101,
            },
            PluginDataReadRequest {
                category: PluginDataCategory::AppPreferences,
                offset: 1,
                limit: 1,
            },
            PluginDataReadRequest {
                category: PluginDataCategory::AppPreferences,
                offset: 0,
                limit: 100,
            },
        ] {
            assert_eq!(request.validate(), Err(PluginApiErrorCode::InvalidRequest));
        }
    }
}
