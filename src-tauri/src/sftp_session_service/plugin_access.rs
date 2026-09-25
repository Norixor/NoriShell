//! Trusted plugin SFTP bridge.
//!
//! This module is intentionally reachable only from the Core-side plugin broker.
//! Guest handles are resolved there before these methods run; no method accepts a
//! guest HostId, session ID, or ambient remote path.

use super::*;

#[derive(Clone)]
pub(crate) struct PluginSftpAccess {
    session_id: wire::SftpSessionId,
    generation: wire::WireSequence,
    root_path: RemotePath,
}

pub(crate) struct PluginSftpBinaryRead {
    pub(crate) bytes: Vec<u8>,
    pub(crate) next_offset: u64,
    pub(crate) total_bytes: u64,
    pub(crate) eof: bool,
}

/// Core-only state for a guest-driven, bounded binary upload. The guest sees
/// only the driver-generated upload handle; the remote target and temporary
/// path stay in this trusted layer.
pub(crate) struct PluginSftpUpload {
    session_id: wire::SftpSessionId,
    generation: wire::WireSequence,
    target_path: RemotePath,
    temporary_path: RemotePath,
    target_precondition: Option<RemoteObjectPrecondition>,
    expected_bytes: u64,
    received_bytes: u64,
}

pub(crate) struct PluginSftpUploadProgress {
    pub(crate) received_bytes: u64,
    pub(crate) expected_bytes: u64,
}

impl PluginSftpUpload {
    pub(crate) fn expected_bytes(&self) -> u64 {
        self.expected_bytes
    }
}

#[cfg(test)]
impl PluginSftpAccess {
    pub(crate) fn fixture(root_path: Vec<u8>) -> Self {
        Self {
            session_id: wire::SftpSessionId::new(),
            generation: wire::WireSequence::new(1),
            root_path: normalize_plugin_root_path(root_path).expect("fixture root"),
        }
    }
}

