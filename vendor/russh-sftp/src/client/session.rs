// Modified for NoriShell; see vendor/README.md at the repository root for upstream provenance.
use std::{collections::VecDeque, sync::Arc};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{
    error::Error,
    fs::{File, Metadata, ReadDir},
    rawsession::{Limits, SftpResult},
    runtime, RawSftpSession,
};
use crate::{
    client::Config,
    extensions::{self, Statvfs},
    protocol::{FileAttributes, OpenFlags, StatusCode},
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Features {
    pub hardlink: bool,
    pub posix_rename: bool,
    pub fsync: bool,
    pub statvfs: bool,
    pub expand_path: bool,
    pub limits: Option<Limits>,
    pub max_concurrent_writes: usize,
    pub max_packet_len: u32,
}

/// High-level SFTP implementation for easy interaction with a remote file system.
/// Contains most methods similar to the native [filesystem](std::fs)
#[derive(Clone)]
pub struct SftpSession {
    session: Arc<RawSftpSession>,
    features: Features,
}

/// Core-owned live directory handle for protocol-level bounded pagination.
///
/// Unlike [`SftpSession::read_dir`], this stream issues `READDIR` lazily and
/// keeps at most one server response plus the requested page in memory. Call
/// [`DirectoryStream::close`] when abandoning a continuation before EOF.
pub struct DirectoryStream {
    session: Arc<RawSftpSession>,
    parent: Arc<str>,
    handle: Option<String>,
    pending: VecDeque<(String, Metadata)>,
    complete: bool,
}

const MAX_READDIR_REQUESTS_PER_PAGE: usize = 16;

impl DirectoryStream {
    /// Reads at most `maximum_entries` from the live directory handle.
    pub async fn next_page(&mut self, maximum_entries: usize) -> SftpResult<ReadDir> {
        if maximum_entries == 0 {
            return Err(Error::UnexpectedBehavior(
                "directory page size must be positive".to_owned(),
            ));
        }
        let mut requests = 0_usize;
        while self.pending.len() < maximum_entries && !self.complete {
            if requests >= MAX_READDIR_REQUESTS_PER_PAGE {
                let error = Error::UnexpectedBehavior(
                    "server returned too many empty directory replies".to_owned(),
                );
                let _ = self.close().await;
                return Err(error);
            }
            let Some(handle) = self.handle.as_deref() else {
                self.complete = true;
                break;
            };
            requests = requests.saturating_add(1);
            match self.session.readdir(handle).await {
                Ok(name) => self.pending.extend(
                    name.files
                        .into_iter()
                        .filter(|file| file.filename != "." && file.filename != "..")
                        .map(|file| (file.filename, file.attrs)),
                ),
                Err(Error::Status(status)) if status.status_code == StatusCode::Eof => {
                    self.close().await?;
                }
                Err(error) => {
                    let _ = self.close().await;
                    return Err(error);
                }
            }
        }
        let take = maximum_entries.min(self.pending.len());
        let entries = self.pending.drain(..take).collect();
        Ok(ReadDir {
            parent: self.parent.clone(),
            entries,
        })
    }

    /// Returns true only after the server reported EOF and buffered entries
    /// have all been returned.
    pub fn is_complete(&self) -> bool {
        self.complete && self.pending.is_empty()
    }

    /// Closes the live directory handle. Repeated calls are harmless.
    pub async fn close(&mut self) -> SftpResult<()> {
        if let Some(handle) = self.handle.as_ref() {
            self.session.close(handle.clone()).await?;
            self.handle.take();
        }
        self.complete = true;
        Ok(())
    }
}

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        let session = self.session.clone();
        runtime::spawn(async move {
            let _ = session.close(handle).await;
        });
    }
}

