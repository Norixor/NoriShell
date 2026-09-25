//! Core-owned driver for plugin-private non-secret storage.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use norishell_app_persistence::{
    AppPersistenceError, PLUGIN_PRIVATE_STORAGE_QUOTA_ERROR, PluginPrivateStorageBlobRead,
    PluginPrivateStorageEntry, PluginPrivateStorageMutation, PluginPrivateStorageOwner,
    PluginPrivateStorageState,
};
use norishell_core_api::{
    PluginApiErrorCode, PluginStorageBlobRead, PluginStorageEntry, PluginStorageMutation,
    PluginStorageOperation, PluginStorageResult, PluginStorageState,
};

use crate::host_service::HostService;

use super::{MAX_CHUNK_BYTES, ResourceFence, ResourceOwner};

const MAX_STORAGE_VALUE_BYTES: usize = 64 * 1024;

/// The durable repository gets only the signer-bound owner. Package and
/// generation are intentionally kept in the Core fence so an update/restart
/// retains same-signer data without accepting an old instance.
#[derive(Clone)]
pub(crate) struct PluginStorageDriver {
    hosts: HostService,
}

impl PluginStorageDriver {
    pub(crate) fn new(hosts: HostService) -> Self {
        Self { hosts }
    }

    pub(crate) fn invoke(
        &self,
        owner: &ResourceOwner,
        fence: ResourceFence,
        operation: &PluginStorageOperation,
    ) -> Result<PluginStorageResult, PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let durable_owner = PluginPrivateStorageOwner {
            plugin_id: owner.plugin_id.clone(),
            signer_fingerprint_sha256: owner.signer.clone(),
        };
        let result = match operation {
            PluginStorageOperation::KvGet { key } => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.get_plugin_private_storage_kv(&durable_owner, key, fence.as_ref())
                })
                .map(|value| PluginStorageResult::KvValue {
                    state: storage_state(value.state),
                    entry: value.entry.map(storage_entry),
                }),
            PluginStorageOperation::KvSet {
                key,
                value_base64,
                expected_revision,
            } => {
                let value = decode_storage_bytes(value_base64, MAX_STORAGE_VALUE_BYTES, false)?;
                self.hosts
                    .with_plugin_repository(|repository| {
                        repository.set_plugin_private_storage_kv(
                            &durable_owner,
                            key,
                            &value,
                            *expected_revision,
                            fence.as_ref(),
                        )
                    })
                    .map(|value| PluginStorageResult::KvValue {
                        state: storage_state(value.state),
                        entry: value.entry.map(storage_entry),
                    })
            }
            PluginStorageOperation::KvDelete {
                key,
                expected_revision,
            } => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.delete_plugin_private_storage_kv(
                        &durable_owner,
                        key,
                        *expected_revision,
                        fence.as_ref(),
                    )
                })
                .map(|state| PluginStorageResult::State {
                    state: storage_state(state),
                }),
            PluginStorageOperation::KvList {
                prefix,
                cursor,
                limit,
            } => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.list_plugin_private_storage_kv(
                        &durable_owner,
                        prefix,
                        cursor.as_deref(),
                        *limit,
                        fence.as_ref(),
                    )
                })
                .map(|page| PluginStorageResult::KvPage {
                    state: storage_state(page.state),
                    entries: page.entries.into_iter().map(storage_entry).collect(),
                    next_cursor: page.next_cursor,
                }),
            PluginStorageOperation::BlobRead {
                key,
                offset,
                length,
            } => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.read_plugin_private_storage_blob(
                        &durable_owner,
                        key,
                        *offset,
                        *length,
                        fence.as_ref(),
                    )
                })
                .map(storage_blob_read),
            PluginStorageOperation::BlobWrite {
                key,
                offset,
                data_base64,
                expected_revision,
            } => {
                let bytes = decode_storage_bytes(data_base64, MAX_CHUNK_BYTES as usize, true)?;
                self.hosts
                    .with_plugin_repository(|repository| {
                        repository.write_plugin_private_storage_blob(
                            &durable_owner,
                            key,
                            *offset,
                            &bytes,
                            *expected_revision,
                            fence.as_ref(),
                        )
                    })
                    .map(|write| PluginStorageResult::BlobWritten {
                        state: storage_state(write.state),
                        key: write.key,
                        revision: write.revision,
                        total_bytes: write.total_bytes,
                    })
            }
            PluginStorageOperation::BlobDelete {
                key,
                expected_revision,
            } => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.delete_plugin_private_storage_blob(
                        &durable_owner,
                        key,
                        *expected_revision,
                        fence.as_ref(),
                    )
                })
                .map(|state| PluginStorageResult::State {
                    state: storage_state(state),
                }),
            PluginStorageOperation::CacheGet { key } => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.get_plugin_private_storage_cache(&durable_owner, key, fence.as_ref())
                })
                .map(|value| PluginStorageResult::CacheValue {
                    state: storage_state(value.state),
                    entry: value.entry.map(storage_entry),
                }),
            PluginStorageOperation::CacheSet {
                key,
                value_base64,
                ttl_ms,
            } => {
                let value = decode_storage_bytes(value_base64, MAX_STORAGE_VALUE_BYTES, false)?;
                self.hosts
                    .with_plugin_repository(|repository| {
                        repository.set_plugin_private_storage_cache(
                            &durable_owner,
                            key,
                            &value,
                            *ttl_ms,
                            fence.as_ref(),
                        )
                    })
                    .map(|value| PluginStorageResult::CacheValue {
                        state: storage_state(value.state),
                        entry: value.entry.map(storage_entry),
                    })
            }
            PluginStorageOperation::CacheDelete { key } => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.delete_plugin_private_storage_cache(
                        &durable_owner,
                        key,
                        fence.as_ref(),
                    )
                })
                .map(|state| PluginStorageResult::State {
                    state: storage_state(state),
                }),
            PluginStorageOperation::CacheClear {} => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.clear_plugin_private_storage_cache(&durable_owner, fence.as_ref())
                })
                .map(|state| PluginStorageResult::State {
                    state: storage_state(state),
                }),
            PluginStorageOperation::SchemaGet {} => self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.get_plugin_private_storage_schema(&durable_owner, fence.as_ref())
                })
                .map(|state| PluginStorageResult::State {
                    state: storage_state(state),
                }),
            PluginStorageOperation::SchemaCommit {
                expected_version,
                new_version,
                mutations,
            } => {
                let mutations = decode_storage_mutations(mutations)?;
                self.hosts
                    .with_plugin_repository(|repository| {
                        repository.commit_plugin_private_storage_schema(
                            &durable_owner,
                            *expected_version,
                            *new_version,
                            &mutations,
                            fence.as_ref(),
                        )
                    })
                    .map(|state| PluginStorageResult::State {
                        state: storage_state(state),
                    })
            }
        };
        result.map_err(|error| map_storage_error(error, &fence))
    }
}

