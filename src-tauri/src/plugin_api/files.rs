//! Capability-handle local filesystem broker for plugins.
//!
//! Picker code creates a `PluginFileGrant` from an already-open `cap_std::fs::Dir`.
//! After registration this module uses only handle-relative operations; a guest never
//! supplies an ambient path. Existing-file replacement is deliberately documented as
//! best-effort conflict detection: portable filesystems provide no atomic, external
//! process compare-and-replace operation.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::{self, Read as _, Seek as _, SeekFrom, Write as _},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
#[cfg(unix)]
use cap_std::fs::MetadataExt as _;
#[cfg(windows)]
use cap_std::fs::MetadataExt as _;
use cap_std::fs::{Dir, File, OpenOptions};
use norishell_core_api::{
    PLUGIN_FILE_MAX_BYTES, PLUGIN_FILE_MAX_CHUNK_BYTES, PLUGIN_FILE_MAX_LIST_ENTRIES,
    PluginApiErrorCode, PluginApiResourceEventKind, PluginFileEntry, PluginFileEntryKind,
    PluginFileOperation, PluginFileResult, PluginFileWatchChange,
};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex as AsyncMutex, watch};
use uuid::Uuid;

use super::{ResourceConsumer, ResourceFence, ResourceOwner, ResourceRegistry};

const MAX_PATH_BYTES: usize = 4 * 1024;
const MIN_WATCH_INTERVAL_MS: u32 = 250;
const MAX_WATCH_INTERVAL_MS: u32 = 60_000;

/// This scope comes solely from the protected Core approval, not a guest request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PluginFileAccessScope {
    pub allow_read: bool,
    pub allow_write: bool,
    pub allow_list: bool,
    pub allow_rename: bool,
    pub allow_remove: bool,
    pub allow_recursive_remove: bool,
    pub allow_watch: bool,
}

/// A one-use, Core-created capability. It intentionally does not implement Clone:
/// one approved picker result can be registered exactly once.
pub(crate) struct PluginFileGrant {
    owner: ResourceOwner,
    scope: PluginFileAccessScope,
    root: GrantRoot,
}

enum GrantRoot {
    Directory(Dir),
    SelectedFile {
        parent: Dir,
        file_name: OsString,
        file: File,
    },
}

impl PluginFileGrant {
    pub(crate) fn directory(
        owner: ResourceOwner,
        directory: Dir,
        scope: PluginFileAccessScope,
    ) -> Self {
        Self {
            owner,
            scope,
            root: GrantRoot::Directory(directory),
        }
    }

    /// `parent` and `file_name` are created by Core from the native picker; callers must
    /// pass the opened selected file as well so registration can bind its current identity.
    pub(crate) fn selected_file(
        owner: ResourceOwner,
        parent: Dir,
        file_name: OsString,
        file: File,
        scope: PluginFileAccessScope,
    ) -> Result<Self, PluginApiErrorCode> {
        validate_leaf_name(&file_name)?;
        let from_parent = selected_file_identity(&parent, &file_name)?;
        let selected_object = object_identity(&file.metadata().map_err(map_io)?)?;
        if selected_object != from_parent.object_identity {
            return Err(PluginApiErrorCode::Conflict);
        }
        Ok(Self {
            owner,
            scope,
            root: GrantRoot::SelectedFile {
                parent,
                file_name,
                file,
            },
        })
    }
}

