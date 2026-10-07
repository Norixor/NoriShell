//! Android Storage Access Framework documents opened through the bridge.

use std::{
    fs::File,
    io::{self, Read, Seek as _, SeekFrom, Write as _},
    os::fd::{AsRawFd as _, FromRawFd as _},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager as _, Runtime};
use tauri_plugin_norishell_android as bridge;

use super::ContentAccessError;

const MAX_URI_BYTES: usize = 4096;
const MAX_FILE_NAME_BYTES: usize = 200;
const COPY_CHUNK_BYTES: usize = 64 * 1024;

/// A `content://` document URI chosen by the user. The URI text is never
/// logged or returned to a WebView by Core.
#[derive(Clone)]
pub(crate) struct ContentUri(String);

impl std::fmt::Debug for ContentUri {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ContentUri(..)")
    }
}

/// An opened document. `file` owns the descriptor the provider handed out.
pub(crate) struct ContentFile {
    pub(crate) file: File,
    pub(crate) display_name: Option<String>,
    /// Exact length for regular descriptors and spooled copies; the provider's
    /// advertised size otherwise.
    pub(crate) len: Option<u64>,
}

#[derive(Serialize)]
struct OpenRequest<'a> {
    uri: &'a str,
    mode: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenResponse {
    fd: i32,
    display_name: Option<String>,
    size: Option<i64>,
}

impl ContentUri {
    pub(crate) fn parse(text: &str) -> Result<Self, ContentAccessError> {
        if !text.starts_with("content://")
            || text.len() > MAX_URI_BYTES
            || text.chars().any(char::is_control)
        {
            return Err(ContentAccessError::Rejected);
        }
        Ok(Self(text.to_owned()))
    }

    /// Opens the document. Blocks on the bridge; never call it on the UI thread.
    fn open(&self, mode: &str) -> Result<ContentFile, ContentAccessError> {
        let response: OpenResponse =
            bridge::run_command("contentUriOpen", OpenRequest { uri: &self.0, mode }).map_err(
                |error| match error.code() {
                    "content_access.uri_rejected" => ContentAccessError::Rejected,
                    "content_access.not_found" => ContentAccessError::NotFound,
                    "content_access.permission_denied" => ContentAccessError::PermissionDenied,
                    "content_access.open_failed" | "content_access.invalid_request" => {
                        if mode == "r" {
                            ContentAccessError::ReadFailed
                        } else {
                            ContentAccessError::WriteFailed
                        }
                    }
                    _ => ContentAccessError::Unavailable,
                },
            )?;
        if response.fd < 0 {
            return Err(ContentAccessError::Unavailable);
        }
        // SAFETY: Kotlin detached this descriptor from its ParcelFileDescriptor
        // and transferred ownership to Core; nothing else closes it.
        let file = unsafe { File::from_raw_fd(response.fd) };
        // Binder usually delivers close-on-exec descriptors; enforce it so a
        // local terminal shell spawned later never inherits a user document.
        // SAFETY: plain fcntl on a descriptor this function owns.
        unsafe {
            let flags = libc::fcntl(file.as_raw_fd(), libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
        Ok(ContentFile {
            file,
            display_name: response.display_name,
            len: response.size.and_then(|size| u64::try_from(size).ok()),
        })
    }

    /// The provider's display name, read without opening the document (a save
    /// target must not be truncated before data is ready). `None` when the
    /// provider reports none or the query fails.
    pub(crate) fn display_name(&self) -> Option<String> {
        #[derive(Serialize)]
        struct QueryRequest<'a> {
            uri: &'a str,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct QueryResponse {
            display_name: Option<String>,
        }
        bridge::run_command::<QueryResponse>("contentUriQuery", QueryRequest { uri: &self.0 })
            .ok()
            .and_then(|response| response.display_name)
    }

    /// Opens the document for streaming reads without any private copy. Used
    /// for small secrets (private keys) that must not be spooled to disk.
    pub(crate) fn open_read(&self) -> Result<ContentFile, ContentAccessError> {
        self.open("r")
    }

    /// Opens the document as a seekable regular file of known length.
    /// Providers that hand out pipes are spooled into an anonymous (already
    /// unlinked) file under the private root, bounded by `max_bytes`.
    pub(crate) fn open_regular<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        max_bytes: u64,
    ) -> Result<ContentFile, ContentAccessError> {
        let mut opened = self.open("r")?;
        let metadata = opened
            .file
            .metadata()
            .map_err(|_| ContentAccessError::ReadFailed)?;
        if metadata.is_file() {
            if metadata.len() > max_bytes {
                return Err(ContentAccessError::TooLarge);
            }
            opened.len = Some(metadata.len());
            return Ok(opened);
        }
        let root = private_root(app)?;
        let mut spool = tempfile::tempfile_in(root).map_err(|_| ContentAccessError::Unavailable)?;
        let copied = copy_bounded(&mut opened.file, &mut spool, max_bytes)?;
        spool
            .seek(SeekFrom::Start(0))
            .map_err(|_| ContentAccessError::ReadFailed)?;
        Ok(ContentFile {
            file: spool,
            display_name: opened.display_name,
            len: Some(copied),
        })
    }

