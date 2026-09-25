use std::collections::BTreeMap;

use rusqlite::params;

use crate::{AppPersistenceError, AppRepository, Result};

/// Local clocks are keyed by local UUID. A deleted row remains here so its
/// actual deletion time can be exported as a portable tombstone later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SshSyncLocalItemKind {
    Host,
    DesktopProfile,
    Identity,
    Credential,
    Secret,
    Route,
    AuthenticationPlan,
    AlgorithmPolicy,
    HeartbeatPolicy,
    MonitoringPolicy,
    LoginAutomation,
}

impl SshSyncLocalItemKind {
    fn as_db(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::DesktopProfile => "desktop_profile",
            Self::Identity => "identity",
            Self::Credential => "credential",
            Self::Secret => "secret",
            Self::Route => "route",
            Self::AuthenticationPlan => "authentication_plan",
            Self::AlgorithmPolicy => "algorithm_policy",
            Self::HeartbeatPolicy => "heartbeat_policy",
            Self::MonitoringPolicy => "monitoring_policy",
            Self::LoginAutomation => "login_automation",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "host" => Ok(Self::Host),
            "desktop_profile" => Ok(Self::DesktopProfile),
            "identity" => Ok(Self::Identity),
            "credential" => Ok(Self::Credential),
            "secret" => Ok(Self::Secret),
            "route" => Ok(Self::Route),
            "authentication_plan" => Ok(Self::AuthenticationPlan),
            "algorithm_policy" => Ok(Self::AlgorithmPolicy),
            "heartbeat_policy" => Ok(Self::HeartbeatPolicy),
            "monitoring_policy" => Ok(Self::MonitoringPolicy),
            "login_automation" => Ok(Self::LoginAutomation),
            _ => Err(AppPersistenceError::InvalidStoredData),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SshSyncLocalItemTime {
    pub update_time_unix_ms: i64,
    pub deleted: bool,
}

/// A trusted, authenticated remote clock to install in the same metadata
/// transaction as the corresponding object change. `None` means unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSyncLocalItemTimeOverride {
    pub kind: SshSyncLocalItemKind,
    pub local_object_id: String,
    pub update_time_unix_ms: Option<i64>,
    pub deleted: bool,
}

impl AppRepository {
    /// Forget a previous secret clock before mutating the independent Vault.
    /// A failed mutation remains unknown rather than claiming the old payload time.
    pub fn invalidate_ssh_sync_secret_time(&self, secret_ref_id: &str) -> Result<()> {
        validate_secret_ref_id(secret_ref_id)?;
        self.connection.execute(
            "DELETE FROM ssh_sync_local_item_times
             WHERE object_kind = 'secret' AND local_object_id = ?1",
            [secret_ref_id],
        )?;
        Ok(())
    }

    /// Record the time of a completed Vault secret mutation, never a snapshot time.
    pub fn record_ssh_sync_secret_time(&self, secret_ref_id: &str) -> Result<()> {
        validate_secret_ref_id(secret_ref_id)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| AppPersistenceError::InvalidStoredData)?
            .as_millis();
        let now = i64::try_from(now).map_err(|_| AppPersistenceError::InvalidStoredData)?;
        self.connection.execute(
            "INSERT INTO ssh_sync_local_item_times
             (object_kind, local_object_id, update_time_unix_ms, deleted)
             VALUES ('secret', ?1, ?2, 0)
             ON CONFLICT(object_kind, local_object_id) DO UPDATE SET
               update_time_unix_ms = excluded.update_time_unix_ms, deleted = 0",
            params![secret_ref_id, now],
        )?;
        Ok(())
    }

    pub fn list_ssh_sync_local_item_times(
        &self,
    ) -> Result<BTreeMap<(SshSyncLocalItemKind, String), SshSyncLocalItemTime>> {
        let mut statement = self.connection.prepare(
            "SELECT object_kind, local_object_id, update_time_unix_ms, deleted
             FROM ssh_sync_local_item_times",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (kind, id, time, deleted) = row?;
            if time <= 0 || uuid::Uuid::parse_str(&id).is_err() {
                return Err(AppPersistenceError::InvalidStoredData);
            }
            Ok((
                (SshSyncLocalItemKind::from_db(&kind)?, id),
                SshSyncLocalItemTime {
                    update_time_unix_ms: time,
                    deleted,
                },
            ))
        })
        .collect()
    }
}