enum Root {
    Directory(Dir),
    Selected {
        parent: Dir,
        file_name: Mutex<OsString>,
        /// This is refreshed only after a successful broker operation. It prevents a
        /// picker-selected path from being silently retargeted by an outside rename.
        identity: Mutex<SelectedFileIdentity>,
        /// Keep the original approved handle alive. It is never cloned into another grant.
        _file: Mutex<File>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedFileIdentity {
    object_identity: String,
}

struct Entry {
    owner: ResourceOwner,
    consumer: Option<ResourceConsumer>,
    scope: PluginFileAccessScope,
    authorization_fence: ResourceFence,
    /// Serializes all filesystem I/O for this approved root. Besides preventing two local
    /// writes from passing the same expected-fingerprint check, cleanup holds this lock until
    /// any in-flight blocking I/O has reached its cancellation check.
    operation_lock: AsyncMutex<()>,
    root: Root,
    revoked: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct PluginFileService {
    resources: ResourceRegistry,
    entries: Arc<Mutex<BTreeMap<String, Arc<Entry>>>>,
    consumer: Option<ResourceConsumer>,
}

impl PluginFileService {
    pub(crate) fn new(resources: ResourceRegistry) -> Self {
        Self {
            resources,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            consumer: None,
        }
    }

    /// A consumer view shares the Core-owned root ledger and global resource quota, while every
    /// opaque root and watch handle remains private to this provider/task invocation.
    pub(crate) fn for_consumer(&self, consumer: ResourceConsumer) -> Self {
        Self {
            resources: self.resources.for_consumer(consumer.clone()),
            entries: self.entries.clone(),
            consumer: Some(consumer),
        }
    }

    /// Registers the capability itself as a registry resource. Therefore ordinary
    /// `stop_plugin`, `stop_plugin_generation`, crash recovery, and app shutdown use the
    /// same cancellation path as timers and sockets.
    pub(crate) async fn register(
        &self,
        grant: PluginFileGrant,
        authorization_fence: ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        let owner = grant.owner.clone();
        let consumer = self.consumer.clone();
        let entry = tokio::task::spawn_blocking(move || -> Result<Entry, PluginApiErrorCode> {
            Ok(match grant.root {
                GrantRoot::Directory(directory) => Entry {
                    owner: grant.owner.clone(),
                    consumer: consumer.clone(),
                    scope: grant.scope,
                    authorization_fence: authorization_fence.clone(),
                    operation_lock: AsyncMutex::new(()),
                    root: Root::Directory(directory),
                    revoked: AtomicBool::new(false),
                },
                GrantRoot::SelectedFile {
                    parent,
                    file_name,
                    file,
                } => Entry {
                    owner: grant.owner.clone(),
                    consumer,
                    scope: grant.scope,
                    authorization_fence: authorization_fence.clone(),
                    operation_lock: AsyncMutex::new(()),
                    root: Root::Selected {
                        identity: Mutex::new(selected_file_identity(&parent, &file_name)?),
                        parent,
                        file_name: Mutex::new(file_name),
                        _file: Mutex::new(file),
                    },
                    revoked: AtomicBool::new(false),
                },
            })
        })
        .await
        .map_err(|_| PluginApiErrorCode::Unavailable)??;
        self.register_entry(owner, Arc::new(entry)).await
    }

    /// Explicitly copies only approved, unscoped picker roots into one task/provider consumer.
    /// The source root stays usable by its parent; descendants are never imported independently.
    /// Returned handles are fresh, so a task cannot present a parent opaque handle as its own.
    pub(crate) async fn adopt_scopes(
        &self,
        owner: &ResourceOwner,
        source_root_handles: &[String],
        target_consumer: ResourceConsumer,
    ) -> Result<BTreeMap<String, String>, PluginApiErrorCode> {
        if self.consumer.is_some() {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        if source_root_handles.iter().collect::<BTreeSet<_>>().len() != source_root_handles.len() {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let source_entries = source_root_handles
            .iter()
            .map(|handle| {
                if Uuid::parse_str(handle).is_err() {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                self.entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(handle)
                    .filter(|entry| {
                        entry.owner == *owner
                            && entry.consumer.is_none()
                            && !entry.revoked.load(Ordering::Acquire)
                            && (entry.authorization_fence)()
                    })
                    .cloned()
                    .ok_or(PluginApiErrorCode::NotFound)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let target = self.for_consumer(target_consumer);
        let mut imported = BTreeMap::new();
        for (source_handle, source) in source_root_handles.iter().zip(source_entries) {
            let copied = match source.copy_for_consumer(target.consumer.clone()) {
                Ok(copied) => Arc::new(copied),
                Err(error) => {
                    return rollback_adopted_roots(&target, owner, imported, error).await;
                }
            };
            match target.register_entry(owner.clone(), copied).await {
                Ok(target_handle) => {
                    imported.insert(source_handle.clone(), target_handle);
                }
                Err(error) => {
                    return rollback_adopted_roots(&target, owner, imported, error).await;
                }
            }
        }
        Ok(imported)
    }

    async fn register_entry(
        &self,
        owner: ResourceOwner,
        entry: Arc<Entry>,
    ) -> Result<String, PluginApiErrorCode> {
        let (slot, mut driver_slot) = watch::channel(None::<String>);
        let entries = self.entries.clone();
        let driver_entry = entry.clone();
        let handle =
            self.resources
                .spawn(owner, "local-file-root", move |mut cancel, _| async move {
                    loop {
                        if *cancel.borrow_and_update() || !(driver_entry.authorization_fence)() {
                            break;
                        }
                        tokio::select! {
                            changed = cancel.changed() => {
                                if changed.is_err() || *cancel.borrow_and_update() { break; }
                            }
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        }
                    }
                    driver_entry.revoked.store(true, Ordering::Release);
                    // Closing a capability acknowledges only after every synchronous filesystem
                    // operation released its lock. This keeps a failed shutdown visible rather than
                    // reporting a closed root while a blocking write is still alive.
                    let _inflight = driver_entry.operation_lock.lock().await;
                    let handle = loop {
                        if let Some(handle) = driver_slot.borrow_and_update().clone() {
                            break handle;
                        }
                        if driver_slot.changed().await.is_err() {
                            return Ok(());
                        }
                    };
                    entries
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&handle);
                    Ok(())
                })?;
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(handle.clone(), entry);
        slot.send_replace(Some(handle.clone()));
        Ok(handle)
    }

    pub(crate) async fn invoke(
        &self,
        owner: &ResourceOwner,
        operation: &PluginFileOperation,
        fence: ResourceFence,
    ) -> Result<PluginFileResult, PluginApiErrorCode> {
        let (root_handle, path) = operation_root_and_path(operation);
        let entry = self.entry(owner, root_handle)?;
        ensure_live(&entry, &fence)?;
        match operation {
            PluginFileOperation::Read { offset, .. } => {
                require(entry.scope.allow_read)?;
                let _operation = entry.operation_lock.lock().await;
                let entry = entry.clone();
                let path = path.to_owned();
                let fence = fence.clone();
                let offset = *offset;
                blocking(move || {
                    ensure_live(&entry, &fence)?;
                    let (_, path) = entry.resolve(&path)?;
                    let fingerprint = fingerprint_entry(&entry, &path)?;
                    let (data, eof) = entry.read_file(&path, offset)?;
                    ensure_live(&entry, &fence)?;
                    Ok(PluginFileResult::Read {
                        data_base64: BASE64.encode(data),
                        fingerprint,
                        eof,
                    })
                })
                .await
            }
            PluginFileOperation::Write {
                expected_fingerprint,
                offset,
                data_base64,
                final_size,
                ..
            } => {
                require(entry.scope.allow_write)?;
                let data = BASE64
                    .decode(data_base64)
                    .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
                if data.len() > PLUGIN_FILE_MAX_CHUNK_BYTES as usize {
                    return Err(PluginApiErrorCode::QuotaExceeded);
                }
                let _operation = entry.operation_lock.lock().await;
                let entry = entry.clone();
                let path = path.to_owned();
                let expected = expected_fingerprint.clone();
                let fence = fence.clone();
                let offset = *offset;
                let final_size = *final_size;
                blocking(move || {
                    ensure_live(&entry, &fence)?;
                    let (_, path) = entry.resolve(&path)?;
                    entry
                        .write_snapshot(
                            &path,
                            expected.as_deref(),
                            offset,
                            &data,
                            final_size,
                            &fence,
                        )
                        .map(|fingerprint| PluginFileResult::Written { fingerprint })
                })
                .await
            }
            PluginFileOperation::List { cursor, limit, .. } => {
                require(entry.scope.allow_list)?;
                if *limit == 0 || *limit > PLUGIN_FILE_MAX_LIST_ENTRIES {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                if matches!(&entry.root, Root::Selected { .. }) {
                    return Err(PluginApiErrorCode::PermissionDenied);
                }
                let _operation = entry.operation_lock.lock().await;
                let entry = entry.clone();
                let path = path.to_owned();
                let cursor = cursor.clone();
                let fence = fence.clone();
                let limit = *limit;
                blocking(move || {
                    ensure_live(&entry, &fence)?;
                    let (dir, path) = entry.resolve(&path)?;
                    let (entries, next_cursor) = list(&dir, &path, cursor.as_deref(), limit)?;
                    ensure_live(&entry, &fence)?;
                    Ok(PluginFileResult::Listed {
                        entries,
                        next_cursor,
                    })
                })
                .await
            }
            PluginFileOperation::Rename {
                from_path,
                to_path,
                expected_fingerprint,
                ..
            } => {
                require(entry.scope.allow_rename)?;
                let _operation = entry.operation_lock.lock().await;
                let entry = entry.clone();
                let from_path = from_path.clone();
                let to_path = to_path.clone();
                let expected = expected_fingerprint.clone();
                let fence = fence.clone();
                blocking(move || {
                    ensure_live(&entry, &fence)?;
                    let (dir, from) = entry.resolve(&from_path)?;
                    let (_, to) = entry.resolve_rename_destination(&to_path)?;
                    entry
                        .rename(&dir, &from, &to, &expected, &fence)
                        .map(|fingerprint| PluginFileResult::Renamed { fingerprint })
                })
                .await
            }
            PluginFileOperation::Remove {
                expected_fingerprint,
                recursive,
                ..
            } => {
                require(entry.scope.allow_remove)?;
                if *recursive && !entry.scope.allow_recursive_remove {
                    return Err(PluginApiErrorCode::PermissionDenied);
                }
                let _operation = entry.operation_lock.lock().await;
                let entry = entry.clone();
                let path = path.to_owned();
                let expected = expected_fingerprint.clone();
                let fence = fence.clone();
                let recursive = *recursive;
                blocking(move || {
                    ensure_live(&entry, &fence)?;
                    let (dir, path) = entry.resolve(&path)?;
                    entry.remove(&dir, &path, &expected, recursive, &fence)?;
                    Ok(PluginFileResult::Removed {})
                })
                .await
            }
            PluginFileOperation::WatchStart { interval_ms, .. } => {
                require(entry.scope.allow_watch)?;
                if !(MIN_WATCH_INTERVAL_MS..=MAX_WATCH_INTERVAL_MS).contains(interval_ms) {
                    return Err(PluginApiErrorCode::InvalidRequest);
                }
                if matches!(&entry.root, Root::Selected { .. }) && path.is_empty() {
                    // A selected-file approval can watch that file, but cannot turn into a
                    // parent-directory observation capability.
                }
                let path = parse_relative_path(path)?;
                let root_handle = root_handle.to_owned();
                let interval_ms = *interval_ms;
                let watch_entry = entry.clone();
                let watch_fence = fence.clone();
                let _operation = entry.operation_lock.lock().await;
                let initial_entry = entry.clone();
                let initial_path = path.clone();
                let initial_fence = fence.clone();
                let mut previous = blocking(move || {
                    ensure_live(&initial_entry, &initial_fence)?;
                    watch_fingerprint(&initial_entry, &initial_path)
                })
                .await?;
                drop(_operation);
                let handle = self.resources.spawn(owner.clone(), "local-file-watch", move |mut cancel, events| async move {
                    loop {
                        tokio::select! {
                            changed = cancel.changed() => {
                                if changed.is_err() || *cancel.borrow_and_update() { break; }
                            }
                            _ = tokio::time::sleep(Duration::from_millis(u64::from(interval_ms))) => {
                                if ensure_live(&watch_entry, &watch_fence).is_err() { break; }
                                let current_entry = watch_entry.clone();
                                let current_path = path.clone();
                                let current_fence = watch_fence.clone();
                                let current = match async {
                                    let _operation = current_entry.operation_lock.lock().await;
                                    let operation_entry = current_entry.clone();
                                    blocking(move || {
                                        ensure_live(&operation_entry, &current_fence)?;
                                        watch_fingerprint(&operation_entry, &current_path)
                                    }).await
                                }.await {
                                    Ok(value) => value,
                                    // Permission/temporary I/O failure is not a deletion event.
                                    Err(_) => continue,
                                };
                                if current != previous {
                                    let event = PluginApiResourceEventKind::FileChanged {
                                        change: PluginFileWatchChange {
                                            root_handle: root_handle.clone(),
                                            relative_path: path.to_string_lossy().into_owned(),
                                            fingerprint: current.clone(),
                                        },
                                    };
                                    if events.emit_backpressured(event, &mut cancel, &*watch_fence).await.is_err() { break; }
                                    previous = current;
                                }
                            }
                        }
                    }
                    Ok(())
                })?;
                Ok(PluginFileResult::WatchStarted { handle })
            }
        }
    }

    fn entry(&self, owner: &ResourceOwner, handle: &str) -> Result<Arc<Entry>, PluginApiErrorCode> {
        if Uuid::parse_str(handle).is_err() {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(handle)
            .filter(|entry| entry.owner == *owner && entry.consumer == self.consumer)
            .cloned()
            .ok_or(PluginApiErrorCode::NotFound)
    }
}

async fn rollback_adopted_roots(
    target: &PluginFileService,
    owner: &ResourceOwner,
    imported: BTreeMap<String, String>,
    original: PluginApiErrorCode,
) -> Result<BTreeMap<String, String>, PluginApiErrorCode> {
    for handle in imported.into_values() {
        if target.resources.close(owner, &handle).await.is_err() {
            return Err(PluginApiErrorCode::CleanupIncomplete);
        }
    }
    Err(original)
}

impl Entry {
    fn copy_for_consumer(
        &self,
        consumer: Option<ResourceConsumer>,
    ) -> Result<Self, PluginApiErrorCode> {
        let root = match &self.root {
            Root::Directory(directory) => Root::Directory(directory.try_clone().map_err(map_io)?),
            Root::Selected {
                parent,
                file_name,
                identity,
                _file,
            } => Root::Selected {
                parent: parent.try_clone().map_err(map_io)?,
                file_name: Mutex::new(
                    file_name
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone(),
                ),
                identity: Mutex::new(
                    identity
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone(),
                ),
                _file: Mutex::new(
                    _file
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .try_clone()
                        .map_err(map_io)?,
                ),
            },
        };
        Ok(Self {
            owner: self.owner.clone(),
            consumer,
            scope: self.scope,
            authorization_fence: self.authorization_fence.clone(),
            operation_lock: AsyncMutex::new(()),
            root,
            revoked: AtomicBool::new(false),
        })
    }

    fn resolve(&self, requested: &str) -> Result<(Dir, PathBuf), PluginApiErrorCode> {
        let path = parse_relative_path(requested)?;
        match &self.root {
            Root::Directory(dir) => Ok((dir.try_clone().map_err(map_io)?, path)),
            Root::Selected {
                parent,
                file_name,
                identity,
                ..
            } => {
                let file_name = file_name
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if !(path.as_os_str().is_empty() || path == Path::new(&file_name)) {
                    return Err(PluginApiErrorCode::PermissionDenied);
                }
                let current = selected_file_identity(parent, &file_name)?;
                if current
                    != *identity
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                {
                    return Err(PluginApiErrorCode::Conflict);
                }
                Ok((
                    parent.try_clone().map_err(map_io)?,
                    PathBuf::from(file_name),
                ))
            }
        }
    }

    fn resolve_rename_destination(
        &self,
        requested: &str,
    ) -> Result<(Dir, PathBuf), PluginApiErrorCode> {
        let path = parse_relative_path(requested)?;
        match &self.root {
            Root::Directory(dir) => Ok((dir.try_clone().map_err(map_io)?, path)),
            Root::Selected { parent, .. } => {
                validate_leaf_name(path.as_os_str())?;
                Ok((parent.try_clone().map_err(map_io)?, path))
            }
        }
    }

    fn read_file(&self, path: &Path, offset: u64) -> Result<(Vec<u8>, bool), PluginApiErrorCode> {
        let (dir, path) = self.resolve_path(path)?;
        let mut file = open_regular_file(&dir, &path)?;
        let size = file.metadata().map_err(map_io)?.len();
        if size > PLUGIN_FILE_MAX_BYTES || offset > size {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        file.seek(SeekFrom::Start(offset)).map_err(map_io)?;
        let mut data = vec![0; PLUGIN_FILE_MAX_CHUNK_BYTES as usize];
        let count = file.read(&mut data).map_err(map_io)?;
        data.truncate(count);
        Ok((data, offset.saturating_add(count as u64) >= size))
    }

    fn write_snapshot(
        &self,
        path: &Path,
        expected: Option<&str>,
        offset: u64,
        data: &[u8],
        final_size: Option<u64>,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        ensure_live(self, fence)?;
        let (parent, leaf) = self.parent_and_leaf(path)?;
        let old = read_optional_regular_file(&parent, Path::new(&leaf))?;
        match (&old, expected) {
            (None, None) => {}
            (Some((_, actual)), Some(expected)) if actual == expected => {}
            _ => return Err(PluginApiErrorCode::Conflict),
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(PluginApiErrorCode::QuotaExceeded)?;
        let target_size = final_size.unwrap_or_else(|| {
            old.as_ref()
                .map_or(end, |(value, _)| value.len() as u64)
                .max(end)
        });
        if target_size > PLUGIN_FILE_MAX_BYTES || target_size < end {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        let mut snapshot = old.map_or_else(Vec::new, |(value, _)| value);
        snapshot.resize(target_size as usize, 0);
        snapshot[offset as usize..end as usize].copy_from_slice(data);
        let temporary = temporary_leaf(&leaf)?;
        let write_result = (|| -> Result<String, PluginApiErrorCode> {
            let mut temp = parent
                .open_with(&temporary, OpenOptions::new().write(true).create_new(true))
                .map_err(map_io)?;
            temp.write_all(&snapshot).map_err(map_io)?;
            temp.sync_all().map_err(map_io)?;
            ensure_live(self, fence)?;
            let now = read_optional_regular_file(&parent, Path::new(&leaf))?;
            match (&now, expected) {
                (None, None) => {}
                (Some((_, actual)), Some(expected)) if actual == expected => {}
                _ => return Err(PluginApiErrorCode::Conflict),
            }
            // This rename is atomic once accepted by the filesystem, but the preceding
            // equality recheck cannot become a portable cross-process CAS.
            ensure_live(self, fence)?;
            parent.rename(&temporary, &parent, &leaf).map_err(map_io)?;
            let fingerprint = fingerprint_regular_file(&parent, Path::new(&leaf))?
                .ok_or(PluginApiErrorCode::Unavailable)?;
            self.refresh_selected_identity(&leaf)?;
            Ok(fingerprint)
        })();
        if write_result.is_err() {
            let _ = parent.remove_file(&temporary);
        }
        write_result
    }

    fn rename(
        &self,
        dir: &Dir,
        from: &Path,
        to: &Path,
        expected: &str,
        fence: &ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        ensure_live(self, fence)?;
        if dir.symlink_metadata(to).is_ok() {
            return Err(PluginApiErrorCode::Conflict);
        }
        let fingerprint = fingerprint_path(dir, from)?;
        if fingerprint != expected {
            return Err(PluginApiErrorCode::Conflict);
        }
        ensure_live(self, fence)?;
        if fingerprint_path(dir, from)? != expected || dir.symlink_metadata(to).is_ok() {
            return Err(PluginApiErrorCode::Conflict);
        }
        ensure_live(self, fence)?;
        dir.rename(from, dir, to).map_err(map_io)?;
        self.refresh_selected_rename(from, to)?;
        Ok(fingerprint)
    }

    fn remove(
        &self,
        dir: &Dir,
        path: &Path,
        expected: &str,
        recursive: bool,
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        ensure_live(self, fence)?;
        let metadata = dir.symlink_metadata(path).map_err(map_io)?;
        if metadata.is_symlink() {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        if fingerprint_path(dir, path)? != expected {
            return Err(PluginApiErrorCode::Conflict);
        }
        ensure_live(self, fence)?;
        if fingerprint_path(dir, path)? != expected {
            return Err(PluginApiErrorCode::Conflict);
        }
        ensure_live(self, fence)?;
        if metadata.is_file() {
            dir.remove_file(path).map_err(map_io)?;
        } else if metadata.is_dir() && recursive {
            dir.remove_dir_all(path).map_err(map_io)?;
        } else if metadata.is_dir() {
            dir.remove_dir(path).map_err(map_io)?;
        } else {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        Ok(())
    }

    fn resolve_path(&self, path: &Path) -> Result<(Dir, PathBuf), PluginApiErrorCode> {
        self.resolve(path.to_str().ok_or(PluginApiErrorCode::InvalidRequest)?)
    }

    fn parent_and_leaf(&self, path: &Path) -> Result<(Dir, OsString), PluginApiErrorCode> {
        let leaf = path
            .file_name()
            .ok_or(PluginApiErrorCode::InvalidRequest)?
            .to_os_string();
        validate_leaf_name(&leaf)?;
        let parent_path = path.parent().unwrap_or(Path::new(""));
        let (root, _) = self.resolve("")?;
        let parent = if parent_path.as_os_str().is_empty() {
            root
        } else {
            open_safe_directory(&root, parent_path)?
        };
        Ok((parent, leaf))
    }

    fn refresh_selected_identity(&self, leaf: &OsString) -> Result<(), PluginApiErrorCode> {
        if let Root::Selected {
            parent,
            file_name,
            identity,
            ..
        } = &self.root
        {
            let current = file_name
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if &*current != leaf {
                return Err(PluginApiErrorCode::Conflict);
            }
            let current_identity = selected_file_identity(parent, leaf)?;
            *identity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = current_identity;
        }
        Ok(())
    }

    fn refresh_selected_rename(&self, from: &Path, to: &Path) -> Result<(), PluginApiErrorCode> {
        if let Root::Selected {
            parent,
            file_name,
            identity,
            ..
        } = &self.root
        {
            let from = from.file_name().ok_or(PluginApiErrorCode::InvalidRequest)?;
            let to = to.file_name().ok_or(PluginApiErrorCode::InvalidRequest)?;
            let mut current = file_name
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if current.as_os_str() != from {
                return Err(PluginApiErrorCode::Conflict);
            }
            *current = to.to_os_string();
            let current_identity = selected_file_identity(parent, to)?;
            *identity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = current_identity;
        }
        Ok(())
    }
}

fn operation_root_and_path(operation: &PluginFileOperation) -> (&str, &str) {
    match operation {
        PluginFileOperation::Read {
            root_handle,
            relative_path,
            ..
        }
        | PluginFileOperation::Write {
            root_handle,
            relative_path,
            ..
        }
        | PluginFileOperation::List {
            root_handle,
            relative_path,
            ..
        }
        | PluginFileOperation::Remove {
            root_handle,
            relative_path,
            ..
        }
        | PluginFileOperation::WatchStart {
            root_handle,
            relative_path,
            ..
        } => (root_handle, relative_path),
        PluginFileOperation::Rename {
            root_handle,
            from_path,
            ..
        } => (root_handle, from_path),
    }
}

fn ensure_live(entry: &Entry, fence: &ResourceFence) -> Result<(), PluginApiErrorCode> {
    if entry.revoked.load(Ordering::Acquire) || !(entry.authorization_fence)() || !fence() {
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

async fn blocking<T, F>(operation: F) -> Result<T, PluginApiErrorCode>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, PluginApiErrorCode> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| PluginApiErrorCode::Unavailable)?
}

fn parse_relative_path(value: &str) -> Result<PathBuf, PluginApiErrorCode> {
    if value.len() > MAX_PATH_BYTES
        || value.contains('\0')
        || value.contains('\\')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    if value.is_empty() {
        return Ok(PathBuf::new());
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(path.to_path_buf())
}

fn validate_leaf_name(name: &std::ffi::OsStr) -> Result<(), PluginApiErrorCode> {
    let path = Path::new(name);
    if name.is_empty()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        Err(PluginApiErrorCode::InvalidRequest)
    } else {
        Ok(())
    }
}

fn open_safe_directory(root: &Dir, path: &Path) -> Result<Dir, PluginApiErrorCode> {
    let mut current = root.try_clone().map_err(map_io)?;
    for part in path.components() {
        let Component::Normal(name) = part else {
            return Err(PluginApiErrorCode::InvalidRequest);
        };
        let metadata = current.symlink_metadata(name).map_err(map_io)?;
        if metadata.is_symlink() || !metadata.is_dir() {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        current = current.open_dir(name).map_err(map_io)?;
    }
    Ok(current)
}

fn open_regular_file(dir: &Dir, path: &Path) -> Result<File, PluginApiErrorCode> {
    let metadata = dir.symlink_metadata(path).map_err(map_io)?;
    if metadata.is_symlink() || !metadata.is_file() {
        return Err(PluginApiErrorCode::PermissionDenied);
    }
    dir.open(path).map_err(map_io)
}

fn read_optional_regular_file(
    dir: &Dir,
    path: &Path,
) -> Result<Option<(Vec<u8>, String)>, PluginApiErrorCode> {
    match dir.symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(map_io(error)),
        Ok(metadata) if metadata.is_symlink() || !metadata.is_file() => {
            Err(PluginApiErrorCode::PermissionDenied)
        }
        Ok(metadata) if metadata.len() > PLUGIN_FILE_MAX_BYTES => {
            Err(PluginApiErrorCode::QuotaExceeded)
        }
        Ok(_) => {
            let mut file = dir.open(path).map_err(map_io)?;
            let mut content = Vec::new();
            file.read_to_end(&mut content).map_err(map_io)?;
            if content.len() as u64 > PLUGIN_FILE_MAX_BYTES {
                return Err(PluginApiErrorCode::QuotaExceeded);
            }
            Ok(Some((content.clone(), fingerprint_bytes(&content))))
        }
    }
}

fn fingerprint_regular_file(dir: &Dir, path: &Path) -> Result<Option<String>, PluginApiErrorCode> {
    Ok(read_optional_regular_file(dir, path)?.map(|(_, fingerprint)| fingerprint))
}

fn selected_file_identity(
    parent: &Dir,
    file_name: &std::ffi::OsStr,
) -> Result<SelectedFileIdentity, PluginApiErrorCode> {
    let metadata = parent
        .symlink_metadata(Path::new(file_name))
        .map_err(map_io)?;
    if metadata.is_symlink() || !metadata.is_file() {
        return Err(PluginApiErrorCode::Conflict);
    }
    Ok(SelectedFileIdentity {
        object_identity: object_identity(&metadata)?,
    })
}

#[cfg(unix)]
fn object_identity(metadata: &cap_std::fs::Metadata) -> Result<String, PluginApiErrorCode> {
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn object_identity(metadata: &cap_std::fs::Metadata) -> Result<String, PluginApiErrorCode> {
    Ok(format!(
        "{}:{}",
        metadata
            .volume_serial_number()
            .ok_or(PluginApiErrorCode::Unavailable)?,
        metadata
            .file_index()
            .ok_or(PluginApiErrorCode::Unavailable)?,
    ))
}

#[cfg(not(any(unix, windows)))]
fn object_identity(_metadata: &cap_std::fs::Metadata) -> Result<String, PluginApiErrorCode> {
    Err(PluginApiErrorCode::Unsupported)
}

fn fingerprint_entry(entry: &Entry, path: &Path) -> Result<String, PluginApiErrorCode> {
    let (dir, path) = entry.resolve_path(path)?;
    fingerprint_path(&dir, &path)
}

fn watch_fingerprint(entry: &Entry, path: &Path) -> Result<Option<String>, PluginApiErrorCode> {
    let (dir, path) = entry.resolve_path(path)?;
    match dir.symlink_metadata(&path) {
        Ok(_) => fingerprint_path(&dir, &path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(map_io(error)),
    }
}

fn fingerprint_path(dir: &Dir, path: &Path) -> Result<String, PluginApiErrorCode> {
    let metadata = dir.symlink_metadata(path).map_err(map_io)?;
    if metadata.is_symlink() {
        return Err(PluginApiErrorCode::PermissionDenied);
    }
    if metadata.is_file() {
        return fingerprint_regular_file(dir, path)?.ok_or(PluginApiErrorCode::NotFound);
    }
    if !metadata.is_dir() {
        return Err(PluginApiErrorCode::PermissionDenied);
    }
    let mut names = Vec::new();
    for entry in dir.read_dir(path).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let file_type = entry.file_type().map_err(map_io)?;
        if file_type.is_symlink() {
            return Err(PluginApiErrorCode::PermissionDenied);
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| PluginApiErrorCode::Unsupported)?;
        names.push((
            name,
            if file_type.is_file() {
                b'f'
            } else if file_type.is_dir() {
                b'd'
            } else {
                b'o'
            },
        ));
    }
    names.sort();
    let mut hasher = Sha256::new();
    hasher.update(b"directory-v1\0");
    for (name, kind) in names {
        hasher.update(name.as_bytes());
        hasher.update([0, kind]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn list(
    dir: &Dir,
    path: &Path,
    cursor: Option<&str>,
    limit: u16,
) -> Result<(Vec<PluginFileEntry>, Option<String>), PluginApiErrorCode> {
    let start = cursor
        .map(|cursor| {
            cursor
                .parse::<usize>()
                .map_err(|_| PluginApiErrorCode::InvalidRequest)
        })
        .transpose()?
        .unwrap_or(0);
    let mut entries = Vec::new();
    for item in dir.read_dir(path).map_err(map_io)? {
        let item = item.map_err(map_io)?;
        let file_type = item.file_type().map_err(map_io)?;
        if file_type.is_symlink() {
            continue;
        }
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| PluginApiErrorCode::Unsupported)?;
        if file_type.is_file() {
            entries.push(PluginFileEntry {
                name,
                kind: PluginFileEntryKind::File,
                size: Some(item.metadata().map_err(map_io)?.len()),
                fingerprint: None,
            });
        } else if file_type.is_dir() {
            let fingerprint = fingerprint_path(dir, Path::new(&name))?;
            entries.push(PluginFileEntry {
                name,
                kind: PluginFileEntryKind::Directory,
                size: None,
                fingerprint: Some(fingerprint),
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let end = start.saturating_add(limit as usize).min(entries.len());
    if start > entries.len() {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    let next = (end < entries.len()).then(|| end.to_string());
    Ok((entries[start..end].to_vec(), next))
}

fn temporary_leaf(leaf: &std::ffi::OsStr) -> Result<OsString, PluginApiErrorCode> {
    let name = leaf.to_str().ok_or(PluginApiErrorCode::Unsupported)?;
    let temporary = format!(".{name}.norishell-plugin-{}.tmp", Uuid::new_v4());
    if temporary.len() > 255 {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(OsString::from(temporary))
}

fn fingerprint_bytes(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

fn map_io(error: io::Error) -> PluginApiErrorCode {
    match error.kind() {
        io::ErrorKind::NotFound => PluginApiErrorCode::NotFound,
        io::ErrorKind::AlreadyExists => PluginApiErrorCode::Conflict,
        io::ErrorKind::PermissionDenied => PluginApiErrorCode::PermissionDenied,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => {
            PluginApiErrorCode::InvalidRequest
        }
        _ => PluginApiErrorCode::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;
    use norishell_core_api::{PluginId, WireSequence};
    use uuid::Uuid;

    #[test]
    fn guest_paths_cannot_escape_or_smuggle_windows_separators() {
        for path in ["/tmp/x", "../x", "a/../x", "a\\b", "a//b", "\0"] {
            assert!(parse_relative_path(path).is_err(), "{path:?}");
        }
        assert_eq!(parse_relative_path("a/b").unwrap(), PathBuf::from("a/b"));
    }

    #[test]
    fn chunk_limit_and_file_limit_are_part_of_the_public_contract() {
        assert_eq!(PLUGIN_FILE_MAX_CHUNK_BYTES, 16 * 1024);
        assert_eq!(PLUGIN_FILE_MAX_BYTES, 64 * 1024 * 1024);
    }

    fn owner() -> ResourceOwner {
        ResourceOwner {
            plugin_id: PluginId::parse("org.norishell.file-test").unwrap(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        }
    }

    fn consumer(stream: Uuid) -> ResourceConsumer {
        ResourceConsumer {
            connection: Uuid::new_v4(),
            generation: WireSequence::new(1),
            stream,
        }
    }

    #[tokio::test]
    async fn consumer_views_reject_other_roots_and_scoped_stop_keeps_sibling_open() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("approved.txt"), b"approved").unwrap();
        let resources = ResourceRegistry::default();
        let service = PluginFileService::new(resources.clone());
        let owner = owner();
        let first_consumer = consumer(Uuid::new_v4());
        let second_consumer = ResourceConsumer {
            stream: Uuid::new_v4(),
            ..first_consumer.clone()
        };
        let first = service.for_consumer(first_consumer);
        let second = service.for_consumer(second_consumer);
        let first_root = first
            .register(
                PluginFileGrant::directory(
                    owner.clone(),
                    Dir::open_ambient_dir(temporary.path(), ambient_authority()).unwrap(),
                    PluginFileAccessScope {
                        allow_read: true,
                        allow_write: true,
                        ..Default::default()
                    },
                ),
                Arc::new(|| true),
            )
            .await
            .unwrap();
        let second_root = second
            .register(
                PluginFileGrant::directory(
                    owner.clone(),
                    Dir::open_ambient_dir(temporary.path(), ambient_authority()).unwrap(),
                    PluginFileAccessScope {
                        allow_read: true,
                        ..Default::default()
                    },
                ),
                Arc::new(|| true),
            )
            .await
            .unwrap();
        let read = |root_handle| PluginFileOperation::Read {
            root_handle,
            relative_path: "approved.txt".to_owned(),
            offset: 0,
        };
        assert!(
            first
                .invoke(&owner, &read(first_root.clone()), Arc::new(|| true))
                .await
                .is_ok()
        );
        assert_eq!(
            second
                .invoke(&owner, &read(first_root.clone()), Arc::new(|| true))
                .await,
            Err(PluginApiErrorCode::NotFound)
        );
        assert_eq!(
            second
                .invoke(
                    &owner,
                    &PluginFileOperation::Write {
                        root_handle: first_root.clone(),
                        relative_path: "approved.txt".to_owned(),
                        expected_fingerprint: None,
                        offset: 0,
                        data_base64: BASE64.encode(b"blocked"),
                        final_size: None,
                    },
                    Arc::new(|| true),
                )
                .await,
            Err(PluginApiErrorCode::NotFound)
        );
        assert_eq!(
            second.resources.close(&owner, &first_root).await,
            Err(PluginApiErrorCode::NotFound)
        );
        first
            .resources
            .stop_plugin_generation(&owner.plugin_id, owner.generation)
            .await
            .unwrap();
        assert!(
            second
                .invoke(&owner, &read(second_root), Arc::new(|| true))
                .await
                .is_ok()
        );
        resources.stop_plugin(&owner.plugin_id).await.unwrap();
    }

    #[tokio::test]
    async fn explicit_import_copies_only_unscoped_roots_and_keeps_parent_open() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("approved.txt"), b"approved").unwrap();
        let resources = ResourceRegistry::default();
        let service = PluginFileService::new(resources.clone());
        let owner = owner();
        let parent_root = service
            .register(
                PluginFileGrant::directory(
                    owner.clone(),
                    Dir::open_ambient_dir(temporary.path(), ambient_authority()).unwrap(),
                    PluginFileAccessScope {
                        allow_read: true,
                        ..Default::default()
                    },
                ),
                Arc::new(|| true),
            )
            .await
            .unwrap();
        let task_consumer = consumer(Uuid::new_v4());
        let imported = service
            .adopt_scopes(
                &owner,
                std::slice::from_ref(&parent_root),
                task_consumer.clone(),
            )
            .await
            .unwrap();
        let task_root = imported.get(&parent_root).unwrap().clone();
        let task = service.for_consumer(task_consumer);
        let read = |root_handle| PluginFileOperation::Read {
            root_handle,
            relative_path: "approved.txt".to_owned(),
            offset: 0,
        };
        assert!(
            service
                .invoke(&owner, &read(parent_root.clone()), Arc::new(|| true))
                .await
                .is_ok()
        );
        assert!(
            task.invoke(&owner, &read(task_root.clone()), Arc::new(|| true))
                .await
                .is_ok()
        );
        assert_eq!(
            task.invoke(&owner, &read(parent_root.clone()), Arc::new(|| true))
                .await,
            Err(PluginApiErrorCode::NotFound)
        );
        assert_eq!(
            service
                .adopt_scopes(
                    &owner,
                    std::slice::from_ref(&task_root),
                    consumer(Uuid::new_v4()),
                )
                .await,
            Err(PluginApiErrorCode::NotFound)
        );
        task.resources
            .stop_plugin_generation(&owner.plugin_id, owner.generation)
            .await
            .unwrap();
        assert!(
            service
                .invoke(&owner, &read(parent_root), Arc::new(|| true))
                .await
                .is_ok()
        );
        resources.stop_plugin(&owner.plugin_id).await.unwrap();
    }

    #[tokio::test]
    async fn directory_write_is_chunked_create_only_and_owned_by_registry_cleanup() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = Dir::open_ambient_dir(temporary.path(), ambient_authority()).unwrap();
        let resources = ResourceRegistry::default();
        let service = PluginFileService::new(resources.clone());
        let owner = owner();
        let root = service
            .register(
                PluginFileGrant::directory(
                    owner.clone(),
                    directory,
                    PluginFileAccessScope {
                        allow_read: true,
                        allow_write: true,
                        ..Default::default()
                    },
                ),
                Arc::new(|| true),
            )
            .await
            .unwrap();
        let fence: ResourceFence = Arc::new(|| true);
        let first = vec![b'a'; PLUGIN_FILE_MAX_CHUNK_BYTES as usize];
        let fingerprint = match service
            .invoke(
                &owner,
                &PluginFileOperation::Write {
                    root_handle: root.clone(),
                    relative_path: "large.bin".to_owned(),
                    expected_fingerprint: None,
                    offset: 0,
                    data_base64: BASE64.encode(&first),
                    final_size: None,
                },
                fence.clone(),
            )
            .await
            .unwrap()
        {
            PluginFileResult::Written { fingerprint } => fingerprint,
            _ => unreachable!(),
        };
        let second = b"tail";
        let fingerprint = match service
            .invoke(
                &owner,
                &PluginFileOperation::Write {
                    root_handle: root.clone(),
                    relative_path: "large.bin".to_owned(),
                    expected_fingerprint: Some(fingerprint),
                    offset: u64::from(PLUGIN_FILE_MAX_CHUNK_BYTES),
                    data_base64: BASE64.encode(second),
                    final_size: Some(u64::from(PLUGIN_FILE_MAX_CHUNK_BYTES) + second.len() as u64),
                },
                fence.clone(),
            )
            .await
            .unwrap()
        {
            PluginFileResult::Written { fingerprint } => fingerprint,
            _ => unreachable!(),
        };
        assert_eq!(fingerprint.len(), 64);
        let read = service
            .invoke(
                &owner,
                &PluginFileOperation::Read {
                    root_handle: root.clone(),
                    relative_path: "large.bin".to_owned(),
                    offset: u64::from(PLUGIN_FILE_MAX_CHUNK_BYTES),
                },
                fence.clone(),
            )
            .await
            .unwrap();
        assert!(
            matches!(read, PluginFileResult::Read { ref data_base64, eof: true, .. } if BASE64.decode(data_base64).unwrap().as_slice() == second)
        );
        assert_eq!(
            service
                .invoke(
                    &owner,
                    &PluginFileOperation::Write {
                        root_handle: root.clone(),
                        relative_path: "large.bin".to_owned(),
                        expected_fingerprint: None,
                        offset: 0,
                        data_base64: BASE64.encode(b"x"),
                        final_size: None,
                    },
                    fence,
                )
                .await,
            Err(PluginApiErrorCode::Conflict)
        );
        resources.stop_plugin(&owner.plugin_id).await.unwrap();
        assert!(matches!(
            service.entry(&owner, &root),
            Err(PluginApiErrorCode::NotFound)
        ));
    }

    #[tokio::test]
    async fn approval_fence_revocation_fails_closed_and_releases_the_root() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = Dir::open_ambient_dir(temporary.path(), ambient_authority()).unwrap();
        let resources = ResourceRegistry::default();
        let service = PluginFileService::new(resources);
        let owner = owner();
        let authorized = Arc::new(AtomicBool::new(true));
        let fence: ResourceFence = {
            let authorized = authorized.clone();
            Arc::new(move || authorized.load(Ordering::Acquire))
        };
        let root = service
            .register(
                PluginFileGrant::directory(
                    owner.clone(),
                    directory,
                    PluginFileAccessScope {
                        allow_read: true,
                        ..Default::default()
                    },
                ),
                fence.clone(),
            )
            .await
            .unwrap();
        authorized.store(false, Ordering::Release);
        assert_eq!(
            service
                .invoke(
                    &owner,
                    &PluginFileOperation::Read {
                        root_handle: root.clone(),
                        relative_path: "missing.txt".to_owned(),
                        offset: 0,
                    },
                    fence,
                )
                .await,
            Err(PluginApiErrorCode::Revoked)
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(matches!(
            service.entry(&owner, &root),
            Err(PluginApiErrorCode::NotFound)
        ));
    }

    #[tokio::test]
    async fn selected_file_allows_in_place_edits_but_rejects_a_replaced_object() {
        let temporary = tempfile::tempdir().unwrap();
        let selected = temporary.path().join("selected.txt");
        std::fs::write(&selected, b"before").unwrap();
        let parent = Dir::open_ambient_dir(temporary.path(), ambient_authority()).unwrap();
        let file = parent.open("selected.txt").unwrap();
        let owner = owner();
        let resources = ResourceRegistry::default();
        let service = PluginFileService::new(resources);
        let root = service
            .register(
                PluginFileGrant::selected_file(
                    owner.clone(),
                    parent,
                    OsString::from("selected.txt"),
                    file,
                    PluginFileAccessScope {
                        allow_read: true,
                        ..Default::default()
                    },
                )
                .unwrap(),
                Arc::new(|| true),
            )
            .await
            .unwrap();
        std::fs::write(&selected, b"changed in place").unwrap();
        let read = service
            .invoke(
                &owner,
                &PluginFileOperation::Read {
                    root_handle: root.clone(),
                    relative_path: String::new(),
                    offset: 0,
                },
                Arc::new(|| true),
            )
            .await
            .unwrap();
        assert!(
            matches!(read, PluginFileResult::Read { ref data_base64, .. } if BASE64.decode(data_base64).unwrap() == b"changed in place")
        );
        let replacement = temporary.path().join("replacement.txt");
        std::fs::write(&replacement, b"changed in place").unwrap();
        std::fs::rename(&replacement, &selected).unwrap();
        assert!(matches!(
            service
                .invoke(
                    &owner,
                    &PluginFileOperation::Read {
                        root_handle: root,
                        relative_path: String::new(),
                        offset: 0,
                    },
                    Arc::new(|| true),
                )
                .await,
            Err(PluginApiErrorCode::Conflict)
        ));
    }
}
