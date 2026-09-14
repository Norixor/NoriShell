//! Durable, signer-bound, non-secret private storage for one plugin.
//!
//! The public broker supplies the owner from a verified Plugin Host instance.
//! This repository deliberately knows only the durable owner (`plugin_id` plus
//! signer); package and runtime generation remain authorization fences owned by
//! Core and are checked immediately before every SQLite commit.

use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use norishell_core_api::PluginId;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{AppPersistenceError, AppRepository, Result, u64_to_i64};

pub const MAX_PLUGIN_PRIVATE_STORAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PLUGIN_PRIVATE_STORAGE_KEYS: usize = 4_096;
pub const MAX_PLUGIN_PRIVATE_STORAGE_KEY_BYTES: usize = 128;
pub const MAX_PLUGIN_PRIVATE_STORAGE_KV_VALUE_BYTES: usize = 64 * 1024;
pub const MAX_PLUGIN_PRIVATE_STORAGE_BLOB_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES: usize = 16 * 1024;
pub const MAX_PLUGIN_PRIVATE_STORAGE_LIST_LIMIT: u16 = 256;
pub const MAX_PLUGIN_PRIVATE_STORAGE_BATCH_MUTATIONS: usize = 64;

/// Kept as a stable internal code so the broker can map quota failures without
/// exposing a database error to a plugin.
pub const PLUGIN_PRIVATE_STORAGE_QUOTA_ERROR: &str = "plugin private storage quota exceeded";

