//! Trusted-main-window Core IPC for the six global non-secret preference groups.

use norishell_app_persistence::AppPersistenceError;
use norishell_core_api::{
    ApplicationPreferencesGetRequest, ApplicationPreferencesReplaceRequest,
    ApplicationPreferencesSnapshot, CoreApiError, ErrorCategory, RequestId, RetryStrategy,
};
use tauri::State;

use crate::{core_api_error::core_error, host_service::HostService};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

#[tauri::command]
pub(crate) fn application_preferences_get(
    request: ApplicationPreferencesGetRequest,
    hosts: State<'_, HostService>,
) -> CoreResult<ApplicationPreferencesSnapshot> {
    hosts
        .with_desktop_repository(|repository| repository.get_application_preferences(request.group))
        .map_err(|error| map_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) fn application_preferences_replace(
    request: ApplicationPreferencesReplaceRequest,
    hosts: State<'_, HostService>,
) -> CoreResult<ApplicationPreferencesSnapshot> {
    hosts
        .with_desktop_repository(|repository| {
            repository.replace_application_preferences(
                request.group,
                request.expected_revision,
                &request.value,
            )
        })
        .map_err(|error| map_error(request.meta.request_id, error))
}

fn map_error(request_id: RequestId, error: AppPersistenceError) -> Box<CoreApiError> {
    let (code, category, retry, key) = match error {
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::Endpoint(_) => (
            "application_preferences.invalid_input",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.applicationPreferences.invalidInput",
        ),
        AppPersistenceError::Conflict
        | AppPersistenceError::IdempotencyConflict
        | AppPersistenceError::DatabaseNotFresh => (
            "application_preferences.conflict",
            ErrorCategory::Conflict,
            RetryStrategy::RefreshSnapshot,
            "errors.applicationPreferences.conflict",
        ),
        AppPersistenceError::UnsupportedSchema(_) => (
            "application_preferences.unsupported_schema",
            ErrorCategory::Incompatible,
            RetryStrategy::Upgrade,
            "errors.applicationPreferences.unsupportedSchema",
        ),
        AppPersistenceError::NotFound
        | AppPersistenceError::InvalidStoredData
        | AppPersistenceError::RequiresReload
        | AppPersistenceError::RestoreCommitUnknown => (
            "application_preferences.requires_reconciliation",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::Reconcile,
            "errors.applicationPreferences.requiresReconciliation",
        ),
        AppPersistenceError::KnownHostMismatch { .. }
        | AppPersistenceError::KnownHostAlgorithmChanged { .. }
        | AppPersistenceError::Database(_)
        | AppPersistenceError::Io(_) => (
            "application_preferences.persistence_unavailable",
            ErrorCategory::Unavailable,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.applicationPreferences.persistenceUnavailable",
        ),
    };
    core_error(request_id, code, category, retry, key)
}
