//! Desktop profiles store only non-secret fields; foreign keys protect referenced hosts and credentials.
use crate::{
    AppPersistenceError, AppRepository, CredentialRecordDetails, HostCreatePasswordStageState,
    Result,
};
use norishell_core_api::{
    CredentialRefId, DesktopPasswordStage, DesktopProfile, HostCreatePasswordStageId, IdentityId,
    OperationId, WireSequence,
};
use norishell_ssh_domain::Endpoint;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::Value;

pub fn validate_desktop_profile(profile: &DesktopProfile) -> Result<()> {
    let invalid = || AppPersistenceError::InvalidInput("invalid desktop profile");
    uuid::Uuid::parse_str(&profile.id).map_err(|_| invalid())?;
    Endpoint::parse(&profile.address, profile.port).map_err(|_| invalid())?;
    if profile.label.trim().is_empty()
        || profile.label.len() > 256
        || profile.username.len() > 256
        || profile.domain.len() > 256
        || [&profile.label, &profile.username, &profile.domain]
            .iter()
            .any(|value| value.chars().any(char::is_control))
        || (profile.protocol == norishell_core_api::DesktopProtocol::Vnc
            && profile.audio_playback_enabled)
        || profile.width == 0
        || profile.height == 0
        || profile.width > 8192
        || profile.height > 8192
        || u32::from(profile.width) * u32::from(profile.height) > 16_777_216
    {
        return Err(invalid());
    }
    Ok(())
}