pub(crate) fn validate_operation(
    operation: &PluginStorageOperation,
) -> Result<(), PluginApiErrorCode> {
    match operation {
        PluginStorageOperation::KvSet { value_base64, .. }
        | PluginStorageOperation::CacheSet { value_base64, .. } => {
            decode_storage_bytes(value_base64, MAX_STORAGE_VALUE_BYTES, false).map(|_| ())
        }
        PluginStorageOperation::BlobWrite { data_base64, .. } => {
            decode_storage_bytes(data_base64, MAX_CHUNK_BYTES as usize, true).map(|_| ())
        }
        PluginStorageOperation::SchemaCommit {
            expected_version,
            new_version,
            mutations,
        } => {
            if new_version <= expected_version {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            decode_storage_mutations(mutations).map(|_| ())
        }
        _ => Ok(()),
    }
}

fn decode_storage_mutations(
    mutations: &[PluginStorageMutation],
) -> Result<Vec<PluginPrivateStorageMutation>, PluginApiErrorCode> {
    mutations
        .iter()
        .map(|mutation| match mutation {
            PluginStorageMutation::KvSet {
                key,
                value_base64,
                expected_revision,
            } => decode_storage_bytes(value_base64, MAX_STORAGE_VALUE_BYTES, false).map(
                |value_bytes| PluginPrivateStorageMutation::KvSet {
                    key: key.clone(),
                    value_bytes,
                    expected_revision: *expected_revision,
                },
            ),
            PluginStorageMutation::KvDelete {
                key,
                expected_revision,
            } => Ok(PluginPrivateStorageMutation::KvDelete {
                key: key.clone(),
                expected_revision: *expected_revision,
            }),
        })
        .collect()
}

fn decode_storage_bytes(
    encoded: &str,
    max_bytes: usize,
    require_nonempty: bool,
) -> Result<Vec<u8>, PluginApiErrorCode> {
    let max_encoded = max_bytes
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .ok_or(PluginApiErrorCode::InvalidRequest)?;
    if encoded.len() > max_encoded {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    if bytes.len() > max_bytes
        || (require_nonempty && bytes.is_empty())
        || STANDARD.encode(&bytes) != encoded
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(bytes)
}

fn storage_state(state: PluginPrivateStorageState) -> PluginStorageState {
    PluginStorageState {
        store_revision: state.store_revision,
        schema_version: state.schema_version,
    }
}

fn storage_entry(entry: PluginPrivateStorageEntry) -> PluginStorageEntry {
    PluginStorageEntry {
        key: entry.key,
        value_base64: STANDARD.encode(entry.value_bytes),
        revision: entry.revision,
    }
}

fn storage_blob_read(read: PluginPrivateStorageBlobRead) -> PluginStorageResult {
    PluginStorageResult::BlobRead {
        state: storage_state(read.state),
        blob: PluginStorageBlobRead {
            key: read.key,
            revision: read.revision,
            total_bytes: read.total_bytes,
            offset: read.offset,
            data_base64: STANDARD.encode(read.bytes),
            eof: read.eof,
        },
    }
}

fn map_storage_error(error: AppPersistenceError, fence: &ResourceFence) -> PluginApiErrorCode {
    if !fence() {
        return PluginApiErrorCode::Revoked;
    }
    match error {
        AppPersistenceError::InvalidInput(PLUGIN_PRIVATE_STORAGE_QUOTA_ERROR) => {
            PluginApiErrorCode::QuotaExceeded
        }
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::Endpoint(_) => {
            PluginApiErrorCode::InvalidRequest
        }
        AppPersistenceError::NotFound => PluginApiErrorCode::NotFound,
        AppPersistenceError::Conflict
        | AppPersistenceError::IdempotencyConflict
        | AppPersistenceError::DatabaseNotFresh => PluginApiErrorCode::Conflict,
        AppPersistenceError::UnsupportedSchema(_)
        | AppPersistenceError::KnownHostMismatch { .. }
        | AppPersistenceError::KnownHostAlgorithmChanged { .. }
        | AppPersistenceError::RequiresReload
        | AppPersistenceError::RestoreCommitUnknown
        | AppPersistenceError::InvalidStoredData
        | AppPersistenceError::Database(_)
        | AppPersistenceError::Io(_) => PluginApiErrorCode::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_app_persistence::PluginInstalledRecord;
    use norishell_core_api::{PluginCapability, PluginId, PluginInstallState, WireSequence};
    use std::sync::Arc;

    fn owner() -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse("org.norishell.storage-driver").expect("plugin id"),
            signer: "1".repeat(64),
            package: "a".repeat(64),
            generation: WireSequence::new(1),
        }
    }

    fn install_fixture(hosts: &HostService, owner: &ResourceOwner) {
        hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    None,
                    &PluginInstalledRecord {
                        plugin_id: owner.plugin_id.clone(),
                        name: "Storage Driver Fixture".to_owned(),
                        publisher: "NoriShell".to_owned(),
                        signer_fingerprint_sha256: owner.signer.clone(),
                        active_version: "1.0.0".to_owned(),
                        package_sha256: owner.package.clone(),
                        capabilities: vec![PluginCapability::StoragePlugin],
                        state: PluginInstallState::Enabled,
                        state_version: WireSequence::new(1),
                        installed_at_unix_ms: 1,
                        updated_at_unix_ms: 1,
                    },
                    1,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    None,
                )
            })
            .expect("install fixture");
    }

    #[test]
    fn storage_bytes_must_be_canonical_base64_and_fit_raw_limits() {
        assert_eq!(decode_storage_bytes("AA==", 1, true).unwrap(), vec![0]);
        assert_eq!(
            decode_storage_bytes("AA", 1, true),
            Err(PluginApiErrorCode::InvalidRequest)
        );
        assert_eq!(
            decode_storage_bytes(" A A = =", 1, true),
            Err(PluginApiErrorCode::InvalidRequest)
        );
        assert_eq!(
            decode_storage_bytes("", 1, true),
            Err(PluginApiErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn driver_uses_the_core_owner_and_retains_data_across_package_generation_change() {
        let directory = tempfile::tempdir().expect("tempdir");
        let hosts = HostService::start(directory.path()).expect("host service");
        let initial_owner = owner();
        install_fixture(&hosts, &initial_owner);
        let driver = PluginStorageDriver::new(hosts.clone());
        let granted: ResourceFence = Arc::new(|| true);
        let written = driver
            .invoke(
                &initial_owner,
                granted.clone(),
                &PluginStorageOperation::KvSet {
                    key: "mode".to_owned(),
                    value_base64: "ZGFyaw==".to_owned(),
                    expected_revision: 0,
                },
            )
            .expect("write");
        assert!(matches!(written, PluginStorageResult::KvValue { .. }));

        hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    Some(WireSequence::new(1)),
                    &PluginInstalledRecord {
                        plugin_id: initial_owner.plugin_id.clone(),
                        name: "Storage Driver Fixture".to_owned(),
                        publisher: "NoriShell".to_owned(),
                        signer_fingerprint_sha256: initial_owner.signer.clone(),
                        active_version: "1.1.0".to_owned(),
                        package_sha256: "b".repeat(64),
                        capabilities: vec![PluginCapability::StoragePlugin],
                        state: PluginInstallState::Enabled,
                        state_version: WireSequence::new(2),
                        installed_at_unix_ms: 1,
                        updated_at_unix_ms: 2,
                    },
                    1,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    None,
                )
            })
            .expect("same-signer package update");
        let restarted_owner = ResourceOwner {
            package: "b".repeat(64),
            generation: WireSequence::new(2),
            ..initial_owner
        };
        let read = driver
            .invoke(
                &restarted_owner,
                granted,
                &PluginStorageOperation::KvGet {
                    key: "mode".to_owned(),
                },
            )
            .expect("same-signer data survives restart");
        assert!(matches!(
            read,
            PluginStorageResult::KvValue {
                entry: Some(PluginStorageEntry { value_base64, .. }),
                ..
            } if value_base64 == "ZGFyaw=="
        ));
        assert_eq!(
            driver.invoke(
                &restarted_owner,
                Arc::new(|| false),
                &PluginStorageOperation::KvGet {
                    key: "mode".to_owned(),
                },
            ),
            Err(PluginApiErrorCode::Revoked)
        );
    }
}
