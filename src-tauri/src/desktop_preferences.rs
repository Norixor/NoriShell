//! Core-owned desktop preferences command surface.
//!
//! Preferences are intentionally read from the application's one SQLite
//! repository on every request. There is no independent in-memory projection:
//! failed or uncertain persistence therefore cannot be presented as a rolled
//! back successful setting change.

use norishell_app_persistence::{
    AppPersistenceError, DesktopPreferencesRecord, Result as PersistenceResult,
};
use norishell_core_api::{
    CoreApiError, DesktopPreferences, DesktopPreferencesGetRequest,
    DesktopPreferencesReplaceRequest, DesktopPreferencesSnapshot, DesktopWindowCloseBehavior,
    ErrorCategory, RequestId, RetryStrategy, WireSequence,
};
use tauri::{AppHandle, State};

use crate::{core_api_error::core_error, host_service::HostService};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

#[derive(Clone)]
pub(crate) struct DesktopPreferencesService {
    hosts: HostService,
}

impl DesktopPreferencesService {
    pub(crate) fn new(hosts: HostService) -> Self {
        Self { hosts }
    }

    /// Reads the complete durable preference snapshot for Core workers such as
    /// the tray and notification dispatcher. Errors stay visible to callers;
    /// callers must never substitute a default that could turn an unavailable
    /// durable setting into an unintended action.
    pub(crate) fn snapshot(&self) -> PersistenceResult<DesktopPreferencesSnapshot> {
        self.hosts
            .with_desktop_repository(|repository| repository.get_desktop_preferences())
            .map(snapshot_from_record)
    }

    /// Reads only the current preference values for Core consumers that do
    /// not require the compare-and-swap revision.
    pub(crate) fn preferences(&self) -> PersistenceResult<DesktopPreferences> {
        self.snapshot().map(|snapshot| snapshot.preferences)
    }

    pub(crate) fn window_close_behavior(&self) -> PersistenceResult<DesktopWindowCloseBehavior> {
        self.preferences()
            .map(|preferences| preferences.window_close_behavior)
    }

    fn replace(
        &self,
        expected_revision: WireSequence,
        preferences: &DesktopPreferences,
    ) -> PersistenceResult<DesktopPreferencesSnapshot> {
        self.hosts
            .with_desktop_repository(|repository| {
                repository.replace_desktop_preferences(expected_revision, preferences)
            })
            .map(snapshot_from_record)
    }
}

#[tauri::command]
pub(crate) fn desktop_preferences_get(
    request: DesktopPreferencesGetRequest,
    service: State<'_, DesktopPreferencesService>,
) -> CoreResult<DesktopPreferencesSnapshot> {
    service
        .snapshot()
        .map_err(|error| map_preferences_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) fn desktop_preferences_replace<R: tauri::Runtime>(
    app: AppHandle<R>,
    request: DesktopPreferencesReplaceRequest,
    service: State<'_, DesktopPreferencesService>,
) -> CoreResult<DesktopPreferencesSnapshot> {
    let request_id = request.meta.request_id;
    let snapshot = service
        .replace(request.expected_revision, &request.preferences)
        .map_err(|error| map_preferences_error(request_id, error))?;
    crate::tray_service::preferences_changed(&app);
    Ok(snapshot)
}

fn snapshot_from_record(record: DesktopPreferencesRecord) -> DesktopPreferencesSnapshot {
    DesktopPreferencesSnapshot {
        preferences: record.preferences,
        revision: record.revision,
    }
}

/// Projects storage errors to a stable public contract without exposing
/// database paths, SQLite messages, or other host metadata.
pub(crate) fn map_preferences_error(
    request_id: RequestId,
    error: AppPersistenceError,
) -> Box<CoreApiError> {
    let (code, category, retry_strategy, message_key) = match error {
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::Endpoint(_) => (
            "desktop_preferences.invalid_input",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.desktopPreferences.invalidInput",
        ),
        AppPersistenceError::Conflict | AppPersistenceError::IdempotencyConflict => (
            "desktop_preferences.conflict",
            ErrorCategory::Conflict,
            RetryStrategy::RefreshSnapshot,
            "errors.desktopPreferences.conflict",
        ),
        AppPersistenceError::UnsupportedSchema(_) => (
            "desktop_preferences.unsupported_schema",
            ErrorCategory::Incompatible,
            RetryStrategy::Upgrade,
            "errors.desktopPreferences.unsupportedSchema",
        ),
        AppPersistenceError::NotFound
        | AppPersistenceError::InvalidStoredData
        | AppPersistenceError::RequiresReload
        | AppPersistenceError::RestoreCommitUnknown => (
            "desktop_preferences.requires_reconciliation",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::Reconcile,
            "errors.desktopPreferences.requiresReconciliation",
        ),
        AppPersistenceError::KnownHostMismatch { .. }
        | AppPersistenceError::KnownHostAlgorithmChanged { .. }
        | AppPersistenceError::Database(_)
        | AppPersistenceError::Io(_) => (
            "desktop_preferences.persistence_unavailable",
            ErrorCategory::Unavailable,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.desktopPreferences.persistenceUnavailable",
        ),
    };
    core_error(request_id, code, category, retry_strategy, message_key)
}

pub(crate) fn main_window_required_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "desktop_preferences.main_window_required",
        ErrorCategory::Permission,
        RetryStrategy::Never,
        "errors.desktopPreferences.mainWindowRequired",
    )
}

pub(crate) fn tray_unavailable_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "desktop_preferences.tray_unavailable",
        ErrorCategory::Unavailable,
        RetryStrategy::AfterMilliseconds(1_000),
        "errors.desktopPreferences.trayUnavailable",
    )
}

pub(crate) fn window_hide_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "desktop_preferences.window_hide_failed",
        ErrorCategory::Unavailable,
        RetryStrategy::AfterMilliseconds(1_000),
        "errors.desktopPreferences.windowHideFailed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_conflict_keeps_a_refreshable_typed_error() {
        let error = map_preferences_error(RequestId::new(), AppPersistenceError::Conflict);
        assert_eq!(error.code, "desktop_preferences.conflict");
        assert_eq!(error.category, ErrorCategory::Conflict);
        assert_eq!(error.retry_strategy, RetryStrategy::RefreshSnapshot);
    }

    #[test]
    fn unavailable_tray_is_a_typed_failure() {
        let error = tray_unavailable_error(RequestId::new());
        assert_eq!(error.code, "desktop_preferences.tray_unavailable");
        assert_eq!(error.category, ErrorCategory::Unavailable);
    }
}
