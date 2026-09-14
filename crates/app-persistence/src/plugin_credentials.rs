//! Durable, signer-bound metadata for plugin-owned network credentials.
//!
//! Secret bytes never enter this repository. A create intent is committed
//! before the corresponding Vault write, and revocation is committed before
//! the Vault entry is deleted.

use norishell_core_api::{
    PluginCredentialInjection, PluginCredentialState, PluginCredentialSummary,
    PluginCredentialTarget, PluginId, SecretRefId, WireSequence,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::{AppPersistenceError, AppRepository, Result, u64_to_i64, unix_time_ms};

const MAX_LABEL_BYTES: usize = 128;
const MAX_ORIGIN_BYTES: usize = 2_048;
const MAX_HEADER_NAME_BYTES: usize = 128;
const MAX_OPERATION_ID_BYTES: usize = 128;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCredentialOwner {
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginCredentialDurableState {
    PendingVault,
    Ready,
    CleanupPending,
    Revoked,
}

impl PluginCredentialDurableState {
    fn as_db(self) -> &'static str {
        match self {
            Self::PendingVault => "pending_vault",
            Self::Ready => "ready",
            Self::CleanupPending => "cleanup_pending",
            Self::Revoked => "revoked",
        }
    }

    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "pending_vault" => Ok(Self::PendingVault),
            "ready" => Ok(Self::Ready),
            "cleanup_pending" => Ok(Self::CleanupPending),
            "revoked" => Ok(Self::Revoked),
            _ => Err(invalid_stored_column()),
        }
    }

    fn public(self) -> PluginCredentialState {
        match self {
            Self::PendingVault => PluginCredentialState::PendingVault,
            Self::Ready => PluginCredentialState::Ready,
            Self::CleanupPending => PluginCredentialState::CleanupPending,
            Self::Revoked => PluginCredentialState::Revoked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCredentialRecord {
    pub owner: PluginCredentialOwner,
    pub handle: String,
    pub secret_ref_id: SecretRefId,
    pub operation_id: String,
    pub idempotency_key: String,
    pub label: String,
    pub target: PluginCredentialTarget,
    pub revision: WireSequence,
    pub state: PluginCredentialDurableState,
}

impl PluginCredentialRecord {
    #[must_use]
    pub fn summary(&self) -> PluginCredentialSummary {
        PluginCredentialSummary {
            handle: self.handle.clone(),
            label: self.label.clone(),
            target: self.target.clone(),
            revision: self.revision,
            state: self.state.public(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginCredentialCreateInput {
    pub owner: PluginCredentialOwner,
    pub operation_id: String,
    pub idempotency_key: String,
    pub label: String,
    pub target: PluginCredentialTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCredentialCreateBegin {
    Created(PluginCredentialRecord),
    Existing(PluginCredentialRecord),
}

impl AppRepository {
    /// Commits the metadata intent before any Vault write. Exact replay returns
    /// the stable handle and SecretRef; operation/idempotency reuse with drift
    /// fails closed.
    pub fn begin_plugin_credential_create(
        &mut self,
        input: &PluginCredentialCreateInput,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginCredentialCreateBegin> {
        validate_create_input(input)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_active_owner(&transaction, &input.owner)?;

        let matching = lookup_create_replay(&transaction, input)?;
        if !matching.is_empty() {
            if matching.len() == 1 && record_matches_create(&matching[0], input) {
                require_fence(fence)?;
                transaction.commit()?;
                return Ok(PluginCredentialCreateBegin::Existing(
                    matching.into_iter().next().expect("one matching row"),
                ));
            }
            return Err(AppPersistenceError::IdempotencyConflict);
        }

        let handle = Uuid::new_v4().to_string();
        let secret_ref_id = SecretRefId::new();
        let (injection_kind, header_name) = encode_injection(&input.target.injection);
        let now = unix_time_ms();
        transaction.execute(
            "INSERT INTO plugin_credentials
             (handle, plugin_id, signer_fingerprint_sha256, secret_ref_id,
              operation_id, idempotency_key, label, origin, injection_kind,
              header_name, revision, state, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1,
                     'pending_vault', ?11, ?11)",
            params![
                handle,
                input.owner.plugin_id.as_str(),
                input.owner.signer_fingerprint_sha256,
                secret_ref_id.as_str(),
                input.operation_id,
                input.idempotency_key,
                input.label,
                input.target.origin,
                injection_kind,
                header_name,
                now,
            ],
        )?;
        require_fence(fence)?;
        transaction.commit()?;
        self.get_plugin_credential(&input.owner, &handle)?
            .map(PluginCredentialCreateBegin::Created)
            .ok_or(AppPersistenceError::InvalidStoredData)
    }

    pub fn get_plugin_credential(
        &self,
        owner: &PluginCredentialOwner,
        handle: &str,
    ) -> Result<Option<PluginCredentialRecord>> {
        validate_owner(owner)?;
        validate_handle(handle)?;
        read_record_by_handle(&self.connection, owner, handle)
    }

    /// Reconciles a create call whose SQLite commit result was not observed.
    /// Only an exact operation plus idempotency match is returned.
    pub fn find_plugin_credential_create(
        &self,
        input: &PluginCredentialCreateInput,
    ) -> Result<Option<PluginCredentialRecord>> {
        validate_create_input(input)?;
        let matching = lookup_create_replay(&self.connection, input)?;
        match matching.as_slice() {
            [] => Ok(None),
            [record] if record_matches_create(record, input) => Ok(Some(record.clone())),
            _ => Err(AppPersistenceError::IdempotencyConflict),
        }
    }

    pub fn list_plugin_credentials(
        &self,
        owner: &PluginCredentialOwner,
        fence: &dyn Fn() -> bool,
    ) -> Result<Vec<PluginCredentialRecord>> {
        validate_owner(owner)?;
        require_fence(fence)?;
        require_active_owner(&self.connection, owner)?;
        let mut statement = self.connection.prepare(
            "SELECT handle, plugin_id, signer_fingerprint_sha256, secret_ref_id,
                    operation_id, idempotency_key, label, origin, injection_kind,
                    header_name, revision, state
             FROM plugin_credentials
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
             ORDER BY created_at_ms, handle",
        )?;
        let records = statement
            .query_map(
                params![owner.plugin_id.as_str(), owner.signer_fingerprint_sha256],
                read_record,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        require_fence(fence)?;
        Ok(records)
    }

    pub fn mark_plugin_credential_ready(
        &mut self,
        intent: &PluginCredentialRecord,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginCredentialRecord> {
        transition_exact(
            &mut self.connection,
            intent,
            PluginCredentialDurableState::PendingVault,
            PluginCredentialDurableState::Ready,
            false,
            fence,
        )
    }

    /// Makes a pending create or ready credential unusable before Vault cleanup.
    pub fn mark_plugin_credential_cleanup_pending(
        &mut self,
        owner: &PluginCredentialOwner,
        handle: &str,
        expected_revision: WireSequence,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginCredentialRecord> {
        validate_owner(owner)?;
        validate_handle(handle)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        let current = read_record_by_handle(&transaction, owner, handle)?
            .ok_or(AppPersistenceError::NotFound)?;
        if current.revision != expected_revision {
            return Err(AppPersistenceError::Conflict);
        }
        if matches!(
            current.state,
            PluginCredentialDurableState::CleanupPending | PluginCredentialDurableState::Revoked
        ) {
            transaction.commit()?;
            return Ok(current);
        }
        let next_revision = current
            .revision
            .get()
            .checked_add(1)
            .ok_or(AppPersistenceError::Conflict)?;
        let changed = transaction.execute(
            "UPDATE plugin_credentials
             SET state = 'cleanup_pending', revision = ?1, updated_at_ms = ?2
             WHERE handle = ?3 AND plugin_id = ?4 AND signer_fingerprint_sha256 = ?5
               AND revision = ?6 AND state IN ('pending_vault', 'ready')",
            params![
                u64_to_i64(next_revision)?,
                unix_time_ms(),
                handle,
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                u64_to_i64(expected_revision.get())?,
            ],
        )?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        require_fence(fence)?;
        transaction.commit()?;
        read_record_by_handle(&self.connection, owner, handle)?
            .ok_or(AppPersistenceError::InvalidStoredData)
    }

    pub fn finish_plugin_credential_cleanup(
        &mut self,
        cleanup: &PluginCredentialRecord,
    ) -> Result<PluginCredentialRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE plugin_credentials SET state = 'revoked', updated_at_ms = ?1
             WHERE handle = ?2 AND plugin_id = ?3 AND signer_fingerprint_sha256 = ?4
               AND secret_ref_id = ?5 AND revision = ?6 AND state = 'cleanup_pending'",
            params![
                unix_time_ms(),
                cleanup.handle,
                cleanup.owner.plugin_id.as_str(),
                cleanup.owner.signer_fingerprint_sha256,
                cleanup.secret_ref_id.as_str(),
                u64_to_i64(cleanup.revision.get())?,
            ],
        )?;
        if changed != 1 {
            let current = read_record_by_handle(&transaction, &cleanup.owner, &cleanup.handle)?
                .ok_or(AppPersistenceError::NotFound)?;
            if current.state == PluginCredentialDurableState::Revoked
                && current.revision == cleanup.revision
                && current.secret_ref_id == cleanup.secret_ref_id
            {
                transaction.commit()?;
                return Ok(current);
            }
            return Err(AppPersistenceError::Conflict);
        }
        transaction.commit()?;
        read_record_by_handle(&self.connection, &cleanup.owner, &cleanup.handle)?
            .ok_or(AppPersistenceError::InvalidStoredData)
    }

    /// Promotes abandoned create intents and all owner data to durable cleanup.
    /// This table intentionally has no installation FK, so uninstall cannot
    /// cascade away the only SecretRef reconciliation record.
    pub fn prepare_plugin_credential_cleanup(
        &mut self,
        owner: Option<&PluginCredentialOwner>,
    ) -> Result<Vec<PluginCredentialRecord>> {
        if let Some(owner) = owner {
            validate_owner(owner)?;
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = unix_time_ms();
        match owner {
            Some(owner) => {
                transaction.execute(
                    "UPDATE plugin_credentials
                     SET state = 'cleanup_pending', revision = revision + 1, updated_at_ms = ?1
                     WHERE plugin_id = ?2 AND signer_fingerprint_sha256 = ?3
                       AND state IN ('pending_vault', 'ready')",
                    params![
                        now,
                        owner.plugin_id.as_str(),
                        owner.signer_fingerprint_sha256
                    ],
                )?;
            }
            None => {
                let _ = now;
            }
        }
        let mut statement = transaction.prepare(
            "SELECT handle, plugin_id, signer_fingerprint_sha256, secret_ref_id,
                    operation_id, idempotency_key, label, origin, injection_kind,
                    header_name, revision, state
             FROM plugin_credentials
             WHERE state = 'cleanup_pending'
               AND (?1 IS NULL OR (plugin_id = ?1 AND signer_fingerprint_sha256 = ?2))
             ORDER BY updated_at_ms, handle",
        )?;
        let (plugin_id, signer) = owner
            .map(|owner| {
                (
                    Some(owner.plugin_id.as_str()),
                    Some(owner.signer_fingerprint_sha256.as_str()),
                )
            })
            .unwrap_or((None, None));
        let records = statement
            .query_map(params![plugin_id, signer], read_record)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        transaction.commit()?;
        Ok(records)
    }

    /// One-shot startup recovery performed before this service is exposed to
    /// plugin calls. Runtime cleanup must not classify a newly prompted create
    /// intent as abandoned.
    pub fn recover_plugin_credentials_startup(&mut self) -> Result<Vec<PluginCredentialRecord>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE plugin_credentials
             SET state = 'cleanup_pending', revision = revision + 1, updated_at_ms = ?1
             WHERE state = 'pending_vault'",
            [unix_time_ms()],
        )?;
        let mut statement = transaction.prepare(
            "SELECT handle, plugin_id, signer_fingerprint_sha256, secret_ref_id,
                    operation_id, idempotency_key, label, origin, injection_kind,
                    header_name, revision, state
             FROM plugin_credentials WHERE state = 'cleanup_pending'
             ORDER BY updated_at_ms, handle",
        )?;
        let records = statement
            .query_map([], read_record)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        transaction.commit()?;
        Ok(records)
    }

    /// Uninstall reconciliation selects every historical signer for a plugin.
    /// It does not depend on `plugin_installations`, which may already have
    /// been removed by an earlier uninstall attempt.
    pub fn prepare_plugin_credential_cleanup_by_plugin(
        &mut self,
        plugin_id: &PluginId,
    ) -> Result<Vec<PluginCredentialRecord>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE plugin_credentials
             SET state = 'cleanup_pending', revision = revision + 1, updated_at_ms = ?1
             WHERE plugin_id = ?2 AND state IN ('pending_vault', 'ready')",
            params![unix_time_ms(), plugin_id.as_str()],
        )?;
        let mut statement = transaction.prepare(
            "SELECT handle, plugin_id, signer_fingerprint_sha256, secret_ref_id,
                    operation_id, idempotency_key, label, origin, injection_kind,
                    header_name, revision, state
             FROM plugin_credentials
             WHERE plugin_id = ?1 AND state = 'cleanup_pending'
             ORDER BY updated_at_ms, handle",
        )?;
        let records = statement
            .query_map([plugin_id.as_str()], read_record)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        transaction.commit()?;
        Ok(records)
    }
}

fn transition_exact(
    connection: &mut rusqlite::Connection,
    expected: &PluginCredentialRecord,
    from: PluginCredentialDurableState,
    to: PluginCredentialDurableState,
    increment_revision: bool,
    fence: &dyn Fn() -> bool,
) -> Result<PluginCredentialRecord> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_fence(fence)?;
    let next_revision = if increment_revision {
        expected
            .revision
            .get()
            .checked_add(1)
            .ok_or(AppPersistenceError::Conflict)?
    } else {
        expected.revision.get()
    };
    let changed = transaction.execute(
        "UPDATE plugin_credentials SET state = ?1, revision = ?2, updated_at_ms = ?3
         WHERE handle = ?4 AND plugin_id = ?5 AND signer_fingerprint_sha256 = ?6
           AND secret_ref_id = ?7 AND operation_id = ?8 AND idempotency_key = ?9
           AND revision = ?10 AND state = ?11",
        params![
            to.as_db(),
            u64_to_i64(next_revision)?,
            unix_time_ms(),
            expected.handle,
            expected.owner.plugin_id.as_str(),
            expected.owner.signer_fingerprint_sha256,
            expected.secret_ref_id.as_str(),
            expected.operation_id,
            expected.idempotency_key,
            u64_to_i64(expected.revision.get())?,
            from.as_db(),
        ],
    )?;
    if changed != 1 {
        return Err(AppPersistenceError::Conflict);
    }
    require_fence(fence)?;
    transaction.commit()?;
    read_record_by_handle(connection, &expected.owner, &expected.handle)?
        .ok_or(AppPersistenceError::InvalidStoredData)
}

fn lookup_create_replay(
    transaction: &rusqlite::Connection,
    input: &PluginCredentialCreateInput,
) -> Result<Vec<PluginCredentialRecord>> {
    let mut statement = transaction.prepare(
        "SELECT handle, plugin_id, signer_fingerprint_sha256, secret_ref_id,
                operation_id, idempotency_key, label, origin, injection_kind,
                header_name, revision, state
         FROM plugin_credentials
         WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
           AND (operation_id = ?3 OR idempotency_key = ?4)
         ORDER BY handle",
    )?;
    statement
        .query_map(
            params![
                input.owner.plugin_id.as_str(),
                input.owner.signer_fingerprint_sha256,
                input.operation_id,
                input.idempotency_key,
            ],
            read_record,
        )?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AppPersistenceError::from)
}

fn read_record_by_handle(
    connection: &rusqlite::Connection,
    owner: &PluginCredentialOwner,
    handle: &str,
) -> Result<Option<PluginCredentialRecord>> {
    connection
        .query_row(
            "SELECT handle, plugin_id, signer_fingerprint_sha256, secret_ref_id,
                    operation_id, idempotency_key, label, origin, injection_kind,
                    header_name, revision, state
             FROM plugin_credentials
             WHERE handle = ?1 AND plugin_id = ?2 AND signer_fingerprint_sha256 = ?3",
            params![
                handle,
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256
            ],
            read_record,
        )
        .optional()
        .map_err(AppPersistenceError::from)
}

fn read_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginCredentialRecord> {
    let injection_kind = row.get::<_, String>(8)?;
    let header_name = row.get::<_, Option<String>>(9)?;
    let injection = match (injection_kind.as_str(), header_name) {
        ("bearer", None) => PluginCredentialInjection::Bearer {},
        ("header", Some(name)) if valid_header_name(&name) => {
            PluginCredentialInjection::Header { name }
        }
        _ => return Err(invalid_stored_column()),
    };
    let revision = u64::try_from(row.get::<_, i64>(10)?).map_err(invalid_column)?;
    if revision == 0 {
        return Err(invalid_stored_column());
    }
    let owner = PluginCredentialOwner {
        plugin_id: PluginId::parse(row.get::<_, String>(1)?).map_err(invalid_parse_message)?,
        signer_fingerprint_sha256: row.get(2)?,
    };
    validate_owner(&owner).map_err(invalid_column)?;
    let handle = row.get::<_, String>(0)?;
    validate_handle(&handle).map_err(invalid_column)?;
    let origin = row.get::<_, String>(7)?;
    if !basic_origin_shape_valid(&origin) {
        return Err(invalid_stored_column());
    }
    Ok(PluginCredentialRecord {
        owner,
        handle,
        secret_ref_id: SecretRefId::parse(row.get::<_, String>(3)?)
            .map_err(invalid_parse_message)?,
        operation_id: row.get(4)?,
        idempotency_key: row.get(5)?,
        label: row.get(6)?,
        target: PluginCredentialTarget { origin, injection },
        revision: WireSequence::new(revision),
        state: PluginCredentialDurableState::from_db(&row.get::<_, String>(11)?)?,
    })
}

fn record_matches_create(
    record: &PluginCredentialRecord,
    input: &PluginCredentialCreateInput,
) -> bool {
    record.owner == input.owner
        && record.operation_id == input.operation_id
        && record.idempotency_key == input.idempotency_key
        && record.label == input.label
        && record.target == input.target
}

fn encode_injection(injection: &PluginCredentialInjection) -> (&'static str, Option<&str>) {
    match injection {
        PluginCredentialInjection::Bearer {} => ("bearer", None),
        PluginCredentialInjection::Header { name } => ("header", Some(name.as_str())),
    }
}

fn validate_create_input(input: &PluginCredentialCreateInput) -> Result<()> {
    validate_owner(&input.owner)?;
    if input.operation_id.is_empty()
        || input.operation_id.len() > MAX_OPERATION_ID_BYTES
        || input.operation_id.contains(['\0', '\r', '\n'])
        || input.idempotency_key.is_empty()
        || input.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || input.idempotency_key.contains(['\0', '\r', '\n'])
        || input.label.trim().is_empty()
        || input.label.len() > MAX_LABEL_BYTES
        || input.label.contains(['\0', '\r', '\n'])
        || !basic_origin_shape_valid(&input.target.origin)
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin credential metadata",
        ));
    }
    if let PluginCredentialInjection::Header { name } = &input.target.injection
        && !valid_header_name(name)
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin credential header",
        ));
    }
    Ok(())
}

