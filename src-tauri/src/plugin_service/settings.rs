//! Core-owned plugin settings API and package-update reconciliation.

use norishell_app_persistence::{PluginSettingsInstall, PluginSettingsRecord};
use norishell_core_api::{
    EVENT_PLUGIN_SETTINGS_CHANGED, PluginId, PluginSettingsChanged, PluginSettingsGetRequest,
    PluginSettingsReplaceRequest, PluginSettingsResetRequest, PluginSettingsSnapshot,
    PluginSettingsValues, RequestId, WireSequence,
};
use norishell_plugin_platform::{
    InspectedPluginSettings, default_settings_values, inspect_plugin_settings,
    reconcile_settings_values, settings_target_is_visible, settings_values_json,
    validate_settings_values,
};
use tauri::{Emitter, State};
use uuid::Uuid;

use super::*;

#[derive(Debug, Clone)]
pub(super) struct ReconciledPluginSettings {
    schema_json: String,
    schema_sha256: String,
    values_json: String,
    expected_previous_revision: Option<WireSequence>,
}

impl ReconciledPluginSettings {
    pub(super) fn install(&self) -> PluginSettingsInstall<'_> {
        PluginSettingsInstall {
            schema_json: &self.schema_json,
            schema_sha256: &self.schema_sha256,
            values_json: &self.values_json,
            expected_previous_revision: self.expected_previous_revision,
        }
    }

    fn expected_revision(&self) -> WireSequence {
        WireSequence::new(
            self.expected_previous_revision
                .map_or(1, |revision| revision.get().saturating_add(1)),
        )
    }

    pub(super) fn matches_record(&self, record: Option<&PluginSettingsRecord>) -> bool {
        record.is_some_and(|record| {
            record.schema_json == self.schema_json
                && record.schema_sha256 == self.schema_sha256
                && record.values_json == self.values_json
                && record.revision == self.expected_revision()
        })
    }
}

