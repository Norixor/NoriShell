//! Opaque local filesystem capabilities used by SFTP transfers.
//!
//! This module owns platform file handles, identity checks, handle-relative
//! traversal, and temporary-file commit semantics. The parent SFTP service
//! owns capability registries, transfer orchestration, generations, and IPC.

use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(unix)]
use std::{
    ffi::{CStr, CString, OsStr},
    fs::{File, OpenOptions},
    io,
    mem::MaybeUninit,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    sync::atomic::Ordering,
};

use norishell_core_api as wire;

use super::{MAX_LABEL_BYTES, TransferFailureCode, is_unsafe_display_character};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::{
    NativeDirectoryStream, NativeFileIdentity, NativeLocalBoundary, NativeLocalDirectoryCapability,
};

#[cfg(unix)]
use super::{CleanupOutcome, TransferId};

#[derive(Debug, Clone)]
pub(super) struct LocalBoundary {
    pub(super) kind: wire::SftpLocalBoundaryKind,
    pub(super) display_name: String,
    pub(super) size: Option<u64>,
    pub(super) capability: Arc<LocalBoundaryCapability>,
}

#[derive(Debug, Clone)]
pub(super) struct LocalDirectoryCapability {
    pub(super) revision: u64,
    pub(super) capability: Arc<LocalDirectoryCapabilityHandle>,
    pub(super) rememberable_path: Option<String>,
    pub(super) revoked: Arc<AtomicBool>,
    pub(super) expires_at: std::time::Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalObjectKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalObjectIdentity {
    pub(super) kind: LocalObjectKind,
    pub(super) device: u64,
    pub(super) object: u64,
    pub(super) size: u64,
    pub(super) modified_seconds: i64,
    pub(super) modified_nanoseconds: i64,
}

#[derive(Debug, Clone)]
pub(super) struct LocalDirectoryEntryReference {
    pub(super) directory_ref: String,
    pub(super) directory_revision: u64,
    pub(super) name: Vec<u8>,
    pub(super) identity: LocalObjectIdentity,
    pub(super) expires_at: std::time::Instant,
}

#[derive(Debug)]
pub(super) struct LocalDirectoryCursor {
    pub(super) directory_ref: String,
    pub(super) directory_revision: u64,
    #[cfg(any(unix, windows))]
    pub(super) stream: NativeDirectoryStream,
    #[cfg(not(any(unix, windows)))]
    pub(super) unsupported: (),
    pub(super) expires_at: std::time::Instant,
}

#[derive(Debug)]
pub(super) enum LocalDirectoryCapabilityHandle {
    #[cfg(any(unix, windows))]
    Native(NativeLocalDirectoryCapability),
    #[cfg(not(any(unix, windows)))]
    Unsupported,
}

#[cfg(unix)]
#[derive(Debug)]
pub(super) struct NativeLocalDirectoryCapability {
    pub(super) directory: File,
    pub(super) device: u64,
    pub(super) inode: u64,
}

#[cfg(unix)]
#[derive(Debug)]
pub(super) struct NativeDirectoryStream(pub(super) *mut libc::DIR);

#[cfg(unix)]
type UnixDirectoryPage = (
    Vec<(Vec<u8>, NativeFileIdentity)>,
    Option<NativeDirectoryStream>,
);

// SAFETY: each stream is removed from the cursor map before use, so it has one
// owner and is never iterated concurrently. Drop closes it on the owning task.
#[cfg(unix)]
unsafe impl Send for NativeDirectoryStream {}

#[cfg(unix)]
impl Drop for NativeDirectoryStream {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { libc::closedir(self.0) };
        }
    }
}

#[derive(Debug)]
pub(super) enum LocalBoundaryCapability {
    #[cfg(any(unix, windows))]
    Native(NativeLocalBoundary),
    #[cfg(not(any(unix, windows)))]
    Unsupported,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NativeFileIdentity {
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) mode: u32,
    pub(super) size: u64,
    pub(super) modified_seconds: i64,
    pub(super) modified_nanoseconds: i64,
}

#[cfg(unix)]
#[derive(Debug)]
pub(super) struct NativeLocalBoundary {
    pub(super) parent: File,
    pub(super) parent_device: u64,
    pub(super) parent_inode: u64,
    pub(super) name: CString,
    pub(super) upload_source: Option<File>,
    pub(super) upload_identity: Option<NativeFileIdentity>,
}

