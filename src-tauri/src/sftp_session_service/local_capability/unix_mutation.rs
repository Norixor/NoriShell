//! Unix local file actions inside one registered directory capability.
//!
//! Every system call is relative to the retained directory descriptor, names
//! are single components, symbolic links are never followed, new objects are
//! created exclusively, renames never replace, and a recursive delete walks
//! descriptors it opened itself with `O_NOFOLLOW`.

use std::{
    ffi::{CStr, CString},
    fs::File,
    io,
    os::fd::AsRawFd,
};

use super::{
    LocalBusySet, LocalMutationFailure, LocalObjectIdentity, LocalObjectKind, NativeFileIdentity,
    NativeLocalDirectoryCapability, openat_file,
};

/// Bounds one recursive delete so a huge or adversarial tree cannot pin the
/// local I/O pool indefinitely.
const MAX_RECURSIVE_DELETE_ENTRIES: usize = 100_000;
const MAX_RECURSIVE_DELETE_DEPTH: usize = 64;

pub(super) fn local_mutation_failure(error: &io::Error) -> LocalMutationFailure {
    match error.raw_os_error() {
        Some(libc::EEXIST) => LocalMutationFailure::TargetExists,
        Some(libc::ENOTEMPTY) => LocalMutationFailure::DirectoryNotEmpty,
        Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP) => {
            LocalMutationFailure::PreconditionChanged
        }
        Some(libc::EACCES | libc::EPERM | libc::EROFS) => LocalMutationFailure::PermissionDenied,
        Some(libc::ENAMETOOLONG | libc::EINVAL) => LocalMutationFailure::InvalidName,
        _ if error.kind() == io::ErrorKind::InvalidInput => LocalMutationFailure::InvalidName,
        _ if error.kind() == io::ErrorKind::PermissionDenied => {
            LocalMutationFailure::PermissionDenied
        }
        _ => LocalMutationFailure::Io,
    }
}

/// One name component; `..`, `.`, separators and NUL never reach a syscall.
fn component(name: &[u8]) -> Result<CString, LocalMutationFailure> {
    if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') || name.len() > 255
    {
        return Err(LocalMutationFailure::InvalidName);
    }
    CString::new(name).map_err(|_| LocalMutationFailure::InvalidName)
}

/// Directories are fenced by identity only: their size and timestamps change
/// with their contents. Other objects must still match the listed facts.
fn listed_identity_matches(expected: &LocalObjectIdentity, observed: &NativeFileIdentity) -> bool {
    let current = LocalObjectIdentity::from_native(observed);
    if expected.kind != current.kind
        || expected.device != current.device
        || expected.object != current.object
    {
        return false;
    }
    expected.kind == LocalObjectKind::Directory || expected == &current
}

fn last_failure() -> LocalMutationFailure {
    local_mutation_failure(&io::Error::last_os_error())
}

impl NativeLocalDirectoryCapability {
    fn busy_failure(&self, busy: &LocalBusySet, name: &CStr) -> Result<(), LocalMutationFailure> {
        if busy.entry_busy(self.device, self.inode, name.to_bytes()) {
            Err(LocalMutationFailure::InUseByTransfer)
        } else {
            Ok(())
        }
    }

    fn observed_entry(
        &self,
        name: &CStr,
        expected: &LocalObjectIdentity,
    ) -> Result<NativeFileIdentity, LocalMutationFailure> {
        let observed = self
            .entry_identity(name)
            .map_err(|error| local_mutation_failure(&error))?;
        if !listed_identity_matches(expected, &observed) {
            return Err(LocalMutationFailure::PreconditionChanged);
        }
        Ok(observed)
    }

