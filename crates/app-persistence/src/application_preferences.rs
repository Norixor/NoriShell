//! Per-group Core-owned preference rows, with first-write and revision CAS.

use norishell_core_api::{
    ApplicationPreferenceGroupId, ApplicationPreferencesSnapshot,
    validate_application_preference_value,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

use crate::{
    AppPersistenceError, AppRepository, Result, WireSequence, next_revision, u64_to_i64,
    unix_time_ms,
};

impl AppRepository {
    pub fn get_application_preferences(
        &self,
        group: ApplicationPreferenceGroupId,
    ) -> Result<ApplicationPreferencesSnapshot> {
        read_snapshot(&self.connection, group)
    }

    pub fn replace_application_preferences(
        &mut self,
        group: ApplicationPreferenceGroupId,
        expected_revision: Option<WireSequence>,
        value: &Value,
    ) -> Result<ApplicationPreferencesSnapshot> {
        validate_application_preference_value(group, value)
            .map_err(AppPersistenceError::InvalidInput)?;
        let json = serde_json::to_string(value).map_err(|_| {
            AppPersistenceError::InvalidInput("application_preferences.invalid_input")
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = match expected_revision {
            None => transaction.execute(
                "INSERT INTO application_preferences (group_id, revision, value_json, updated_at_ms)
                 VALUES (?1, 1, ?2, ?3) ON CONFLICT(group_id) DO NOTHING",
                params![group.as_str(), json, unix_time_ms()],
            )?,
            Some(expected) => {
                if expected.get() == 0 { return Err(AppPersistenceError::Conflict); }
                let next = u64_to_i64(next_revision(expected)?)?;
                transaction.execute(
                    "UPDATE application_preferences
                     SET revision = ?1, value_json = ?2, updated_at_ms = ?3
                     WHERE group_id = ?4 AND revision = ?5",
                    params![next, json, unix_time_ms(), group.as_str(), u64_to_i64(expected.get())?],
                )?
            }
        };
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        self.get_application_preferences(group)
    }
}

fn read_snapshot(
    connection: &rusqlite::Connection,
    group: ApplicationPreferenceGroupId,
) -> Result<ApplicationPreferencesSnapshot> {
    let row: Option<(i64, String)> = connection
        .query_row(
            "SELECT revision, value_json FROM application_preferences WHERE group_id = ?1",
            [group.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((revision, json)) = row else {
        return Ok(ApplicationPreferencesSnapshot {
            group,
            revision: None,
            value: None,
        });
    };
    let revision = u64::try_from(revision).map_err(|_| AppPersistenceError::InvalidStoredData)?;
    if revision == 0 || json.len() > 64 * 1024 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    let value: Value =
        serde_json::from_str(&json).map_err(|_| AppPersistenceError::InvalidStoredData)?;
    validate_application_preference_value(group, &value)
        .map_err(|_| AppPersistenceError::InvalidStoredData)?;
    Ok(ApplicationPreferencesSnapshot {
        group,
        revision: Some(WireSequence::new(revision)),
        value: Some(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repository() -> (tempfile::TempDir, AppRepository) {
        let directory = tempfile::tempdir().unwrap();
        let repository = AppRepository::open(directory.path().join("app.sqlite3")).unwrap();
        (directory, repository)
    }

    #[test]
    fn first_write_and_replace_are_independent_cas() {
        let (_directory, mut repository) = repository();
        let group = ApplicationPreferenceGroupId::Files;
        assert_eq!(
            repository
                .get_application_preferences(group)
                .unwrap()
                .revision,
            None
        );
        let value = json!({"browser":{"showHidden":true,"foldersFirst":false,"sort":"name"},"rememberLastDirectory":false});
        let initial = repository
            .replace_application_preferences(group, None, &value)
            .unwrap();
        assert_eq!(initial.revision, Some(WireSequence::new(1)));
        assert!(matches!(
            repository.replace_application_preferences(group, None, &value),
            Err(AppPersistenceError::Conflict)
        ));
        let changed = json!({"browser":{"showHidden":true,"foldersFirst":false,"sort":"size"},"rememberLastDirectory":false});
        let next = repository
            .replace_application_preferences(group, initial.revision, &changed)
            .unwrap();
        assert_eq!(next.revision, Some(WireSequence::new(2)));
        assert_eq!(next.value, Some(changed));
        assert!(matches!(
            repository.replace_application_preferences(group, initial.revision, &value),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(
            repository
                .get_application_preferences(ApplicationPreferenceGroupId::Application)
                .unwrap()
                .revision,
            None
        );
    }

    #[test]
    fn malformed_or_local_only_fields_never_enter_core() {
        let (_directory, mut repository) = repository();
        let group = ApplicationPreferenceGroupId::Files;
        for invalid in [
            json!({"browser":{"showHidden":true,"foldersFirst":false,"sort":"name"},"rememberLastDirectory":false,"directories":{"local":"/secret"}}),
            json!({"browser":{"showHidden":true,"foldersFirst":false,"sort":"unknown"},"rememberLastDirectory":false}),
        ] {
            assert!(matches!(
                repository.replace_application_preferences(group, None, &invalid),
                Err(AppPersistenceError::InvalidInput(_))
            ));
        }
        assert_eq!(
            repository
                .get_application_preferences(group)
                .unwrap()
                .revision,
            None
        );
    }
}