#[cfg(unix)]
impl NativeFileIdentity {
    pub(super) fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;

        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }

    pub(super) fn is_regular(&self) -> bool {
        self.mode & u32::from(libc::S_IFMT) == u32::from(libc::S_IFREG)
    }

    pub(super) fn is_directory(&self) -> bool {
        self.mode & u32::from(libc::S_IFMT) == u32::from(libc::S_IFDIR)
    }

    pub(super) fn is_symlink(&self) -> bool {
        self.mode & u32::from(libc::S_IFMT) == u32::from(libc::S_IFLNK)
    }
}

#[cfg(any(unix, windows))]
impl LocalObjectIdentity {
    pub(super) fn from_native(identity: &NativeFileIdentity) -> Self {
        let kind = if identity.is_regular() {
            LocalObjectKind::File
        } else if identity.is_directory() {
            LocalObjectKind::Directory
        } else if identity.is_symlink() {
            LocalObjectKind::Symlink
        } else {
            LocalObjectKind::Other
        };
        Self {
            kind,
            device: identity.device,
            object: identity.inode,
            size: identity.size,
            modified_seconds: identity.modified_seconds,
            modified_nanoseconds: identity.modified_nanoseconds,
        }
    }

    pub(super) fn matches_native(&self, identity: &NativeFileIdentity) -> bool {
        self == &Self::from_native(identity)
    }
}