    pub(in super::super) fn create_directory_entry(
        &self,
        name: &[u8],
        busy: &LocalBusySet,
    ) -> Result<(), LocalMutationFailure> {
        self.verify()
            .map_err(|error| local_mutation_failure(&error))?;
        let name = component(name)?;
        self.busy_failure(busy, &name)?;
        // SAFETY: NUL-terminated single component relative to the owned descriptor.
        if unsafe { libc::mkdirat(self.directory.as_raw_fd(), name.as_ptr(), 0o777) } != 0 {
            return Err(last_failure());
        }
        Ok(())
    }

    pub(in super::super) fn create_empty_file(
        &self,
        name: &[u8],
        busy: &LocalBusySet,
    ) -> Result<(), LocalMutationFailure> {
        self.verify()
            .map_err(|error| local_mutation_failure(&error))?;
        let name = component(name)?;
        self.busy_failure(busy, &name)?;
        openat_file(
            &self.directory,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o666,
        )
        .map(drop)
        .map_err(|error| local_mutation_failure(&error))
    }

    pub(in super::super) fn rename_entry(
        &self,
        name: &[u8],
        expected: &LocalObjectIdentity,
        new_name: &[u8],
        busy: &LocalBusySet,
    ) -> Result<(), LocalMutationFailure> {
        let name = component(name)?;
        let new_name = component(new_name)?;
        self.busy_failure(busy, &name)?;
        self.busy_failure(busy, &new_name)?;
        let observed = self.observed_entry(&name, expected)?;
        rename_no_replace_at(&self.directory, &name, &new_name, observed.is_regular())
    }

    pub(in super::super) fn delete_entry(
        &self,
        name: &[u8],
        expected: &LocalObjectIdentity,
        recursive: bool,
        busy: &LocalBusySet,
    ) -> Result<(), LocalMutationFailure> {
        let name = component(name)?;
        self.busy_failure(busy, &name)?;
        let observed = self.observed_entry(&name, expected)?;
        if !observed.is_directory() {
            // SAFETY: unlinkat never follows a final symbolic link.
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(last_failure());
            }
            return Ok(());
        }
        if recursive {
            let child = openat_file(
                &self.directory,
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            )
            .map_err(|error| local_mutation_failure(&error))?;
            let opened = NativeFileIdentity::from_metadata(
                &child
                    .metadata()
                    .map_err(|error| local_mutation_failure(&error))?,
            );
            if opened.device != observed.device || opened.inode != observed.inode {
                return Err(LocalMutationFailure::PreconditionChanged);
            }
            // Check the whole tree before removing anything, so a busy
            // descendant never leaves a half-deleted directory behind.
            let mut budget = MAX_RECURSIVE_DELETE_ENTRIES;
            walk_tree(&child, busy, &mut budget, 0, false)?;
            let mut budget = MAX_RECURSIVE_DELETE_ENTRIES;
            walk_tree(&child, busy, &mut budget, 0, true)?;
        }
        // SAFETY: removes the now-empty directory entry relative to the descriptor.
        if unsafe {
            libc::unlinkat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        } != 0
        {
            let failure = last_failure();
            return Err(match failure {
                LocalMutationFailure::TargetExists => LocalMutationFailure::DirectoryNotEmpty,
                other => other,
            });
        }
        Ok(())
    }
}

