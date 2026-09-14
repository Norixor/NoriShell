//! Core-owned desktop preferences persisted with the application's SQLite metadata.
//!
//! The row is deliberately fixed-width rather than a JSON blob: all reads stay
//! bounded, SQLite enforces the version-one domain, and replacement is a
//! compare-and-swap transaction over the one durable row.

use crate::{
    AppPersistenceError, AppRepository, Result, WireSequence, next_revision, u64_to_i64,
    unix_time_ms,
};
use norishell_core_api::{DesktopPreferences, DesktopWindowCloseBehavior};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopPreferencesRecord {
    pub preferences: DesktopPreferences,
    pub revision: WireSequence,
}

struct RawDesktopPreferences {
    revision: i64,
    window_close_behavior: String,
    tray_show_status: i64,
    tray_recent_limit: i64,
    tray_show_host_names: i64,
    notification_background_only: i64,
    notification_failure_only: i64,
    notify_transfer_completed: i64,
    notify_transfer_failed: i64,
    notify_disconnected: i64,
}

impl AppRepository {
    /// Reads the current durable singleton. There is no service-side cache, so
    /// callers never observe an in-memory preference change after a failed
    /// SQLite write.
    pub fn get_desktop_preferences(&self) -> Result<DesktopPreferencesRecord> {
        read_desktop_preferences(&self.connection)
    }

    /// Replaces the singleton only when the caller still holds its revision.
    /// The update and revision advance share one `BEGIN IMMEDIATE` SQLite
    /// transaction; a failed transaction leaves no in-memory mirror to roll
    /// back or misrepresent.
    pub fn replace_desktop_preferences(
        &mut self,
        expected_revision: WireSequence,
        preferences: &DesktopPreferences,
    ) -> Result<DesktopPreferencesRecord> {
        validate_preferences(preferences)?;
        let next = u64_to_i64(next_revision(expected_revision)?)?;
        let expected_revision = u64_to_i64(expected_revision.get())?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE desktop_preferences
             SET revision = ?1,
                 window_close_behavior = ?2,
                 tray_show_status = ?3,
                 tray_recent_limit = ?4,
                 tray_show_host_names = ?5,
                 notification_background_only = ?6,
                 notification_failure_only = ?7,
                 notify_transfer_completed = ?8,
                 notify_transfer_failed = ?9,
                 notify_disconnected = ?10,
                 updated_at_ms = ?11
             WHERE singleton = 1 AND revision = ?12",
            params![
                next,
                close_behavior_value(preferences.window_close_behavior),
                sqlite_bool(preferences.tray_show_status),
                i64::from(preferences.tray_recent_limit),
                sqlite_bool(preferences.tray_show_host_names),
                sqlite_bool(preferences.notification_background_only),
                sqlite_bool(preferences.notification_failure_only),
                sqlite_bool(preferences.notify_transfer_completed),
                sqlite_bool(preferences.notify_transfer_failed),
                sqlite_bool(preferences.notify_disconnected),
                unix_time_ms(),
                expected_revision,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;

        // Read after commit rather than manufacturing a response from the
        // request. The returned snapshot is the actual, bounded durable row.
        self.get_desktop_preferences()
    }
}

fn read_desktop_preferences(connection: &rusqlite::Connection) -> Result<DesktopPreferencesRecord> {
    let raw = connection
        .query_row(
            "SELECT revision,
                    window_close_behavior,
                    tray_show_status,
                    tray_recent_limit,
                    tray_show_host_names,
                    notification_background_only,
                    notification_failure_only,
                    notify_transfer_completed,
                    notify_transfer_failed,
                    notify_disconnected
             FROM desktop_preferences
             WHERE singleton = 1",
            [],
            |row| {
                Ok(RawDesktopPreferences {
                    revision: row.get(0)?,
                    window_close_behavior: row.get(1)?,
                    tray_show_status: row.get(2)?,
                    tray_recent_limit: row.get(3)?,
                    tray_show_host_names: row.get(4)?,
                    notification_background_only: row.get(5)?,
                    notification_failure_only: row.get(6)?,
                    notify_transfer_completed: row.get(7)?,
                    notify_transfer_failed: row.get(8)?,
                    notify_disconnected: row.get(9)?,
                })
            },
        )
        .optional()?
        .ok_or(AppPersistenceError::InvalidStoredData)?;
    decode_preferences(raw)
}

fn decode_preferences(raw: RawDesktopPreferences) -> Result<DesktopPreferencesRecord> {
    let preferences = DesktopPreferences {
        window_close_behavior: match raw.window_close_behavior.as_str() {
            "hide" => DesktopWindowCloseBehavior::Hide,
            "quit" => DesktopWindowCloseBehavior::Quit,
            _ => return Err(AppPersistenceError::InvalidStoredData),
        },
        tray_show_status: decode_bool(raw.tray_show_status)?,
        tray_recent_limit: u8::try_from(raw.tray_recent_limit)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
        tray_show_host_names: decode_bool(raw.tray_show_host_names)?,
        notification_background_only: decode_bool(raw.notification_background_only)?,
        notification_failure_only: decode_bool(raw.notification_failure_only)?,
        notify_transfer_completed: decode_bool(raw.notify_transfer_completed)?,
        notify_transfer_failed: decode_bool(raw.notify_transfer_failed)?,
        notify_disconnected: decode_bool(raw.notify_disconnected)?,
    };
    validate_preferences(&preferences).map_err(|_| AppPersistenceError::InvalidStoredData)?;
    let revision = u64::try_from(raw.revision)
        .map(WireSequence::new)
        .map_err(|_| AppPersistenceError::InvalidStoredData)?;
    if revision.get() == 0 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(DesktopPreferencesRecord {
        preferences,
        revision,
    })
}

fn validate_preferences(preferences: &DesktopPreferences) -> Result<()> {
    preferences
        .validate()
        .map_err(AppPersistenceError::InvalidInput)
}

const fn sqlite_bool(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn decode_bool(value: i64) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(AppPersistenceError::InvalidStoredData),
    }
}