impl SftpSessionService {
    /// Opens a fresh dedicated saved-Host SFTP transport for a plugin only
    /// after this service has re-resolved the full resource-neutral SSH profile
    /// and matched the exact revision approved by the user. This must never be
    /// replaced with `open_on_ssh_session`: plugins do not inherit terminal
    /// channels or their lifetime.
    pub(crate) async fn plugin_open_saved_host(
        &self,
        request: &wire::SftpSessionOpenRequest,
        expected_connection_revision: &str,
        admission_fence: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<wire::SftpSessionSummary, SftpProductionError> {
        if expected_connection_revision.is_empty() || !admission_fence() {
            return Err(SftpRuntimeError::Conflict.into());
        }
        self.open_with_connection_gate(
            request,
            Some(expected_connection_revision),
            Some(admission_fence),
            None,
        )
        .await
    }

    /// Binds an already-open dedicated SFTP generation to one lexically-normalized
    /// Core-approved remote root. This is a client-side path range, never an OS jail.
    pub(crate) async fn plugin_access(
        &self,
        session_id: wire::SftpSessionId,
        generation: wire::WireSequence,
        root_path: Vec<u8>,
    ) -> Result<PluginSftpAccess, SftpProductionError> {
        let root_path = normalize_plugin_root_path(root_path)?;
        let access = PluginSftpAccess {
            session_id,
            generation,
            root_path,
        };
        self.plugin_require_directory(&access, &access.root_path)
            .await?;
        Ok(access)
    }

    pub(crate) async fn plugin_list_root(
        &self,
        access: &PluginSftpAccess,
        cursor: Option<Vec<u8>>,
        page_size: u16,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<wire::SftpDirectoryListing, SftpProductionError> {
        self.plugin_list_path(
            access,
            access.root_path.clone(),
            cursor,
            page_size,
            operation_id,
            idempotency_key,
        )
        .await
    }

    pub(crate) async fn plugin_list_directory(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        cursor: Option<Vec<u8>>,
        page_size: u16,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<wire::SftpDirectoryListing, SftpProductionError> {
        let path = self.plugin_directory_path(access, directory_ref).await?;
        self.plugin_list_path(
            access,
            path,
            cursor,
            page_size,
            operation_id,
            idempotency_key,
        )
        .await
    }

    pub(crate) async fn plugin_list_child_directory(
        &self,
        access: &PluginSftpAccess,
        parent_directory_ref: &str,
        entry_ref: &str,
        page_size: u16,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<wire::SftpDirectoryListing, SftpProductionError> {
        let parent = self
            .plugin_directory_path(access, parent_directory_ref)
            .await?;
        self.plugin_require_directory(access, &parent).await?;
        let entry = self
            .plugin_entry(access, parent_directory_ref, entry_ref)
            .await?;
        if entry.precondition.kind != RemoteEntryKind::Directory
            || !plugin_path_is_direct_child(&parent, &entry.path)
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        self.plugin_list_path(
            access,
            entry.path,
            None,
            page_size,
            operation_id,
            idempotency_key,
        )
        .await
    }

    pub(crate) async fn plugin_read_binary(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        entry_ref: &str,
        offset: u64,
        maximum_bytes: usize,
    ) -> Result<PluginSftpBinaryRead, SftpProductionError> {
        if maximum_bytes == 0 || maximum_bytes > 16 * 1024 {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let entry = self.plugin_entry(access, directory_ref, entry_ref).await?;
        if entry.precondition.kind != RemoteEntryKind::File {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let total_bytes = entry
            .precondition
            .size
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if offset > total_bytes {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let session_key = access.session_id.to_string();
        let generation = SftpGeneration::new(access.generation.get())?;
        let (start_offset, next_offset, observed_total_bytes, reset, bytes) = {
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&session_key)
                .ok_or(SftpRuntimeError::InvalidInput)?;
            self.reconcile_record_liveness(&session_key, record).await;
            let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
            // `tail_file_limited` is the existing bounded binary stream primitive. Its
            // own lstat does not carry a listing precondition, so validate the listing
            // fact first and reject any differing total/reset returned by the read.
            live.verify_remote_file(generation, &entry.path, &entry.precondition)
                .await?;
            live.read_remote_file_tail_limited(generation, &entry.path, offset, maximum_bytes)
                .await?
        };
        if start_offset != offset || reset || observed_total_bytes != total_bytes {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(PluginSftpBinaryRead {
            bytes,
            next_offset,
            total_bytes,
            eof: next_offset >= total_bytes,
        })
    }

    /// Replaces one previously listed regular file with a bounded binary
    /// payload. The implementation shares the same temporary-target and
    /// revalidation rules as streamed plugin uploads, rather than using a text
    /// mutation or a direct target truncate.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn plugin_write_binary(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        entry_ref: &str,
        expected: &wire::SftpRemoteObjectPrecondition,
        bytes: &[u8],
        operation_id: wire::OperationId,
    ) -> Result<(), SftpProductionError> {
        if bytes.len() > usize::from(wire::PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES) {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let mut upload = self
            .plugin_begin_upload_replace(
                access,
                directory_ref,
                entry_ref,
                expected,
                u64::try_from(bytes.len()).map_err(|_| SftpRuntimeError::InvalidInput)?,
                operation_id,
            )
            .await?;
        if !bytes.is_empty()
            && let Err(error) = self
                .plugin_append_upload(access, &mut upload, 0, bytes)
                .await
        {
            return self.plugin_abort_after_error(access, &upload, error).await;
        }
        if let Err(error) = self.plugin_commit_upload(access, &upload).await {
            return self.plugin_abort_after_error(access, &upload, error).await;
        }
        Ok(())
    }

    /// Starts a create-only upload. The target stays absent at both staging and
    /// commit checks; SFTP's hardlink extension performs the final no-replace
    /// commit when the server supports it.
    pub(crate) async fn plugin_begin_upload_create(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        name: &str,
        expected_bytes: u64,
        operation_id: wire::OperationId,
    ) -> Result<PluginSftpUpload, SftpProductionError> {
        let target = self.plugin_child_path(access, directory_ref, name).await?;
        self.plugin_begin_upload_path(access, target, None, expected_bytes, operation_id)
            .await
    }

    /// Starts a replacement upload only for an exact, still-current listed
    /// regular-file reference. The listing fact is checked once before staging
    /// and again immediately before the atomic replacement attempt.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn plugin_begin_upload_replace(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        entry_ref: &str,
        expected: &wire::SftpRemoteObjectPrecondition,
        expected_bytes: u64,
        operation_id: wire::OperationId,
    ) -> Result<PluginSftpUpload, SftpProductionError> {
        let entry = self.plugin_entry(access, directory_ref, entry_ref).await?;
        let actual = plugin_wire_precondition(&entry.precondition);
        if entry.precondition.kind != RemoteEntryKind::File || actual != *expected {
            return Err(SftpRuntimeError::Conflict.into());
        }
        self.plugin_begin_upload_path(
            access,
            entry.path,
            Some(entry.precondition),
            expected_bytes,
            operation_id,
        )
        .await
    }

    /// Appends one ordered bounded chunk. Every append rechecks the current
    /// temporary-file type and length, so a changed or replaced staging file
    /// cannot be resumed from an untrusted byte count.
    pub(crate) async fn plugin_append_upload(
        &self,
        access: &PluginSftpAccess,
        upload: &mut PluginSftpUpload,
        offset: u64,
        bytes: &[u8],
    ) -> Result<PluginSftpUploadProgress, SftpProductionError> {
        Self::plugin_require_upload_access(access, upload)?;
        if bytes.is_empty()
            || bytes.len() > usize::from(wire::PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES)
            || offset != upload.received_bytes
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let next = offset
            .checked_add(u64::try_from(bytes.len()).map_err(|_| SftpRuntimeError::InvalidInput)?)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        if next > upload.expected_bytes {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let session_key = access.session_id.to_string();
        let generation = SftpGeneration::new(access.generation.get())?;
        let temporary_path = remote_path_utf8(&upload.temporary_path)?;
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(&session_key)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        self.reconcile_record_liveness(&session_key, record).await;
        let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
        live.require_generation(generation)?;
        let metadata = live
            .bounded_protocol(live.transport.client().symlink_metadata(temporary_path))
            .await?;
        if !metadata.is_regular() || metadata.is_symlink() || metadata.len() != offset {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let mut file = live
            .bounded_protocol(
                live.transport
                    .client()
                    .open_with_flags(temporary_path, OpenFlags::WRITE),
            )
            .await?;
        let write_result = async {
            file.seek(io::SeekFrom::Start(offset))
                .await
                .map_err(|_| sftp_protocol_error())?;
            live.bounded_protocol(file.write_all(bytes)).await?;
            live.bounded_protocol(file.sync_all()).await?;
            let observed = live.bounded_protocol(file.metadata()).await?;
            live.bounded_protocol(file.close()).await?;
            if !observed.is_regular() || observed.is_symlink() || observed.len() != next {
                return Err(SftpRuntimeError::LengthMismatch.into());
            }
            Ok::<(), SftpProductionError>(())
        }
        .await;
        // The caller still owns the opaque upload state and can issue a
        // cleanup. A failed partial write never advances `received_bytes`.
        write_result?;
        upload.received_bytes = next;
        Ok(PluginSftpUploadProgress {
            received_bytes: next,
            expected_bytes: upload.expected_bytes,
        })
    }

    /// Commits an exact-length staged file. The SFTP v3 protocol does not offer
    /// a generic conditional rename; Core therefore revalidates the complete
    /// target fact immediately before the atomic POSIX replacement and reports
    /// a conflict whenever the observed fact differs.
    pub(crate) async fn plugin_commit_upload(
        &self,
        access: &PluginSftpAccess,
        upload: &PluginSftpUpload,
    ) -> Result<(), SftpProductionError> {
        Self::plugin_require_upload_access(access, upload)?;
        if upload.received_bytes != upload.expected_bytes {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let session_key = access.session_id.to_string();
        let generation = SftpGeneration::new(access.generation.get())?;
        let temporary_path = remote_path_utf8(&upload.temporary_path)?;
        let target_path = remote_path_utf8(&upload.target_path)?;
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(&session_key)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        self.reconcile_record_liveness(&session_key, record).await;
        let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
        live.require_generation(generation)?;
        let temporary = live
            .bounded_protocol(live.transport.client().symlink_metadata(temporary_path))
            .await?;
        if !temporary.is_regular()
            || temporary.is_symlink()
            || temporary.len() != upload.expected_bytes
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        if let Some(expected) = &upload.target_precondition {
            let target = live
                .bounded_protocol(live.transport.client().symlink_metadata(target_path))
                .await?;
            if !remote_metadata_matches(&target, expected) {
                return Err(SftpRuntimeError::Conflict.into());
            }
            let replaced = live
                .bounded_protocol(
                    live.transport
                        .client()
                        .posix_rename(temporary_path, target_path),
                )
                .await?;
            if !replaced {
                return Err(SftpRuntimeError::UnsafeReplaceUnsupported.into());
            }
        } else {
            if live
                .bounded_protocol(live.transport.client().try_exists(target_path))
                .await?
            {
                return Err(SftpRuntimeError::Conflict.into());
            }
            let linked = live
                .bounded_protocol(
                    live.transport
                        .client()
                        .hardlink(temporary_path, target_path),
                )
                .await?;
            if !linked {
                return Err(SftpRuntimeError::UnsafeReplaceUnsupported.into());
            }
            live.bounded_protocol(live.transport.client().remove_file(temporary_path))
                .await
                .map_err(|_| SftpRuntimeError::CleanupIncomplete)?;
        }
        let final_metadata = live
            .bounded_protocol(live.transport.client().symlink_metadata(target_path))
            .await?;
        if !final_metadata.is_regular()
            || final_metadata.is_symlink()
            || final_metadata.len() != upload.expected_bytes
        {
            return Err(SftpRuntimeError::LengthMismatch.into());
        }
        Ok(())
    }

    /// Removes a staging target and makes residual cleanup visible to the root
    /// resource close path rather than silently dropping it.
    pub(crate) async fn plugin_abort_upload(
        &self,
        access: &PluginSftpAccess,
        upload: &PluginSftpUpload,
    ) -> Result<(), SftpProductionError> {
        Self::plugin_require_upload_access(access, upload)?;
        let session_key = access.session_id.to_string();
        let generation = SftpGeneration::new(access.generation.get())?;
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(&session_key)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        self.reconcile_record_liveness(&session_key, record).await;
        let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
        live.require_generation(generation)?;
        match live
            .cleanup_temporary_target(generation, &upload.temporary_path)
            .await?
        {
            CleanupOutcome::Cleaned => Ok(()),
            CleanupOutcome::Residual { .. } => Err(SftpRuntimeError::CleanupIncomplete.into()),
        }
    }

    pub(crate) async fn plugin_create_directory(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        name: &str,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<(), SftpProductionError> {
        let path = self.plugin_child_path(access, directory_ref, name).await?;
        self.plugin_mutate(
            access,
            operation_id,
            idempotency_key,
            wire::SftpFileMutation::CreateDirectory {
                path: wire::SftpRemotePath {
                    bytes: path.as_bytes().to_vec(),
                },
            },
        )
        .await
    }

    pub(crate) async fn plugin_create_empty_file(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        name: &str,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<(), SftpProductionError> {
        let path = self.plugin_child_path(access, directory_ref, name).await?;
        self.plugin_mutate(
            access,
            operation_id,
            idempotency_key,
            wire::SftpFileMutation::CreateEmptyFile {
                path: wire::SftpRemotePath {
                    bytes: path.as_bytes().to_vec(),
                },
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn plugin_rename_no_replace(
        &self,
        access: &PluginSftpAccess,
        source_directory_ref: &str,
        source_entry_ref: &str,
        expected: &wire::SftpRemoteObjectPrecondition,
        target_directory_ref: &str,
        target_name: &str,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<(), SftpProductionError> {
        let source = self
            .plugin_entry(access, source_directory_ref, source_entry_ref)
            .await?;
        let actual = plugin_wire_precondition(&source.precondition);
        if source.precondition.kind != RemoteEntryKind::File || actual != *expected {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let target = self
            .plugin_child_path(access, target_directory_ref, target_name)
            .await?;
        self.plugin_mutate(
            access,
            operation_id,
            idempotency_key,
            wire::SftpFileMutation::RenameNoReplace {
                source: wire::SftpRemotePath {
                    bytes: source.path.as_bytes().to_vec(),
                },
                target: wire::SftpRemotePath {
                    bytes: target.as_bytes().to_vec(),
                },
                source_precondition: actual,
            },
        )
        .await
    }

    pub(crate) async fn plugin_remove(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        entry_ref: &str,
        expected: &wire::SftpRemoteObjectPrecondition,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<(), SftpProductionError> {
        let entry = self.plugin_entry(access, directory_ref, entry_ref).await?;
        let actual = plugin_wire_precondition(&entry.precondition);
        if !matches!(
            entry.precondition.kind,
            RemoteEntryKind::File | RemoteEntryKind::Directory
        ) || actual != *expected
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        self.plugin_mutate(
            access,
            operation_id,
            idempotency_key,
            wire::SftpFileMutation::Delete {
                path: wire::SftpRemotePath {
                    bytes: entry.path.as_bytes().to_vec(),
                },
                precondition: actual,
                // A scope is created only by the protected root-rights decision. The guest
                // cannot fabricate this confirmation flag or target a raw remote path.
                irreversible_confirmed: true,
            },
        )
        .await
    }

    pub(crate) async fn plugin_disconnect(
        &self,
        access: &PluginSftpAccess,
    ) -> Result<(), SftpProductionError> {
        let summary = self
            .disconnect(wire::SftpSessionDisconnectRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                operation_id: wire::OperationId::new(),
                idempotency_key: uuid::Uuid::now_v7().to_string(),
                session_id: access.session_id.clone(),
                expected_generation: access.generation,
            })
            .await?;
        if summary.state == wire::SftpSessionState::Closed {
            Ok(())
        } else {
            Err(SftpRuntimeError::CleanupIncomplete.into())
        }
    }

    async fn plugin_list_path(
        &self,
        access: &PluginSftpAccess,
        path: RemotePath,
        cursor: Option<Vec<u8>>,
        page_size: u16,
        operation_id: wire::OperationId,
        idempotency_key: String,
    ) -> Result<wire::SftpDirectoryListing, SftpProductionError> {
        self.plugin_require_directory(access, &path).await?;
        let listing = self
            .list_directory(wire::SftpDirectoryListRequest {
                meta: wire::RequestMeta {
                    request_id: wire::RequestId::new(),
                },
                operation_id,
                idempotency_key,
                session_id: access.session_id.clone(),
                expected_generation: access.generation,
                path: wire::SftpRemotePath {
                    bytes: path.as_bytes().to_vec(),
                },
                cursor,
                page_size,
            })
            .await?;
        if listing.session_id != access.session_id || listing.generation != access.generation {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let listing_path = RemotePath::parse(listing.path.bytes.clone())?;
        if !plugin_path_is_within_root(&access.root_path, &listing_path)
            || !listing.entries.iter().all(|entry| {
                RemotePath::parse(entry.path.bytes.clone())
                    .is_ok_and(|path| plugin_path_is_within_root(&access.root_path, &path))
            })
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(listing)
    }

    async fn plugin_directory_path(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
    ) -> Result<RemotePath, SftpProductionError> {
        if directory_ref.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let directory = self
            .remote_directory_refs
            .lock()
            .await
            .get(directory_ref)
            .cloned()
            .ok_or(SftpRuntimeError::Conflict)?;
        if directory.session_id != access.session_id.as_str()
            || directory.generation != access.generation.get()
            || directory.expires_at <= std::time::Instant::now()
            || !plugin_path_is_within_root(&access.root_path, &directory.path)
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(directory.path)
    }

    async fn plugin_entry(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        entry_ref: &str,
    ) -> Result<RemoteEntryReference, SftpProductionError> {
        if directory_ref.trim().is_empty() || entry_ref.trim().is_empty() {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let entry = self
            .remote_entry_refs
            .lock()
            .await
            .get(entry_ref)
            .cloned()
            .ok_or(SftpRuntimeError::Conflict)?;
        let parent = self.plugin_directory_path(access, directory_ref).await?;
        if entry.directory_ref != directory_ref
            || entry.session_id != access.session_id.as_str()
            || entry.generation != access.generation.get()
            || entry.expires_at <= std::time::Instant::now()
            || !plugin_path_is_within_root(&access.root_path, &entry.path)
            || !plugin_path_is_direct_child(&parent, &entry.path)
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(entry)
    }

    async fn plugin_child_path(
        &self,
        access: &PluginSftpAccess,
        directory_ref: &str,
        name: &str,
    ) -> Result<RemotePath, SftpProductionError> {
        validate_plugin_leaf_name(name)?;
        let directory = self.plugin_directory_path(access, directory_ref).await?;
        self.plugin_require_directory(access, &directory).await?;
        let path = remote_child_path(&directory, name.as_bytes())?;
        if !plugin_path_is_within_root(&access.root_path, &path) {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(path)
    }

    async fn plugin_begin_upload_path(
        &self,
        access: &PluginSftpAccess,
        target_path: RemotePath,
        target_precondition: Option<RemoteObjectPrecondition>,
        expected_bytes: u64,
        operation_id: wire::OperationId,
    ) -> Result<PluginSftpUpload, SftpProductionError> {
        if expected_bytes > wire::PLUGIN_SFTP_MAX_UPLOAD_BYTES
            || !plugin_path_is_within_root(&access.root_path, &target_path)
        {
            return Err(SftpRuntimeError::InvalidInput.into());
        }
        let temporary_path = remote_edit_temporary_path(&target_path, operation_id.as_str())?;
        if !plugin_path_is_within_root(&access.root_path, &temporary_path)
            || target_path.parent() != temporary_path.parent()
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let session_key = access.session_id.to_string();
        let generation = SftpGeneration::new(access.generation.get())?;
        let target = remote_path_utf8(&target_path)?;
        let temporary = remote_path_utf8(&temporary_path)?;
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(&session_key)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        self.reconcile_record_liveness(&session_key, record).await;
        let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
        live.require_generation(generation)?;
        if live
            .bounded_protocol(live.transport.client().try_exists(temporary))
            .await?
        {
            return Err(SftpRuntimeError::Conflict.into());
        }
        match &target_precondition {
            Some(expected) => {
                let target_metadata = live
                    .bounded_protocol(live.transport.client().symlink_metadata(target))
                    .await?;
                if !remote_metadata_matches(&target_metadata, expected) {
                    return Err(SftpRuntimeError::Conflict.into());
                }
            }
            None => {
                if live
                    .bounded_protocol(live.transport.client().try_exists(target))
                    .await?
                {
                    return Err(SftpRuntimeError::Conflict.into());
                }
            }
        }
        let file = live
            .bounded_protocol(live.transport.client().open_with_flags(
                temporary,
                OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            ))
            .await?;
        if let Err(error) = live.bounded_protocol(file.close()).await {
            let cleanup = live
                .cleanup_temporary_target(generation, &temporary_path)
                .await;
            return match cleanup {
                Ok(CleanupOutcome::Cleaned) => Err(error),
                Ok(CleanupOutcome::Residual { .. }) | Err(_) => {
                    Err(SftpRuntimeError::CleanupIncomplete.into())
                }
            };
        }
        Ok(PluginSftpUpload {
            session_id: access.session_id.clone(),
            generation: access.generation,
            target_path,
            temporary_path,
            target_precondition,
            expected_bytes,
            received_bytes: 0,
        })
    }

    fn plugin_require_upload_access(
        access: &PluginSftpAccess,
        upload: &PluginSftpUpload,
    ) -> Result<(), SftpProductionError> {
        if upload.session_id != access.session_id
            || upload.generation != access.generation
            || !plugin_path_is_within_root(&access.root_path, &upload.target_path)
            || !plugin_path_is_within_root(&access.root_path, &upload.temporary_path)
            || upload.target_path.parent() != upload.temporary_path.parent()
            || upload.received_bytes > upload.expected_bytes
        {
            Err(SftpRuntimeError::Conflict.into())
        } else {
            Ok(())
        }
    }

    async fn plugin_abort_after_error<T>(
        &self,
        access: &PluginSftpAccess,
        upload: &PluginSftpUpload,
        error: SftpProductionError,
    ) -> Result<T, SftpProductionError> {
        match self.plugin_abort_upload(access, upload).await {
            Ok(()) => Err(error),
            Err(_) => Err(SftpRuntimeError::CleanupIncomplete.into()),
        }
    }

    async fn plugin_mutate(
        &self,
        access: &PluginSftpAccess,
        operation_id: wire::OperationId,
        idempotency_key: String,
        mutation: wire::SftpFileMutation,
    ) -> Result<(), SftpProductionError> {
        self.mutate_file(wire::SftpFileMutationRequest {
            meta: wire::RequestMeta {
                request_id: wire::RequestId::new(),
            },
            operation_id,
            idempotency_key,
            session_id: access.session_id.clone(),
            expected_generation: access.generation,
            mutation,
        })
        .await
        .map(|_| ())
    }

    async fn plugin_require_directory(
        &self,
        access: &PluginSftpAccess,
        path: &RemotePath,
    ) -> Result<(), SftpProductionError> {
        if !plugin_path_is_within_root(&access.root_path, path) {
            return Err(SftpRuntimeError::Conflict.into());
        }
        let session_key = access.session_id.to_string();
        let generation = SftpGeneration::new(access.generation.get())?;
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(&session_key)
            .ok_or(SftpRuntimeError::InvalidInput)?;
        self.reconcile_record_liveness(&session_key, record).await;
        let live = record.live.as_ref().ok_or(SftpRuntimeError::InvalidState)?;
        live.require_generation(generation)?;
        let metadata = live
            .bounded_protocol(
                live.transport
                    .client()
                    .symlink_metadata(remote_path_utf8(path)?),
            )
            .await?;
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(SftpRuntimeError::Conflict.into());
        }
        Ok(())
    }
}

fn normalize_plugin_root_path(value: Vec<u8>) -> SftpRuntimeResult<RemotePath> {
    let path = RemotePath::parse(value)?;
    if path.as_bytes() == b"/" {
        return Ok(path);
    }
    let mut normalized = Vec::with_capacity(path.as_bytes().len());
    normalized.push(b'/');
    let mut first = true;
    for component in path.as_bytes().split(|byte| *byte == b'/').skip(1) {
        if component.is_empty() {
            continue;
        }
        if component == b"." || component == b".." || component.contains(&b'\\') {
            return Err(SftpRuntimeError::InvalidInput);
        }
        if !first {
            normalized.push(b'/');
        }
        normalized.extend_from_slice(component);
        first = false;
    }
    if first {
        return Err(SftpRuntimeError::InvalidInput);
    }
    RemotePath::parse(normalized)
}

fn validate_plugin_leaf_name(name: &str) -> SftpRuntimeResult<()> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        Err(SftpRuntimeError::InvalidInput)
    } else {
        Ok(())
    }
}

fn plugin_path_is_within_root(root: &RemotePath, candidate: &RemotePath) -> bool {
    root.as_bytes() == b"/"
        || candidate.as_bytes() == root.as_bytes()
        || candidate
            .as_bytes()
            .strip_prefix(root.as_bytes())
            .is_some_and(|suffix| suffix.starts_with(b"/"))
}

fn plugin_path_is_direct_child(parent: &RemotePath, candidate: &RemotePath) -> bool {
    let suffix = if parent.as_bytes() == b"/" {
        candidate.as_bytes().strip_prefix(b"/")
    } else {
        candidate
            .as_bytes()
            .strip_prefix(parent.as_bytes())
            .and_then(|suffix| suffix.strip_prefix(b"/"))
    };
    suffix.is_some_and(|name| validate_name_component(name).is_ok())
}

fn plugin_wire_precondition(
    value: &RemoteObjectPrecondition,
) -> wire::SftpRemoteObjectPrecondition {
    wire::SftpRemoteObjectPrecondition {
        kind: match value.kind {
            RemoteEntryKind::File => wire::SftpRemoteEntryKind::File,
            RemoteEntryKind::Directory => wire::SftpRemoteEntryKind::Directory,
            RemoteEntryKind::Symlink => wire::SftpRemoteEntryKind::Symlink,
            RemoteEntryKind::Other => wire::SftpRemoteEntryKind::Other,
        },
        size: value.size,
        modified_at_unix_ms: value.modified_at_unix_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_normalization_rejects_parent_components_and_backslashes() {
        assert_eq!(
            normalize_plugin_root_path(b"/srv//plugin/".to_vec())
                .unwrap()
                .as_bytes(),
            b"/srv/plugin"
        );
        for path in [b"relative".as_slice(), b"/srv/../secret", b"/srv\\secret"] {
            assert!(normalize_plugin_root_path(path.to_vec()).is_err());
        }
    }

    #[test]
    fn scoped_path_check_does_not_accept_prefix_lookalikes() {
        let root = normalize_plugin_root_path(b"/srv/plugin".to_vec()).unwrap();
        assert!(plugin_path_is_within_root(
            &root,
            &normalize_plugin_root_path(b"/srv/plugin/a".to_vec()).unwrap()
        ));
        assert!(!plugin_path_is_within_root(
            &root,
            &normalize_plugin_root_path(b"/srv/plugin-other".to_vec()).unwrap()
        ));
    }

    #[test]
    fn leaf_names_cannot_escape_the_trusted_directory() {
        for name in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(validate_plugin_leaf_name(name).is_err());
        }
        assert!(validate_plugin_leaf_name("normal.txt").is_ok());
    }

    #[test]
    fn upload_state_is_bound_to_the_exact_sftp_generation_and_root() {
        let access = PluginSftpAccess::fixture(b"/approved".to_vec());
        let mut upload = PluginSftpUpload {
            session_id: access.session_id.clone(),
            generation: access.generation,
            target_path: RemotePath::parse(b"/approved/file.bin".to_vec()).unwrap(),
            temporary_path: RemotePath::parse(
                b"/approved/.file.bin.norishell-edit-test.part".to_vec(),
            )
            .unwrap(),
            target_precondition: None,
            expected_bytes: 8,
            received_bytes: 0,
        };
        assert!(SftpSessionService::plugin_require_upload_access(&access, &upload).is_ok());
        upload.generation = wire::WireSequence::new(access.generation.get().saturating_add(1));
        assert!(matches!(
            SftpSessionService::plugin_require_upload_access(&access, &upload),
            Err(SftpProductionError::Runtime(SftpRuntimeError::Conflict))
        ));
        upload.generation = access.generation;
        upload.temporary_path = RemotePath::parse(b"/other/.file.bin.part".to_vec()).unwrap();
        assert!(matches!(
            SftpSessionService::plugin_require_upload_access(&access, &upload),
            Err(SftpProductionError::Runtime(SftpRuntimeError::Conflict))
        ));
    }

    /// This is deliberately an external, isolated sshd acceptance test. It
    /// exercises the real SFTP v3 server rather than a directory-reference
    /// fixture, while keeping the saved Host/Vault admission path out of the
    /// transport test. The state file contains only ephemeral fixture paths,
    /// port, user, and public host-key material created by the QA harness.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "requires NORISHELL_PLUGIN_SFTP_QA_STATE isolated sshd fixture"]
    async fn real_plugin_sftp_binary_upload_cas_abort_and_cleanup() {
        use std::{collections::VecDeque, sync::Arc};

        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
        use norishell_ssh_transport::{Authentication, ConnectRequest, VerifiedTransport};

        let state_path = std::env::var("NORISHELL_PLUGIN_SFTP_QA_STATE")
            .expect("NORISHELL_PLUGIN_SFTP_QA_STATE");
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state_path).expect("fixture state")).unwrap();
        let port = state["port"].as_u64().expect("fixture port") as u16;
        let username = state["username"].as_str().expect("fixture user");
        let root = std::path::PathBuf::from(state["root"].as_str().expect("fixture root"));
        let private_key = std::fs::read(state["privateKey"].as_str().expect("fixture key"))
            .expect("fixture private key");
        let host_key_algorithm = state["hostKeyAlgorithm"]
            .as_str()
            .expect("fixture host key algorithm");
        let host_key_blob = BASE64
            .decode(
                state["hostKeyBase64"]
                    .as_str()
                    .expect("fixture host key blob"),
            )
            .expect("fixture host key blob base64");

        std::fs::write(root.join("read.bin"), b"before").expect("seed readable file");
        std::fs::write(root.join("replace.bin"), b"before").expect("seed replace file");
        let application = tempfile::tempdir().expect("temporary application state");
        let hosts = HostService::start(application.path()).expect("host service");
        hosts
            .trust_known_host("127.0.0.1", port, host_key_algorithm, &host_key_blob)
            .expect("trust isolated sshd key");
        let service = SftpSessionService::production(
            hosts.clone(),
            VaultService::start(application.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let verified = VerifiedTransport::connect(
            ConnectRequest::new("127.0.0.1", port).expect("fixture endpoint"),
            Arc::new(TrustedSftpHostKeyVerifier {
                hosts: hosts.clone(),
                unknown_capture: None,
            }),
        )
        .await
        .expect("verify isolated SFTP host key");
        let transport = verified
            .authenticate(username, Authentication::private_key(private_key, None))
            .await
            .expect("authenticate isolated SFTP fixture");
        // `open_sftp` is the independent subsystem path; this test never
        // requests a terminal PTY, Shell, exec, or parent SSH lease.
        let subsystem = transport.open_sftp().await.expect("open SFTP subsystem");
        let session_id = wire::SftpSessionId::new();
        let host_id = wire::HostId::new();
        let mut actor = SftpSessionActor::new(
            SftpSessionId::parse(session_id.to_string()).expect("session id"),
            host_id.to_string(),
        )
        .expect("SFTP actor");
        let generation = actor.start("plugin-fixture-revision").expect("generation");
        actor
            .authenticated_transport_ready(generation, "plugin-fixture-transport".to_owned())
            .expect("authenticated state");
        actor
            .subsystem_opened(generation, "plugin-fixture-subsystem".to_owned())
            .expect("subsystem state");
        service.records.lock().await.insert(
            session_id.to_string(),
            SftpServiceRecord {
                actor,
                live: Some(SftpProductionSession {
                    generation,
                    transport: SftpLiveTransport::Dedicated(subsystem),
                    transport_heartbeats: Vec::new(),
                    transport_keepalive: None,
                }),
                shared_cleanup_incomplete: false,
                shared_parent_channels: None,
                heartbeat_tasks: None,
                pending_transfers: VecDeque::new(),
                active_transfer_id: None,
                open_operation_id: "plugin-fixture-open".to_owned(),
                open_idempotency_key: "plugin-fixture-key".to_owned(),
                open_fingerprint: b"plugin-fixture-open".to_vec(),
            },
        );
        let access = service
            .plugin_access(
                session_id.clone(),
                wire::WireSequence::new(generation.get()),
                root.as_os_str().as_encoded_bytes().to_vec(),
            )
            .await
            .expect("bind approved root");

        let listing = service
            .plugin_list_root(
                &access,
                None,
                100,
                wire::OperationId::new(),
                "fixture-list-read".to_owned(),
            )
            .await
            .expect("list approved root");
        let read_entry = listing
            .entries
            .iter()
            .find(|entry| entry.display_name == "read.bin")
            .expect("read entry");
        let read = service
            .plugin_read_binary(
                &access,
                &listing.directory_ref,
                &read_entry.entry_ref,
                0,
                16 * 1024,
            )
            .await
            .expect("bounded remote read");
        assert_eq!(read.bytes, b"before");

        service
            .plugin_write_binary(
                &access,
                &listing.directory_ref,
                &read_entry.entry_ref,
                &plugin_wire_precondition(&RemoteObjectPrecondition {
                    kind: RemoteEntryKind::File,
                    size: read_entry.size,
                    modified_at_unix_ms: read_entry.modified_at_unix_ms,
                }),
                b"binary\0write",
                wire::OperationId::new(),
            )
            .await
            .expect("binary atomic replacement");
        assert_eq!(
            std::fs::read(root.join("read.bin")).unwrap(),
            b"binary\0write"
        );

        let mut upload = service
            .plugin_begin_upload_create(
                &access,
                &listing.directory_ref,
                "upload.bin",
                13,
                wire::OperationId::new(),
            )
            .await
            .expect("start upload");
        service
            .plugin_append_upload(&access, &mut upload, 0, b"chunk-")
            .await
            .expect("first upload chunk");
        service
            .plugin_append_upload(&access, &mut upload, 6, b"payload")
            .await
            .expect("second upload chunk");
        service
            .plugin_commit_upload(&access, &upload)
            .await
            .expect("commit no-replace upload");
        assert_eq!(
            std::fs::read(root.join("upload.bin")).unwrap(),
            b"chunk-payload"
        );

        let replacement_listing = service
            .plugin_list_root(
                &access,
                None,
                100,
                wire::OperationId::new(),
                "fixture-list-replace".to_owned(),
            )
            .await
            .expect("list replacement entry");
        let replacement = replacement_listing
            .entries
            .iter()
            .find(|entry| entry.display_name == "replace.bin")
            .expect("replace entry");
        let expected = plugin_wire_precondition(&RemoteObjectPrecondition {
            kind: RemoteEntryKind::File,
            size: replacement.size,
            modified_at_unix_ms: replacement.modified_at_unix_ms,
        });
        let mut stale = service
            .plugin_begin_upload_replace(
                &access,
                &replacement_listing.directory_ref,
                &replacement.entry_ref,
                &expected,
                5,
                wire::OperationId::new(),
            )
            .await
            .expect("stage replacement");
        service
            .plugin_append_upload(&access, &mut stale, 0, b"after")
            .await
            .expect("stage replacement bytes");
        std::fs::write(root.join("replace.bin"), b"outside-change").expect("external conflict");
        assert!(matches!(
            service.plugin_commit_upload(&access, &stale).await,
            Err(SftpProductionError::Runtime(SftpRuntimeError::Conflict))
        ));
        service
            .plugin_abort_upload(&access, &stale)
            .await
            .expect("cleanup conflict staging target");

        let mut aborted = service
            .plugin_begin_upload_create(
                &access,
                &listing.directory_ref,
                "abort.bin",
                5,
                wire::OperationId::new(),
            )
            .await
            .expect("stage abort target");
        service
            .plugin_append_upload(&access, &mut aborted, 0, b"abort")
            .await
            .expect("write abort target");
        service
            .plugin_abort_upload(&access, &aborted)
            .await
            .expect("abort cleanup");
        assert!(!root.join("abort.bin").exists());
        assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".abort.bin.norishell-edit-")
        }));

        service
            .plugin_disconnect(&access)
            .await
            .expect("close independent plugin SFTP transport");
    }
}