#[cfg(unix)]
impl NativeLocalDirectoryCapability {
    pub(super) fn register(selected: &Path) -> io::Result<Self> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(selected)?;
        let identity = NativeFileIdentity::from_metadata(&directory.metadata()?);
        if !identity.is_directory() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local capability is not a directory",
            ));
        }
        Ok(Self {
            directory,
            device: identity.device,
            inode: identity.inode,
        })
    }

    pub(super) fn verify(&self) -> io::Result<()> {
        let identity = NativeFileIdentity::from_metadata(&self.directory.metadata()?);
        if identity.device != self.device
            || identity.inode != self.inode
            || !identity.is_directory()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local directory capability changed",
            ));
        }
        Ok(())
    }

    pub(super) fn entry_identity(&self, name: &CStr) -> io::Result<NativeFileIdentity> {
        self.verify()?;
        let mut stat = MaybeUninit::<libc::stat>::zeroed();
        let result = unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        let stat = unsafe { stat.assume_init() };
        let (modified_seconds, modified_nanoseconds) = (stat.st_mtime, stat.st_mtime_nsec);
        Ok(NativeFileIdentity {
            device: u64::try_from(stat.st_dev).unwrap_or(0),
            inode: stat.st_ino,
            mode: u32::from(stat.st_mode),
            size: u64::try_from(stat.st_size).unwrap_or(0),
            modified_seconds,
            modified_nanoseconds,
        })
    }

    pub(super) fn list_page(
        &self,
        stream: Option<NativeDirectoryStream>,
        page_size: usize,
        revoked: &AtomicBool,
    ) -> io::Result<UnixDirectoryPage> {
        self.verify()?;
        if revoked.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local directory capability revoked",
            ));
        }
        let stream = match stream {
            Some(stream) => stream,
            None => {
                let descriptor = unsafe {
                    libc::openat(
                        self.directory.as_raw_fd(),
                        c".".as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if descriptor < 0 {
                    return Err(io::Error::last_os_error());
                }
                let directory = unsafe { libc::fdopendir(descriptor) };
                if directory.is_null() {
                    unsafe { libc::close(descriptor) };
                    return Err(io::Error::last_os_error());
                }
                NativeDirectoryStream(directory)
            }
        };
        let mut entries = Vec::new();
        while entries.len() < page_size {
            if revoked.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local directory capability revoked",
                ));
            }
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let identity = self.entry_identity(name)?;
            entries.push((name.to_bytes().to_vec(), identity));
        }
        let next_stream = (entries.len() == page_size).then_some(stream);
        Ok((entries, next_stream))
    }

    pub(super) fn derive_upload_boundary(
        &self,
        name_bytes: &[u8],
        expected: &LocalObjectIdentity,
        expected_bytes: u64,
    ) -> io::Result<LocalBoundary> {
        let name = CString::new(name_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid local name"))?;
        let observed = self.entry_identity(&name)?;
        if expected.kind != LocalObjectKind::File
            || !observed.is_regular()
            || !expected.matches_native(&observed)
            || observed.size != expected_bytes
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local source precondition changed",
            ));
        }
        let source = openat_file(
            &self.directory,
            &name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0,
        )?;
        let opened = NativeFileIdentity::from_metadata(&source.metadata()?);
        if opened != observed || !opened.is_regular() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local source identity changed while opening",
            ));
        }
        Ok(LocalBoundary {
            kind: wire::SftpLocalBoundaryKind::UploadSource,
            display_name: safe_local_name_display(name_bytes),
            size: Some(expected_bytes),
            capability: Arc::new(LocalBoundaryCapability::Native(NativeLocalBoundary {
                parent: self.directory.try_clone()?,
                parent_device: self.device,
                parent_inode: self.inode,
                name,
                upload_source: Some(source),
                upload_identity: Some(opened),
            })),
        })
    }

    pub(super) fn derive_download_boundary(&self, name_bytes: &[u8]) -> io::Result<LocalBoundary> {
        let name = CString::new(name_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid local name"))?;
        if name.to_bytes().is_empty()
            || name.to_bytes() == b"."
            || name.to_bytes() == b".."
            || name.to_bytes().contains(&b'/')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid local target name",
            ));
        }
        match self.entry_identity(&name) {
            Ok(identity) if !identity.is_regular() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local target is not a regular file",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(LocalBoundary {
            kind: wire::SftpLocalBoundaryKind::DownloadTarget,
            display_name: safe_local_name_display(name_bytes),
            size: None,
            capability: Arc::new(LocalBoundaryCapability::Native(NativeLocalBoundary {
                parent: self.directory.try_clone()?,
                parent_device: self.device,
                parent_inode: self.inode,
                name,
                upload_source: None,
                upload_identity: None,
            })),
        })
    }

    pub(super) fn open_child(
        &self,
        name_bytes: &[u8],
        expected: &LocalObjectIdentity,
    ) -> io::Result<Self> {
        let name = CString::new(name_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid local name"))?;
        let observed = self.entry_identity(&name)?;
        if expected.kind != LocalObjectKind::Directory
            || !observed.is_directory()
            || !expected.matches_native(&observed)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local child directory precondition changed",
            ));
        }
        let directory = openat_file(
            &self.directory,
            &name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )?;
        let opened = NativeFileIdentity::from_metadata(&directory.metadata()?);
        if opened != observed || !opened.is_directory() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local child directory identity changed while opening",
            ));
        }
        Ok(Self {
            directory,
            device: opened.device,
            inode: opened.inode,
        })
    }

    pub(super) fn create_child(&self, name_bytes: &[u8]) -> io::Result<Self> {
        self.verify()?;
        let name = CString::new(name_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid local name"))?;
        if name.to_bytes().is_empty()
            || name.to_bytes() == b"."
            || name.to_bytes() == b".."
            || name.to_bytes().contains(&b'/')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid local child directory name",
            ));
        }
        let result = unsafe { libc::mkdirat(self.directory.as_raw_fd(), name.as_ptr(), 0o777) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        let identity = self.entry_identity(&name)?;
        if !identity.is_directory() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "created local child is not a directory",
            ));
        }
        self.open_child(
            name.to_bytes(),
            &LocalObjectIdentity::from_native(&identity),
        )
    }
}

#[cfg(any(unix, windows))]
fn local_modified_at_unix_ms(identity: &NativeFileIdentity) -> Option<i64> {
    identity.modified_seconds.checked_mul(1_000)?.checked_add(
        identity
            .modified_nanoseconds
            .checked_div(1_000_000)
            .unwrap_or(0),
    )
}

pub(super) fn safe_local_name_display(bytes: &[u8]) -> String {
    if let Ok(value) = std::str::from_utf8(bytes)
        && !value.chars().any(is_unsafe_display_character)
    {
        return value.chars().take(MAX_LABEL_BYTES).collect();
    }
    bytes
        .iter()
        .take(MAX_LABEL_BYTES / 4)
        .map(|byte| format!("\\x{byte:02x}"))
        .collect()
}