fn validate_secret_ref_id(secret_ref_id: &str) -> Result<()> {
    uuid::Uuid::parse_str(secret_ref_id)
        .map(|_| ())
        .map_err(|_| AppPersistenceError::InvalidInput("invalid SSH sync secret reference"))
}

pub(super) fn apply_local_item_time_overrides(
    transaction: &rusqlite::Transaction<'_>,
    overrides: &[SshSyncLocalItemTimeOverride],
) -> Result<()> {
    if overrides.len() > 65_536 {
        return Err(AppPersistenceError::InvalidInput(
            "too many SSH sync item times",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for item in overrides {
        if uuid::Uuid::parse_str(&item.local_object_id).is_err()
            || item.update_time_unix_ms.is_some_and(|value| value <= 0)
            || !seen.insert((item.kind, item.local_object_id.as_str()))
        {
            return Err(AppPersistenceError::InvalidInput(
                "invalid SSH sync item time",
            ));
        }
        if let Some(time) = item.update_time_unix_ms {
            transaction.execute(
                "INSERT INTO ssh_sync_local_item_times
                 (object_kind, local_object_id, update_time_unix_ms, deleted)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(object_kind, local_object_id) DO UPDATE SET
                   update_time_unix_ms = excluded.update_time_unix_ms,
                   deleted = excluded.deleted",
                params![item.kind.as_db(), item.local_object_id, time, item.deleted],
            )?;
        } else {
            transaction.execute(
                "DELETE FROM ssh_sync_local_item_times
                 WHERE object_kind = ?1 AND local_object_id = ?2",
                params![item.kind.as_db(), item.local_object_id],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_real_edits_and_deletions_and_preserves_authenticated_source_time() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("norishell.sqlite3");
        let mut repository = AppRepository::open(&path).unwrap();
        let identity_id = uuid::Uuid::new_v4().to_string();
        repository
            .connection
            .execute(
                "INSERT INTO identities
                 (id, label, username, state_version, created_at_ms, updated_at_ms)
                 VALUES (?1, 'first', NULL, 1, 1000, 1000)",
                [&identity_id],
            )
            .unwrap();
        let key = (SshSyncLocalItemKind::Identity, identity_id.clone());
        let times = repository.list_ssh_sync_local_item_times().unwrap();
        assert_eq!(times[&key].update_time_unix_ms, 1000);
        assert!(!times[&key].deleted);

        repository
            .connection
            .execute(
                "UPDATE identities SET label = 'second', state_version = 2,
                 updated_at_ms = 2000 WHERE id = ?1",
                [&identity_id],
            )
            .unwrap();
        assert_eq!(
            repository.list_ssh_sync_local_item_times().unwrap()[&key].update_time_unix_ms,
            2000
        );

        let transaction = repository.connection.transaction().unwrap();
        transaction
            .execute(
                "UPDATE identities SET label = 'remote' WHERE id = ?1",
                [&identity_id],
            )
            .unwrap();
        apply_local_item_time_overrides(
            &transaction,
            &[SshSyncLocalItemTimeOverride {
                kind: SshSyncLocalItemKind::Identity,
                local_object_id: identity_id.clone(),
                update_time_unix_ms: Some(1500),
                deleted: false,
            }],
        )
        .unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            repository.list_ssh_sync_local_item_times().unwrap()[&key].update_time_unix_ms,
            1500
        );

        repository
            .connection
            .execute("DELETE FROM identities WHERE id = ?1", [&identity_id])
            .unwrap();
        let deleted = repository.list_ssh_sync_local_item_times().unwrap()[&key];
        assert!(deleted.deleted);
        assert!(deleted.update_time_unix_ms > 1500);
        drop(repository);
        let reopened = AppRepository::open(&path).unwrap();
        assert_eq!(
            reopened.list_ssh_sync_local_item_times().unwrap()[&key],
            deleted
        );

        let secret_ref_id = uuid::Uuid::new_v4().to_string();
        reopened
            .record_ssh_sync_secret_time(&secret_ref_id)
            .unwrap();
        let secret_key = (SshSyncLocalItemKind::Secret, secret_ref_id.clone());
        let secret = reopened.list_ssh_sync_local_item_times().unwrap()[&secret_key];
        assert!(!secret.deleted);
        assert!(secret.update_time_unix_ms > 1500);
        reopened
            .invalidate_ssh_sync_secret_time(&secret_ref_id)
            .unwrap();
        assert!(
            !reopened
                .list_ssh_sync_local_item_times()
                .unwrap()
                .contains_key(&secret_key)
        );
    }
}