/// Visits one directory by descriptor. With `delete` false it only proves the
/// tree is within bounds and free of busy entries; with `delete` true it
/// removes every child (never following symbolic links).
fn walk_tree(
    directory: &File,
    busy: &LocalBusySet,
    budget: &mut usize,
    depth: usize,
    delete: bool,
) -> Result<(), LocalMutationFailure> {
    if depth > MAX_RECURSIVE_DELETE_DEPTH {
        return Err(LocalMutationFailure::LimitExceeded);
    }
    let identity = NativeFileIdentity::from_metadata(
        &directory
            .metadata()
            .map_err(|error| local_mutation_failure(&error))?,
    );
    if busy.directory_busy(identity.device, identity.inode) {
        return Err(LocalMutationFailure::InUseByTransfer);
    }
    for name in directory_names(directory, budget)? {
        let child_identity = entry_identity_at(directory, &name)?;
        if child_identity.is_directory() {
            let child = openat_file(
                directory,
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            )
            .map_err(|error| local_mutation_failure(&error))?;
            let opened = NativeFileIdentity::from_metadata(
                &child
                    .metadata()
                    .map_err(|error| local_mutation_failure(&error))?,
            );
            if opened.device != child_identity.device || opened.inode != child_identity.inode {
                return Err(LocalMutationFailure::PreconditionChanged);
            }
            walk_tree(&child, busy, budget, depth + 1, delete)?;
            drop(child);
            if delete
                // SAFETY: relative to the descriptor this walk opened.
                && unsafe {
                    libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
            {
                return Err(last_failure());
            }
        } else if delete
            // SAFETY: unlinkat removes the link itself, never its target.
            && unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0
        {
            return Err(last_failure());
        }
    }
    Ok(())
}

/// Reads all names of one directory (bounded by the shared budget) before any
/// of them is removed, so deletion never races the directory stream.
fn directory_names(
    directory: &File,
    budget: &mut usize,
) -> Result<Vec<CString>, LocalMutationFailure> {
    // SAFETY: a fresh descriptor for the same directory; fdopendir owns it.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(last_failure());
    }
    // SAFETY: descriptor is a valid directory descriptor owned by this call.
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        // SAFETY: fdopendir failed, so the descriptor is still owned here.
        unsafe { libc::close(descriptor) };
        return Err(last_failure());
    }
    let stream = super::NativeDirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        // SAFETY: the stream stays open for the whole loop.
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        // SAFETY: readdir returned a valid entry with a NUL-terminated name.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        if *budget == 0 {
            return Err(LocalMutationFailure::LimitExceeded);
        }
        *budget -= 1;
        names.push(name.to_owned());
    }
    Ok(names)
}

// stat field widths differ between macOS, Linux and Android.
#[allow(clippy::useless_conversion)]
fn entry_identity_at(
    directory: &File,
    name: &CStr,
) -> Result<NativeFileIdentity, LocalMutationFailure> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: AT_SYMLINK_NOFOLLOW reports a link itself, never its target.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(last_failure());
    }
    // SAFETY: fstatat succeeded and initialized the buffer.
    let stat = unsafe { stat.assume_init() };
    Ok(NativeFileIdentity {
        device: u64::try_from(stat.st_dev).unwrap_or(0),
        inode: stat.st_ino,
        mode: u32::from(stat.st_mode),
        size: u64::try_from(stat.st_size).unwrap_or(0),
        modified_seconds: stat.st_mtime,
        modified_nanoseconds: stat.st_mtime_nsec,
    })
}