fn validate_owner(owner: &PluginCredentialOwner) -> Result<()> {
    let signer = owner.signer_fingerprint_sha256.as_bytes();
    if signer.len() != 64
        || signer
            .iter()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin credential owner",
        ));
    }
    Ok(())
}

fn require_active_owner(
    connection: &rusqlite::Connection,
    owner: &PluginCredentialOwner,
) -> Result<()> {
    let signer = connection
        .query_row(
            "SELECT signer_fingerprint_sha256 FROM plugin_installations WHERE plugin_id = ?1",
            [owner.plugin_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if signer.as_deref() != Some(owner.signer_fingerprint_sha256.as_str()) {
        return Err(AppPersistenceError::NotFound);
    }
    Ok(())
}

fn validate_handle(handle: &str) -> Result<()> {
    let value = Uuid::parse_str(handle)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin credential handle"))?;
    if value.is_nil() {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin credential handle",
        ));
    }
    Ok(())
}

fn basic_origin_shape_valid(origin: &str) -> bool {
    !origin.is_empty()
        && origin.len() <= MAX_ORIGIN_BYTES
        && origin.is_ascii()
        && !origin.contains(['\0', '\r', '\n', '@', '?', '#'])
        && (origin.starts_with("https://") || origin.starts_with("wss://"))
        && !origin[origin.find("://").unwrap_or_default() + 3..].contains('/')
}

fn valid_header_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_HEADER_NAME_BYTES
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return false;
    }
    let lowercase = name.to_ascii_lowercase();
    !lowercase.starts_with("proxy-")
        && !lowercase.starts_with("sec-websocket-")
        && !matches!(
            lowercase.as_str(),
            "cookie"
                | "set-cookie"
                | "host"
                | "connection"
                | "content-length"
                | "transfer-encoding"
                | "te"
                | "trailer"
                | "upgrade"
                | "proxy-authorization"
                | "proxy-authenticate"
                | "keep-alive"
                | "origin"
        )
}

fn require_fence(fence: &dyn Fn() -> bool) -> Result<()> {
    if fence() {
        Ok(())
    } else {
        Err(AppPersistenceError::Conflict)
    }
}

fn invalid_stored_column() -> rusqlite::Error {
    invalid_column(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "invalid stored plugin credential",
    ))
}

fn invalid_parse_message(message: &'static str) -> rusqlite::Error {
    invalid_column(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

fn invalid_column(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