impl AppRepository {
    pub fn list_desktop_profiles(&self) -> Result<Vec<DesktopProfile>> {
        let mut statement = self
            .connection
            .prepare("SELECT profile_json FROM desktop_profiles ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            let json = row?;
            if json.len() > 8192 {
                return Err(AppPersistenceError::InvalidInput("invalid desktop profile"));
            }
            let value: Value = serde_json::from_str(&json)
                .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
            if value.get("protocol").and_then(Value::as_str) == Some("xdmcp") {
                return Ok(None);
            }
            let profile: DesktopProfile = serde_json::from_value(value)
                .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
            validate_desktop_profile(&profile)?;
            Ok(Some(profile))
        })
        .filter_map(Result::transpose)
        .collect()
    }

    pub fn save_desktop_profile(&mut self, profile: &DesktopProfile) -> Result<DesktopProfile> {
        validate_desktop_profile(profile)?;
        self.validate_desktop_profile_references(profile)?;
        let mut saved = profile.clone();
        let next = profile
            .revision
            .get()
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(AppPersistenceError::Conflict)?;
        saved.revision = WireSequence::new(next);
        let json = serde_json::to_string(&saved)
            .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
        if json.len() > 8192 {
            return Err(AppPersistenceError::InvalidInput("invalid desktop profile"));
        }
        let changed = if profile.revision.get() == 0 {
            self.connection.execute("INSERT INTO desktop_profiles (id, profile_json, revision, host_id, gateway_host_id, credential_ref_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO NOTHING",
                params![saved.id, json, next as i64, saved.host_id.as_ref().map(|id| id.as_str()), saved.gateway_host_id.as_ref().map(|id| id.as_str()), saved.credential_ref_id.as_ref().map(|id| id.as_str())])?
        } else {
            self.connection.execute("UPDATE desktop_profiles SET profile_json=?2, revision=?3, host_id=?4, gateway_host_id=?5, credential_ref_id=?6 WHERE id=?1 AND revision=?7",
                params![saved.id, json, next as i64, saved.host_id.as_ref().map(|id| id.as_str()), saved.gateway_host_id.as_ref().map(|id| id.as_str()), saved.credential_ref_id.as_ref().map(|id| id.as_str()), profile.revision.get() as i64])?
        };
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        Ok(saved)
    }

    /// Consumes a Host-create password stage as the first save of one desktop profile. The
    /// stage, dedicated Identity/credential, profile and durable replay receipt commit together.
    pub fn save_desktop_profile_with_password_stage(
        &mut self,
        profile: &DesktopProfile,
        password_stage: &DesktopPasswordStage,
    ) -> Result<DesktopProfile> {
        if profile.revision.get() != 0 || profile.credential_ref_id.is_some() {
            return Err(AppPersistenceError::InvalidInput(
                "staged desktop password requires a new profile without a credential",
            ));
        }
        let normalized_profile = normalized_staged_desktop_profile(profile)?;
        self.validate_desktop_profile_references(&normalized_profile)?;
        let idempotency_key = super::validated_idempotency_key(&password_stage.idempotency_key)?;
        let request_json = desktop_password_save_request_json(
            &normalized_profile,
            &password_stage.operation_id,
            &idempotency_key,
            &password_stage.staged_password_id,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        let existing = lookup_desktop_password_receipt(
            &transaction,
            &password_stage.staged_password_id,
            &password_stage.operation_id,
            &idempotency_key,
        )?;
        if !existing.is_empty() {
            if existing.len() != 1
                || !desktop_password_receipt_matches_request(
                    &existing[0],
                    &password_stage.staged_password_id,
                    &password_stage.operation_id,
                    &idempotency_key,
                    &request_json,
                )
            {
                return Err(AppPersistenceError::IdempotencyConflict);
            }
            let replay = replay_desktop_password_receipt(&transaction, &existing[0])?;
            transaction.commit()?;
            return Ok(replay);
        }

        let now = super::unix_time_ms();
        let staged = super::require_staged_host_create_password(
            &transaction,
            &password_stage.staged_password_id,
            &password_stage.operation_id,
            &idempotency_key,
            now,
        )?;
        let identity_id = IdentityId::new();
        transaction.execute(
            "INSERT INTO identities
             (id, label, username, state_version, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 1, ?4, ?4)",
            params![
                identity_id.as_str(),
                staged.identity_label,
                normalized_profile.username,
                now,
            ],
        )?;
        let credential_ref_id = super::insert_ready_password_credential(
            &transaction,
            &identity_id,
            &staged.secret_ref_id,
            &staged.credential_label,
            now,
        )?;
        let mut saved = normalized_profile.clone();
        saved.credential_ref_id = Some(credential_ref_id.clone());
        saved.revision = WireSequence::new(1);
        let saved_json = profile_json(&saved)?;
        let inserted = transaction.execute(
            "INSERT INTO desktop_profiles
             (id, profile_json, revision, host_id, gateway_host_id, credential_ref_id)
             VALUES (?1, ?2, 1, ?3, ?4, ?5)",
            params![
                saved.id,
                saved_json,
                saved.host_id.as_ref().map(|id| id.as_str()),
                saved.gateway_host_id.as_ref().map(|id| id.as_str()),
                credential_ref_id.as_str(),
            ],
        )?;
        if inserted != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let consumed = transaction.execute(
            "UPDATE host_create_password_stages
             SET state = 'consumed', updated_at_ms = ?1
             WHERE stage_id = ?2 AND state = 'staged'",
            params![now, staged.staged_password_id.as_str()],
        )?;
        if consumed != 1 {
            return Err(AppPersistenceError::RequiresReload);
        }
        transaction.execute(
            "INSERT INTO desktop_profile_password_receipts
             (consumer, stage_id, profile_id, identity_id, credential_ref_id, operation_id,
              idempotency_key, request_json, saved_profile_json, created_at_ms)
             VALUES ('desktop', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                staged.staged_password_id.as_str(),
                saved.id,
                identity_id.as_str(),
                credential_ref_id.as_str(),
                password_stage.operation_id.as_str(),
                idempotency_key,
                request_json,
                saved_json,
                now,
            ],
        )?;
        transaction.commit()?;
        Ok(saved)
    }

    fn validate_desktop_profile_references(&self, profile: &DesktopProfile) -> Result<()> {
        for id in [&profile.host_id, &profile.gateway_host_id]
            .into_iter()
            .flatten()
        {
            self.get_host(id)?;
        }
        if let Some(id) = &profile.credential_ref_id
            && !matches!(
                self.get_ready_credential_record(id)?.details,
                CredentialRecordDetails::Password { .. }
            )
        {
            return Err(AppPersistenceError::InvalidInput(
                "desktop requires password credential",
            ));
        }
        Ok(())
    }

    pub fn delete_desktop_profile(&mut self, id: &str, revision: WireSequence) -> Result<()> {
        if revision.get() > i64::MAX as u64 {
            return Err(AppPersistenceError::Conflict);
        }
        let existing: Option<i64> = self
            .connection
            .query_row(
                "SELECT revision FROM desktop_profiles WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if existing != Some(revision.get() as i64) {
            return Err(AppPersistenceError::Conflict);
        }
        let changed = self.connection.execute(
            "DELETE FROM desktop_profiles WHERE id=?1 AND revision=?2",
            params![id, revision.get() as i64],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct DesktopPasswordReceipt {
    stage_id: HostCreatePasswordStageId,
    profile_id: String,
    identity_id: IdentityId,
    credential_ref_id: CredentialRefId,
    operation_id: OperationId,
    idempotency_key: String,
    request_json: String,
    saved_profile_json: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NormalizedDesktopPasswordSaveRequest<'a> {
    consumer: &'static str,
    profile: &'a DesktopProfile,
    operation_id: &'a OperationId,
    idempotency_key: &'a str,
    staged_password_id: &'a HostCreatePasswordStageId,
}

fn normalized_staged_desktop_profile(profile: &DesktopProfile) -> Result<DesktopProfile> {
    validate_desktop_profile(profile)?;
    let endpoint = Endpoint::parse(&profile.address, profile.port)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
    let id = uuid::Uuid::parse_str(&profile.id)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
    let mut normalized = profile.clone();
    normalized.id = id.to_string();
    normalized.label = profile.label.trim().to_owned();
    normalized.address = endpoint.normalized_address().to_owned();
    normalized.username = profile.username.trim().to_owned();
    normalized.domain = profile.domain.trim().to_owned();
    validate_desktop_profile(&normalized)?;
    Ok(normalized)
}

fn desktop_password_save_request_json(
    profile: &DesktopProfile,
    operation_id: &OperationId,
    idempotency_key: &str,
    staged_password_id: &HostCreatePasswordStageId,
) -> Result<String> {
    let request = NormalizedDesktopPasswordSaveRequest {
        consumer: "desktop",
        profile,
        operation_id,
        idempotency_key,
        staged_password_id,
    };
    let json = serde_json::to_string(&request)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
    if json.len() > 16 * 1024 {
        return Err(AppPersistenceError::InvalidInput("invalid desktop profile"));
    }
    Ok(json)
}

fn profile_json(profile: &DesktopProfile) -> Result<String> {
    let json = serde_json::to_string(profile)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid desktop profile"))?;
    if json.len() > 8192 {
        return Err(AppPersistenceError::InvalidInput("invalid desktop profile"));
    }
    Ok(json)
}

fn lookup_desktop_password_receipt(
    transaction: &Transaction<'_>,
    staged_password_id: &HostCreatePasswordStageId,
    operation_id: &OperationId,
    idempotency_key: &str,
) -> Result<Vec<DesktopPasswordReceipt>> {
    let mut statement = transaction.prepare(
        "SELECT stage_id, profile_id, identity_id, credential_ref_id, operation_id,
                idempotency_key, request_json, saved_profile_json
         FROM desktop_profile_password_receipts
         WHERE stage_id = ?1 OR operation_id = ?2 OR idempotency_key = ?3
         ORDER BY stage_id",
    )?;
    statement
        .query_map(
            params![
                staged_password_id.as_str(),
                operation_id.as_str(),
                idempotency_key,
            ],
            |row| {
                Ok(DesktopPasswordReceipt {
                    stage_id: super::parse_id(
                        row.get::<_, String>(0)?,
                        HostCreatePasswordStageId::parse,
                    )?,
                    profile_id: row.get(1)?,
                    identity_id: super::parse_id(row.get::<_, String>(2)?, IdentityId::parse)?,
                    credential_ref_id: super::parse_id(
                        row.get::<_, String>(3)?,
                        CredentialRefId::parse,
                    )?,
                    operation_id: super::parse_id(row.get::<_, String>(4)?, OperationId::parse)?,
                    idempotency_key: row.get(5)?,
                    request_json: row.get(6)?,
                    saved_profile_json: row.get(7)?,
                })
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn desktop_password_receipt_matches_request(
    receipt: &DesktopPasswordReceipt,
    staged_password_id: &HostCreatePasswordStageId,
    operation_id: &OperationId,
    idempotency_key: &str,
    request_json: &str,
) -> bool {
    receipt.stage_id == *staged_password_id
        && receipt.operation_id == *operation_id
        && receipt.idempotency_key == idempotency_key
        && receipt.request_json == request_json
}

fn replay_desktop_password_receipt(
    transaction: &Transaction<'_>,
    receipt: &DesktopPasswordReceipt,
) -> Result<DesktopProfile> {
    let staged = super::lookup_host_create_password_stage(
        transaction,
        "stage_id",
        receipt.stage_id.as_str(),
    )?
    .ok_or(AppPersistenceError::RequiresReload)?;
    if staged.state != HostCreatePasswordStageState::Consumed
        || staged.operation_id != receipt.operation_id
        || staged.idempotency_key != receipt.idempotency_key
    {
        return Err(AppPersistenceError::RequiresReload);
    }
    let target: Option<(String, Option<String>)> = transaction
        .query_row(
            "SELECT profile_json, credential_ref_id FROM desktop_profiles WHERE id = ?1",
            [receipt.profile_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if target.as_ref().is_none_or(|(json, credential_ref_id)| {
        json != &receipt.saved_profile_json
            || credential_ref_id.as_deref() != Some(receipt.credential_ref_id.as_str())
    }) {
        return Err(AppPersistenceError::Conflict);
    }
    let credential: Option<(String, String, String, String)> = transaction
        .query_row(
            "SELECT credential_refs.identity_id, credential_refs.kind, credential_refs.import_state,
                    credential_secret_slots.secret_ref_id
             FROM credential_refs
             JOIN credential_secret_slots
               ON credential_secret_slots.credential_ref_id = credential_refs.id
              AND credential_secret_slots.slot_index = 0
              AND credential_secret_slots.slot_kind = 'password'
             WHERE credential_refs.id = ?1",
            [receipt.credential_ref_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    if credential
        .as_ref()
        .is_none_or(|(identity_id, kind, import_state, secret_ref_id)| {
            identity_id != receipt.identity_id.as_str()
                || kind != "password"
                || import_state != "ready"
                || secret_ref_id != staged.secret_ref_id.as_str()
        })
    {
        return Err(AppPersistenceError::RequiresReload);
    }
    if receipt.saved_profile_json.len() > 8192 {
        return Err(AppPersistenceError::RequiresReload);
    }
    let profile: DesktopProfile = serde_json::from_str(&receipt.saved_profile_json)
        .map_err(|_| AppPersistenceError::RequiresReload)?;
    if profile.id != receipt.profile_id
        || profile.credential_ref_id.as_ref() != Some(&receipt.credential_ref_id)
        || profile.revision != WireSequence::new(1)
        || validate_desktop_profile(&profile).is_err()
    {
        return Err(AppPersistenceError::RequiresReload);
    }
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::{DesktopPasswordStage, DesktopProtocol, OperationId};

    fn profile() -> DesktopProfile {
        DesktopProfile {
            id: uuid::Uuid::new_v4().to_string(),
            label: "Desktop".into(),
            protocol: DesktopProtocol::Rdp,
            address: "localhost".into(),
            port: 3389,
            username: "user".into(),
            domain: String::new(),
            host_id: None,
            gateway_host_id: None,
            credential_ref_id: None,
            width: 1280,
            height: 720,
            clipboard_enabled: false,
            audio_playback_enabled: false,
            revision: WireSequence::new(0),
        }
    }

    fn stage_password(
        repository: &mut AppRepository,
        operation_id: &OperationId,
        idempotency_key: &str,
    ) -> crate::HostCreatePasswordStageRecord {
        let staged = repository
            .begin_host_create_password_stage_with_outcome(
                operation_id,
                idempotency_key,
                "Desktop password identity",
                "Desktop password",
            )
            .unwrap()
            .record;
        repository
            .mark_host_create_password_staged(&staged.staged_password_id)
            .unwrap()
    }

    fn password_stage(
        operation_id: OperationId,
        idempotency_key: &str,
        staged_password_id: HostCreatePasswordStageId,
    ) -> DesktopPasswordStage {
        DesktopPasswordStage {
            operation_id,
            idempotency_key: idempotency_key.to_owned(),
            staged_password_id,
        }
    }

    #[test]
    fn audio_is_opt_in_persisted_and_rejected_for_vnc() {
        let mut value = serde_json::to_value(profile()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("audioPlaybackEnabled");
        let restored: DesktopProfile = serde_json::from_value(value).unwrap();
        assert!(!restored.audio_playback_enabled);
        let mut audio = restored;
        audio.audio_playback_enabled = true;
        assert!(validate_desktop_profile(&audio).is_ok());
        let encoded = serde_json::to_string(&audio).unwrap();
        assert!(
            serde_json::from_str::<DesktopProfile>(&encoded)
                .unwrap()
                .audio_playback_enabled
        );
        audio.protocol = norishell_core_api::DesktopProtocol::Vnc;
        assert!(validate_desktop_profile(&audio).is_err());
    }

    #[test]
    fn rdp_and_vnc_password_stages_create_dedicated_ready_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("desktop.sqlite3")).unwrap();
        for (protocol, port, idempotency_key) in [
            (DesktopProtocol::Rdp, 3389, "desktop-rdp-password"),
            (DesktopProtocol::Vnc, 5900, "desktop-vnc-password"),
        ] {
            let operation_id = OperationId::new();
            let staged = stage_password(&mut repository, &operation_id, idempotency_key);
            let mut request = profile();
            request.protocol = protocol;
            request.port = port;
            let saved = repository
                .save_desktop_profile_with_password_stage(
                    &request,
                    &password_stage(
                        operation_id,
                        idempotency_key,
                        staged.staged_password_id.clone(),
                    ),
                )
                .unwrap();
            let credential_ref_id = saved.credential_ref_id.as_ref().unwrap();
            let credential = repository
                .get_ready_credential_record(credential_ref_id)
                .unwrap();
            assert!(matches!(
                credential.details,
                CredentialRecordDetails::Password { ref secret_ref_id }
                    if *secret_ref_id == staged.secret_ref_id
            ));
            assert_eq!(
                repository
                    .get_identity(&credential.identity_id)
                    .unwrap()
                    .label,
                "Desktop password identity"
            );
            let receipt_count: i64 = repository
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM desktop_profile_password_receipts WHERE profile_id = ?1",
                    [saved.id.as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(receipt_count, 1);
        }
        assert_eq!(repository.list_hosts().unwrap().len(), 0);
    }

    #[test]
    fn staged_desktop_password_replay_is_exact_and_target_bound() {
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("desktop.sqlite3")).unwrap();
        let operation_id = OperationId::new();
        let staged = stage_password(&mut repository, &operation_id, "desktop-save-once");
        let request = profile();
        let password_stage = password_stage(
            operation_id,
            "desktop-save-once",
            staged.staged_password_id.clone(),
        );
        let saved = repository
            .save_desktop_profile_with_password_stage(&request, &password_stage)
            .unwrap();
        assert_eq!(
            repository
                .save_desktop_profile_with_password_stage(&request, &password_stage)
                .unwrap(),
            saved
        );

        let mut changed = request.clone();
        changed.label = "Changed desktop".to_owned();
        assert!(matches!(
            repository.save_desktop_profile_with_password_stage(&changed, &password_stage),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        let mut clone = request.clone();
        clone.id = uuid::Uuid::new_v4().to_string();
        assert!(matches!(
            repository.save_desktop_profile_with_password_stage(&clone, &password_stage),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        let mut changed_target = saved;
        changed_target.label = "Current target changed".to_owned();
        repository.save_desktop_profile(&changed_target).unwrap();
        assert!(matches!(
            repository.save_desktop_profile_with_password_stage(&request, &password_stage),
            Err(AppPersistenceError::Conflict)
        ));
    }

    #[test]
    fn staged_desktop_password_rejects_expired_or_mismatched_pairs_and_rolls_back() {
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("desktop.sqlite3")).unwrap();
        let operation_id = OperationId::new();
        let staged = stage_password(&mut repository, &operation_id, "desktop-stage-rollback");
        let request = profile();
        let mismatched = password_stage(
            OperationId::new(),
            "desktop-stage-rollback",
            staged.staged_password_id.clone(),
        );
        assert!(matches!(
            repository.save_desktop_profile_with_password_stage(&request, &mismatched),
            Err(AppPersistenceError::IdempotencyConflict)
        ));
        repository
            .connection
            .execute(
                "UPDATE host_create_password_stages SET expires_at_ms = 0 WHERE stage_id = ?1",
                [staged.staged_password_id.as_str()],
            )
            .unwrap();
        let expired = password_stage(
            operation_id,
            "desktop-stage-rollback",
            staged.staged_password_id.clone(),
        );
        assert!(matches!(
            repository.save_desktop_profile_with_password_stage(&request, &expired),
            Err(AppPersistenceError::IdempotencyConflict)
        ));

        let rollback_operation = OperationId::new();
        let rollback_stage = stage_password(
            &mut repository,
            &rollback_operation,
            "desktop-stage-transaction-rollback",
        );
        let colliding = profile();
        repository.save_desktop_profile(&colliding).unwrap();
        assert!(matches!(
            repository.save_desktop_profile_with_password_stage(
                &colliding,
                &password_stage(
                    rollback_operation.clone(),
                    "desktop-stage-transaction-rollback",
                    rollback_stage.staged_password_id.clone(),
                ),
            ),
            Err(AppPersistenceError::Database(_)) | Err(AppPersistenceError::Conflict)
        ));
        let state: String = repository
            .connection
            .query_row(
                "SELECT state FROM host_create_password_stages WHERE stage_id = ?1",
                [rollback_stage.staged_password_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "staged");
        let cleanup = repository
            .begin_host_create_password_cleanup(
                &rollback_operation,
                "desktop-stage-transaction-rollback",
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            cleanup.staged_password_id,
            rollback_stage.staged_password_id
        );
        assert!(
            repository
                .finalize_host_create_password_cleanup(&rollback_stage.staged_password_id)
                .unwrap()
                .cancelled
        );
    }

    #[test]
    fn desktop_profile_save_without_a_stage_remains_available() {
        let directory = tempfile::tempdir().unwrap();
        let mut repository = AppRepository::open(directory.path().join("desktop.sqlite3")).unwrap();
        let saved = repository.save_desktop_profile(&profile()).unwrap();
        assert!(saved.credential_ref_id.is_none());
        let receipt_count: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM desktop_profile_password_receipts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receipt_count, 0);
    }

    #[test]
    fn v39_migration_preserves_existing_desktops_and_password_stage_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.sqlite3");
        let mut repository = AppRepository::open(&path).unwrap();
        let saved = repository.save_desktop_profile(&profile()).unwrap();
        let stage = stage_password(&mut repository, &OperationId::new(), "migration-stage");
        super::super::remove_schema_added_after_fixture_version(&repository.connection, 39)
            .unwrap();
        repository
            .connection
            .pragma_update(None, "user_version", 39)
            .unwrap();
        drop(repository);
        let repository = AppRepository::open(&path).unwrap();
        assert_eq!(repository.list_desktop_profiles().unwrap(), vec![saved]);
        assert!(
            repository
                .list_host_create_password_cleanup_candidates(16)
                .unwrap()
                .is_empty()
        );
        let version: i64 = repository
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::super::SCHEMA_VERSION);
        let staged = super::super::lookup_host_create_password_stage(
            &repository.connection,
            "stage_id",
            stage.staged_password_id.as_str(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(staged.state, HostCreatePasswordStageState::Staged);
    }

    #[test]
    fn v30_migration_preserves_hosts_and_enforces_desktop_references() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop.sqlite3");
        let mut repository = AppRepository::open(&path).unwrap();
        let host = repository
            .create_host("Existing gateway", "localhost", 22, Some("qa"), None, false)
            .unwrap();
        super::super::remove_schema_added_after_fixture_version(&repository.connection, 30)
            .unwrap();
        repository
            .connection
            .execute_batch(
                "DROP TABLE IF EXISTS desktop_profiles;
                 DROP TABLE IF EXISTS plugin_credentials;
                 DROP TABLE IF EXISTS plugin_workflow_task_steps;
                 DROP TABLE IF EXISTS plugin_workflow_tasks;
                 PRAGMA user_version = 30;",
            )
            .unwrap();
        drop(repository);

        let mut repository = AppRepository::open(&path).unwrap();
        assert_eq!(
            repository.get_host(&host.host_id).unwrap().label,
            "Existing gateway"
        );
        let mut desktop = profile();
        desktop.gateway_host_id = Some(host.host_id.clone());
        let saved = repository.save_desktop_profile(&desktop).unwrap();
        assert!(
            repository
                .delete_host(&host.host_id, host.state_version)
                .is_err()
        );
        repository
            .delete_desktop_profile(&saved.id, saved.revision)
            .unwrap();
        repository
            .delete_host(&host.host_id, host.state_version)
            .unwrap();
    }

    #[test]
    fn stale_profile_cannot_overwrite_or_delete_newer_revision() {
        let directory = tempfile::tempdir().unwrap();
        let mut repo = AppRepository::open(directory.path().join("desktop.sqlite3")).unwrap();
        let initial = profile();
        let saved = repo.save_desktop_profile(&initial).unwrap();
        assert!(matches!(
            repo.save_desktop_profile(&initial),
            Err(AppPersistenceError::Conflict)
        ));
        let mut edit = saved.clone();
        edit.label = "Updated".into();
        let updated = repo.save_desktop_profile(&edit).unwrap();
        assert!(matches!(
            repo.save_desktop_profile(&saved),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repo.delete_desktop_profile(&saved.id, saved.revision),
            Err(AppPersistenceError::Conflict)
        ));
        assert_eq!(repo.list_desktop_profiles().unwrap(), vec![updated.clone()]);
        repo.delete_desktop_profile(&updated.id, updated.revision)
            .unwrap();
        assert!(repo.list_desktop_profiles().unwrap().is_empty());
    }

    #[test]
    fn profile_rejects_secret_fields_and_invalid_resource_bounds() {
        let mut value = serde_json::to_value(profile()).unwrap();
        value["password"] = serde_json::json!("must never persist");
        assert!(serde_json::from_value::<DesktopProfile>(value).is_err());
        let mut oversized = profile();
        oversized.width = 8192;
        oversized.height = 8192;
        assert!(validate_desktop_profile(&oversized).is_err());
    }

    fn insert_raw_profile(repo: &mut AppRepository, id: &str, json: &str) {
        repo.connection
            .execute(
                "INSERT INTO desktop_profiles (id, profile_json, revision, host_id, gateway_host_id, credential_ref_id) VALUES (?1, ?2, 1, NULL, NULL, NULL)",
                params![id, json],
            )
            .unwrap();
    }

    #[test]
    fn retired_xdmcp_rows_are_preserved_and_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let mut repo = AppRepository::open(directory.path().join("desktop.sqlite3")).unwrap();
        let legacy_id = uuid::Uuid::new_v4().to_string();
        let mut legacy = serde_json::to_value(profile()).unwrap();
        legacy["id"] = serde_json::json!(&legacy_id);
        legacy["protocol"] = serde_json::json!("xdmcp");
        let legacy_json = serde_json::to_string(&legacy).unwrap();
        insert_raw_profile(&mut repo, &legacy_id, &legacy_json);

        let rdp = repo.save_desktop_profile(&profile()).unwrap();
        let mut vnc_profile = profile();
        vnc_profile.protocol = DesktopProtocol::Vnc;
        vnc_profile.port = 5900;
        let vnc = repo.save_desktop_profile(&vnc_profile).unwrap();

        let mut expected = vec![rdp, vnc];
        expected.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(repo.list_desktop_profiles().unwrap(), expected);
        assert_eq!(
            repo.connection
                .query_row("SELECT COUNT(*) FROM desktop_profiles", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            repo.connection
                .query_row(
                    "SELECT profile_json FROM desktop_profiles WHERE id=?1",
                    [&legacy_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            legacy_json
        );
    }

    #[test]
    fn malformed_or_unknown_desktop_profiles_are_not_hidden() {
        let directory = tempfile::tempdir().unwrap();
        let mut repo = AppRepository::open(directory.path().join("desktop.sqlite3")).unwrap();
        insert_raw_profile(&mut repo, &uuid::Uuid::new_v4().to_string(), "{");
        assert!(matches!(
            repo.list_desktop_profiles(),
            Err(AppPersistenceError::InvalidInput("invalid desktop profile"))
        ));

        repo.connection
            .execute("DELETE FROM desktop_profiles", [])
            .unwrap();
        let mut unknown = serde_json::to_value(profile()).unwrap();
        unknown["protocol"] = serde_json::json!("unknown");
        let unknown_json = serde_json::to_string(&unknown).unwrap();
        insert_raw_profile(&mut repo, &uuid::Uuid::new_v4().to_string(), &unknown_json);
        assert!(matches!(
            repo.list_desktop_profiles(),
            Err(AppPersistenceError::InvalidInput("invalid desktop profile"))
        ));
    }
}