/// Atomic rename that never replaces an existing name.
fn rename_no_replace_at(
    directory: &File,
    name: &CStr,
    new_name: &CStr,
    regular_file: bool,
) -> Result<(), LocalMutationFailure> {
    let descriptor = directory.as_raw_fd();
    #[cfg(target_vendor = "apple")]
    {
        let _ = regular_file;
        // SAFETY: both names are NUL-terminated components of the same directory.
        let result = unsafe {
            libc::renameatx_np(
                descriptor,
                name.as_ptr(),
                descriptor,
                new_name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if result != 0 {
            return Err(last_failure());
        }
        Ok(())
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: raw renameat2 with RENAME_NOREPLACE; bionic only wraps it from API 30.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::c_long::from(descriptor),
                name.as_ptr(),
                libc::c_long::from(descriptor),
                new_name.as_ptr(),
                libc::c_long::from(libc::RENAME_NOREPLACE),
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) {
            return Err(local_mutation_failure(&error));
        }
        // No atomic no-replace rename here: a regular file can still move
        // safely as a new link plus unlink; a directory cannot.
        if !regular_file {
            return Err(LocalMutationFailure::Unsupported);
        }
        // SAFETY: linkat fails with EEXIST instead of replacing.
        if unsafe { libc::linkat(descriptor, name.as_ptr(), descriptor, new_name.as_ptr(), 0) } != 0
        {
            return Err(last_failure());
        }
        // SAFETY: removes the old link of the same inode.
        if unsafe { libc::unlinkat(descriptor, name.as_ptr(), 0) } != 0 {
            return Err(last_failure());
        }
        Ok(())
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
    {
        let _ = (descriptor, name, new_name, regular_file);
        Err(LocalMutationFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::CString, fs, os::unix::fs::symlink, path::Path};

    use super::super::{LocalBusySet, LocalMutationFailure, LocalObjectIdentity};
    use super::NativeLocalDirectoryCapability;

    fn capability(path: &Path) -> NativeLocalDirectoryCapability {
        NativeLocalDirectoryCapability::register(path).unwrap()
    }

    fn listed(capability: &NativeLocalDirectoryCapability, name: &str) -> LocalObjectIdentity {
        LocalObjectIdentity::from_native(
            &capability
                .entry_identity(&CString::new(name).unwrap())
                .unwrap(),
        )
    }

    #[test]
    fn creation_is_exclusive_and_names_are_single_components() {
        let root = tempfile::tempdir().unwrap();
        let directory = capability(root.path());
        let busy = LocalBusySet::default();
        directory.create_directory_entry(b"folder", &busy).unwrap();
        directory.create_empty_file(b"empty.txt", &busy).unwrap();
        assert!(root.path().join("folder").is_dir());
        assert_eq!(fs::read(root.path().join("empty.txt")).unwrap(), b"");
        fs::write(root.path().join("empty.txt"), b"kept").unwrap();
        assert_eq!(
            directory.create_empty_file(b"empty.txt", &busy),
            Err(LocalMutationFailure::TargetExists)
        );
        assert_eq!(fs::read(root.path().join("empty.txt")).unwrap(), b"kept");
        assert_eq!(
            directory.create_directory_entry(b"folder", &busy),
            Err(LocalMutationFailure::TargetExists)
        );
        for name in [&b".."[..], b".", b"", b"a/b", b"../escape"] {
            assert_eq!(
                directory.create_directory_entry(name, &busy),
                Err(LocalMutationFailure::InvalidName),
                "{name:?}"
            );
        }
        assert!(!root.path().parent().unwrap().join("escape").exists());
    }

    #[test]
    fn creating_through_a_dangling_symlink_never_writes_outside() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path().join("planted"), root.path().join("link")).unwrap();
        let directory = capability(root.path());
        assert_eq!(
            directory.create_empty_file(b"link", &LocalBusySet::default()),
            Err(LocalMutationFailure::TargetExists)
        );
        assert!(!outside.path().join("planted").exists());
    }

    #[test]
    fn rename_never_replaces_and_requires_the_listed_identity() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), b"a").unwrap();
        fs::write(root.path().join("b.txt"), b"b").unwrap();
        fs::create_dir(root.path().join("dir")).unwrap();
        let directory = capability(root.path());
        let busy = LocalBusySet::default();
        let a = listed(&directory, "a.txt");
        assert_eq!(
            directory.rename_entry(b"a.txt", &a, b"b.txt", &busy),
            Err(LocalMutationFailure::TargetExists)
        );
        assert_eq!(fs::read(root.path().join("b.txt")).unwrap(), b"b");
        assert_eq!(
            directory.rename_entry(b"a.txt", &a, b"../out.txt", &busy),
            Err(LocalMutationFailure::InvalidName)
        );
        directory
            .rename_entry(b"a.txt", &a, b"c.txt", &busy)
            .unwrap();
        assert_eq!(fs::read(root.path().join("c.txt")).unwrap(), b"a");
        assert!(!root.path().join("a.txt").exists());

        let dir = listed(&directory, "dir");
        directory
            .rename_entry(b"dir", &dir, b"renamed", &busy)
            .unwrap();
        assert!(root.path().join("renamed").is_dir());

        // A different object now behind the listed name is refused.
        let b = listed(&directory, "b.txt");
        fs::remove_file(root.path().join("b.txt")).unwrap();
        fs::write(root.path().join("b.txt"), b"replaced object").unwrap();
        assert_eq!(
            directory.rename_entry(b"b.txt", &b, b"d.txt", &busy),
            Err(LocalMutationFailure::PreconditionChanged)
        );
    }

    #[test]
    fn delete_removes_links_not_targets_and_needs_recursive_for_contents() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("precious.txt"), b"keep").unwrap();
        symlink(outside.path(), root.path().join("link-to-outside")).unwrap();
        fs::create_dir_all(root.path().join("tree/nested/deeper")).unwrap();
        fs::write(root.path().join("tree/nested/file.txt"), b"x").unwrap();
        symlink(outside.path(), root.path().join("tree/nested/escape")).unwrap();
        let directory = capability(root.path());
        let busy = LocalBusySet::default();

        let link = listed(&directory, "link-to-outside");
        directory
            .delete_entry(b"link-to-outside", &link, true, &busy)
            .unwrap();
        assert!(!root.path().join("link-to-outside").exists());
        assert_eq!(
            fs::read(outside.path().join("precious.txt")).unwrap(),
            b"keep"
        );

        let tree = listed(&directory, "tree");
        assert_eq!(
            directory.delete_entry(b"tree", &tree, false, &busy),
            Err(LocalMutationFailure::DirectoryNotEmpty)
        );
        assert!(root.path().join("tree/nested/file.txt").exists());
        directory.delete_entry(b"tree", &tree, true, &busy).unwrap();
        assert!(!root.path().join("tree").exists());
        assert_eq!(
            fs::read(outside.path().join("precious.txt")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn objects_of_unfinished_transfers_are_refused_before_anything_changes() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("tree/active")).unwrap();
        fs::write(root.path().join("tree/active/download.bin"), b"part").unwrap();
        fs::write(root.path().join("tree/other.txt"), b"other").unwrap();
        fs::write(root.path().join("busy.bin"), b"busy").unwrap();
        let directory = capability(root.path());
        let active = capability(&root.path().join("tree/active"));
        let mut busy = LocalBusySet::default();
        busy.insert(active.device, active.inode, b"download.bin".to_vec());
        busy.insert(directory.device, directory.inode, b"busy.bin".to_vec());

        let file = listed(&directory, "busy.bin");
        assert_eq!(
            directory.delete_entry(b"busy.bin", &file, false, &busy),
            Err(LocalMutationFailure::InUseByTransfer)
        );
        assert_eq!(
            directory.rename_entry(b"busy.bin", &file, b"moved.bin", &busy),
            Err(LocalMutationFailure::InUseByTransfer)
        );
        assert_eq!(
            directory.create_empty_file(b"busy.bin", &busy),
            Err(LocalMutationFailure::InUseByTransfer)
        );
        let tree = listed(&directory, "tree");
        assert_eq!(
            directory.delete_entry(b"tree", &tree, true, &busy),
            Err(LocalMutationFailure::InUseByTransfer)
        );
        // The pre-scan refused before deleting any sibling.
        assert!(root.path().join("tree/other.txt").exists());
        assert!(root.path().join("tree/active/download.bin").exists());
    }

    #[test]
    fn actions_follow_the_retained_directory_after_its_path_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        fs::create_dir(&selected).unwrap();
        let directory = capability(&selected);
        fs::rename(&selected, root.path().join("moved")).unwrap();
        fs::create_dir(&selected).unwrap();
        directory
            .create_empty_file(b"new.txt", &LocalBusySet::default())
            .unwrap();
        assert!(root.path().join("moved/new.txt").exists());
        assert!(!selected.join("new.txt").exists());
    }
}
