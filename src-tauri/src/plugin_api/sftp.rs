//! Core-owned SFTP capability driver for plugins.
//!
//! Secure-surface code creates `PluginSftpGrant`; this module never resolves a
//! HostHandle, prompts for trust, opens Vault, or accepts authority from a guest.
//! A remote root is only a client-side lexical range. Generic SFTP cannot turn it
//! into a server-side jail or make directory-replacement races atomic.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::{
    HostId, PLUGIN_SFTP_MAX_LIST_ENTRIES, PLUGIN_SFTP_MAX_READ_BYTES, PLUGIN_SFTP_MAX_UPLOAD_BYTES,
    PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES, PluginApiErrorCode, PluginApiResourceEventKind,
    PluginSftpDirectoryEntry, PluginSftpEntryKind, PluginSftpEvent, PluginSftpObjectPrecondition,
    PluginSftpOperation, PluginSftpResult, RequestId, RequestMeta, SftpDirectoryListing,
    SftpRemoteEntryKind, SftpRemoteObjectPrecondition, SftpSessionDisconnectRequest, SftpSessionId,
    SftpSessionOpenRequest, SftpSessionState, WireSequence,
};
use tokio::sync::{Mutex as AsyncMutex, watch};
use uuid::Uuid;

use crate::sftp_session_service::{
    PluginSftpAccess, PluginSftpBinaryRead, PluginSftpUpload, SftpProductionError,
    SftpRuntimeError, SftpSessionService,
};

use super::{
    ResourceConsumer, ResourceEventWriter, ResourceFence, ResourceOwner, ResourceRegistry,
};

const MAX_DIRECTORIES_PER_ROOT: usize = 256;
const MAX_ENTRIES_PER_ROOT: usize = 4_096;
const MAX_CURSORS_PER_ROOT: usize = 256;
const MAX_UPLOADS_PER_ROOT: usize = 4;
const SFTP_OPEN_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Rights are constructed only by the protected Host/root scope surface. They
/// intentionally are not serializable and do not appear in the plugin ABI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PluginSftpScopeFlags {
    pub(crate) allow_list: bool,
    pub(crate) allow_read: bool,
    pub(crate) allow_create_directory: bool,
    pub(crate) allow_create_empty_file: bool,
    pub(crate) allow_binary_write: bool,
    pub(crate) allow_upload: bool,
    pub(crate) allow_rename_no_replace: bool,
    pub(crate) allow_remove: bool,
}

impl PluginSftpScopeFlags {
    /// The protected service chooses this once for the exact Host/root approval.
    /// Read-only grants never acquire a writable local or remote transfer path.
    pub(crate) fn for_access(write: bool) -> Self {
        Self {
            allow_list: true,
            allow_read: true,
            allow_create_directory: write,
            allow_create_empty_file: write,
            allow_binary_write: write,
            allow_upload: write,
            allow_rename_no_replace: write,
            allow_remove: write,
        }
    }
}

/// A one-use Core-only grant. A plugin cannot deserialize, clone, or manufacture
/// it. The secure surface has already matched owner, saved Host, root, rights,
/// policy epoch, and its admission/resource fences before calling `open`.
pub(crate) struct PluginSftpGrant {
    owner: ResourceOwner,
    host_id: HostId,
    host_revision: WireSequence,
    expected_connection_revision: String,
    root_path: Vec<u8>,
    scope: PluginSftpScopeFlags,
    /// Current user action + complete resolved connection/profile revision. It
    /// is used only up to the point a new SFTP transport is ready.
    admission_fence: ResourceFence,
    /// Owner/package generation, SFTP capability epoch, and Host/root policy
    /// scope. It deliberately outlives the approval dialog/action.
    resource_fence: ResourceFence,
}

impl PluginSftpGrant {
    // A grant is assembled from the exact approval, resource, and connection identity facts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        owner: ResourceOwner,
        host_id: HostId,
        host_revision: WireSequence,
        expected_connection_revision: String,
        root_path: Vec<u8>,
        scope: PluginSftpScopeFlags,
        admission_fence: ResourceFence,
        resource_fence: ResourceFence,
    ) -> Self {
        Self {
            owner,
            host_id,
            host_revision,
            expected_connection_revision,
            root_path,
            scope,
            admission_fence,
            resource_fence,
        }
    }
}

struct Entry {
    owner: ResourceOwner,
    consumer: Option<ResourceConsumer>,
    scope: PluginSftpScopeFlags,
    access: PluginSftpAccess,
    resource_fence: ResourceFence,
    operation_lock: AsyncMutex<()>,
    projection: Mutex<Projection>,
    uploads: Mutex<BTreeMap<String, UploadRecord>>,
    events: Mutex<Option<ResourceEventWriter>>,
    event_cancel: AsyncMutex<watch::Receiver<bool>>,
    revoked: AtomicBool,
}

#[derive(Default)]
struct Projection {
    directories: BTreeMap<String, DirectoryRecord>,
    entries: BTreeMap<String, EntryRecord>,
    cursors: BTreeMap<String, CursorRecord>,
}

struct DirectoryRecord {
    service_directory_ref: String,
}

struct EntryRecord {
    directory_handle: String,
    service_directory_ref: String,
    service_entry_ref: String,
    kind: PluginSftpEntryKind,
    precondition: PluginSftpObjectPrecondition,
}

struct CursorRecord {
    directory_handle: String,
    service_cursor: Vec<u8>,
}

struct UploadRecord {
    upload: PluginSftpUpload,
    replaced_entry_handle: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PluginSftpDriver {
    sftp: SftpSessionService,
    resources: ResourceRegistry,
    entries: Arc<Mutex<BTreeMap<String, Arc<Entry>>>>,
    consumer: Option<ResourceConsumer>,
}

impl PluginSftpDriver {
    pub(crate) fn new(sftp: SftpSessionService, resources: ResourceRegistry) -> Self {
        Self {
            sftp,
            resources,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            consumer: None,
        }
    }

    /// Preserve the shared SFTP handle ledger while binding all opaque handles and the backing
    /// registry resource to one provider/task consumer.
    pub(crate) fn for_consumer(&self, consumer: ResourceConsumer) -> Self {
        Self {
            sftp: self.sftp.clone(),
            resources: self.resources.for_consumer(consumer.clone()),
            entries: self.entries.clone(),
            consumer: Some(consumer),
        }
    }