impl SftpSession {
    /// Creates a new session by initializing the protocol and extensions
    pub async fn new<S>(stream: S) -> SftpResult<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::new_with_config(stream, Config::default()).await
    }

    /// Creates a new session with custom configuration
    pub async fn new_with_config<S>(stream: S, cfg: Config) -> SftpResult<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let max_concurrent_writes = cfg.max_concurrent_writes;
        let max_packet_len = cfg.max_packet_len;
        let mut session = RawSftpSession::new_with_config(stream, cfg);

        let version = session.init().await?;
        let has_extension = |name, ver| version.extensions.get(name).is_some_and(|v| v == ver);

        let mut features = Features {
            hardlink: has_extension(extensions::HARDLINK, "1"),
            posix_rename: has_extension(extensions::POSIX_RENAME, "1"),
            fsync: has_extension(extensions::FSYNC, "1"),
            statvfs: has_extension(extensions::STATVFS, "2"),
            expand_path: has_extension(extensions::EXPAND_PATH, "1"),
            limits: None,
            max_concurrent_writes,
            max_packet_len,
        };

        if has_extension(extensions::LIMITS, "1") {
            let limits = Limits::from(session.limits().await?);
            session.set_limits(limits);
            features.limits = Some(limits);
            if let Some(plen) = limits.packet_len {
                features.max_packet_len = (plen as u32).min(max_packet_len);
            }
        }

        Ok(Self {
            session: Arc::new(session),
            features,
        })
    }

    /// Set the maximum response time in seconds.
    /// Default: 10 seconds
    pub fn set_timeout(&self, secs: u64) {
        self.session.set_timeout(secs);
    }

    /// Closes the inner channel stream.
    pub async fn close(&self) -> SftpResult<()> {
        self.session.close_session()
    }

    /// Attempts to open a file in read-only mode.
    pub async fn open<T: Into<String>>(&self, filename: T) -> SftpResult<File> {
        self.open_with_flags(filename, OpenFlags::READ).await
    }

    /// Opens a file in write-only mode.
    ///
    /// This function will create a file if it does not exist, and will truncate it if it does.
    pub async fn create<T: Into<String>>(&self, filename: T) -> SftpResult<File> {
        self.open_with_flags(
            filename,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
    }

    /// Attempts to open or create the file in the specified mode
    pub async fn open_with_flags<T: Into<String>>(
        &self,
        filename: T,
        flags: OpenFlags,
    ) -> SftpResult<File> {
        self.open_with_flags_and_attributes(filename, flags, FileAttributes::empty())
            .await
    }

    /// Attempts to open or create the file in the specified mode and with specified file attributes
    pub async fn open_with_flags_and_attributes<T: Into<String>>(
        &self,
        filename: T,
        flags: OpenFlags,
        attributes: FileAttributes,
    ) -> SftpResult<File> {
        let handle = self.session.open(filename, flags, attributes).await?.handle;
        Ok(File::new(self.session.clone(), handle, self.features))
    }

    /// Requests the remote party for the absolute from the relative path.
    pub async fn canonicalize<T: Into<String>>(&self, path: T) -> SftpResult<String> {
        let name = self.session.realpath(path).await?;
        match name.files.first() {
            Some(file) => Ok(file.filename.to_owned()),
            None => Err(Error::UnexpectedBehavior("no file".to_owned())),
        }
    }

    /// Creates a new empty directory.
    pub async fn create_dir<T: Into<String>>(&self, path: T) -> SftpResult<()> {
        self.session
            .mkdir(path, FileAttributes::empty())
            .await
            .map(|_| ())
    }

    /// Reads the contents of a file located at the specified path to the end.
    pub async fn read<P: Into<String>>(&self, path: P) -> SftpResult<Vec<u8>> {
        let mut file = self.open(path).await?;
        let mut buffer = Vec::new();

        file.read_to_end(&mut buffer).await?;

        Ok(buffer)
    }

    /// Writes the contents to a file whose path is specified.
    pub async fn write<P: Into<String>>(&self, path: P, data: &[u8]) -> SftpResult<()> {
        let mut file = self.open_with_flags(path, OpenFlags::WRITE).await?;
        file.write_all(data).await?;
        Ok(())
    }

    /// Checks a file or folder exists at the specified path
    pub async fn try_exists<P: Into<String>>(&self, path: P) -> SftpResult<bool> {
        match self.metadata(path).await {
            Ok(_) => Ok(true),
            Err(Error::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Returns an iterator over the entries within a directory.
    pub async fn read_dir<P: Into<String>>(&self, path: P) -> SftpResult<ReadDir> {
        let path: String = path.into();
        let parent = Arc::from(path.as_str());

        let handle = self.session.opendir(path).await?.handle;
        let mut files = vec![];

        loop {
            match self.session.readdir(handle.as_str()).await {
                Ok(name) => {
                    files = name
                        .files
                        .into_iter()
                        .map(|f| (f.filename, f.attrs))
                        .chain(files)
                        .collect();
                }
                Err(Error::Status(status)) if status.status_code == StatusCode::Eof => break,
                Err(err) => return Err(err),
            }
        }

        self.session.close(handle).await?;

        Ok(ReadDir {
            parent,
            entries: files.into(),
        })
    }

    /// Opens a live directory stream for protocol-level bounded pagination.
    pub async fn open_dir_stream<P: Into<String>>(&self, path: P) -> SftpResult<DirectoryStream> {
        let path = path.into();
        let parent = Arc::from(path.as_str());
        let handle = self.session.opendir(path).await?.handle;
        Ok(DirectoryStream {
            session: self.session.clone(),
            parent,
            handle: Some(handle),
            pending: VecDeque::new(),
            complete: false,
        })
    }

    /// Reads a symbolic link, returning the file that the link points to.
    pub async fn read_link<P: Into<String>>(&self, path: P) -> SftpResult<String> {
        let name = self.session.readlink(path).await?;
        match name.files.first() {
            Some(file) => Ok(file.filename.to_owned()),
            None => Err(Error::UnexpectedBehavior("no file".to_owned())),
        }
    }

    /// Removes the specified folder.
    pub async fn remove_dir<P: Into<String>>(&self, path: P) -> SftpResult<()> {
        self.session.rmdir(path).await.map(|_| ())
    }

    /// Removes the specified file.
    pub async fn remove_file<T: Into<String>>(&self, filename: T) -> SftpResult<()> {
        self.session.remove(filename).await.map(|_| ())
    }

    /// Rename a file or directory to a new name.
    pub async fn rename<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<()>
    where
        O: Into<String>,
        N: Into<String>,
    {
        self.session.rename(oldpath, newpath).await.map(|_| ())
    }

    /// Creates a symlink of the specified target.
    pub async fn symlink<P, T>(&self, path: P, target: T) -> SftpResult<()>
    where
        P: Into<String>,
        T: Into<String>,
    {
        self.session.symlink(path, target).await.map(|_| ())
    }

    /// Queries metadata about the remote file.
    pub async fn metadata<P: Into<String>>(&self, path: P) -> SftpResult<Metadata> {
        Ok(self.session.stat(path).await?.attrs)
    }

    /// Sets metadata for a remote file.
    pub async fn set_metadata<P: Into<String>>(
        &self,
        path: P,
        metadata: Metadata,
    ) -> Result<(), Error> {
        self.session.setstat(path, metadata).await.map(|_| ())
    }

    pub async fn symlink_metadata<P: Into<String>>(&self, path: P) -> SftpResult<Metadata> {
        Ok(self.session.lstat(path).await?.attrs)
    }

    pub async fn hardlink<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<bool>
    where
        O: Into<String>,
        N: Into<String>,
    {
        if !self.features.hardlink {
            return Ok(false);
        }

        self.session.hardlink(oldpath, newpath).await.map(|_| true)
    }

    /// Atomically replaces `newpath` with `oldpath` when the OpenSSH
    /// `posix-rename@openssh.com` extension is available.
    pub async fn posix_rename<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<bool>
    where
        O: Into<String>,
        N: Into<String>,
    {
        if !self.features.posix_rename {
            return Ok(false);
        }

        self.session
            .posix_rename(oldpath, newpath)
            .await
            .map(|_| true)
    }

    /// Performs a statvfs on the remote file system path.
    /// Returns `Ok(None)` if the remote SFTP server does not support `statvfs@openssh.com` extension v2.
    pub async fn fs_info<P: Into<String>>(&self, path: P) -> SftpResult<Option<Statvfs>> {
        if !self.features.statvfs {
            return Ok(None);
        }

        self.session.statvfs(path).await.map(Some)
    }

    /// Expands a `~`/`~user`-prefixed or relative path and returns its canonicalized absolute form.
    /// Returns `Ok(None)` if the remote SFTP server does not support `expand-path@openssh.com` extension v1.
    pub async fn expand_path<P: Into<String>>(&self, path: P) -> SftpResult<Option<String>> {
        if !self.features.expand_path {
            return Ok(None);
        }

        let name = self.session.expand_path(path).await?;
        match name.files.first() {
            Some(file) => Ok(Some(file.filename.to_owned())),
            None => Err(Error::UnexpectedBehavior("no file".to_owned())),
        }
    }
}