pub(super) fn rememberable_local_directory_path(path: &Path) -> Option<String> {
    // Never derive this from a display name. Canonicalization also prevents a
    // relative registration request from becoming a process-CWD-dependent
    // preference when it is registered again later.
    let canonical = std::fs::canonicalize(path).ok()?;
    let value = canonical.to_str()?.to_owned();
    (value.len() <= 4_096 && !value.contains('\0')).then_some(value)
}

pub(super) fn rememberable_local_child_path(parent: Option<&str>, name: &[u8]) -> Option<String> {
    let parent = parent?;
    let name = std::str::from_utf8(name).ok()?;
    if name.is_empty() || matches!(name, "." | "..") || name.contains('/') || name.contains('\0') {
        return None;
    }
    rememberable_local_directory_path(&Path::new(parent).join(name))
}

#[cfg(any(unix, windows))]
pub(super) fn map_local_entry(
    entry_ref: String,
    name: &[u8],
    identity: &NativeFileIdentity,
) -> wire::SftpLocalDirectoryEntry {
    let kind = if identity.is_regular() {
        wire::SftpLocalEntryKind::File
    } else if identity.is_directory() {
        wire::SftpLocalEntryKind::Directory
    } else if identity.is_symlink() {
        wire::SftpLocalEntryKind::Symlink
    } else {
        wire::SftpLocalEntryKind::Other
    };
    let size = (kind == wire::SftpLocalEntryKind::File).then_some(identity.size);
    let modified_at_unix_ms = local_modified_at_unix_ms(identity);
    wire::SftpLocalDirectoryEntry {
        entry_ref,
        display_name: safe_local_name_display(name),
        kind,
        size,
        modified_at_unix_ms,
    }
}