impl PluginService {
    pub(super) fn installed_has_settings(
        &self,
        request_id: RequestId,
        installed: &PluginInstalledRecord,
    ) -> CoreResult<bool> {
        self.hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_settings(&installed.plugin_id)
            })
            .map_err(|error| map_persistence_error(request_id, error))
            .map(|record| {
                record.is_some_and(|record| record.package_sha256 == installed.package_sha256)
            })
    }

    pub(super) fn reconcile_package_settings(
        &self,
        request_id: RequestId,
        plugin_id: &PluginId,
        inspected: Option<&InspectedPluginSettings>,
    ) -> CoreResult<Option<ReconciledPluginSettings>> {
        let Some(inspected) = inspected else {
            return Ok(None);
        };
        let previous = self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_settings(plugin_id))
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        let previous_inspected = previous.as_ref().and_then(|record| {
            inspect_plugin_settings(record.schema_json.as_bytes())
                .ok()
                .filter(|schema| {
                    schema.schema_json == record.schema_json
                        && schema.schema_sha256 == record.schema_sha256
                })
        });
        let previous_schema = previous_inspected.as_ref().map(|settings| &settings.schema);
        let previous_values = previous.as_ref().and_then(|record| {
            serde_json::from_str::<PluginSettingsValues>(&record.values_json).ok()
        });
        let (values, _reset_keys) =
            reconcile_settings_values(&inspected.schema, previous_schema, previous_values.as_ref())
                .map_err(|_| plugin_validation_error(request_id.clone()))?;
        let values_json = settings_values_json(&inspected.schema, &values)
            .map_err(|_| plugin_validation_error(request_id))?;
        Ok(Some(ReconciledPluginSettings {
            schema_json: inspected.schema_json.clone(),
            schema_sha256: inspected.schema_sha256.clone(),
            values_json,
            expected_previous_revision: previous.as_ref().map(|record| record.revision),
        }))
    }

    pub(super) fn package_settings_commit_matches(
        &self,
        plugin_id: &PluginId,
        package_sha256: &str,
        expected: Option<&ReconciledPluginSettings>,
    ) -> bool {
        let Some(expected) = expected else {
            return true;
        };
        self.hosts
            .with_plugin_repository(|repository| repository.get_plugin_settings(plugin_id))
            .is_ok_and(|record| {
                record
                    .as_ref()
                    .is_some_and(|record| record.package_sha256 == package_sha256)
                    && expected.matches_record(record.as_ref())
            })
    }

    pub(super) fn settings_snapshot_for_installation(
        &self,
        request_id: RequestId,
        installed: &PluginInstalledRecord,
    ) -> CoreResult<Option<PluginSettingsSnapshot>> {
        let record = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_settings(&installed.plugin_id)
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        let Some(record) =
            record.filter(|record| record.package_sha256 == installed.package_sha256)
        else {
            return Ok(None);
        };
        let inspected = inspect_plugin_settings(record.schema_json.as_bytes())
            .map_err(|_| plugin_runtime_error(request_id.clone(), None))?;
        if inspected.schema_json != record.schema_json
            || inspected.schema_sha256 != record.schema_sha256
        {
            return Err(plugin_runtime_error(request_id, None));
        }
        let values = serde_json::from_str::<PluginSettingsValues>(&record.values_json)
            .map_err(|_| plugin_runtime_error(request_id.clone(), None))?;
        validate_settings_values(&inspected.schema, &values)
            .map_err(|_| plugin_runtime_error(request_id.clone(), None))?;
        Ok(Some(PluginSettingsSnapshot {
            plugin_id: installed.plugin_id.clone(),
            package_sha256: installed.package_sha256.clone(),
            installed_state_version: installed.state_version,
            schema_sha256: record.schema_sha256,
            revision: record.revision,
            schema: inspected.schema,
            values,
        }))
    }

    pub(super) fn settings_target_visible(
        &self,
        request_id: RequestId,
        installed: &PluginInstalledRecord,
        expected_revision: Option<WireSequence>,
        target_id: &str,
    ) -> CoreResult<bool> {
        let snapshot = self.settings_snapshot_for_installation(request_id, installed)?;
        Ok(match snapshot {
            Some(snapshot) => {
                expected_revision.is_none_or(|revision| revision == snapshot.revision)
                    && settings_target_is_visible(&snapshot.schema, &snapshot.values, target_id)
            }
            None => expected_revision.is_none(),
        })
    }

    fn get_settings(
        &self,
        request: PluginSettingsGetRequest,
    ) -> CoreResult<Option<PluginSettingsSnapshot>> {
        self.require_ready(request.meta.request_id.clone())?;
        let installed = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&request.plugin_id)
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        self.settings_snapshot_for_installation(request.meta.request_id, &installed)
    }

    fn settings_operation_in_flight(&self, plugin_id: &PluginId) -> bool {
        if self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get(plugin_id.as_str())
            .is_some_and(|instance| instance.contribution_action_in_flight)
        {
            return true;
        }
        self.operations
            .as_ref()
            .is_some_and(|operations| operations.active_plugin_ids().contains(plugin_id))
            || self
                .resources
                .as_ref()
                .is_some_and(|resources| resources.active_plugin_ids().contains(plugin_id))
    }

    #[allow(clippy::too_many_arguments)]
    fn replace_settings_values(
        &self,
        request_id: RequestId,
        plugin_id: PluginId,
        expected_package_sha256: String,
        expected_installed_state_version: WireSequence,
        expected_schema_sha256: String,
        expected_revision: WireSequence,
        values: PluginSettingsValues,
    ) -> CoreResult<PluginSettingsSnapshot> {
        self.require_ready(request_id.clone())?;
        let operation_id = Uuid::new_v4();
        let _reservation = self
            .reserve_plugin_mutation(&plugin_id, operation_id)
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        if self.settings_operation_in_flight(&plugin_id) {
            return Err(plugin_conflict_error(request_id, None));
        }
        let installed = self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_installation(&plugin_id))
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        let snapshot = self
            .settings_snapshot_for_installation(request_id.clone(), &installed)?
            .ok_or_else(|| plugin_not_found_error(request_id.clone()))?;
        if snapshot.package_sha256 != expected_package_sha256
            || snapshot.installed_state_version != expected_installed_state_version
            || snapshot.schema_sha256 != expected_schema_sha256
            || snapshot.revision != expected_revision
        {
            return Err(plugin_conflict_error(request_id, Some(snapshot.revision)));
        }
        let values_json = settings_values_json(&snapshot.schema, &values)
            .map_err(|_| plugin_validation_error(request_id.clone()))?;
        self.hosts
            .with_plugin_repository(|repository| {
                repository.replace_plugin_settings(
                    &plugin_id,
                    &expected_package_sha256,
                    expected_installed_state_version,
                    &expected_schema_sha256,
                    expected_revision,
                    &values_json,
                )
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        let updated = self
            .settings_snapshot_for_installation(request_id.clone(), &installed)?
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        if let Some(instance) = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get_mut(plugin_id.as_str())
        {
            instance.settings_revision = Some(updated.revision);
        }
        self.api.subscriptions.publish_settings(
            crate::plugin_api::subscriptions::PluginSettingsSourceEvent {
                plugin_id: plugin_id.clone(),
                revision: updated.revision,
            },
        );
        if let Some(app) = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            let _ = app.emit_to(
                "main",
                EVENT_PLUGIN_SETTINGS_CHANGED,
                PluginSettingsChanged {
                    plugin_id,
                    revision: updated.revision,
                },
            );
        }
        Ok(updated)
    }

    fn replace_settings(
        &self,
        request: PluginSettingsReplaceRequest,
    ) -> CoreResult<PluginSettingsSnapshot> {
        self.replace_settings_values(
            request.meta.request_id,
            request.plugin_id,
            request.expected_package_sha256,
            request.expected_installed_state_version,
            request.expected_schema_sha256,
            request.expected_revision,
            request.values,
        )
    }

    fn reset_settings(
        &self,
        request: PluginSettingsResetRequest,
    ) -> CoreResult<PluginSettingsSnapshot> {
        let current = self
            .get_settings(PluginSettingsGetRequest {
                meta: request.meta.clone(),
                plugin_id: request.plugin_id.clone(),
            })?
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        let values = default_settings_values(&current.schema);
        self.replace_settings_values(
            request.meta.request_id,
            request.plugin_id,
            request.expected_package_sha256,
            request.expected_installed_state_version,
            request.expected_schema_sha256,
            request.expected_revision,
            values,
        )
    }
}

#[tauri::command]
pub(crate) fn plugin_settings_get(
    request: PluginSettingsGetRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Option<PluginSettingsSnapshot>> {
    service.get_settings(request)
}

#[tauri::command]
pub(crate) fn plugin_settings_replace(
    request: PluginSettingsReplaceRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginSettingsSnapshot> {
    service.replace_settings(request)
}

#[tauri::command]
pub(crate) fn plugin_settings_reset(
    request: PluginSettingsResetRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginSettingsSnapshot> {
    service.reset_settings(request)
}