    /// Opens one fresh saved-Host SFTP transport and registers it as a Core
    /// resource. It never adopts an SSH terminal channel.
    pub(crate) async fn open(
        &self,
        grant: PluginSftpGrant,
    ) -> Result<PluginSftpResult, PluginApiErrorCode> {
        if grant.expected_connection_revision.is_empty()
            || !(grant.admission_fence)()
            || !(grant.resource_fence)()
        {
            return Err(PluginApiErrorCode::Revoked);
        }
        // The registry slot is reserved before any transport work. A burst of
        // approved prompts therefore cannot create untracked SFTP transports
        // beyond the per-plugin/global resource quotas.
        let owner = grant.owner.clone();
        let (slot, driver_slot) = watch::channel(None::<String>);
        let (ready, received) = tokio::sync::oneshot::channel();
        let entries = self.entries.clone();
        let sftp = self.sftp.clone();
        let consumer = self.consumer.clone();
        let handle =
            match self
                .resources
                .spawn(owner.clone(), "plugin-sftp-root", move |cancel, events| {
                    run_sftp_root(
                        grant,
                        sftp,
                        entries,
                        consumer,
                        cancel,
                        events,
                        driver_slot,
                        ready,
                    )
                }) {
                Ok(handle) => handle,
                Err(error) => return Err(error),
            };
        slot.send_replace(Some(handle.clone()));
        match tokio::time::timeout(SFTP_OPEN_READY_TIMEOUT, received).await {
            Ok(Ok(Ok(()))) => Ok(PluginSftpResult::Opened {
                root_handle: handle,
            }),
            Ok(Ok(Err(code))) => {
                let _ = self.resources.close(&owner, &handle).await;
                Err(code)
            }
            Ok(Err(_)) => {
                let _ = self.resources.close(&owner, &handle).await;
                Err(PluginApiErrorCode::Unavailable)
            }
            Err(_) => {
                let _ = self.resources.close(&owner, &handle).await;
                Err(PluginApiErrorCode::OutcomeUnknown)
            }
        }
    }

