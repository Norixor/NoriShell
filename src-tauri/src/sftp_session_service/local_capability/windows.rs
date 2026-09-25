//! Windows local browsing and upload capabilities. Enumeration and child opens
//! remain relative to retained handles even if an ancestor is renamed.
use super::{
    LocalBoundary, LocalBoundaryCapability, LocalObjectIdentity, LocalObjectKind,
    safe_local_name_display,
};
use cap_fs_ext::{
    DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _, OpenOptionsMaybeDirExt as _,
};
use cap_std::fs::{Dir, OpenOptions as CapOpenOptions, OpenOptionsExt as _};
use norishell_core_api as wire;
use std::{
    collections::VecDeque,
    ffi::CStr,
    fs::{File, OpenOptions},
    io::{self, Seek as _, SeekFrom},
    os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use windows_sys::Win32::{Foundation::ERROR_NO_MORE_FILES, Storage::FileSystem::*};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in super::super) struct NativeFileIdentity {
    pub(in super::super) device: u64,
    pub(in super::super) inode: u64,
    pub(in super::super) size: u64,
    pub(in super::super) modified_seconds: i64,
    pub(in super::super) modified_nanoseconds: i64,
    attributes: u32,
}
impl NativeFileIdentity {
    fn from_file(file: &File) -> io::Result<Self> {
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: the live File owns the handle and the output is correctly sized.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let info = unsafe { info.assume_init() };
        let modified = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime);
        Ok(Self::new(
            u64::from(info.dwVolumeSerialNumber),
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            modified as i64,
            info.dwFileAttributes,
        ))
    }
    fn new(device: u64, inode: u64, size: u64, modified: i64, attributes: u32) -> Self {
        let ticks = modified.saturating_sub(116_444_736_000_000_000);
        Self {
            device,
            inode,
            size,
            modified_seconds: ticks.div_euclid(10_000_000),
            modified_nanoseconds: ticks.rem_euclid(10_000_000) * 100,
            attributes,
        }
    }
    pub(in super::super) fn is_symlink(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    pub(in super::super) fn is_directory(&self) -> bool {
        !self.is_symlink() && self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }
    pub(in super::super) fn is_regular(&self) -> bool {
        !self.is_symlink()
            && self.attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_DEVICE) == 0
    }
}

fn component(bytes: &[u8]) -> io::Result<&str> {
    let value =
        std::str::from_utf8(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'
                )
        })
        || value.ends_with(['.', ' '])
    {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let base = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        base.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    }) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    Ok(value)
}

#[derive(Debug)]
pub(in super::super) struct NativeLocalDirectoryCapability {
    directory: Dir,
    identity: NativeFileIdentity,
}
#[derive(Debug)]
pub(in super::super) struct NativeDirectoryStream {
    file: File,
    pending: VecDeque<(Vec<u8>, NativeFileIdentity)>,
    ended: bool,
}
type DirectoryPage = (
    Vec<(Vec<u8>, NativeFileIdentity)>,
    Option<NativeDirectoryStream>,
);