const KV_NAMESPACE: &str = "kv";
const BLOB_NAMESPACE: &str = "blob";
const CACHE_NAMESPACE: &str = "cache";
const STATE_KEY: &str = "__plugin_private_storage_state_v1";
const PERSISTENT_STATE_KEY: &str = "__norishell_persistent_state_v1";
const STATE_CHUNK_INDEX: i64 = 0;
const BLOB_MANIFEST_CHUNK_INDEX: i64 = -1;
const CURSOR_VERSION: u8 = 1;
const MAX_CURSOR_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPrivateStorageOwner {
    pub plugin_id: PluginId,
    pub signer_fingerprint_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginPrivateStorageState {
    pub store_revision: u64,
    pub schema_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPrivateStorageEntry {
    pub key: String,
    pub value_bytes: Vec<u8>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPrivateStorageValue {
    pub state: PluginPrivateStorageState,
    pub entry: Option<PluginPrivateStorageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPrivateStoragePage {
    pub state: PluginPrivateStorageState,
    pub entries: Vec<PluginPrivateStorageEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPrivateStorageBlobRead {
    pub state: PluginPrivateStorageState,
    pub key: String,
    pub revision: u64,
    pub total_bytes: u64,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPrivateStorageBlobWrite {
    pub state: PluginPrivateStorageState,
    pub key: String,
    pub revision: u64,
    pub total_bytes: u64,
}

/// Host-managed declarative UI state retained for a plugin. It uses the same
/// durable owner and quota as the public Storage API but stays in the private
/// metadata namespace so ordinary guest KV calls cannot collide with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPrivatePersistentState {
    pub state: PluginPrivateStorageState,
    pub value_json: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginPrivateStorageMutation {
    KvSet {
        key: String,
        value_bytes: Vec<u8>,
        expected_revision: u64,
    },
    KvDelete {
        key: String,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone)]
struct StorageRecord {
    value_bytes: Vec<u8>,
    value_revision: u64,
    expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageListCursor {
    version: u8,
    plugin_id: String,
    signer_fingerprint_sha256: String,
    namespace: String,
    prefix: String,
    store_revision: u64,
    after_key: String,
}

impl AppRepository {
    pub fn get_plugin_private_storage_kv(
        &self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageValue> {
        validate_storage_key(key)?;
        require_fence(fence)?;
        require_storage_owner(&self.connection, owner)?;
        let state = read_storage_state(&self.connection, owner)?;
        let entry = read_storage_record(&self.connection, owner, KV_NAMESPACE, key, 0)?
            .map(|record| {
                require_nonexpiring(&record)?;
                Ok::<_, AppPersistenceError>(PluginPrivateStorageEntry {
                    key: key.to_owned(),
                    value_bytes: record.value_bytes,
                    revision: record.value_revision,
                })
            })
            .transpose()?;
        require_fence(fence)?;
        Ok(PluginPrivateStorageValue { state, entry })
    }

    pub fn set_plugin_private_storage_kv(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        value_bytes: &[u8],
        expected_revision: u64,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageValue> {
        validate_storage_key(key)?;
        validate_kv_value(value_bytes)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        prune_expired_cache(&transaction, owner, now_unix_ms())?;
        let current = read_storage_record(&transaction, owner, KV_NAMESPACE, key, 0)?;
        current.as_ref().map(require_nonexpiring).transpose()?;
        require_expected_revision(current.as_ref(), expected_revision, true)?;
        let next_value_revision = next_value_revision(current.as_ref())?;
        let next_store_revision = next_store_revision(state)?;
        let (used_bytes, key_count) = storage_usage(&transaction, owner)?;
        let used_bytes = replace_record_bytes(
            used_bytes,
            key,
            current.as_ref().map(|record| record.value_bytes.as_slice()),
            Some(value_bytes),
        )?;
        let key_count = key_count + usize::from(current.is_none());
        require_storage_quota(used_bytes, key_count)?;
        write_storage_record(
            &transaction,
            owner,
            KV_NAMESPACE,
            key,
            0,
            value_bytes,
            next_value_revision,
            next_store_revision,
            None,
        )?;
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(PluginPrivateStorageValue {
            state: next_state,
            entry: Some(PluginPrivateStorageEntry {
                key: key.to_owned(),
                value_bytes: value_bytes.to_vec(),
                revision: next_value_revision,
            }),
        })
    }

    pub fn delete_plugin_private_storage_kv(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        expected_revision: u64,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageState> {
        validate_storage_key(key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        prune_expired_cache(&transaction, owner, now_unix_ms())?;
        let current = read_storage_record(&transaction, owner, KV_NAMESPACE, key, 0)?;
        current.as_ref().map(require_nonexpiring).transpose()?;
        require_expected_revision(current.as_ref(), expected_revision, false)?;
        let changed = delete_storage_record(&transaction, owner, KV_NAMESPACE, key, 0)?;
        if changed != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision(state)?,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(next_state)
    }

    pub fn list_plugin_private_storage_kv(
        &self,
        owner: &PluginPrivateStorageOwner,
        prefix: &str,
        cursor: Option<&str>,
        limit: u16,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStoragePage> {
        validate_storage_prefix(prefix)?;
        if limit == 0 || limit > MAX_PLUGIN_PRIVATE_STORAGE_LIST_LIMIT {
            return Err(AppPersistenceError::InvalidInput(
                "invalid plugin storage list limit",
            ));
        }
        require_fence(fence)?;
        require_storage_owner(&self.connection, owner)?;
        let state = read_storage_state(&self.connection, owner)?;
        let after_key = cursor
            .map(|cursor| decode_storage_cursor(cursor, owner, prefix, state.store_revision))
            .transpose()?;
        let mut statement = self.connection.prepare(
            "SELECT storage_key, value_bytes, value_revision, expires_at_ms
             FROM plugin_private_storage
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND namespace = 'kv' AND chunk_index = 0
               AND substr(storage_key, 1, length(?3)) = ?3
               AND (?4 IS NULL OR storage_key > ?4)
             ORDER BY storage_key COLLATE BINARY
             LIMIT ?5",
        )?;
        let requested = usize::from(limit);
        let rows = statement
            .query_map(
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    prefix,
                    after_key,
                    i64::try_from(requested + 1)
                        .map_err(|_| AppPersistenceError::InvalidStoredData)?
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let has_more = rows.len() > requested;
        let mut entries = rows
            .into_iter()
            .take(requested)
            .map(|(key, value_bytes, value_revision, expires_at_ms)| {
                if expires_at_ms.is_some() {
                    return Err(AppPersistenceError::InvalidStoredData);
                }
                let revision = db_revision(value_revision)?;
                validate_kv_value(&value_bytes)?;
                Ok(PluginPrivateStorageEntry {
                    key,
                    value_bytes,
                    revision,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = if has_more {
            entries
                .last()
                .map(|entry| encode_storage_cursor(owner, prefix, state.store_revision, &entry.key))
                .transpose()?
        } else {
            None
        };
        // Keep the vector mutable only until all cursor validation has completed,
        // then avoid retaining a database-backed iterator across the fence check.
        entries.shrink_to_fit();
        require_fence(fence)?;
        Ok(PluginPrivateStoragePage {
            state,
            entries,
            next_cursor,
        })
    }

    pub fn read_plugin_private_storage_blob(
        &self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        offset: u64,
        length: u16,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageBlobRead> {
        validate_storage_key(key)?;
        validate_blob_range(offset, usize::from(length), false)?;
        require_fence(fence)?;
        require_storage_owner(&self.connection, owner)?;
        let state = read_storage_state(&self.connection, owner)?;
        let manifest = read_storage_record(
            &self.connection,
            owner,
            BLOB_NAMESPACE,
            key,
            BLOB_MANIFEST_CHUNK_INDEX,
        )?
        .ok_or(AppPersistenceError::NotFound)?;
        require_nonexpiring(&manifest)?;
        let total_bytes = decode_blob_length(&manifest.value_bytes)?;
        if offset > total_bytes {
            return Err(AppPersistenceError::InvalidInput(
                "plugin storage blob offset exceeds length",
            ));
        }
        let requested = u64::from(length);
        let available = total_bytes - offset;
        let read_len = available.min(requested);
        let bytes = if read_len == 0 {
            Vec::new()
        } else {
            validate_blob_range(
                offset,
                usize::try_from(read_len).map_err(|_| AppPersistenceError::InvalidStoredData)?,
                false,
            )?;
            let chunk_index = blob_chunk_index(offset)?;
            let chunk_offset = blob_chunk_offset(offset)?;
            let chunk =
                read_storage_record(&self.connection, owner, BLOB_NAMESPACE, key, chunk_index)?
                    .ok_or(AppPersistenceError::InvalidStoredData)?;
            require_nonexpiring(&chunk)?;
            if chunk.value_bytes.len() > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES {
                return Err(AppPersistenceError::InvalidStoredData);
            }
            let read_end = chunk_offset
                .checked_add(
                    usize::try_from(read_len)
                        .map_err(|_| AppPersistenceError::InvalidStoredData)?,
                )
                .ok_or(AppPersistenceError::InvalidStoredData)?;
            if read_end > chunk.value_bytes.len() {
                return Err(AppPersistenceError::InvalidStoredData);
            }
            chunk.value_bytes[chunk_offset..read_end].to_vec()
        };
        require_fence(fence)?;
        Ok(PluginPrivateStorageBlobRead {
            state,
            key: key.to_owned(),
            revision: manifest.value_revision,
            total_bytes,
            offset,
            eof: offset
                .checked_add(
                    u64::try_from(bytes.len())
                        .map_err(|_| AppPersistenceError::InvalidStoredData)?,
                )
                .is_some_and(|end| end == total_bytes),
            bytes,
        })
    }

    pub fn write_plugin_private_storage_blob(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        offset: u64,
        bytes: &[u8],
        expected_revision: u64,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageBlobWrite> {
        validate_storage_key(key)?;
        validate_blob_range(offset, bytes.len(), true)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        prune_expired_cache(&transaction, owner, now_unix_ms())?;
        let manifest = read_storage_record(
            &transaction,
            owner,
            BLOB_NAMESPACE,
            key,
            BLOB_MANIFEST_CHUNK_INDEX,
        )?;
        manifest.as_ref().map(require_nonexpiring).transpose()?;
        require_expected_revision(manifest.as_ref(), expected_revision, true)?;
        let current_length = manifest
            .as_ref()
            .map(|record| decode_blob_length(&record.value_bytes))
            .transpose()?
            .unwrap_or(0);
        if offset > current_length {
            return Err(AppPersistenceError::InvalidInput(
                "plugin storage blob writes cannot create holes",
            ));
        }
        let write_end = offset
            .checked_add(u64::try_from(bytes.len()).map_err(|_| {
                AppPersistenceError::InvalidInput("invalid plugin storage blob chunk")
            })?)
            .ok_or(AppPersistenceError::InvalidInput(
                "invalid plugin storage blob chunk",
            ))?;
        if write_end > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_BYTES as u64 {
            return Err(AppPersistenceError::InvalidInput(
                "plugin storage blob exceeds its limit",
            ));
        }
        if offset < current_length && write_end > current_length {
            return Err(AppPersistenceError::InvalidInput(
                "plugin storage blob overwrite exceeds length",
            ));
        }
        let chunk_index = blob_chunk_index(offset)?;
        let chunk_offset = blob_chunk_offset(offset)?;
        let current_chunk =
            read_storage_record(&transaction, owner, BLOB_NAMESPACE, key, chunk_index)?;
        current_chunk
            .as_ref()
            .map(require_nonexpiring)
            .transpose()?;
        if current_chunk.as_ref().is_some_and(|chunk| {
            chunk.value_bytes.len() > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES
        }) {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        let replacement =
            if offset == current_length {
                if chunk_offset == 0 {
                    if current_chunk.is_some() {
                        return Err(AppPersistenceError::InvalidStoredData);
                    }
                    bytes.to_vec()
                } else {
                    let current_chunk = current_chunk
                        .as_ref()
                        .ok_or(AppPersistenceError::InvalidStoredData)?;
                    if current_chunk.value_bytes.len() != chunk_offset {
                        return Err(AppPersistenceError::InvalidStoredData);
                    }
                    let mut value = current_chunk.value_bytes.clone();
                    value.extend_from_slice(bytes);
                    value
                }
            } else {
                let current_chunk = current_chunk
                    .as_ref()
                    .ok_or(AppPersistenceError::InvalidStoredData)?;
                let write_end = chunk_offset.checked_add(bytes.len()).ok_or(
                    AppPersistenceError::InvalidInput("invalid plugin storage blob chunk"),
                )?;
                if write_end > current_chunk.value_bytes.len() {
                    return Err(AppPersistenceError::InvalidInput(
                        "plugin storage blob overwrite exceeds length",
                    ));
                }
                let mut value = current_chunk.value_bytes.clone();
                value[chunk_offset..write_end].copy_from_slice(bytes);
                value
            };
        if replacement.len() > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES {
            return Err(AppPersistenceError::InvalidStoredData);
        }
        let next_value_revision = next_value_revision(manifest.as_ref())?;
        let next_store_revision = next_store_revision(state)?;
        let (used_bytes, key_count) = storage_usage(&transaction, owner)?;
        let used_bytes = replace_record_bytes(
            used_bytes,
            key,
            manifest
                .as_ref()
                .map(|record| record.value_bytes.as_slice()),
            Some(&encode_blob_length(write_end)),
        )?;
        let used_bytes = replace_record_bytes(
            used_bytes,
            key,
            current_chunk
                .as_ref()
                .map(|record| record.value_bytes.as_slice()),
            Some(&replacement),
        )?;
        let key_count = key_count + usize::from(manifest.is_none());
        require_storage_quota(used_bytes, key_count)?;
        write_storage_record(
            &transaction,
            owner,
            BLOB_NAMESPACE,
            key,
            BLOB_MANIFEST_CHUNK_INDEX,
            &encode_blob_length(write_end),
            next_value_revision,
            next_store_revision,
            None,
        )?;
        write_storage_record(
            &transaction,
            owner,
            BLOB_NAMESPACE,
            key,
            chunk_index,
            &replacement,
            next_value_revision,
            next_store_revision,
            None,
        )?;
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(PluginPrivateStorageBlobWrite {
            state: next_state,
            key: key.to_owned(),
            revision: next_value_revision,
            total_bytes: write_end,
        })
    }

    pub fn delete_plugin_private_storage_blob(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        expected_revision: u64,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageState> {
        validate_storage_key(key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        prune_expired_cache(&transaction, owner, now_unix_ms())?;
        let manifest = read_storage_record(
            &transaction,
            owner,
            BLOB_NAMESPACE,
            key,
            BLOB_MANIFEST_CHUNK_INDEX,
        )?;
        manifest.as_ref().map(require_nonexpiring).transpose()?;
        require_expected_revision(manifest.as_ref(), expected_revision, false)?;
        let deleted = transaction.execute(
            "DELETE FROM plugin_private_storage
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND namespace = 'blob' AND storage_key = ?3",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                key
            ],
        )?;
        if deleted == 0 {
            return Err(AppPersistenceError::Conflict);
        }
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision(state)?,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(next_state)
    }

    pub fn get_plugin_private_storage_cache(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageValue> {
        validate_storage_key(key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        let record = read_storage_record(&transaction, owner, CACHE_NAMESPACE, key, 0)?;
        let Some(record) = record else {
            require_fence(fence)?;
            transaction.commit()?;
            return Ok(PluginPrivateStorageValue { state, entry: None });
        };
        let expires_at_ms = record
            .expires_at_ms
            .ok_or(AppPersistenceError::InvalidStoredData)?;
        if expires_at_ms > now_unix_ms() {
            require_fence(fence)?;
            transaction.commit()?;
            return Ok(PluginPrivateStorageValue {
                state,
                entry: Some(PluginPrivateStorageEntry {
                    key: key.to_owned(),
                    value_bytes: record.value_bytes,
                    revision: record.value_revision,
                }),
            });
        }
        let deleted = delete_storage_record(&transaction, owner, CACHE_NAMESPACE, key, 0)?;
        if deleted != 1 {
            return Err(AppPersistenceError::Conflict);
        }
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision(state)?,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(PluginPrivateStorageValue {
            state: next_state,
            entry: None,
        })
    }

    pub fn set_plugin_private_storage_cache(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        value_bytes: &[u8],
        ttl_ms: u64,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageValue> {
        validate_storage_key(key)?;
        validate_kv_value(value_bytes)?;
        let now = now_unix_ms();
        let ttl_ms = i64::try_from(ttl_ms)
            .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin storage cache ttl"))?;
        if ttl_ms <= 0 {
            return Err(AppPersistenceError::InvalidInput(
                "invalid plugin storage cache ttl",
            ));
        }
        let expires_at_ms = now
            .checked_add(ttl_ms)
            .ok_or(AppPersistenceError::InvalidInput(
                "invalid plugin storage cache ttl",
            ))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        prune_expired_cache(&transaction, owner, now)?;
        let current = read_storage_record(&transaction, owner, CACHE_NAMESPACE, key, 0)?;
        let next_value_revision = next_value_revision(current.as_ref())?;
        let next_store_revision = next_store_revision(state)?;
        let (used_bytes, key_count) = storage_usage(&transaction, owner)?;
        let used_bytes = replace_record_bytes(
            used_bytes,
            key,
            current.as_ref().map(|record| record.value_bytes.as_slice()),
            Some(value_bytes),
        )?;
        let key_count = key_count + usize::from(current.is_none());
        require_storage_quota(used_bytes, key_count)?;
        write_storage_record(
            &transaction,
            owner,
            CACHE_NAMESPACE,
            key,
            0,
            value_bytes,
            next_value_revision,
            next_store_revision,
            Some(expires_at_ms),
        )?;
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(PluginPrivateStorageValue {
            state: next_state,
            entry: Some(PluginPrivateStorageEntry {
                key: key.to_owned(),
                value_bytes: value_bytes.to_vec(),
                revision: next_value_revision,
            }),
        })
    }

    pub fn delete_plugin_private_storage_cache(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        key: &str,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageState> {
        validate_storage_key(key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        prune_expired_cache(&transaction, owner, now_unix_ms())?;
        delete_storage_record(&transaction, owner, CACHE_NAMESPACE, key, 0)?;
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision(state)?,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(next_state)
    }

    pub fn clear_plugin_private_storage_cache(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageState> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        transaction.execute(
            "DELETE FROM plugin_private_storage
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2 AND namespace = 'cache'",
            params![owner.plugin_id.as_str(), owner.signer_fingerprint_sha256],
        )?;
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision(state)?,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(next_state)
    }

    pub fn get_plugin_private_storage_schema(
        &self,
        owner: &PluginPrivateStorageOwner,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageState> {
        require_fence(fence)?;
        require_storage_owner(&self.connection, owner)?;
        let state = read_storage_state(&self.connection, owner)?;
        require_fence(fence)?;
        Ok(state)
    }

    pub fn commit_plugin_private_storage_schema(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        expected_version: u64,
        new_version: u64,
        mutations: &[PluginPrivateStorageMutation],
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivateStorageState> {
        if new_version <= expected_version
            || mutations.len() > MAX_PLUGIN_PRIVATE_STORAGE_BATCH_MUTATIONS
        {
            return Err(AppPersistenceError::InvalidInput(
                "invalid plugin storage schema commit",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        if state.schema_version != expected_version {
            return Err(AppPersistenceError::Conflict);
        }
        prune_expired_cache(&transaction, owner, now_unix_ms())?;
        let mut seen = BTreeSet::new();
        let mut planned = Vec::with_capacity(mutations.len());
        let (mut used_bytes, mut key_count) = storage_usage(&transaction, owner)?;
        for mutation in mutations {
            let (key, expected_revision, replacement) = match mutation {
                PluginPrivateStorageMutation::KvSet {
                    key,
                    value_bytes,
                    expected_revision,
                } => {
                    validate_storage_key(key)?;
                    validate_kv_value(value_bytes)?;
                    (
                        key.as_str(),
                        *expected_revision,
                        Some(value_bytes.as_slice()),
                    )
                }
                PluginPrivateStorageMutation::KvDelete {
                    key,
                    expected_revision,
                } => {
                    validate_storage_key(key)?;
                    (key.as_str(), *expected_revision, None)
                }
            };
            if !seen.insert(key.to_owned()) {
                return Err(AppPersistenceError::InvalidInput(
                    "duplicate plugin storage schema mutation key",
                ));
            }
            let current = read_storage_record(&transaction, owner, KV_NAMESPACE, key, 0)?;
            current.as_ref().map(require_nonexpiring).transpose()?;
            let allows_absent = replacement.is_some();
            require_expected_revision(current.as_ref(), expected_revision, allows_absent)?;
            used_bytes = replace_record_bytes(
                used_bytes,
                key,
                current.as_ref().map(|record| record.value_bytes.as_slice()),
                replacement,
            )?;
            key_count = match (current.is_some(), replacement.is_some()) {
                (false, true) => key_count + 1,
                (true, false) => key_count
                    .checked_sub(1)
                    .ok_or(AppPersistenceError::InvalidStoredData)?,
                _ => key_count,
            };
            let next_revision = replacement
                .map(|_| next_value_revision(current.as_ref()))
                .transpose()?;
            planned.push((
                key.to_owned(),
                replacement.map(|value| value.to_vec()),
                next_revision,
            ));
        }
        require_storage_quota(used_bytes, key_count)?;
        let next_store_revision = next_store_revision(state)?;
        for (key, replacement, revision) in planned {
            if let Some(value_bytes) = replacement {
                write_storage_record(
                    &transaction,
                    owner,
                    KV_NAMESPACE,
                    &key,
                    0,
                    &value_bytes,
                    revision.ok_or(AppPersistenceError::InvalidStoredData)?,
                    next_store_revision,
                    None,
                )?;
            } else if delete_storage_record(&transaction, owner, KV_NAMESPACE, &key, 0)? != 1 {
                return Err(AppPersistenceError::Conflict);
            }
        }
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision,
            schema_version: new_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(next_state)
    }

    /// Reads the host-managed declarative UI document for this plugin.
    ///
    /// This is deliberately separate from the guest-facing KV namespace. It
    /// preserves the pre-storage-API UI contract while sharing the same
    /// signer-bound owner, durable quota, state revision, and commit fence.
    pub fn get_plugin_private_persistent_state(
        &self,
        owner: &PluginPrivateStorageOwner,
        fence: &dyn Fn() -> bool,
    ) -> Result<Option<PluginPrivatePersistentState>> {
        require_fence(fence)?;
        require_storage_owner(&self.connection, owner)?;
        let state = read_storage_state(&self.connection, owner)?;
        let record = read_storage_record(
            &self.connection,
            owner,
            "meta",
            PERSISTENT_STATE_KEY,
            STATE_CHUNK_INDEX,
        )?;
        let persistent_state = record
            .map(|record| {
                require_nonexpiring(&record)?;
                require_valid_persistent_state_json(&record.value_bytes)?;
                Ok::<_, AppPersistenceError>(PluginPrivatePersistentState {
                    state,
                    value_json: String::from_utf8(record.value_bytes)
                        .map_err(|_| AppPersistenceError::InvalidStoredData)?,
                    revision: record.value_revision,
                })
            })
            .transpose()?;
        require_fence(fence)?;
        Ok(persistent_state)
    }

    /// Replaces the host-managed declarative UI document under its exact
    /// revision fence. The document remains non-secret JSON object data and
    /// is charged to the same durable private-storage byte quota.
    pub fn replace_plugin_private_persistent_state(
        &mut self,
        owner: &PluginPrivateStorageOwner,
        expected_revision: u64,
        value_json: &str,
        fence: &dyn Fn() -> bool,
    ) -> Result<PluginPrivatePersistentState> {
        validate_persistent_state_json(value_json.as_bytes())?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_fence(fence)?;
        require_storage_owner(&transaction, owner)?;
        let state = read_storage_state(&transaction, owner)?;
        prune_expired_cache(&transaction, owner, now_unix_ms())?;
        let current = read_storage_record(
            &transaction,
            owner,
            "meta",
            PERSISTENT_STATE_KEY,
            STATE_CHUNK_INDEX,
        )?;
        current.as_ref().map(require_nonexpiring).transpose()?;
        if let Some(current) = current.as_ref() {
            require_valid_persistent_state_json(&current.value_bytes)?;
        }
        require_expected_revision(current.as_ref(), expected_revision, true)?;
        let next_value_revision = next_value_revision(current.as_ref())?;
        let next_store_revision = next_store_revision(state)?;
        let (used_bytes, key_count) = storage_usage(&transaction, owner)?;
        let used_bytes = replace_record_bytes(
            used_bytes,
            PERSISTENT_STATE_KEY,
            current.as_ref().map(|record| record.value_bytes.as_slice()),
            Some(value_json.as_bytes()),
        )?;
        let key_count = key_count + usize::from(current.is_none());
        require_storage_quota(used_bytes, key_count)?;
        write_storage_record(
            &transaction,
            owner,
            "meta",
            PERSISTENT_STATE_KEY,
            STATE_CHUNK_INDEX,
            value_json.as_bytes(),
            next_value_revision,
            next_store_revision,
            None,
        )?;
        let next_state = PluginPrivateStorageState {
            store_revision: next_store_revision,
            schema_version: state.schema_version,
        };
        write_storage_state(&transaction, owner, next_state)?;
        commit_storage_transaction(transaction, fence)?;
        Ok(PluginPrivatePersistentState {
            state: next_state,
            value_json: value_json.to_owned(),
            revision: next_value_revision,
        })
    }
}

fn require_fence(fence: &dyn Fn() -> bool) -> Result<()> {
    if fence() {
        Ok(())
    } else {
        // The desktop driver turns this into `Revoked`. Keeping this error
        // generic prevents persistence from importing runtime authority types.
        Err(AppPersistenceError::Conflict)
    }
}

fn commit_storage_transaction(
    transaction: Transaction<'_>,
    fence: &dyn Fn() -> bool,
) -> Result<()> {
    require_fence(fence)?;
    transaction.commit()?;
    Ok(())
}

fn require_storage_owner(connection: &Connection, owner: &PluginPrivateStorageOwner) -> Result<()> {
    validate_storage_owner(owner)?;
    let signer: Option<String> = connection
        .query_row(
            "SELECT signer_fingerprint_sha256 FROM plugin_installations WHERE plugin_id = ?1",
            [owner.plugin_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    match signer {
        None => Err(AppPersistenceError::NotFound),
        Some(signer) if signer != owner.signer_fingerprint_sha256 => {
            Err(AppPersistenceError::Conflict)
        }
        Some(_) => Ok(()),
    }
}

fn validate_storage_owner(owner: &PluginPrivateStorageOwner) -> Result<()> {
    if owner.signer_fingerprint_sha256.len() != 64
        || !owner
            .signer_fingerprint_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin storage owner",
        ));
    }
    Ok(())
}

fn validate_storage_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_PLUGIN_PRIVATE_STORAGE_KEY_BYTES || key.contains('\0') {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin storage key",
        ));
    }
    Ok(())
}

fn validate_storage_prefix(prefix: &str) -> Result<()> {
    if prefix.len() > MAX_PLUGIN_PRIVATE_STORAGE_KEY_BYTES || prefix.contains('\0') {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin storage prefix",
        ));
    }
    Ok(())
}

fn validate_kv_value(value_bytes: &[u8]) -> Result<()> {
    if value_bytes.len() > MAX_PLUGIN_PRIVATE_STORAGE_KV_VALUE_BYTES {
        return Err(AppPersistenceError::InvalidInput(
            "plugin storage value exceeds its limit",
        ));
    }
    Ok(())
}

fn validate_persistent_state_json(value_bytes: &[u8]) -> Result<()> {
    if !is_valid_persistent_state_json(value_bytes) {
        return Err(AppPersistenceError::InvalidInput(
            "plugin storage must be a bounded JSON object",
        ));
    }
    Ok(())
}

fn require_valid_persistent_state_json(value_bytes: &[u8]) -> Result<()> {
    if is_valid_persistent_state_json(value_bytes) {
        Ok(())
    } else {
        Err(AppPersistenceError::InvalidStoredData)
    }
}

fn is_valid_persistent_state_json(value_bytes: &[u8]) -> bool {
    value_bytes.len() <= MAX_PLUGIN_PRIVATE_STORAGE_KV_VALUE_BYTES
        && serde_json::from_slice::<serde_json::Value>(value_bytes)
            .is_ok_and(|value| value.is_object())
}

fn require_nonexpiring(record: &StorageRecord) -> Result<()> {
    if record.expires_at_ms.is_some() {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(())
}

fn validate_blob_range(offset: u64, length: usize, require_data: bool) -> Result<()> {
    if (require_data && length == 0)
        || length > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES
        || (!require_data && length == 0)
        || offset > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_BYTES as u64
    {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin storage blob chunk",
        ));
    }
    let within_chunk = usize::try_from(offset % MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES as u64)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin storage blob chunk"))?;
    if within_chunk
        .checked_add(length)
        .is_none_or(|end| end > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES)
    {
        return Err(AppPersistenceError::InvalidInput(
            "plugin storage blob chunk crosses a boundary",
        ));
    }
    Ok(())
}

fn require_expected_revision(
    current: Option<&StorageRecord>,
    expected_revision: u64,
    allow_absent_create: bool,
) -> Result<()> {
    match current {
        Some(current) if current.value_revision == expected_revision => Ok(()),
        None if allow_absent_create && expected_revision == 0 => Ok(()),
        _ => Err(AppPersistenceError::Conflict),
    }
}

fn next_value_revision(current: Option<&StorageRecord>) -> Result<u64> {
    match current {
        None => Ok(1),
        Some(record) => record
            .value_revision
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(AppPersistenceError::InvalidStoredData),
    }
}

fn next_store_revision(state: PluginPrivateStorageState) -> Result<u64> {
    state
        .store_revision
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or(AppPersistenceError::InvalidStoredData)
}

fn db_revision(value: i64) -> Result<u64> {
    let value = u64::try_from(value).map_err(|_| AppPersistenceError::InvalidStoredData)?;
    if value == 0 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(value)
}

fn read_storage_state(
    connection: &Connection,
    owner: &PluginPrivateStorageOwner,
) -> Result<PluginPrivateStorageState> {
    let stored: Option<(Vec<u8>, i64)> = connection
        .query_row(
            "SELECT value_bytes, store_revision FROM plugin_private_storage
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND namespace = 'meta' AND storage_key = ?3 AND chunk_index = ?4",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                STATE_KEY,
                STATE_CHUNK_INDEX
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((encoded_schema_version, store_revision)) = stored else {
        return Ok(PluginPrivateStorageState {
            store_revision: 0,
            schema_version: 0,
        });
    };
    if encoded_schema_version.len() != 8 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    let schema_version = u64::from_be_bytes(
        encoded_schema_version
            .as_slice()
            .try_into()
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
    );
    Ok(PluginPrivateStorageState {
        store_revision: db_revision(store_revision)?,
        schema_version,
    })
}

fn write_storage_state(
    transaction: &Transaction<'_>,
    owner: &PluginPrivateStorageOwner,
    state: PluginPrivateStorageState,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO plugin_private_storage
         (plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index,
          value_bytes, value_revision, store_revision, expires_at_ms)
         VALUES (?1, ?2, 'meta', ?3, ?4, ?5, ?6, ?7, NULL)
         ON CONFLICT(plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index)
         DO UPDATE SET value_bytes = excluded.value_bytes, value_revision = excluded.value_revision,
                       store_revision = excluded.store_revision, expires_at_ms = NULL",
        params![
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            STATE_KEY,
            STATE_CHUNK_INDEX,
            state.schema_version.to_be_bytes().to_vec(),
            u64_to_i64(state.store_revision)?,
            u64_to_i64(state.store_revision)?,
        ],
    )?;
    Ok(())
}

fn read_storage_record(
    connection: &Connection,
    owner: &PluginPrivateStorageOwner,
    namespace: &str,
    key: &str,
    chunk_index: i64,
) -> Result<Option<StorageRecord>> {
    let stored: Option<(Vec<u8>, i64, Option<i64>)> = connection
        .query_row(
            "SELECT value_bytes, value_revision, expires_at_ms FROM plugin_private_storage
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND namespace = ?3 AND storage_key = ?4 AND chunk_index = ?5",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                namespace,
                key,
                chunk_index
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    stored
        .map(|(value_bytes, value_revision, expires_at_ms)| {
            if value_bytes.len() > MAX_PLUGIN_PRIVATE_STORAGE_KV_VALUE_BYTES
                || expires_at_ms.is_some_and(|value| value < 0)
            {
                return Err(AppPersistenceError::InvalidStoredData);
            }
            Ok(StorageRecord {
                value_bytes,
                value_revision: db_revision(value_revision)?,
                expires_at_ms,
            })
        })
        .transpose()
}

#[allow(clippy::too_many_arguments)]
fn write_storage_record(
    transaction: &Transaction<'_>,
    owner: &PluginPrivateStorageOwner,
    namespace: &str,
    key: &str,
    chunk_index: i64,
    value_bytes: &[u8],
    value_revision: u64,
    store_revision: u64,
    expires_at_ms: Option<i64>,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO plugin_private_storage
         (plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index,
          value_bytes, value_revision, store_revision, expires_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index)
         DO UPDATE SET value_bytes = excluded.value_bytes, value_revision = excluded.value_revision,
                       store_revision = excluded.store_revision, expires_at_ms = excluded.expires_at_ms",
        params![
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            namespace,
            key,
            chunk_index,
            value_bytes,
            u64_to_i64(value_revision)?,
            u64_to_i64(store_revision)?,
            expires_at_ms,
        ],
    )?;
    Ok(())
}

fn delete_storage_record(
    transaction: &Transaction<'_>,
    owner: &PluginPrivateStorageOwner,
    namespace: &str,
    key: &str,
    chunk_index: i64,
) -> Result<usize> {
    transaction
        .execute(
            "DELETE FROM plugin_private_storage
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND namespace = ?3 AND storage_key = ?4 AND chunk_index = ?5",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                namespace,
                key,
                chunk_index,
            ],
        )
        .map_err(AppPersistenceError::from)
}

fn prune_expired_cache(
    transaction: &Transaction<'_>,
    owner: &PluginPrivateStorageOwner,
    now_unix_ms: i64,
) -> Result<usize> {
    transaction
        .execute(
            "DELETE FROM plugin_private_storage
             WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
               AND namespace = 'cache' AND expires_at_ms IS NOT NULL AND expires_at_ms <= ?3",
            params![
                owner.plugin_id.as_str(),
                owner.signer_fingerprint_sha256,
                now_unix_ms
            ],
        )
        .map_err(AppPersistenceError::from)
}

fn storage_usage(
    connection: &Connection,
    owner: &PluginPrivateStorageOwner,
) -> Result<(usize, usize)> {
    let used_bytes: i64 = connection.query_row(
        "SELECT COALESCE(SUM(length(CAST(storage_key AS BLOB)) + length(value_bytes)), 0)
         FROM plugin_private_storage
         WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
           AND (namespace IN ('kv', 'blob', 'cache')
                OR (namespace = 'meta' AND storage_key = ?3))",
        params![
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            PERSISTENT_STATE_KEY
        ],
        |row| row.get(0),
    )?;
    let key_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM (
           SELECT namespace, storage_key FROM plugin_private_storage
           WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
             AND (namespace IN ('kv', 'blob', 'cache')
                  OR (namespace = 'meta' AND storage_key = ?3))
           GROUP BY namespace, storage_key
         )",
        params![
            owner.plugin_id.as_str(),
            owner.signer_fingerprint_sha256,
            PERSISTENT_STATE_KEY
        ],
        |row| row.get(0),
    )?;
    Ok((
        usize::try_from(used_bytes).map_err(|_| AppPersistenceError::InvalidStoredData)?,
        usize::try_from(key_count).map_err(|_| AppPersistenceError::InvalidStoredData)?,
    ))
}

fn replace_record_bytes(
    used_bytes: usize,
    key: &str,
    old_value: Option<&[u8]>,
    new_value: Option<&[u8]>,
) -> Result<usize> {
    let old_cost = old_value
        .map(|value| storage_record_cost(key, value))
        .transpose()?
        .unwrap_or(0);
    let new_cost = new_value
        .map(|value| storage_record_cost(key, value))
        .transpose()?
        .unwrap_or(0);
    used_bytes
        .checked_sub(old_cost)
        .and_then(|value| value.checked_add(new_cost))
        .ok_or(AppPersistenceError::InvalidStoredData)
}

fn storage_record_cost(key: &str, value: &[u8]) -> Result<usize> {
    key.len()
        .checked_add(value.len())
        .ok_or(AppPersistenceError::InvalidStoredData)
}

fn require_storage_quota(used_bytes: usize, key_count: usize) -> Result<()> {
    if used_bytes > MAX_PLUGIN_PRIVATE_STORAGE_BYTES || key_count > MAX_PLUGIN_PRIVATE_STORAGE_KEYS
    {
        return Err(AppPersistenceError::InvalidInput(
            PLUGIN_PRIVATE_STORAGE_QUOTA_ERROR,
        ));
    }
    Ok(())
}

fn encode_blob_length(length: u64) -> [u8; 8] {
    length.to_be_bytes()
}

fn decode_blob_length(value: &[u8]) -> Result<u64> {
    if value.len() != 8 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    let length = u64::from_be_bytes(
        value
            .try_into()
            .map_err(|_| AppPersistenceError::InvalidStoredData)?,
    );
    if length > MAX_PLUGIN_PRIVATE_STORAGE_BLOB_BYTES as u64 {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(length)
}

fn blob_chunk_index(offset: u64) -> Result<i64> {
    i64::try_from(offset / MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES as u64)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin storage blob chunk"))
}

fn blob_chunk_offset(offset: u64) -> Result<usize> {
    usize::try_from(offset % MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES as u64)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin storage blob chunk"))
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn encode_storage_cursor(
    owner: &PluginPrivateStorageOwner,
    prefix: &str,
    store_revision: u64,
    after_key: &str,
) -> Result<String> {
    let cursor = StorageListCursor {
        version: CURSOR_VERSION,
        plugin_id: owner.plugin_id.as_str().to_owned(),
        signer_fingerprint_sha256: owner.signer_fingerprint_sha256.clone(),
        namespace: KV_NAMESPACE.to_owned(),
        prefix: prefix.to_owned(),
        store_revision,
        after_key: after_key.to_owned(),
    };
    let encoded =
        serde_json::to_vec(&cursor).map_err(|_| AppPersistenceError::InvalidStoredData)?;
    if encoded.len() > MAX_CURSOR_BYTES {
        return Err(AppPersistenceError::InvalidStoredData);
    }
    Ok(URL_SAFE_NO_PAD.encode(encoded))
}

fn decode_storage_cursor(
    encoded: &str,
    owner: &PluginPrivateStorageOwner,
    prefix: &str,
    store_revision: u64,
) -> Result<String> {
    if encoded.is_empty() || encoded.len() > MAX_CURSOR_BYTES * 2 {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin storage cursor",
        ));
    }
    let raw = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin storage cursor"))?;
    if raw.len() > MAX_CURSOR_BYTES || URL_SAFE_NO_PAD.encode(&raw) != encoded {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin storage cursor",
        ));
    }
    let cursor: StorageListCursor = serde_json::from_slice(&raw)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin storage cursor"))?;
    if cursor.version != CURSOR_VERSION
        || cursor.plugin_id != owner.plugin_id.as_str()
        || cursor.signer_fingerprint_sha256 != owner.signer_fingerprint_sha256
        || cursor.namespace != KV_NAMESPACE
        || cursor.prefix != prefix
        || cursor.store_revision != store_revision
    {
        return Err(AppPersistenceError::Conflict);
    }
    validate_storage_key(&cursor.after_key)
        .map_err(|_| AppPersistenceError::InvalidInput("invalid plugin storage cursor"))?;
    if !cursor.after_key.starts_with(prefix) {
        return Err(AppPersistenceError::InvalidInput(
            "invalid plugin storage cursor",
        ));
    }
    Ok(cursor.after_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginInstalledRecord;
    use norishell_core_api::{PluginCapability, PluginInstallState, WireSequence};
    use rusqlite::params;

    fn repository(directory: &tempfile::TempDir) -> AppRepository {
        AppRepository::open(directory.path().join("data").join("norishell.sqlite3"))
            .expect("repository")
    }

    fn install_owner(
        repository: &mut AppRepository,
        plugin_id: &str,
        signer: char,
    ) -> PluginPrivateStorageOwner {
        let plugin_id = PluginId::parse(plugin_id).expect("plugin id");
        let record = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Private Storage Fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            signer_fingerprint_sha256: signer.to_string().repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::StoragePlugin],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        repository
            .activate_plugin_installation(
                None,
                &record,
                1,
                &"A".repeat(44),
                &"B".repeat(88),
                true,
                None,
            )
            .expect("install fixture");
        PluginPrivateStorageOwner {
            plugin_id,
            signer_fingerprint_sha256: record.signer_fingerprint_sha256,
        }
    }

    fn granted() -> bool {
        true
    }

    #[test]
    fn kv_cursor_is_owner_and_store_revision_bound_and_schema_batches_are_atomic() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let owner = install_owner(&mut repository, "org.norishell.private-storage", '1');
        let other = install_owner(&mut repository, "org.norishell.private-storage-other", '2');

        let first = repository
            .set_plugin_private_storage_kv(&owner, "alpha", b"one", 0, &granted)
            .expect("first write");
        assert_eq!(first.entry.as_ref().unwrap().revision, 1);
        let signer_changed_owner = PluginPrivateStorageOwner {
            plugin_id: owner.plugin_id.clone(),
            signer_fingerprint_sha256: "f".repeat(64),
        };
        assert!(matches!(
            repository.get_plugin_private_storage_kv(&signer_changed_owner, "alpha", &granted),
            Err(AppPersistenceError::Conflict)
        ));
        let second = repository
            .set_plugin_private_storage_kv(&owner, "beta", b"two", 0, &granted)
            .expect("second write");
        assert_eq!(second.state.store_revision, 2);
        assert!(matches!(
            repository.set_plugin_private_storage_kv(&owner, "alpha", b"stale", 0, &granted),
            Err(AppPersistenceError::Conflict)
        ));

        let page = repository
            .list_plugin_private_storage_kv(&owner, "", None, 1, &granted)
            .expect("first page");
        assert_eq!(page.entries.len(), 1);
        let cursor = page.next_cursor.expect("cursor");
        assert!(matches!(
            repository.list_plugin_private_storage_kv(&other, "", Some(&cursor), 1, &granted),
            Err(AppPersistenceError::Conflict)
        ));
        repository
            .set_plugin_private_storage_kv(&owner, "gamma", b"three", 0, &granted)
            .expect("third write");
        assert!(matches!(
            repository.list_plugin_private_storage_kv(&owner, "", Some(&cursor), 1, &granted),
            Err(AppPersistenceError::Conflict)
        ));

        let schema = repository
            .commit_plugin_private_storage_schema(
                &owner,
                0,
                1,
                &[PluginPrivateStorageMutation::KvSet {
                    key: "schema-value".to_owned(),
                    value_bytes: b"v1".to_vec(),
                    expected_revision: 0,
                }],
                &granted,
            )
            .expect("schema commit");
        assert_eq!(schema.schema_version, 1);
        let before_failed_batch = repository
            .get_plugin_private_storage_kv(&owner, "schema-value", &granted)
            .expect("stored schema value")
            .entry
            .expect("entry");
        assert!(matches!(
            repository.commit_plugin_private_storage_schema(
                &owner,
                1,
                2,
                &[
                    PluginPrivateStorageMutation::KvSet {
                        key: "would-rollback".to_owned(),
                        value_bytes: b"new".to_vec(),
                        expected_revision: 0,
                    },
                    PluginPrivateStorageMutation::KvDelete {
                        key: "schema-value".to_owned(),
                        expected_revision: before_failed_batch.revision + 1,
                    },
                ],
                &granted,
            ),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(
            repository
                .get_plugin_private_storage_kv(&owner, "would-rollback", &granted)
                .unwrap()
                .entry
                .is_none()
        );
        assert_eq!(
            repository
                .get_plugin_private_storage_schema(&owner, &granted)
                .unwrap()
                .schema_version,
            1
        );
    }

    #[test]
    fn blob_chunks_are_bounded_revisioned_and_read_without_base64_storage() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let owner = install_owner(&mut repository, "org.norishell.private-blob", '3');
        let first_chunk = vec![7_u8; MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES];
        let first = repository
            .write_plugin_private_storage_blob(&owner, "capture", 0, &first_chunk, 0, &granted)
            .expect("first blob chunk");
        assert_eq!(
            first.total_bytes,
            MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES as u64
        );
        let appended = repository
            .write_plugin_private_storage_blob(
                &owner,
                "capture",
                MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES as u64,
                b"tail",
                first.revision,
                &granted,
            )
            .expect("append chunk");
        assert_eq!(appended.revision, 2);
        let read = repository
            .read_plugin_private_storage_blob(
                &owner,
                "capture",
                MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES as u64,
                4,
                &granted,
            )
            .expect("read tail");
        assert_eq!(read.bytes, b"tail");
        assert!(read.eof);
        let raw_chunk: (String, Vec<u8>) = repository
            .connection
            .query_row(
                "SELECT typeof(value_bytes), value_bytes FROM plugin_private_storage
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND namespace = 'blob' AND storage_key = 'capture' AND chunk_index = 1",
                params![owner.plugin_id.as_str(), owner.signer_fingerprint_sha256,],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("raw blob row");
        assert_eq!(raw_chunk.0, "blob");
        assert_eq!(raw_chunk.1, b"tail");
        assert!(matches!(
            repository.write_plugin_private_storage_blob(
                &owner,
                "capture",
                (MAX_PLUGIN_PRIVATE_STORAGE_BLOB_CHUNK_BYTES - 1) as u64,
                b"xx",
                appended.revision,
                &granted,
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let deleted = repository
            .delete_plugin_private_storage_blob(&owner, "capture", appended.revision, &granted)
            .expect("delete blob");
        assert!(deleted.store_revision > appended.state.store_revision);
        assert!(matches!(
            repository.read_plugin_private_storage_blob(&owner, "capture", 0, 1, &granted),
            Err(AppPersistenceError::NotFound)
        ));
    }

    #[test]
    fn cache_expiry_and_authorization_fence_never_leave_a_partial_write() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let owner = install_owner(&mut repository, "org.norishell.private-cache", '4');
        let cached = repository
            .set_plugin_private_storage_cache(&owner, "preview", b"value", 1, &granted)
            .expect("cache set");
        std::thread::sleep(std::time::Duration::from_millis(3));
        let expired = repository
            .get_plugin_private_storage_cache(&owner, "preview", &granted)
            .expect("expired cache read");
        assert!(expired.entry.is_none());
        assert!(expired.state.store_revision > cached.state.store_revision);
        let before = expired.state;
        let revoked = || false;
        assert!(matches!(
            repository.set_plugin_private_storage_kv(&owner, "blocked", b"no", 0, &revoked),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(
            repository
                .get_plugin_private_storage_kv(&owner, "blocked", &granted)
                .unwrap()
                .entry
                .is_none()
        );
        assert_eq!(
            repository
                .get_plugin_private_storage_schema(&owner, &granted)
                .unwrap(),
            before
        );

        assert!(matches!(
            repository.replace_plugin_private_persistent_state(&owner, 0, "{}", &revoked),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(
            repository
                .get_plugin_private_persistent_state(&owner, &granted)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn persistent_declarative_state_is_json_object_cas_and_shares_store_revision() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let owner = install_owner(
            &mut repository,
            "org.norishell.private-persistent-state",
            '7',
        );

        assert!(
            repository
                .get_plugin_private_persistent_state(&owner, &granted)
                .expect("empty state")
                .is_none()
        );
        let first = repository
            .replace_plugin_private_persistent_state(&owner, 0, r#"{"loggedIn":true}"#, &granted)
            .expect("create persistent state");
        assert_eq!(first.revision, 1);
        assert_eq!(first.state.store_revision, 1);
        assert_eq!(first.value_json, r#"{"loggedIn":true}"#);
        assert!(matches!(
            repository.replace_plugin_private_persistent_state(&owner, 0, "{}", &granted),
            Err(AppPersistenceError::Conflict)
        ));
        assert!(matches!(
            repository.replace_plugin_private_persistent_state(&owner, 1, "[]", &granted),
            Err(AppPersistenceError::InvalidInput(_))
        ));

        repository
            .set_plugin_private_storage_kv(&owner, "shared", b"state", 0, &granted)
            .expect("kv write");
        let snapshot = repository
            .get_plugin_private_persistent_state(&owner, &granted)
            .expect("persistent state")
            .expect("stored state");
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.state.store_revision, 2);

        let replaced = repository
            .replace_plugin_private_persistent_state(&owner, 1, r#"{"loggedIn":false}"#, &granted)
            .expect("replace persistent state");
        assert_eq!(replaced.revision, 2);
        assert_eq!(replaced.state.store_revision, 3);
    }

    #[test]
    fn v32_migration_copies_legacy_declarative_state_to_private_meta_storage() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("data").join("norishell.sqlite3");
        let owner = {
            let mut repository = AppRepository::open(&database_path).expect("current repository");
            crate::remove_schema_added_after_fixture_version(&repository.connection, 31)
                .expect("remove post-v31 tables");
            let owner = install_owner(
                &mut repository,
                "org.norishell.private-state-migration",
                '8',
            );
            repository
                .connection
                .execute_batch(
                    "CREATE TABLE plugin_storage (
                       plugin_id TEXT PRIMARY KEY CHECK(length(plugin_id) BETWEEN 1 AND 160),
                       value_json TEXT NOT NULL
                         CHECK(length(CAST(value_json AS BLOB)) BETWEEN 2 AND 65536),
                       state_version INTEGER NOT NULL CHECK(state_version >= 1),
                       updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
                     ) STRICT;",
                )
                .expect("legacy table");
            repository
                .connection
                .execute(
                    "INSERT INTO plugin_storage
                     (plugin_id, value_json, state_version, updated_at_ms)
                     VALUES (?1, ?2, 7, 1)",
                    params![owner.plugin_id.as_str(), r#"{"legacy":true}"#],
                )
                .expect("legacy state");
            repository
                .connection
                .execute_batch("PRAGMA user_version = 31;")
                .expect("force v31 fixture");
            owner
        };

        let repository = AppRepository::open(&database_path).expect("migrate v31 repository");
        let persistent = repository
            .get_plugin_private_persistent_state(&owner, &granted)
            .expect("read migrated state")
            .expect("migrated state");
        assert_eq!(persistent.value_json, r#"{"legacy":true}"#);
        assert_eq!(persistent.revision, 7);
        assert_eq!(persistent.state.store_revision, 7);
        assert_eq!(persistent.state.schema_version, 0);
        let private_rows: (String, Vec<u8>, i64, i64) = repository
            .connection
            .query_row(
                "SELECT typeof(value_bytes), value_bytes, value_revision, store_revision
                 FROM plugin_private_storage
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND namespace = 'meta' AND storage_key = ?3 AND chunk_index = 0",
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    PERSISTENT_STATE_KEY,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("private state row");
        assert_eq!(private_rows.0, "blob");
        assert_eq!(private_rows.1, br#"{"legacy":true}"#);
        assert_eq!((private_rows.2, private_rows.3), (7, 7));
        let metadata: (Vec<u8>, i64, i64) = repository
            .connection
            .query_row(
                "SELECT value_bytes, value_revision, store_revision
                 FROM plugin_private_storage
                 WHERE plugin_id = ?1 AND signer_fingerprint_sha256 = ?2
                   AND namespace = 'meta' AND storage_key = ?3 AND chunk_index = 0",
                params![
                    owner.plugin_id.as_str(),
                    owner.signer_fingerprint_sha256,
                    STATE_KEY,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("private state metadata row");
        assert_eq!(metadata.0, vec![0; 8]);
        assert_eq!((metadata.1, metadata.2), (7, 7));
        let legacy_table_exists: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'plugin_storage'",
                [],
                |row| row.get(0),
            )
            .expect("legacy table check");
        assert_eq!(legacy_table_exists, 0);
    }

    #[test]
    fn quota_and_foreign_key_cleanup_are_enforced_without_partial_storage() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let owner = install_owner(&mut repository, "org.norishell.private-quota", '5');
        let payload = vec![0_u8; MAX_PLUGIN_PRIVATE_STORAGE_KV_VALUE_BYTES];
        let transaction = repository
            .connection
            .transaction()
            .expect("seed transaction");
        for index in 0..255_u16 {
            let key = format!("quota-{index:03}");
            transaction
                .execute(
                    "INSERT INTO plugin_private_storage
                     (plugin_id, signer_fingerprint_sha256, namespace, storage_key, chunk_index,
                      value_bytes, value_revision, store_revision, expires_at_ms)
                     VALUES (?1, ?2, 'kv', ?3, 0, ?4, 1, 1, NULL)",
                    params![
                        owner.plugin_id.as_str(),
                        owner.signer_fingerprint_sha256,
                        key,
                        payload,
                    ],
                )
                .expect("seed quota row");
        }
        transaction.commit().expect("seed quota rows");
        assert!(matches!(
            repository.set_plugin_private_storage_kv(&owner, "over-limit", &payload, 0, &granted,),
            Err(AppPersistenceError::InvalidInput(
                PLUGIN_PRIVATE_STORAGE_QUOTA_ERROR
            ))
        ));
        let persistent_payload = format!(
            "{{\"payload\":\"{}\"}}",
            "x".repeat(MAX_PLUGIN_PRIVATE_STORAGE_KV_VALUE_BYTES - 14)
        );
        assert_eq!(
            persistent_payload.len(),
            MAX_PLUGIN_PRIVATE_STORAGE_KV_VALUE_BYTES
        );
        assert!(matches!(
            repository.replace_plugin_private_persistent_state(
                &owner,
                0,
                &persistent_payload,
                &granted,
            ),
            Err(AppPersistenceError::InvalidInput(
                PLUGIN_PRIVATE_STORAGE_QUOTA_ERROR
            ))
        ));
        assert!(
            repository
                .get_plugin_private_persistent_state(&owner, &granted)
                .unwrap()
                .is_none()
        );
        assert!(
            repository
                .get_plugin_private_storage_kv(&owner, "over-limit", &granted)
                .unwrap()
                .entry
                .is_none()
        );
        repository
            .connection
            .execute(
                "DELETE FROM plugin_installations WHERE plugin_id = ?1",
                [owner.plugin_id.as_str()],
            )
            .expect("uninstall fixture");
        let remaining: i64 = repository
            .connection
            .query_row(
                "SELECT COUNT(*) FROM plugin_private_storage WHERE plugin_id = ?1",
                [owner.plugin_id.as_str()],
                |row| row.get(0),
            )
            .expect("storage count");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn key_quota_rejects_the_4097th_visible_key_atomically() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut repository = repository(&directory);
        let owner = install_owner(&mut repository, "org.norishell.private-key-quota", '6');
        let mut schema_version = 0;
        for batch in
            0..(MAX_PLUGIN_PRIVATE_STORAGE_KEYS / MAX_PLUGIN_PRIVATE_STORAGE_BATCH_MUTATIONS)
        {
            let mutations = (0..MAX_PLUGIN_PRIVATE_STORAGE_BATCH_MUTATIONS)
                .map(|offset| PluginPrivateStorageMutation::KvSet {
                    key: format!("key-{batch:02}-{offset:02}"),
                    value_bytes: Vec::new(),
                    expected_revision: 0,
                })
                .collect::<Vec<_>>();
            schema_version += 1;
            repository
                .commit_plugin_private_storage_schema(
                    &owner,
                    schema_version - 1,
                    schema_version,
                    &mutations,
                    &granted,
                )
                .expect("fill visible keys");
        }
        assert_eq!(
            repository
                .list_plugin_private_storage_kv(&owner, "", None, 1, &granted)
                .unwrap()
                .state
                .schema_version,
            schema_version
        );
        assert!(matches!(
            repository.commit_plugin_private_storage_schema(
                &owner,
                schema_version,
                schema_version + 1,
                &[PluginPrivateStorageMutation::KvSet {
                    key: "key-over-limit".to_owned(),
                    value_bytes: Vec::new(),
                    expected_revision: 0,
                }],
                &granted,
            ),
            Err(AppPersistenceError::InvalidInput(
                PLUGIN_PRIVATE_STORAGE_QUOTA_ERROR
            ))
        ));
        assert!(
            repository
                .get_plugin_private_storage_kv(&owner, "key-over-limit", &granted)
                .unwrap()
                .entry
                .is_none()
        );
        assert_eq!(
            repository
                .get_plugin_private_storage_schema(&owner, &granted)
                .unwrap()
                .schema_version,
            schema_version
        );
    }
}