#[cfg(unix)]
impl NativeLocalBoundary {
    pub(super) fn register(selected: &Path, kind: wire::SftpLocalBoundaryKind) -> io::Result<Self> {
        let name = selected
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
        let name = unix_component(name)?;
        let parent_path = selected
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent_path)?;
        let parent_identity = NativeFileIdentity::from_metadata(&parent.metadata()?);
        if !parent_identity.is_directory() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local boundary parent is not a directory",
            ));
        }
        let (upload_source, upload_identity) = match kind {
            wire::SftpLocalBoundaryKind::UploadSource => {
                let source = openat_file(
                    &parent,
                    &name,
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    0,
                )?;
                let identity = NativeFileIdentity::from_metadata(&source.metadata()?);
                if !identity.is_regular() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "upload source is not a regular file",
                    ));
                }
                (Some(source), Some(identity))
            }
            wire::SftpLocalBoundaryKind::DownloadTarget => {
                if let Some(identity) = openat_identity(&parent, &name)?
                    && !identity.is_regular()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "download target is not a regular file",
                    ));
                }
                (None, None)
            }
        };
        Ok(Self {
            parent,
            parent_device: parent_identity.device,
            parent_inode: parent_identity.inode,
            name,
            upload_source,
            upload_identity,
        })
    }

    pub(super) fn verify_parent(&self) -> io::Result<()> {
        let identity = NativeFileIdentity::from_metadata(&self.parent.metadata()?);
        if identity.device != self.parent_device
            || identity.inode != self.parent_inode
            || !identity.is_directory()
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local boundary parent capability changed",
            ));
        }
        Ok(())
    }

    pub(super) fn open_upload_source(&self, expected_bytes: u64) -> io::Result<File> {
        use std::io::{Seek as _, SeekFrom};

        self.verify_parent()?;
        let source = self
            .upload_source
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an upload source"))?;
        let expected = self.upload_identity.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "missing source identity")
        })?;
        let observed = NativeFileIdentity::from_metadata(&source.metadata()?);
        if &observed != expected || !observed.is_regular() || observed.size != expected_bytes {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "upload source capability changed",
            ));
        }
        let mut source = source.try_clone()?;
        source.seek(SeekFrom::Start(0))?;
        Ok(source)
    }

    pub(super) fn target_identity(&self) -> io::Result<Option<NativeFileIdentity>> {
        self.verify_parent()?;
        let identity = openat_identity(&self.parent, &self.name)?;
        if identity.as_ref().is_some_and(|value| !value.is_regular()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "download target is not a regular file",
            ));
        }
        Ok(identity)
    }

    pub(super) fn temporary_name(&self, transfer_id: &TransferId) -> io::Result<CString> {
        let mut bytes = Vec::with_capacity(self.name.as_bytes().len().saturating_add(160));
        bytes.push(b'.');
        bytes.extend_from_slice(self.name.as_bytes());
        bytes.extend_from_slice(b".norishell-");
        bytes.extend_from_slice(transfer_id.as_str().as_bytes());
        bytes.extend_from_slice(b".part");
        if bytes.len() > 255 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "temporary file name is too long",
            ));
        }
        CString::new(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid temporary name"))
    }

    pub(super) fn create_temporary(&self, temporary_name: &CStr) -> io::Result<File> {
        self.verify_parent()?;
        openat_file(
            &self.parent,
            temporary_name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    }

    pub(super) fn remove_temporary(&self, temporary_name: &CStr) -> CleanupOutcome {
        if self.verify_parent().is_err() {
            return CleanupOutcome::Residual {
                opaque_location: "local boundary parent capability changed".to_owned(),
            };
        }
        let result = unsafe { libc::unlinkat(self.parent.as_raw_fd(), temporary_name.as_ptr(), 0) };
        if result == 0 {
            CleanupOutcome::Cleaned
        } else {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                CleanupOutcome::Cleaned
            } else {
                CleanupOutcome::Residual {
                    opaque_location: format!(
                        "local temporary file {}",
                        temporary_name.to_string_lossy()
                    ),
                }
            }
        }
    }

    pub(super) fn commit_download(
        &self,
        temporary_name: &CStr,
        temporary_identity: &NativeFileIdentity,
        target_identity: Option<&NativeFileIdentity>,
    ) -> Result<(bool, bool, u64), TransferFailureCode> {
        self.verify_parent()
            .map_err(|_| TransferFailureCode::PermissionDenied)?;
        let observed_temporary = openat_identity(&self.parent, temporary_name)
            .map_err(|_| TransferFailureCode::PermissionDenied)?
            .ok_or(TransferFailureCode::PermissionDenied)?;
        if &observed_temporary != temporary_identity || !observed_temporary.is_regular() {
            return Err(TransferFailureCode::PermissionDenied);
        }
        let observed_target = openat_identity(&self.parent, &self.name)
            .map_err(|_| TransferFailureCode::PermissionDenied)?;
        if observed_target.as_ref() != target_identity {
            return Err(TransferFailureCode::TargetExists);
        }
        let (atomic_no_replace, atomic_replace) = if target_identity.is_some() {
            let result = unsafe {
                libc::renameat(
                    self.parent.as_raw_fd(),
                    temporary_name.as_ptr(),
                    self.parent.as_raw_fd(),
                    self.name.as_ptr(),
                )
            };
            if result != 0 {
                return Err(TransferFailureCode::PermissionDenied);
            }
            (false, true)
        } else {
            let result = unsafe {
                libc::linkat(
                    self.parent.as_raw_fd(),
                    temporary_name.as_ptr(),
                    self.parent.as_raw_fd(),
                    self.name.as_ptr(),
                    0,
                )
            };
            if result != 0 {
                return Err(TransferFailureCode::TargetExists);
            }
            if unsafe { libc::unlinkat(self.parent.as_raw_fd(), temporary_name.as_ptr(), 0) } != 0 {
                return Err(TransferFailureCode::CleanupIncomplete);
            }
            (true, false)
        };
        self.verify_parent()
            .map_err(|_| TransferFailureCode::PermissionDenied)?;
        let final_identity = openat_identity(&self.parent, &self.name)
            .map_err(|_| TransferFailureCode::PermissionDenied)?
            .ok_or(TransferFailureCode::PermissionDenied)?;
        if !final_identity.is_regular()
            || final_identity.device != temporary_identity.device
            || final_identity.inode != temporary_identity.inode
        {
            return Err(TransferFailureCode::PermissionDenied);
        }
        Ok((atomic_no_replace, atomic_replace, final_identity.size))
    }
}

#[cfg(unix)]
fn unix_component(value: &OsStr) -> io::Result<CString> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid local file component",
        ));
    }
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid local file component"))
}