    /// Replaces the document content with everything `source` yields.
    ///
    /// `WriteFailed` means the document was not opened for writing (its
    /// previous content is intact); `WriteIncomplete` means truncation already
    /// happened and the document may be empty or partial.
    pub(crate) fn write_from(&self, source: &mut impl Read) -> Result<u64, ContentAccessError> {
        let mut target = self.open("wt")?;
        let mut buffer = vec![0_u8; COPY_CHUNK_BYTES];
        let mut written = 0_u64;
        loop {
            let read = match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(ContentAccessError::WriteIncomplete),
            };
            target
                .file
                .write_all(&buffer[..read])
                .map_err(|_| ContentAccessError::WriteIncomplete)?;
            written = written.saturating_add(read as u64);
        }
        target
            .file
            .flush()
            .map_err(|_| ContentAccessError::WriteIncomplete)?;
        match target.file.sync_all() {
            Ok(()) => {}
            // Pipes and sockets cannot be synced; the provider persists on close.
            Err(error) if error.raw_os_error() == Some(libc::EINVAL) => {}
            Err(_) => return Err(ContentAccessError::WriteIncomplete),
        }
        Ok(written)
    }

    pub(crate) fn write_bytes(&self, bytes: &[u8]) -> Result<(), ContentAccessError> {
        let mut source = bytes;
        self.write_from(&mut source).map(|_| ())
    }
}

/// Copies at most `max_bytes`; one byte more fails with `TooLarge`.
fn copy_bounded(
    source: &mut impl Read,
    target: &mut File,
    max_bytes: u64,
) -> Result<u64, ContentAccessError> {
    let copied = io::copy(&mut source.take(max_bytes.saturating_add(1)), target)
        .map_err(|_| ContentAccessError::ReadFailed)?;
    if copied > max_bytes {
        return Err(ContentAccessError::TooLarge);
    }
    target.flush().map_err(|_| ContentAccessError::ReadFailed)?;
    Ok(copied)
}

/// Resolved private root; set on first use in this process.
static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// `<app cache>/content-access`, wiped once per process before first use.
fn private_root<R: Runtime>(app: &AppHandle<R>) -> Result<&'static Path, ContentAccessError> {
    if let Some(root) = ROOT.get() {
        return Ok(root.as_path());
    }
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|_| ContentAccessError::Unavailable)?
        .join("content-access");
    // Anything here belongs to a previous process: no copy outlives the
    // process that created it.
    match std::fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(ContentAccessError::Unavailable),
    }
    create_private_directory(&root)?;
    // A concurrent first use may have won; both computed the same path.
    let _ = ROOT.set(root);
    ROOT.get()
        .map(PathBuf::as_path)
        .ok_or(ContentAccessError::Unavailable)
}

fn create_private_directory(path: &Path) -> Result<(), ContentAccessError> {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(path)
        .map_err(|_| ContentAccessError::Unavailable)
}