impl NativeLocalDirectoryCapability {
    pub(in super::super) fn register(selected: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(selected)?;
        Self::from_file(file)
    }
    fn from_file(file: File) -> io::Result<Self> {
        let identity = NativeFileIdentity::from_file(&file)?;
        if !identity.is_directory() {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(Self {
            directory: Dir::from_std_file(file),
            identity,
        })
    }
    fn open_entry(&self, name: &[u8], read: bool) -> io::Result<File> {
        let mut options = CapOpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .maybe_dir(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        if !read {
            options.access_mode(FILE_READ_ATTRIBUTES);
        }
        let file = self
            .directory
            .open_with(component(name)?, &options)?
            .into_std();
        if NativeFileIdentity::from_file(&file)?.is_symlink() {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(file)
    }
    pub(in super::super) fn entry_identity(&self, name: &CStr) -> io::Result<NativeFileIdentity> {
        NativeFileIdentity::from_file(&self.open_entry(name.to_bytes(), false)?)
    }
    pub(in super::super) fn list_page(
        &self,
        stream: Option<NativeDirectoryStream>,
        page_size: usize,
        revoked: &AtomicBool,
    ) -> io::Result<DirectoryPage> {
        let mut stream = match stream {
            Some(stream) => stream,
            None => NativeDirectoryStream {
                file: self.directory.open_dir_nofollow(".")?.into_std_file(),
                pending: VecDeque::new(),
                ended: false,
            },
        };
        let mut entries = Vec::new();
        while entries.len() < page_size {
            if revoked.load(Ordering::Acquire) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            if let Some(entry) = stream.pending.pop_front() {
                entries.push(entry);
                continue;
            }
            if stream.ended {
                break;
            }
            // u64 storage keeps FILE_ID_BOTH_DIR_INFO aligned. Windows reports
            // names and identity without opening protected children of C:\.
            let mut buffer = vec![0_u64; 8192];
            let result = unsafe {
                GetFileInformationByHandleEx(
                    stream.file.as_raw_handle(),
                    FileIdBothDirectoryInfo,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                )
            };
            if result == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    stream.ended = true;
                    continue;
                }
                return Err(error);
            }
            let mut offset = 0;
            loop {
                let header_size = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                if offset + header_size > buffer.len() * 8 {
                    return Err(io::Error::from(io::ErrorKind::InvalidData));
                }
                let item = unsafe {
                    &*buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_ID_BOTH_DIR_INFO>()
                };
                let bytes = item.FileNameLength as usize;
                if bytes % 2 != 0 || offset + header_size + bytes > buffer.len() * 8 {
                    return Err(io::Error::from(io::ErrorKind::InvalidData));
                }
                let name = unsafe { std::slice::from_raw_parts(item.FileName.as_ptr(), bytes / 2) };
                if let Ok(name) = String::from_utf16(name)
                    && !matches!(name.as_str(), "." | "..")
                {
                    stream.pending.push_back((
                        name.into_bytes(),
                        NativeFileIdentity::new(
                            self.identity.device,
                            item.FileId as u64,
                            item.EndOfFile as u64,
                            item.LastWriteTime,
                            item.FileAttributes,
                        ),
                    ));
                }
                if item.NextEntryOffset == 0 {
                    break;
                }
                if (item.NextEntryOffset as usize) < header_size || item.NextEntryOffset % 8 != 0 {
                    return Err(io::Error::from(io::ErrorKind::InvalidData));
                }
                offset += item.NextEntryOffset as usize;
            }
        }
        let next = (!stream.ended || !stream.pending.is_empty()).then_some(stream);
        Ok((entries, next))
    }
    pub(in super::super) fn open_child(
        &self,
        name: &[u8],
        expected: &LocalObjectIdentity,
    ) -> io::Result<Self> {
        let file = self.open_entry(name, true)?;
        let observed = NativeFileIdentity::from_file(&file)?;
        // Directory enumeration reports EndOfFile as zero, while opening the
        // same directory can report its allocated index size. Contents can
        // also change between listing and navigation. The volume and file ID
        // identify the selected directory without relying on that metadata.
        if expected.kind != LocalObjectKind::Directory
            || !observed.is_directory()
            || expected.device != observed.device
            || expected.object != observed.inode
        {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Self::from_file(file)
    }
    pub(in super::super) fn derive_upload_boundary(
        &self,
        name: &[u8],
        expected: &LocalObjectIdentity,
        expected_bytes: u64,
    ) -> io::Result<LocalBoundary> {
        let source = self.open_entry(name, true)?;
        let identity = NativeFileIdentity::from_file(&source)?;
        if !identity.is_regular()
            || !expected.matches_native(&identity)
            || identity.size != expected_bytes
        {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(LocalBoundary {
            kind: wire::SftpLocalBoundaryKind::UploadSource,
            display_name: safe_local_name_display(name),
            size: Some(expected_bytes),
            capability: Arc::new(LocalBoundaryCapability::Native(NativeLocalBoundary {
                upload_source: source,
                upload_identity: Some(identity),
            })),
        })
    }
}

#[derive(Debug)]
pub(in super::super) struct NativeLocalBoundary {
    upload_source: File,
    pub(in super::super) upload_identity: Option<NativeFileIdentity>,
}
impl NativeLocalBoundary {
    pub(in super::super) fn register(
        selected: &Path,
        kind: wire::SftpLocalBoundaryKind,
    ) -> io::Result<Self> {
        if kind != wire::SftpLocalBoundaryKind::UploadSource {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        let parent = NativeLocalDirectoryCapability::register(
            selected.parent().ok_or(io::ErrorKind::InvalidInput)?,
        )?;
        let name = selected
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(io::ErrorKind::InvalidInput)?;
        let source = parent.open_entry(name.as_bytes(), true)?;
        let identity = NativeFileIdentity::from_file(&source)?;
        if !identity.is_regular() {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(Self {
            upload_source: source,
            upload_identity: Some(identity),
        })
    }
    pub(in super::super) fn open_upload_source(&self, expected_bytes: u64) -> io::Result<File> {
        let observed = NativeFileIdentity::from_file(&self.upload_source)?;
        if self.upload_identity.as_ref() != Some(&observed)
            || !observed.is_regular()
            || observed.size != expected_bytes
        {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        let mut file = self.upload_source.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn paginated_listing_upload_and_revocation_use_retained_handles() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("中文 file.txt"), b"payload").unwrap();
        std::fs::create_dir(directory.path().join("child")).unwrap();
        let capability = NativeLocalDirectoryCapability::register(directory.path()).unwrap();
        let revoked = AtomicBool::new(false);
        let (mut entries, mut cursor) = capability.list_page(None, 1, &revoked).unwrap();
        while let Some(stream) = cursor {
            let (next, remaining) = capability.list_page(Some(stream), 1, &revoked).unwrap();
            entries.extend(next);
            cursor = remaining;
        }
        assert_eq!(entries.len(), 2);
        let (directory_name, directory_identity) = entries
            .iter()
            .find(|(_, identity)| identity.is_directory())
            .unwrap();
        let child = capability
            .open_child(
                directory_name,
                &LocalObjectIdentity::from_native(directory_identity),
            )
            .unwrap();
        child.list_page(None, 1, &revoked).unwrap();
        let (name, identity) = entries
            .iter()
            .find(|(_, identity)| identity.is_regular())
            .unwrap();
        let boundary = capability
            .derive_upload_boundary(name, &LocalObjectIdentity::from_native(identity), 7)
            .unwrap();
        let mut contents = String::new();
        boundary
            .native()
            .unwrap()
            .open_upload_source(7)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "payload");
        revoked.store(true, Ordering::Release);
        assert!(capability.list_page(None, 1, &revoked).is_err());
    }

    #[test]
    fn program_files_can_be_opened_by_path_and_from_a_root_listing() {
        let Some(path) = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from) else {
            return;
        };
        let direct = NativeLocalDirectoryCapability::register(&path).unwrap();
        direct
            .list_page(None, 256, &AtomicBool::new(false))
            .unwrap();

        let root = NativeLocalDirectoryCapability::register(path.parent().unwrap()).unwrap();
        let mut stream = None;
        let expected_name = path.file_name().unwrap().to_str().unwrap().as_bytes();
        let identity = loop {
            let (entries, next) = root
                .list_page(stream, 256, &AtomicBool::new(false))
                .unwrap();
            if let Some((_, identity)) = entries
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(expected_name))
            {
                break identity;
            }
            stream = next;
            assert!(
                stream.is_some(),
                "Program Files was missing from the root listing"
            );
        };
        let child = root
            .open_child(expected_name, &LocalObjectIdentity::from_native(&identity))
            .unwrap();
        child.list_page(None, 256, &AtomicBool::new(false)).unwrap();
    }

    #[test]
    fn invalid_components_and_non_file_uploads_are_rejected() {
        for name in [
            "..",
            "a/b",
            "a\\b",
            "C:",
            "file:stream",
            "\\\\server",
            "a\0b",
        ] {
            assert!(component(name.as_bytes()).is_err());
        }
        let directory = tempfile::tempdir().unwrap();
        assert!(
            NativeLocalBoundary::register(
                directory.path(),
                wire::SftpLocalBoundaryKind::UploadSource
            )
            .is_err()
        );
        assert!(
            NativeLocalBoundary::register(
                &directory.path().join("new"),
                wire::SftpLocalBoundaryKind::DownloadTarget
            )
            .is_err()
        );
        let identity = NativeFileIdentity::new(
            1,
            1,
            0,
            0,
            FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY,
        );
        assert!(identity.is_symlink());
        assert!(!identity.is_directory());
        assert!(!identity.is_regular());
    }

    #[test]
    fn changed_source_fails_before_upload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        std::fs::write(&path, b"first").unwrap();
        let boundary =
            NativeLocalBoundary::register(&path, wire::SftpLocalBoundaryKind::UploadSource)
                .unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert!(boundary.open_upload_source(5).is_err());
    }

    #[test]
    fn renamed_parent_cannot_retarget_listing_or_upload() {
        let directory = tempfile::tempdir().unwrap();
        let selected = directory.path().join("selected");
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("original.txt"), b"original").unwrap();
        let capability = NativeLocalDirectoryCapability::register(&selected).unwrap();
        std::fs::rename(&selected, directory.path().join("moved")).unwrap();
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("replacement.txt"), b"replacement").unwrap();
        let (entries, _) = capability
            .list_page(None, 16, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, b"original.txt");
        let boundary = capability
            .derive_upload_boundary(
                &entries[0].0,
                &LocalObjectIdentity::from_native(&entries[0].1),
                8,
            )
            .unwrap();
        let mut contents = String::new();
        boundary
            .native()
            .unwrap()
            .open_upload_source(8)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "original");
    }

    #[test]
    fn symbolic_link_upload_and_directory_registration_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("file.txt"), b"secret").unwrap();
        let link = directory.path().join("link");
        if let Err(error) = std::os::windows::fs::symlink_dir(&target, &link) {
            // Windows may require Developer Mode or SeCreateSymbolicLinkPrivilege.
            if error.raw_os_error() == Some(1314) {
                return;
            }
            panic!("failed to create test symlink: {error}");
        }
        assert!(NativeLocalDirectoryCapability::register(&link).is_err());
        let file_link = directory.path().join("link.txt");
        std::os::windows::fs::symlink_file(target.join("file.txt"), &file_link).unwrap();
        assert!(
            NativeLocalBoundary::register(&file_link, wire::SftpLocalBoundaryKind::UploadSource)
                .is_err()
        );
    }
}