const fn close_behavior_value(value: DesktopWindowCloseBehavior) -> &'static str {
    match value {
        DesktopWindowCloseBehavior::Hide => "hide",
        DesktopWindowCloseBehavior::Quit => "quit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> (tempfile::TempDir, AppRepository) {
        let directory = tempfile::tempdir().expect("tempdir");
        let repository =
            AppRepository::open(directory.path().join("norishell.sqlite3")).expect("repository");
        (directory, repository)
    }

    #[test]
    fn defaults_are_materialized_once_with_revision_one() {
        let (_directory, repository) = repository();
        assert_eq!(
            repository.get_desktop_preferences().expect("snapshot"),
            DesktopPreferencesRecord {
                preferences: DesktopPreferences::default(),
                revision: WireSequence::new(1),
            }
        );
    }

    #[test]
    fn replacement_is_cas_and_returns_the_committed_row() {
        let (_directory, mut repository) = repository();
        let initial = repository
            .get_desktop_preferences()
            .expect("initial snapshot");
        let mut replacement = initial.preferences.clone();
        replacement.window_close_behavior = DesktopWindowCloseBehavior::Quit;
        replacement.tray_recent_limit = 10;
        replacement.notify_transfer_failed = true;

        let committed = repository
            .replace_desktop_preferences(initial.revision, &replacement)
            .expect("commit replacement");
        assert_eq!(committed.revision, WireSequence::new(2));
        assert_eq!(committed.preferences, replacement);
        assert!(matches!(
            repository
                .replace_desktop_preferences(initial.revision, &DesktopPreferences::default()),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .get_desktop_preferences()
                .expect("durable snapshot"),
            committed
        );
    }

    #[test]
    fn invalid_input_does_not_change_the_durable_snapshot() {
        let (_directory, mut repository) = repository();
        let before = repository.get_desktop_preferences().expect("before");
        let mut invalid = before.preferences.clone();
        invalid.tray_recent_limit = 11;

        assert!(matches!(
            repository.replace_desktop_preferences(before.revision, &invalid),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        assert_eq!(repository.get_desktop_preferences().expect("after"), before);
    }

    #[test]
    fn failed_sqlite_write_does_not_change_the_durable_snapshot() {
        let (_directory, mut repository) = repository();
        let before = repository.get_desktop_preferences().expect("before");
        repository
            .connection
            .execute_batch(
                "CREATE TRIGGER desktop_preferences_reject_update
                 BEFORE UPDATE ON desktop_preferences
                 BEGIN SELECT RAISE(ABORT, 'fixture rejects update'); END;",
            )
            .expect("rejecting fixture trigger");

        let mut replacement = before.preferences.clone();
        replacement.window_close_behavior = DesktopWindowCloseBehavior::Quit;
        assert!(matches!(
            repository.replace_desktop_preferences(before.revision, &replacement),
            Err(AppPersistenceError::Database(_))
        ));
        assert_eq!(repository.get_desktop_preferences().expect("after"), before);
    }

    #[test]
    fn version_40_database_migrates_to_the_default_singleton() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("norishell.sqlite3");
        {
            let repository = AppRepository::open(&path).expect("current repository");
            crate::remove_schema_added_after_fixture_version(&repository.connection, 40)
                .expect("v40 schema");
            repository
                .connection
                .execute_batch("PRAGMA user_version = 40;")
                .expect("v40 fixture");
        }

        let repository = AppRepository::open(&path).expect("migrated repository");
        assert_eq!(
            repository
                .get_desktop_preferences()
                .expect("migrated snapshot"),
            DesktopPreferencesRecord {
                preferences: DesktopPreferences::default(),
                revision: WireSequence::new(1),
            }
        );
    }
}