    /// Executes only against an already-open, exact owner-scoped root handle.
    /// Opening is deliberately absent from `PluginSftpOperation`: the protected
    /// service resolves Host/root approval into a one-use grant separately.
    pub(crate) async fn invoke(
        &self,
        owner: &ResourceOwner,
        operation: &PluginSftpOperation,
        fence: ResourceFence,
    ) -> Result<PluginSftpResult, PluginApiErrorCode> {
        let root_handle = operation_root_handle(operation);
        let entry = self.entry(owner, root_handle)?;
        ensure_live(&entry, &fence)?;
        match operation {
            PluginSftpOperation::List {
                directory_handle,
                child_entry_handle,
                cursor,
                limit,
                ..
            } => {
                require(entry.scope.allow_list)?;
                if *limit == 0 || *limit > PLUGIN_SFTP_MAX_LIST_ENTRIES {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                let append_page = cursor.is_some();
                let listing = self
                    .list(&entry, directory_handle, child_entry_handle, cursor, *limit)
                    .await?;
                ensure_live(&entry, &fence)?;
                self.project_listing(&entry, listing, append_page)
            }
            PluginSftpOperation::Read {
                directory_handle,
                entry_handle,
                offset,
                length,
                ..
            } => {
                require(entry.scope.allow_read)?;
                if *length == 0 || *length > PLUGIN_SFTP_MAX_READ_BYTES {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                let record = self.entry_record(&entry, directory_handle, entry_handle)?;
                if record.kind != PluginSftpEntryKind::File {
                    return Err(PluginApiErrorCode::PermissionDenied);
                }
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                let read: PluginSftpBinaryRead = self
                    .sftp
                    .plugin_read_binary(
                        &entry.access,
                        &record.service_directory_ref,
                        &record.service_entry_ref,
                        *offset,
                        usize::from(*length),
                    )
                    .await
                    .map_err(map_sftp_error)?;
                ensure_live(&entry, &fence)?;
                emit_sftp_event(
                    &entry,
                    PluginSftpEvent::DownloadProgress {
                        entry_handle: entry_handle.clone(),
                        next_offset: read.next_offset,
                        total_bytes: read.total_bytes,
                        eof: read.eof,
                    },
                    &fence,
                )
                .await?;
                Ok(PluginSftpResult::Read {
                    data_base64: BASE64.encode(read.bytes),
                    offset: *offset,
                    next_offset: read.next_offset,
                    total_bytes: read.total_bytes,
                    eof: read.eof,
                    precondition: record.precondition,
                })
            }
            PluginSftpOperation::WriteBinary {
                directory_handle,
                entry_handle,
                precondition,
                data_base64,
                ..
            } => {
                require(entry.scope.allow_binary_write)?;
                let record = self.entry_record(&entry, directory_handle, entry_handle)?;
                if record.kind != PluginSftpEntryKind::File || record.precondition != *precondition
                {
                    return Err(PluginApiErrorCode::Conflict);
                }
                let bytes = decode_binary_chunk(data_base64)?;
                let byte_count =
                    u64::try_from(bytes.len()).map_err(|_| PluginApiErrorCode::InvalidRequest)?;
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                self.sftp
                    .plugin_write_binary(
                        &entry.access,
                        &record.service_directory_ref,
                        &record.service_entry_ref,
                        &to_sftp_precondition(precondition),
                        &bytes,
                        norishell_core_api::OperationId::new(),
                    )
                    .await
                    .map_err(map_sftp_commit_error)?;
                self.forget_entry(&entry, entry_handle);
                ensure_live(&entry, &fence)?;
                Ok(PluginSftpResult::Written {
                    total_bytes: byte_count,
                })
            }
            PluginSftpOperation::UploadStart {
                directory_handle,
                name,
                expected_bytes,
                ..
            } => {
                require(entry.scope.allow_upload)?;
                if *expected_bytes > PLUGIN_SFTP_MAX_UPLOAD_BYTES {
                    return Err(PluginApiErrorCode::QuotaExceeded);
                }
                let directory = self.directory_record(&entry, directory_handle)?;
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                if self.upload_count(&entry) >= MAX_UPLOADS_PER_ROOT {
                    return Err(PluginApiErrorCode::QuotaExceeded);
                }
                let upload = self
                    .sftp
                    .plugin_begin_upload_create(
                        &entry.access,
                        &directory.service_directory_ref,
                        name,
                        *expected_bytes,
                        norishell_core_api::OperationId::new(),
                    )
                    .await
                    .map_err(map_sftp_error)?;
                self.store_started_upload(&entry, upload, None, *expected_bytes, &fence)
                    .await
            }
            PluginSftpOperation::UploadReplaceStart {
                directory_handle,
                entry_handle,
                precondition,
                expected_bytes,
                ..
            } => {
                require(entry.scope.allow_upload)?;
                if *expected_bytes > PLUGIN_SFTP_MAX_UPLOAD_BYTES {
                    return Err(PluginApiErrorCode::QuotaExceeded);
                }
                let record = self.entry_record(&entry, directory_handle, entry_handle)?;
                if record.kind != PluginSftpEntryKind::File || record.precondition != *precondition
                {
                    return Err(PluginApiErrorCode::Conflict);
                }
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                if self.upload_count(&entry) >= MAX_UPLOADS_PER_ROOT {
                    return Err(PluginApiErrorCode::QuotaExceeded);
                }
                let upload = self
                    .sftp
                    .plugin_begin_upload_replace(
                        &entry.access,
                        &record.service_directory_ref,
                        &record.service_entry_ref,
                        &to_sftp_precondition(precondition),
                        *expected_bytes,
                        norishell_core_api::OperationId::new(),
                    )
                    .await
                    .map_err(map_sftp_error)?;
                self.store_started_upload(
                    &entry,
                    upload,
                    Some(entry_handle.clone()),
                    *expected_bytes,
                    &fence,
                )
                .await
            }
            PluginSftpOperation::UploadChunk {
                upload_handle,
                offset,
                data_base64,
                ..
            } => {
                require(entry.scope.allow_upload)?;
                let bytes = decode_binary_chunk(data_base64)?;
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                let mut record = self.take_upload(&entry, upload_handle)?;
                let progress = match self
                    .sftp
                    .plugin_append_upload(&entry.access, &mut record.upload, *offset, &bytes)
                    .await
                {
                    Ok(progress) => progress,
                    Err(error) => {
                        self.put_upload(&entry, upload_handle.clone(), record);
                        return Err(map_sftp_error(error));
                    }
                };
                self.put_upload(&entry, upload_handle.clone(), record);
                ensure_live(&entry, &fence)?;
                if emit_sftp_event(
                    &entry,
                    PluginSftpEvent::UploadProgress {
                        upload_handle: upload_handle.clone(),
                        received_bytes: progress.received_bytes,
                        expected_bytes: progress.expected_bytes,
                    },
                    &fence,
                )
                .await
                .is_err()
                {
                    // The bounded remote append has completed but the caller no
                    // longer has a reliable completion boundary.
                    return Err(PluginApiErrorCode::OutcomeUnknown);
                }
                Ok(PluginSftpResult::UploadProgress {
                    upload_handle: upload_handle.clone(),
                    received_bytes: progress.received_bytes,
                    expected_bytes: progress.expected_bytes,
                })
            }
            PluginSftpOperation::UploadCommit { upload_handle, .. } => {
                require(entry.scope.allow_upload)?;
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                let record = self.take_upload(&entry, upload_handle)?;
                if let Err(error) = self
                    .sftp
                    .plugin_commit_upload(&entry.access, &record.upload)
                    .await
                {
                    let cleanup = self
                        .sftp
                        .plugin_abort_upload(&entry.access, &record.upload)
                        .await;
                    if cleanup.is_err() {
                        self.put_upload(&entry, upload_handle.clone(), record);
                        return Err(PluginApiErrorCode::CleanupIncomplete);
                    }
                    return Err(map_sftp_commit_error(error));
                }
                let total_bytes = record.upload.expected_bytes();
                if let Some(replaced) = record.replaced_entry_handle {
                    self.forget_entry(&entry, &replaced);
                }
                ensure_live(&entry, &fence)?;
                if emit_sftp_event(
                    &entry,
                    PluginSftpEvent::UploadCompleted {
                        upload_handle: upload_handle.clone(),
                        total_bytes,
                    },
                    &fence,
                )
                .await
                .is_err()
                {
                    return Err(PluginApiErrorCode::OutcomeUnknown);
                }
                Ok(PluginSftpResult::Uploaded {
                    upload_handle: upload_handle.clone(),
                    total_bytes,
                })
            }
            PluginSftpOperation::UploadAbort { upload_handle, .. } => {
                require(entry.scope.allow_upload)?;
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                let record = self.take_upload(&entry, upload_handle)?;
                if let Err(error) = self
                    .sftp
                    .plugin_abort_upload(&entry.access, &record.upload)
                    .await
                {
                    self.put_upload(&entry, upload_handle.clone(), record);
                    return Err(map_sftp_error(error));
                }
                ensure_live(&entry, &fence)?;
                Ok(PluginSftpResult::UploadAborted {})
            }
            PluginSftpOperation::CreateDirectory {
                directory_handle,
                name,
                ..
            } => {
                require(entry.scope.allow_create_directory)?;
                let directory = self.directory_record(&entry, directory_handle)?;
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                self.sftp
                    .plugin_create_directory(
                        &entry.access,
                        &directory.service_directory_ref,
                        name,
                        norishell_core_api::OperationId::new(),
                        Uuid::now_v7().to_string(),
                    )
                    .await
                    .map_err(map_sftp_error)?;
                ensure_live(&entry, &fence)?;
                Ok(PluginSftpResult::Created {})
            }
            PluginSftpOperation::CreateEmptyFile {
                directory_handle,
                name,
                ..
            } => {
                require(entry.scope.allow_create_empty_file)?;
                let directory = self.directory_record(&entry, directory_handle)?;
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                self.sftp
                    .plugin_create_empty_file(
                        &entry.access,
                        &directory.service_directory_ref,
                        name,
                        norishell_core_api::OperationId::new(),
                        Uuid::now_v7().to_string(),
                    )
                    .await
                    .map_err(map_sftp_error)?;
                ensure_live(&entry, &fence)?;
                Ok(PluginSftpResult::Created {})
            }
            PluginSftpOperation::RenameNoReplace {
                source_directory_handle,
                source_entry_handle,
                source_precondition,
                target_directory_handle,
                target_name,
                ..
            } => {
                require(entry.scope.allow_rename_no_replace)?;
                let source =
                    self.entry_record(&entry, source_directory_handle, source_entry_handle)?;
                let target = self.directory_record(&entry, target_directory_handle)?;
                if source.kind != PluginSftpEntryKind::File
                    || source.precondition != *source_precondition
                {
                    return Err(PluginApiErrorCode::Conflict);
                }
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                self.sftp
                    .plugin_rename_no_replace(
                        &entry.access,
                        &source.service_directory_ref,
                        &source.service_entry_ref,
                        &to_sftp_precondition(source_precondition),
                        &target.service_directory_ref,
                        target_name,
                        norishell_core_api::OperationId::new(),
                        Uuid::now_v7().to_string(),
                    )
                    .await
                    .map_err(map_sftp_error)?;
                self.forget_entry(&entry, source_entry_handle);
                ensure_live(&entry, &fence)?;
                Ok(PluginSftpResult::Renamed {})
            }
            PluginSftpOperation::Remove {
                directory_handle,
                entry_handle,
                precondition,
                ..
            } => {
                require(entry.scope.allow_remove)?;
                let record = self.entry_record(&entry, directory_handle, entry_handle)?;
                if record.precondition != *precondition {
                    return Err(PluginApiErrorCode::Conflict);
                }
                let _operation = entry.operation_lock.lock().await;
                ensure_live(&entry, &fence)?;
                self.sftp
                    .plugin_remove(
                        &entry.access,
                        &record.service_directory_ref,
                        &record.service_entry_ref,
                        &to_sftp_precondition(precondition),
                        norishell_core_api::OperationId::new(),
                        Uuid::now_v7().to_string(),
                    )
                    .await
                    .map_err(map_sftp_error)?;
                self.forget_entry(&entry, entry_handle);
                ensure_live(&entry, &fence)?;
                Ok(PluginSftpResult::Removed {})
            }
        }
    }

    async fn list(
        &self,
        entry: &Entry,
        directory_handle: &Option<String>,
        child_entry_handle: &Option<String>,
        cursor: &Option<String>,
        limit: u16,
    ) -> Result<SftpDirectoryListing, PluginApiErrorCode> {
        if let Some(cursor) = cursor {
            if child_entry_handle.is_some() {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            let directory_handle = directory_handle
                .as_deref()
                .ok_or(PluginApiErrorCode::InvalidRequest)?;
            let cursor = self.cursor_record(entry, directory_handle, cursor)?;
            let directory = self.directory_record(entry, directory_handle)?;
            return self
                .sftp
                .plugin_list_directory(
                    &entry.access,
                    &directory.service_directory_ref,
                    Some(cursor.service_cursor),
                    limit,
                    norishell_core_api::OperationId::new(),
                    Uuid::now_v7().to_string(),
                )
                .await
                .map_err(map_sftp_error);
        }
        match (directory_handle.as_deref(), child_entry_handle.as_deref()) {
            (None, None) => self
                .sftp
                .plugin_list_root(
                    &entry.access,
                    None,
                    limit,
                    norishell_core_api::OperationId::new(),
                    Uuid::now_v7().to_string(),
                )
                .await
                .map_err(map_sftp_error),
            (Some(directory_handle), None) => {
                let directory = self.directory_record(entry, directory_handle)?;
                self.sftp
                    .plugin_list_directory(
                        &entry.access,
                        &directory.service_directory_ref,
                        None,
                        limit,
                        norishell_core_api::OperationId::new(),
                        Uuid::now_v7().to_string(),
                    )
                    .await
                    .map_err(map_sftp_error)
            }
            (Some(directory_handle), Some(entry_handle)) => {
                let child = self.entry_record(entry, directory_handle, entry_handle)?;
                if child.kind != PluginSftpEntryKind::Directory {
                    return Err(PluginApiErrorCode::PermissionDenied);
                }
                self.sftp
                    .plugin_list_child_directory(
                        &entry.access,
                        &child.service_directory_ref,
                        &child.service_entry_ref,
                        limit,
                        norishell_core_api::OperationId::new(),
                        Uuid::now_v7().to_string(),
                    )
                    .await
                    .map_err(map_sftp_error)
            }
            (None, Some(_)) => Err(PluginApiErrorCode::InvalidRequest),
        }
    }

    fn project_listing(
        &self,
        entry: &Entry,
        listing: SftpDirectoryListing,
        append_page: bool,
    ) -> Result<PluginSftpResult, PluginApiErrorCode> {
        let mut projection = entry
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory_handle = projection
            .directories
            .iter()
            .find_map(|(handle, record)| {
                (record.service_directory_ref == listing.directory_ref).then(|| handle.clone())
            })
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        if !projection.directories.contains_key(&directory_handle)
            && projection.directories.len() >= MAX_DIRECTORIES_PER_ROOT
        {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        if !append_page {
            // A refresh replaces the Core facts for this one directory. Keeping its
            // old plugin handles would both permit stale preconditions and let repeat
            // refreshes consume the bounded owner ledger indefinitely. Continuations
            // intentionally append pages from the same active listing instead.
            projection
                .entries
                .retain(|_, record| record.directory_handle != directory_handle);
            projection
                .cursors
                .retain(|_, record| record.directory_handle != directory_handle);
        }
        let accepted = listing
            .entries
            .into_iter()
            .filter_map(|item| project_entry(item).ok())
            .collect::<Vec<_>>();
        let new_entries = accepted.len();
        if projection.entries.len().saturating_add(new_entries) > MAX_ENTRIES_PER_ROOT {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        let has_cursor = listing.next_cursor.is_some();
        if has_cursor && projection.cursors.len() >= MAX_CURSORS_PER_ROOT {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        let service_directory_ref = listing.directory_ref.clone();
        projection.directories.insert(
            directory_handle.clone(),
            DirectoryRecord {
                service_directory_ref: service_directory_ref.clone(),
            },
        );
        let entries = accepted
            .into_iter()
            .map(|item| {
                let handle = Uuid::now_v7().to_string();
                projection.entries.insert(
                    handle.clone(),
                    EntryRecord {
                        directory_handle: directory_handle.clone(),
                        service_directory_ref: service_directory_ref.clone(),
                        service_entry_ref: item.service_entry_ref,
                        kind: item.kind,
                        precondition: item.precondition.clone(),
                    },
                );
                PluginSftpDirectoryEntry {
                    entry_handle: handle,
                    name: item.name,
                    kind: item.kind,
                    size: item.precondition.size,
                    modified_at_unix_ms: item.precondition.modified_at_unix_ms,
                    precondition: item.precondition,
                }
            })
            .collect();
        let next_cursor = listing.next_cursor.map(|service_cursor| {
            let handle = Uuid::now_v7().to_string();
            projection.cursors.insert(
                handle.clone(),
                CursorRecord {
                    directory_handle: directory_handle.clone(),
                    service_cursor,
                },
            );
            handle
        });
        Ok(PluginSftpResult::Page {
            directory_handle,
            entries,
            next_cursor,
        })
    }

    fn entry(&self, owner: &ResourceOwner, handle: &str) -> Result<Arc<Entry>, PluginApiErrorCode> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(handle)
            .filter(|entry| entry.owner == *owner && entry.consumer == self.consumer)
            .cloned()
            .ok_or(PluginApiErrorCode::NotFound)
    }

    fn directory_record(
        &self,
        entry: &Entry,
        directory_handle: &str,
    ) -> Result<DirectoryRecord, PluginApiErrorCode> {
        entry
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .directories
            .get(directory_handle)
            .map(|record| DirectoryRecord {
                service_directory_ref: record.service_directory_ref.clone(),
            })
            .ok_or(PluginApiErrorCode::NotFound)
    }

    fn entry_record(
        &self,
        entry: &Entry,
        directory_handle: &str,
        entry_handle: &str,
    ) -> Result<EntryRecord, PluginApiErrorCode> {
        entry
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(entry_handle)
            .filter(|record| record.directory_handle == directory_handle)
            .map(|record| EntryRecord {
                directory_handle: record.directory_handle.clone(),
                service_directory_ref: record.service_directory_ref.clone(),
                service_entry_ref: record.service_entry_ref.clone(),
                kind: record.kind,
                precondition: record.precondition.clone(),
            })
            .ok_or(PluginApiErrorCode::NotFound)
    }

    fn cursor_record(
        &self,
        entry: &Entry,
        directory_handle: &str,
        cursor: &str,
    ) -> Result<CursorRecord, PluginApiErrorCode> {
        entry
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cursors
            .remove(cursor)
            .filter(|record| record.directory_handle == directory_handle)
            .ok_or(PluginApiErrorCode::NotFound)
    }

    fn forget_entry(&self, entry: &Entry, entry_handle: &str) {
        entry
            .projection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .remove(entry_handle);
    }

    fn upload_count(&self, entry: &Entry) -> usize {
        entry
            .uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn take_upload(
        &self,
        entry: &Entry,
        upload_handle: &str,
    ) -> Result<UploadRecord, PluginApiErrorCode> {
        entry
            .uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(upload_handle)
            .ok_or(PluginApiErrorCode::NotFound)
    }

    fn put_upload(&self, entry: &Entry, upload_handle: String, upload: UploadRecord) {
        entry
            .uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(upload_handle, upload);
    }

    async fn store_started_upload(
        &self,
        entry: &Entry,
        upload: PluginSftpUpload,
        replaced_entry_handle: Option<String>,
        expected_bytes: u64,
        fence: &ResourceFence,
    ) -> Result<PluginSftpResult, PluginApiErrorCode> {
        let upload_handle = Uuid::now_v7().to_string();
        self.put_upload(
            entry,
            upload_handle.clone(),
            UploadRecord {
                upload,
                replaced_entry_handle,
            },
        );
        if let Err(error) = ensure_live(entry, fence) {
            return self
                .abort_started_upload(entry, &upload_handle, error)
                .await;
        }
        if let Err(error) = emit_sftp_event(
            entry,
            PluginSftpEvent::UploadStarted {
                upload_handle: upload_handle.clone(),
                expected_bytes,
            },
            fence,
        )
        .await
        {
            return self
                .abort_started_upload(entry, &upload_handle, error)
                .await;
        }
        Ok(PluginSftpResult::UploadStarted {
            upload_handle,
            expected_bytes,
        })
    }

    async fn abort_started_upload(
        &self,
        entry: &Entry,
        upload_handle: &str,
        original: PluginApiErrorCode,
    ) -> Result<PluginSftpResult, PluginApiErrorCode> {
        let record = self.take_upload(entry, upload_handle)?;
        match self
            .sftp
            .plugin_abort_upload(&entry.access, &record.upload)
            .await
        {
            Ok(()) => Err(original),
            Err(_) => {
                self.put_upload(entry, upload_handle.to_owned(), record);
                Err(PluginApiErrorCode::CleanupIncomplete)
            }
        }
    }
}

async fn emit_sftp_event(
    entry: &Entry,
    event: PluginSftpEvent,
    invocation_fence: &ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    let events = entry
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(events) = events else {
        return Ok(());
    };
    let resource_fence = entry.resource_fence.clone();
    let invocation_fence = invocation_fence.clone();
    let fence: ResourceFence = Arc::new(move || resource_fence() && invocation_fence());
    let mut cancel = entry.event_cancel.lock().await;
    events
        .emit_backpressured(
            PluginApiResourceEventKind::Sftp { event },
            &mut cancel,
            fence.as_ref(),
        )
        .await
}

/// Runs inside the quota-reserved resource task. The admission fence is kept
/// through connection and root validation, then intentionally discarded after
/// the usable handle is reported; only the durable resource fence owns the
/// long-lived independent SFTP transport.
#[allow(clippy::too_many_arguments)]
async fn run_sftp_root(
    grant: PluginSftpGrant,
    sftp: SftpSessionService,
    entries: Arc<Mutex<BTreeMap<String, Arc<Entry>>>>,
    consumer: Option<ResourceConsumer>,
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut slot: watch::Receiver<Option<String>>,
    ready: tokio::sync::oneshot::Sender<Result<(), PluginApiErrorCode>>,
) -> Result<(), PluginApiErrorCode> {
    let handle = loop {
        if let Some(handle) = slot.borrow_and_update().clone() {
            break handle;
        }
        tokio::select! {
            changed = slot.changed() => {
                if changed.is_err() {
                    let _ = ready.send(Err(PluginApiErrorCode::Cancelled));
                    return Ok(());
                }
            }
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    let _ = ready.send(Err(PluginApiErrorCode::Cancelled));
                    return Ok(());
                }
            }
        }
    };
    if *cancel.borrow_and_update() || !(grant.resource_fence)() || !(grant.admission_fence)() {
        let _ = ready.send(Err(PluginApiErrorCode::Revoked));
        return Ok(());
    }

    // This is the last check immediately before Core asks the service to create
    // a transport. `admission_fence` captures the full saved-connection
    // revision that the user reviewed, not merely the Host row revision.
    if !(grant.admission_fence)() {
        let _ = ready.send(Err(PluginApiErrorCode::Revoked));
        return Ok(());
    }
    let summary = match sftp
        .plugin_open_saved_host(
            &SftpSessionOpenRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: norishell_core_api::OperationId::new(),
                idempotency_key: Uuid::now_v7().to_string(),
                session_id: SftpSessionId::new(),
                host_id: grant.host_id.clone(),
                expected_host_state_version: grant.host_revision,
            },
            &grant.expected_connection_revision,
            grant.admission_fence.as_ref(),
        )
        .await
    {
        Ok(summary) => summary,
        Err(error) => {
            let _ = ready.send(Err(map_sftp_error(error)));
            return Ok(());
        }
    };
    if summary.state != SftpSessionState::Ready {
        return finish_sftp_initialization_failure(
            ready,
            cleanup_opened_session(&sftp, &summary).await,
            map_open_failure(&summary),
            &mut cancel,
            || cleanup_opened_session(&sftp, &summary),
        )
        .await;
    }
    if *cancel.borrow_and_update() || !(grant.resource_fence)() || !(grant.admission_fence)() {
        return finish_sftp_initialization_failure(
            ready,
            cleanup_opened_session(&sftp, &summary).await,
            PluginApiErrorCode::Revoked,
            &mut cancel,
            || cleanup_opened_session(&sftp, &summary),
        )
        .await;
    }
    let access = match sftp
        .plugin_access(
            summary.session_id.clone(),
            summary.generation,
            grant.root_path,
        )
        .await
    {
        Ok(access) => access,
        Err(error) => {
            return finish_sftp_initialization_failure(
                ready,
                cleanup_opened_session(&sftp, &summary).await,
                map_sftp_error(error),
                &mut cancel,
                || cleanup_opened_session(&sftp, &summary),
            )
            .await;
        }
    };
    if *cancel.borrow_and_update() || !(grant.resource_fence)() || !(grant.admission_fence)() {
        return finish_sftp_initialization_failure(
            ready,
            cleanup_opened_session(&sftp, &summary).await,
            PluginApiErrorCode::Revoked,
            &mut cancel,
            || cleanup_opened_session(&sftp, &summary),
        )
        .await;
    }

    let entry = Arc::new(Entry {
        owner: grant.owner,
        consumer,
        scope: grant.scope,
        access,
        resource_fence: grant.resource_fence,
        operation_lock: AsyncMutex::new(()),
        projection: Mutex::new(Projection::default()),
        uploads: Mutex::new(BTreeMap::new()),
        events: Mutex::new(Some(events)),
        event_cancel: AsyncMutex::new(cancel.clone()),
        revoked: AtomicBool::new(false),
    });
    entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(handle.clone(), entry.clone());
    if ready.send(Ok(())).is_err() {
        return cleanup_sftp_root(&sftp, &entries, &handle, &entry).await;
    }

    loop {
        if *cancel.borrow_and_update() || !(entry.resource_fence)() {
            break;
        }
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    cleanup_sftp_root(&sftp, &entries, &handle, &entry).await
}

/// The caller sees the original initialization failure only after the first
/// cleanup attempt proves the new independent transport is gone. Otherwise,
/// retain the resource task and exact opened-session identity until a later
/// `ResourceRegistry::close` / shutdown signal gives cleanup another chance.
/// That signal never re-runs open/access business work.
async fn finish_sftp_initialization_failure<F, Fut>(
    ready: tokio::sync::oneshot::Sender<Result<(), PluginApiErrorCode>>,
    cleanup: Result<(), PluginApiErrorCode>,
    fallback: PluginApiErrorCode,
    cancel: &mut watch::Receiver<bool>,
    mut retry_cleanup: F,
) -> Result<(), PluginApiErrorCode>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), PluginApiErrorCode>>,
{
    match cleanup {
        Ok(()) => {
            let _ = ready.send(Err(fallback));
            Ok(())
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            loop {
                cancel
                    .changed()
                    .await
                    .map_err(|_| PluginApiErrorCode::CleanupIncomplete)?;
                if retry_cleanup().await.is_ok() {
                    return Ok(());
                }
            }
        }
    }
}

async fn cleanup_opened_session(
    sftp: &SftpSessionService,
    summary: &norishell_core_api::SftpSessionSummary,
) -> Result<(), PluginApiErrorCode> {
    let closed = sftp
        .disconnect(SftpSessionDisconnectRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: norishell_core_api::OperationId::new(),
            idempotency_key: Uuid::now_v7().to_string(),
            session_id: summary.session_id.clone(),
            expected_generation: summary.generation,
        })
        .await
        .map_err(|_| PluginApiErrorCode::CleanupIncomplete)?;
    (closed.state == SftpSessionState::Closed)
        .then_some(())
        .ok_or(PluginApiErrorCode::CleanupIncomplete)
}

async fn cleanup_sftp_root(
    sftp: &SftpSessionService,
    entries: &Arc<Mutex<BTreeMap<String, Arc<Entry>>>>,
    handle: &str,
    entry: &Arc<Entry>,
) -> Result<(), PluginApiErrorCode> {
    entry.revoked.store(true, Ordering::Release);
    let _inflight = entry.operation_lock.lock().await;
    // Every staging file is Core-created and must be removed before a root's
    // independent transport can disappear. Keep any failed record in the
    // ledger so the remaining resource is visibly blocked rather than claiming
    // a clean close while a remote temporary target is unknown.
    let uploads = {
        let mut uploads = entry
            .uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *uploads)
    };
    let mut unresolved = BTreeMap::new();
    for (upload_handle, upload) in uploads {
        if sftp
            .plugin_abort_upload(&entry.access, &upload.upload)
            .await
            .is_err()
        {
            unresolved.insert(upload_handle, upload);
        }
    }
    if !unresolved.is_empty() {
        entry
            .uploads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(unresolved);
        return Err(PluginApiErrorCode::CleanupIncomplete);
    }
    sftp.plugin_disconnect(&entry.access)
        .await
        .map_err(|_| PluginApiErrorCode::CleanupIncomplete)?;
    entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(handle);
    Ok(())
}

struct ProjectedEntry {
    service_entry_ref: String,
    name: String,
    kind: PluginSftpEntryKind,
    precondition: PluginSftpObjectPrecondition,
}

fn project_entry(
    entry: norishell_core_api::SftpRemoteDirectoryEntry,
) -> Result<ProjectedEntry, PluginApiErrorCode> {
    let kind = match entry.kind {
        SftpRemoteEntryKind::File => PluginSftpEntryKind::File,
        SftpRemoteEntryKind::Directory => PluginSftpEntryKind::Directory,
        SftpRemoteEntryKind::Symlink | SftpRemoteEntryKind::Other => {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
    };
    validate_projected_name(&entry.display_name)?;
    Ok(ProjectedEntry {
        service_entry_ref: entry.entry_ref,
        name: entry.display_name,
        kind,
        precondition: PluginSftpObjectPrecondition {
            kind,
            size: entry.size,
            modified_at_unix_ms: entry.modified_at_unix_ms,
        },
    })
}

fn validate_projected_name(name: &str) -> Result<(), PluginApiErrorCode> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        Err(PluginApiErrorCode::InvalidRequest)
    } else {
        Ok(())
    }
}

fn operation_root_handle(operation: &PluginSftpOperation) -> &str {
    match operation {
        PluginSftpOperation::List { root_handle, .. }
        | PluginSftpOperation::Read { root_handle, .. }
        | PluginSftpOperation::WriteBinary { root_handle, .. }
        | PluginSftpOperation::UploadStart { root_handle, .. }
        | PluginSftpOperation::UploadReplaceStart { root_handle, .. }
        | PluginSftpOperation::UploadChunk { root_handle, .. }
        | PluginSftpOperation::UploadCommit { root_handle, .. }
        | PluginSftpOperation::UploadAbort { root_handle, .. }
        | PluginSftpOperation::CreateDirectory { root_handle, .. }
        | PluginSftpOperation::CreateEmptyFile { root_handle, .. }
        | PluginSftpOperation::RenameNoReplace { root_handle, .. }
        | PluginSftpOperation::Remove { root_handle, .. } => root_handle,
    }
}

fn to_sftp_precondition(value: &PluginSftpObjectPrecondition) -> SftpRemoteObjectPrecondition {
    SftpRemoteObjectPrecondition {
        kind: match value.kind {
            PluginSftpEntryKind::File => SftpRemoteEntryKind::File,
            PluginSftpEntryKind::Directory => SftpRemoteEntryKind::Directory,
        },
        size: value.size,
        modified_at_unix_ms: value.modified_at_unix_ms,
    }
}

fn decode_binary_chunk(value: &str) -> Result<Vec<u8>, PluginApiErrorCode> {
    if value.len() > usize::from(PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES).saturating_mul(2) {
        return Err(PluginApiErrorCode::QuotaExceeded);
    }
    let decoded = BASE64
        .decode(value)
        .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
    if decoded.len() > usize::from(PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES) {
        return Err(PluginApiErrorCode::QuotaExceeded);
    }
    Ok(decoded)
}

fn ensure_live(entry: &Entry, fence: &ResourceFence) -> Result<(), PluginApiErrorCode> {
    if entry.revoked.load(Ordering::Acquire) || !(entry.resource_fence)() || !fence() {
        Err(PluginApiErrorCode::Revoked)
    } else {
        Ok(())
    }
}

fn require(allowed: bool) -> Result<(), PluginApiErrorCode> {
    allowed
        .then_some(())
        .ok_or(PluginApiErrorCode::PermissionDenied)
}

fn map_open_failure(summary: &norishell_core_api::SftpSessionSummary) -> PluginApiErrorCode {
    use norishell_core_api::SftpFailureCode;
    match summary.failure.as_ref().map(|failure| failure.code) {
        Some(
            SftpFailureCode::HostKeyRejected
            | SftpFailureCode::VaultLocked
            | SftpFailureCode::CredentialUnavailable,
        ) => PluginApiErrorCode::InteractionRequired,
        Some(SftpFailureCode::HostKeyMismatch | SftpFailureCode::AuthenticationRejected) => {
            PluginApiErrorCode::PermissionDenied
        }
        _ => PluginApiErrorCode::Unavailable,
    }
}

fn map_sftp_error(error: SftpProductionError) -> PluginApiErrorCode {
    match error {
        SftpProductionError::Runtime(
            SftpRuntimeError::Conflict | SftpRuntimeError::StaleGeneration,
        ) => PluginApiErrorCode::Conflict,
        SftpProductionError::Runtime(SftpRuntimeError::CleanupIncomplete)
        | SftpProductionError::ShutdownIncomplete(_) => PluginApiErrorCode::CleanupIncomplete,
        SftpProductionError::Runtime(
            SftpRuntimeError::InvalidInput
            | SftpRuntimeError::ChallengeMismatch
            | SftpRuntimeError::UnsupportedPathEncoding,
        )
        | SftpProductionError::NonUtf8RemotePath => PluginApiErrorCode::InvalidRequest,
        SftpProductionError::Runtime(
            SftpRuntimeError::InvalidState
            | SftpRuntimeError::UnsafeReplaceUnsupported
            | SftpRuntimeError::ProgressRegression
            | SftpRuntimeError::LengthMismatch
            | SftpRuntimeError::ResumeEvidenceMismatch,
        )
        | SftpProductionError::Profile(_)
        | SftpProductionError::Connection { .. }
        | SftpProductionError::Transport(_) => PluginApiErrorCode::Unavailable,
    }
}

/// Once a staged upload reaches final-commit I/O, a transport failure cannot
/// prove whether the server applied the POSIX rename. Callers must refresh the
/// directory instead of retrying a potentially committed replacement.
fn map_sftp_commit_error(error: SftpProductionError) -> PluginApiErrorCode {
    match error {
        SftpProductionError::Connection { .. } | SftpProductionError::Transport(_) => {
            PluginApiErrorCode::OutcomeUnknown
        }
        other => map_sftp_error(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_operations_cannot_smuggle_path_or_session_identifiers() {
        let operation = PluginSftpOperation::Read {
            root_handle: "root".to_owned(),
            directory_handle: "directory".to_owned(),
            entry_handle: "entry".to_owned(),
            offset: 0,
            length: PLUGIN_SFTP_MAX_READ_BYTES,
        };
        assert_eq!(operation_root_handle(&operation), "root");
        let encoded = serde_json::to_value(operation).unwrap();
        assert!(encoded.get("hostId").is_none());
        assert!(encoded.get("sessionId").is_none());
        assert!(encoded.get("path").is_none());
    }

    #[test]
    fn write_scope_and_binary_frames_are_bounded_before_sftp_io() {
        let read_only = PluginSftpScopeFlags::for_access(false);
        assert!(read_only.allow_list && read_only.allow_read);
        assert!(!read_only.allow_binary_write && !read_only.allow_upload);
        let writable = PluginSftpScopeFlags::for_access(true);
        assert!(writable.allow_binary_write && writable.allow_upload);
        assert_eq!(decode_binary_chunk(&BASE64.encode(b"ok")).unwrap(), b"ok");
        assert!(matches!(
            decode_binary_chunk(&BASE64.encode(vec![
                0_u8;
                usize::from(PLUGIN_SFTP_MAX_WRITE_CHUNK_BYTES)
                    + 1
            ])),
            Err(PluginApiErrorCode::QuotaExceeded)
        ));
    }

    fn owner(generation: u64, package: char) -> ResourceOwner {
        ResourceOwner {
            plugin_id: norishell_core_api::PluginId::parse("org.norishell.plugin-sftp-test")
                .expect("plugin id"),
            signer: "a".repeat(64),
            package: package.to_string().repeat(64),
            generation: WireSequence::new(generation),
        }
    }

    #[tokio::test]
    async fn initialization_failure_with_completed_cleanup_keeps_the_original_open_error() {
        let (ready, received) = tokio::sync::oneshot::channel();
        let (_cancel_sender, mut cancel) = watch::channel(false);

        assert_eq!(
            finish_sftp_initialization_failure(
                ready,
                Ok(()),
                PluginApiErrorCode::Unavailable,
                &mut cancel,
                || async { unreachable!("completed cleanup must not retry") },
            )
            .await,
            Ok(())
        );
        assert_eq!(
            received.await.expect("initialization result"),
            Err(PluginApiErrorCode::Unavailable)
        );
    }

    #[tokio::test]
    async fn failed_initialization_cleanup_retries_and_removes_the_owner_scoped_blocker() {
        let registry = ResourceRegistry::default();
        let resource_owner = owner(1, 'b');
        let (ready, received) = tokio::sync::oneshot::channel();
        let attempts = Arc::new(AtomicUsize::new(0));
        let driver_attempts = attempts.clone();
        let handle = registry
            .spawn(
                resource_owner.clone(),
                "plugin-sftp-root",
                |mut cancel, _events| async move {
                    finish_sftp_initialization_failure(
                        ready,
                        Err(PluginApiErrorCode::CleanupIncomplete),
                        PluginApiErrorCode::Unavailable,
                        &mut cancel,
                        move || {
                            let attempt = driver_attempts.fetch_add(1, Ordering::AcqRel);
                            async move {
                                (attempt > 0)
                                    .then_some(())
                                    .ok_or(PluginApiErrorCode::CleanupIncomplete)
                            }
                        },
                    )
                    .await
                },
            )
            .expect("reserve SFTP resource");

        assert_eq!(
            received.await.expect("initialization result"),
            Err(PluginApiErrorCode::CleanupIncomplete)
        );
        assert_eq!(
            registry.close(&resource_owner, &handle).await,
            Err(PluginApiErrorCode::CleanupIncomplete)
        );
        assert_eq!(registry.list(&resource_owner).len(), 1);
        assert_eq!(
            registry.list(&resource_owner)[0].state,
            norishell_core_api::PluginApiResourceState::CleanupIncomplete
        );
        assert_eq!(attempts.load(Ordering::Acquire), 1);

        registry
            .close(&resource_owner, &handle)
            .await
            .expect("a later close retries only the retained cleanup");
        assert!(registry.list(&resource_owner).is_empty());
        assert_eq!(attempts.load(Ordering::Acquire), 2);
    }

    #[test]
    fn root_records_are_exact_owner_and_generation_scoped() {
        use crate::{
            host_service::HostService, ssh_agent_service::SshAgentService,
            transient_credential_service::TransientCredentialService, vault_service::VaultService,
        };

        let temporary = tempfile::tempdir().expect("temporary data directory");
        let service = SftpSessionService::production(
            HostService::start(temporary.path()).expect("host service"),
            VaultService::start(temporary.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let driver = PluginSftpDriver::new(service, ResourceRegistry::default());
        let first = owner(1, 'b');
        driver.entries.lock().unwrap().insert(
            "root".to_owned(),
            Arc::new(Entry {
                owner: first.clone(),
                consumer: None,
                scope: PluginSftpScopeFlags::default(),
                access: PluginSftpAccess::fixture(b"/approved".to_vec()),
                resource_fence: Arc::new(|| true),
                operation_lock: AsyncMutex::new(()),
                projection: Mutex::new(Projection::default()),
                uploads: Mutex::new(BTreeMap::new()),
                events: Mutex::new(None),
                event_cancel: AsyncMutex::new(watch::channel(false).1),
                revoked: AtomicBool::new(false),
            }),
        );
        assert!(driver.entry(&first, "root").is_ok());
        assert!(matches!(
            driver.entry(&owner(2, 'b'), "root"),
            Err(PluginApiErrorCode::NotFound)
        ));
        assert!(matches!(
            driver.entry(&owner(1, 'c'), "root"),
            Err(PluginApiErrorCode::NotFound)
        ));
    }

    #[test]
    fn consumer_views_cannot_resolve_another_roots_directory_or_cursor_handles() {
        use crate::{
            host_service::HostService, ssh_agent_service::SshAgentService,
            transient_credential_service::TransientCredentialService, vault_service::VaultService,
        };

        let temporary = tempfile::tempdir().expect("temporary data directory");
        let service = SftpSessionService::production(
            HostService::start(temporary.path()).expect("host service"),
            VaultService::start(temporary.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let registry = ResourceRegistry::default();
        let owner = owner(1, 'b');
        let first_consumer = ResourceConsumer {
            connection: Uuid::new_v4(),
            generation: WireSequence::new(1),
            stream: Uuid::new_v4(),
        };
        let first = PluginSftpDriver::new(service, registry).for_consumer(first_consumer.clone());
        let second = first.for_consumer(ResourceConsumer {
            stream: Uuid::new_v4(),
            ..first_consumer
        });
        let mut projection = Projection::default();
        projection.directories.insert(
            "directory".to_owned(),
            DirectoryRecord {
                service_directory_ref: "service-directory".to_owned(),
            },
        );
        projection.cursors.insert(
            "cursor".to_owned(),
            CursorRecord {
                directory_handle: "directory".to_owned(),
                service_cursor: vec![1],
            },
        );
        first.entries.lock().unwrap().insert(
            "root".to_owned(),
            Arc::new(Entry {
                owner: owner.clone(),
                consumer: first.consumer.clone(),
                scope: PluginSftpScopeFlags::default(),
                access: PluginSftpAccess::fixture(b"/approved".to_vec()),
                resource_fence: Arc::new(|| true),
                operation_lock: AsyncMutex::new(()),
                projection: Mutex::new(projection),
                uploads: Mutex::new(BTreeMap::new()),
                events: Mutex::new(None),
                event_cancel: AsyncMutex::new(watch::channel(false).1),
                revoked: AtomicBool::new(false),
            }),
        );
        let root = first.entry(&owner, "root").unwrap();
        assert!(first.directory_record(&root, "directory").is_ok());
        assert!(first.cursor_record(&root, "directory", "cursor").is_ok());
        assert!(matches!(
            second.entry(&owner, "root"),
            Err(PluginApiErrorCode::NotFound)
        ));
    }

    #[test]
    fn projected_entries_filter_symlinks_and_bad_child_names() {
        let base = norishell_core_api::SftpRemoteDirectoryEntry {
            entry_ref: "service-entry".to_owned(),
            path: norishell_core_api::SftpRemotePath {
                bytes: b"/approved/file".to_vec(),
            },
            display_name: "file".to_owned(),
            kind: SftpRemoteEntryKind::File,
            size: Some(1),
            modified_at_unix_ms: None,
            permission_bits: None,
        };
        assert!(project_entry(base.clone()).is_ok());
        assert!(
            project_entry(norishell_core_api::SftpRemoteDirectoryEntry {
                kind: SftpRemoteEntryKind::Symlink,
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            project_entry(norishell_core_api::SftpRemoteDirectoryEntry {
                display_name: "../escape".to_owned(),
                ..base
            })
            .is_err()
        );
    }
}