#[cfg(unix)]
fn openat_file(parent: &File, name: &CStr, flags: i32, mode: libc::mode_t) -> io::Result<File> {
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags,
            libc::c_uint::from(mode),
        )
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[cfg(unix)]
pub(super) fn openat_identity(
    parent: &File,
    name: &CStr,
) -> io::Result<Option<NativeFileIdentity>> {
    match openat_file(
        parent,
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        0,
    ) {
        Ok(file) => Ok(Some(NativeFileIdentity::from_metadata(&file.metadata()?))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

impl LocalBoundary {
    #[cfg(any(unix, windows))]
    pub(super) fn native(&self) -> Result<&NativeLocalBoundary, TransferFailureCode> {
        match self.capability.as_ref() {
            LocalBoundaryCapability::Native(capability) => Ok(capability),
        }
    }

    #[cfg(not(unix))]
    pub(super) fn unsupported(&self) -> Result<(), TransferFailureCode> {
        let _ = self;
        Err(TransferFailureCode::PermissionDenied)
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn rememberable_paths_require_raw_utf8_names_and_never_use_display_escapes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("child")).unwrap();
        let parent = rememberable_local_directory_path(root.path()).unwrap();

        assert_eq!(
            rememberable_local_child_path(Some(&parent), b"child"),
            Some(
                std::fs::canonicalize(root.path().join("child"))
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            )
        );
        assert_eq!(
            rememberable_local_child_path(Some(&parent), b"\xffchild"),
            None
        );
        assert_eq!(
            rememberable_local_child_path(Some(&parent), b"../escape"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_download_no_replace_commit_preserves_a_racing_target() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.bin");
        let boundary =
            NativeLocalBoundary::register(&target, wire::SftpLocalBoundaryKind::DownloadTarget)
                .unwrap();
        let transfer_id = TransferId::parse("transfer-race").unwrap();
        let temporary = boundary.temporary_name(&transfer_id).unwrap();
        let mut file = boundary.create_temporary(&temporary).unwrap();
        file.write_all(b"new").unwrap();
        file.sync_all().unwrap();
        let temporary_identity = NativeFileIdentity::from_metadata(&file.metadata().unwrap());
        std::fs::write(&target, b"old").unwrap();

        assert_eq!(
            boundary.commit_download(&temporary, &temporary_identity, None),
            Err(TransferFailureCode::TargetExists)
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(matches!(
            openat_identity(&boundary.parent, &temporary),
            Ok(Some(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn local_download_confirmed_replace_uses_same_directory_atomic_rename() {
        use std::io::Write as _;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.bin");
        std::fs::write(&target, b"old").unwrap();
        let boundary =
            NativeLocalBoundary::register(&target, wire::SftpLocalBoundaryKind::DownloadTarget)
                .unwrap();
        let target_identity = boundary.target_identity().unwrap().unwrap();
        let temporary = boundary
            .temporary_name(&TransferId::parse("transfer-replace").unwrap())
            .unwrap();
        let mut file = boundary.create_temporary(&temporary).unwrap();
        file.write_all(b"new-content").unwrap();
        file.sync_all().unwrap();
        let temporary_identity = NativeFileIdentity::from_metadata(&file.metadata().unwrap());

        assert_eq!(
            boundary.commit_download(&temporary, &temporary_identity, Some(&target_identity),),
            Ok((false, true, 11))
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"new-content");
        assert!(
            openat_identity(&boundary.parent, &temporary)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_boundary_dirfd_does_not_follow_a_replaced_parent_path() {
        use std::io::Write as _;

        let root = tempfile::tempdir().unwrap();
        let selected_parent = root.path().join("selected");
        let retained_parent = root.path().join("retained");
        std::fs::create_dir(&selected_parent).unwrap();
        let selected_target = selected_parent.join("target.bin");
        let boundary = NativeLocalBoundary::register(
            &selected_target,
            wire::SftpLocalBoundaryKind::DownloadTarget,
        )
        .unwrap();
        std::fs::rename(&selected_parent, &retained_parent).unwrap();
        std::fs::create_dir(&selected_parent).unwrap();

        let temporary = boundary
            .temporary_name(&TransferId::parse("transfer-parent-race").unwrap())
            .unwrap();
        let mut file = boundary.create_temporary(&temporary).unwrap();
        file.write_all(b"capability-owned").unwrap();
        file.sync_all().unwrap();
        let temporary_identity = NativeFileIdentity::from_metadata(&file.metadata().unwrap());
        boundary
            .commit_download(&temporary, &temporary_identity, None)
            .unwrap();

        assert_eq!(
            std::fs::read(retained_parent.join("target.bin")).unwrap(),
            b"capability-owned"
        );
        assert!(!selected_parent.join("target.bin").exists());
    }

    #[cfg(unix)]
    #[test]
    fn upload_boundary_keeps_the_opened_inode_when_path_is_replaced() {
        use std::io::Read as _;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bin");
        let retained = directory.path().join("retained.bin");
        std::fs::write(&source, b"original").unwrap();
        let boundary =
            NativeLocalBoundary::register(&source, wire::SftpLocalBoundaryKind::UploadSource)
                .unwrap();
        std::fs::rename(&source, &retained).unwrap();
        std::fs::write(&source, b"replacement").unwrap();

        let mut opened = boundary.open_upload_source(8).unwrap();
        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original");
    }

    #[cfg(unix)]
    #[test]
    fn local_directory_capability_pages_without_collecting_and_stops_when_revoked() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c", "d"] {
            std::fs::write(directory.path().join(name), name.as_bytes()).unwrap();
        }
        let capability = NativeLocalDirectoryCapability::register(directory.path()).unwrap();
        let revoked = AtomicBool::new(false);
        let (first, cursor) = capability.list_page(None, 2, &revoked).unwrap();
        assert_eq!(first.len(), 2);
        let (second, _) = capability.list_page(cursor, 2, &revoked).unwrap();
        assert_eq!(second.len(), 2);
        let names = first
            .into_iter()
            .chain(second)
            .map(|(name, _)| name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), 4);

        revoked.store(true, Ordering::Release);
        assert!(capability.list_page(None, 1, &revoked).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_entry_identity_change_directory_and_symlink_fail_closed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("file"), b"old").unwrap();
        std::fs::create_dir(directory.path().join("child")).unwrap();
        symlink("file", directory.path().join("link")).unwrap();
        let capability = NativeLocalDirectoryCapability::register(directory.path()).unwrap();

        let file_name = CString::new("file").unwrap();
        let original = capability.entry_identity(&file_name).unwrap();
        let original = LocalObjectIdentity::from_native(&original);
        std::fs::rename(
            directory.path().join("file"),
            directory.path().join("old-file"),
        )
        .unwrap();
        std::fs::write(directory.path().join("file"), b"new").unwrap();
        assert!(
            capability
                .derive_upload_boundary(b"file", &original, 3)
                .is_err()
        );

        for name in ["child", "link"] {
            let name = CString::new(name).unwrap();
            let identity = capability.entry_identity(&name).unwrap();
            assert!(
                capability
                    .derive_upload_boundary(
                        name.to_bytes(),
                        &LocalObjectIdentity::from_native(&identity),
                        identity.size,
                    )
                    .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn child_directory_is_opened_relative_to_parent_handle() {
        let directory = tempfile::tempdir().unwrap();
        let child_path = directory.path().join("child");
        std::fs::create_dir(&child_path).unwrap();
        std::fs::write(child_path.join("inside"), b"data").unwrap();
        let parent = NativeLocalDirectoryCapability::register(directory.path()).unwrap();
        let child_name = CString::new("child").unwrap();
        let child_identity = parent.entry_identity(&child_name).unwrap();
        let child = parent
            .open_child(
                child_name.to_bytes(),
                &LocalObjectIdentity::from_native(&child_identity),
            )
            .unwrap();
        let (entries, next) = child.list_page(None, 8, &AtomicBool::new(false)).unwrap();
        assert!(next.is_none());
        assert_eq!(entries[0].0, b"inside");
    }

    #[cfg(unix)]
    #[test]
    fn child_directory_creation_is_relative_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let parent = NativeLocalDirectoryCapability::register(directory.path()).unwrap();
        let child = parent.create_child(b"copied-tree").unwrap();
        assert!(directory.path().join("copied-tree").is_dir());
        assert!(child.verify().is_ok());
        assert!(parent.create_child(b"copied-tree").is_err());
        assert!(parent.create_child(b"../escape").is_err());
    }
}