/// Shows the system document picker (`ACTION_OPEN_DOCUMENT`) and returns the
/// chosen document, `None` when the user cancelled.
///
/// Unlike the dialog plugin, whose result is delivered to MainActivity only
/// once MainActivity is started again, this works while a Secure Surface, tool
/// window or plugin-isolated surface covers MainActivity: Kotlin hosts the
/// picker in its own translucent Activity on top of whatever is in front. One
/// pick (open or create) runs at a time and Kotlin closes it after a fixed
/// timeout. Blocks; never call it on the UI thread.
pub(crate) fn pick_document(mime_types: &[&str]) -> Result<Option<ContentUri>, ContentAccessError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PickRequest<'a> {
        mime_types: &'a [&'a str],
    }
    picker_command("contentPick", PickRequest { mime_types })
}

/// The system save dialog (`ACTION_CREATE_DOCUMENT`) with the same hosting,
/// cancel and timeout rules as [`pick_document`]. The provider creates an empty
/// document when the user confirms; write it with [`ContentUri::write_from`].
pub(crate) fn create_document(
    mime_type: &str,
    suggested_name: &str,
) -> Result<Option<ContentUri>, ContentAccessError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CreateRequest<'a> {
        mime_type: &'a str,
        suggested_name: &'a str,
    }
    picker_command(
        "contentCreate",
        CreateRequest {
            mime_type,
            suggested_name,
        },
    )
}

fn picker_command(
    command: &str,
    request: impl Serialize,
) -> Result<Option<ContentUri>, ContentAccessError> {
    #[derive(Deserialize)]
    struct PickResponse {
        uri: Option<String>,
    }
    let response: PickResponse =
        bridge::run_command(command, request).map_err(|error| match error.code() {
            "content_access.pick_busy" => ContentAccessError::PickBusy,
            "content_access.pick_timeout" => ContentAccessError::PickTimeout,
            "content_access.invalid_request" => ContentAccessError::Rejected,
            _ => ContentAccessError::Unavailable,
        })?;
    response.uri.map(|uri| ContentUri::parse(&uri)).transpose()
}

/// Removes every private copy. Called on application exit; also safe to call
/// when nothing was ever created.
///
/// Uses the root resolved on first use instead of `app.path()`: Android path
/// resolution is a Kotlin call that panics once no Activity is left, which is
/// exactly the swipe-away / Activity-teardown exit. A process that never used
/// the tree has nothing of its own to remove; older leftovers are wiped by the
/// next process's first use.
pub(crate) fn remove_all_private_copies<R: Runtime>(_app: &AppHandle<R>) {
    if let Some(root) = ROOT.get() {
        let _ = std::fs::remove_dir_all(root);
    }
}

/// A private directory for one operation; removed recursively on drop.
#[derive(Debug)]
pub(crate) struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    pub(crate) fn create<R: Runtime>(
        app: &AppHandle<R>,
        purpose: &str,
    ) -> Result<Self, ContentAccessError> {
        let root = private_root(app)?;
        let path = root.join(format!("{purpose}-{}", uuid::Uuid::new_v4().simple()));
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&path)
            .map_err(|_| ContentAccessError::Unavailable)?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Keeps the directory until the process-start wipe or exit cleanup.
    /// Used where the lifetime of the copy is owned by a capability that has
    /// no drop hook (plugin file grants).
    pub(crate) fn keep_until_exit(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A single safe path component derived from a provider display name.
pub(crate) fn safe_file_name(display_name: Option<&str>, fallback: &str) -> String {
    let candidate = display_name
        .and_then(|name| name.rsplit(['/', '\\']).next())
        .unwrap_or_default();
    let mut name = String::new();
    for character in candidate.chars() {
        if character.is_control() {
            continue;
        }
        if name.len() + character.len_utf8() > MAX_FILE_NAME_BYTES {
            break;
        }
        name.push(character);
    }
    let name = name.trim().to_owned();
    if name.is_empty() || name == "." || name == ".." {
        fallback.to_owned()
    } else {
        name
    }
}
