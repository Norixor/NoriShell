//! Core-owned disk copy of the last verified encrypted remote exchange.
//! The file never contains a decrypted bundle, derived key, or browser projection.

use std::{
    io::{self, Read as _, Write as _},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, DirBuilder, OpenOptions, OpenOptionsExt as _},
};
use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use uuid::Uuid;

const DIRECTORY: &str = "ssh-sync-browser-cache";
const FILE_NAME: &str = "snapshot.bin";
const MAGIC: &[u8; 8] = b"NSBCACHE";
const VERSION: u8 = 2;
const MAC_DOMAIN: &[u8] = b"NoriShell/ssh-sync-browser-cache/v2\0";
const MAC_BYTES: usize = 32;
const MAX_HEADER_BYTES: usize = 8 * 1024;
pub(crate) const MAX_CIPHERTEXT_BYTES: usize = 96 * 1024 * 1024;
const MAX_FILE_BYTES: u64 =
    (MAGIC.len() + 4 + MAX_HEADER_BYTES + MAX_CIPHERTEXT_BYTES + MAC_BYTES) as u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BrowserDiskCacheBinding {
    pub(crate) plugin_id: String,
    pub(crate) signer_sha256: String,
    pub(crate) profile_id: String,
    pub(crate) oauth_configuration_sha256: String,
    pub(crate) oauth_session_id: String,
    pub(crate) canonical_source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserDiskCacheValue {
    pub(crate) ciphertext: Option<Vec<u8>>,
    pub(crate) verified_at_unix_ms: i64,
    pub(crate) remote_updated_at_unix_ms: Option<i64>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum StoreError {
    #[error("invalid browser cache binding")]
    InvalidBinding,
    #[error("browser cache exceeds the size limit")]
    TooLarge,
    #[error("browser cache is corrupt")]
    Corrupt,
    #[error("browser cache path is unsafe")]
    UnsafePath,
    #[error("browser cache I/O failed")]
    Io,
    #[error("browser cache commit state is unknown")]
    CommitStateUnknown,
    #[error("system clock is unavailable")]
    Clock,
}

#[derive(Serialize, Deserialize)]
struct Header {
    version: u8,
    binding: BrowserDiskCacheBinding,
    verified_at_unix_ms: i64,
    remote_updated_at_unix_ms: Option<i64>,
    ciphertext_len: u64,
    ciphertext_sha256: Option<String>,
}

pub(crate) fn save(
    root: &Path,
    binding: &BrowserDiskCacheBinding,
    ciphertext: Option<&[u8]>,
    remote_updated_at_unix_ms: Option<i64>,
    mac_key: &[u8; 32],
) -> Result<(), StoreError> {
    validate_binding(binding)?;
    let payload = ciphertext.unwrap_or_default();
    if ciphertext.is_some_and(|bytes| bytes.is_empty()) {
        return Err(StoreError::Corrupt);
    }
    if payload.len() > MAX_CIPHERTEXT_BYTES {
        return Err(StoreError::TooLarge);
    }
    let verified_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StoreError::Clock)?
        .as_millis()
        .try_into()
        .map_err(|_| StoreError::Clock)?;
    let header = Header {
        version: VERSION,
        binding: binding.clone(),
        verified_at_unix_ms,
        remote_updated_at_unix_ms,
        ciphertext_len: payload.len() as u64,
        ciphertext_sha256: ciphertext.map(|_| sha256_hex(payload)),
    };
    let encoded = serde_json::to_vec(&header).map_err(|_| StoreError::Corrupt)?;
    if encoded.len() > MAX_HEADER_BYTES {
        return Err(StoreError::TooLarge);
    }
    let header_len = (encoded.len() as u32).to_le_bytes();
    let mut mac = new_mac(mac_key)?;
    mac.update(MAGIC);
    mac.update(&header_len);
    mac.update(&encoded);
    mac.update(payload);
    let tag = mac.finalize().into_bytes();
    let directory = cache_directory(root, binding, true)?.ok_or(StoreError::Io)?;
    ensure_regular_or_missing(&directory, FILE_NAME)?;
    let temporary = format!(".snapshot-{}.tmp", Uuid::new_v4());
    let result = (|| -> Result<(), StoreError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = directory.open_with(&temporary, &options).map_err(|error| {
            eprintln!("verified SSH sync browser cache temporary file open failed: {error}");
            StoreError::Io
        })?;
        file.write_all(MAGIC).map_err(|_| StoreError::Io)?;
        file.write_all(&header_len).map_err(|_| StoreError::Io)?;
        file.write_all(&encoded).map_err(|_| StoreError::Io)?;
        file.write_all(payload).map_err(|_| StoreError::Io)?;
        file.write_all(&tag).map_err(|_| StoreError::Io)?;
        file.sync_all().map_err(|error| {
            eprintln!("verified SSH sync browser cache file sync failed: {error}");
            StoreError::Io
        })?;
        ensure_regular_or_missing(&directory, FILE_NAME)?;
        replace_file(root, binding, &directory, &temporary)?;
        sync_directory(&directory).map_err(|_| StoreError::CommitStateUnknown)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = directory.remove_file(&temporary);
    }
    result
}

pub(crate) fn load(
    root: &Path,
    binding: &BrowserDiskCacheBinding,
    mac_key: &[u8; 32],
) -> Result<Option<BrowserDiskCacheValue>, StoreError> {
    validate_binding(binding)?;
    let Some(directory) = cache_directory(root, binding, false)? else {
        return Ok(None);
    };
    let Some(mut file) = open_optional_regular(&directory, FILE_NAME)? else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(|_| StoreError::Io)?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(StoreError::TooLarge);
    }
    if metadata.len() < (MAGIC.len() + 4 + MAC_BYTES) as u64 {
        return Err(StoreError::Corrupt);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    io::Read::by_ref(&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| StoreError::Io)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(StoreError::TooLarge);
    }
    let tag_start = bytes
        .len()
        .checked_sub(MAC_BYTES)
        .ok_or(StoreError::Corrupt)?;
    let (authenticated, tag) = bytes.split_at(tag_start);
    let mut mac = new_mac(mac_key)?;
    mac.update(authenticated);
    mac.verify_slice(tag).map_err(|_| StoreError::Corrupt)?;
    let Some(prefix) = bytes.get(..MAGIC.len()) else {
        return Err(StoreError::Corrupt);
    };
    if prefix != MAGIC {
        return Err(StoreError::Corrupt);
    }
    let header_len = u32::from_le_bytes(
        bytes[MAGIC.len()..MAGIC.len() + 4]
            .try_into()
            .map_err(|_| StoreError::Corrupt)?,
    ) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(StoreError::Corrupt);
    }
    let payload_start = MAGIC.len() + 4 + header_len;
    let header_bytes = bytes
        .get(MAGIC.len() + 4..payload_start)
        .ok_or(StoreError::Corrupt)?;
    let header: Header = serde_json::from_slice(header_bytes).map_err(|_| StoreError::Corrupt)?;
    validate_binding(&header.binding).map_err(|_| StoreError::Corrupt)?;
    if header.version != VERSION || header.verified_at_unix_ms < 0 {
        return Err(StoreError::Corrupt);
    }
    let payload = bytes
        .get(payload_start..tag_start)
        .ok_or(StoreError::Corrupt)?;
    if payload.len() > MAX_CIPHERTEXT_BYTES || payload.len() as u64 != header.ciphertext_len {
        return Err(StoreError::Corrupt);
    }
    let ciphertext = match header.ciphertext_sha256 {
        Some(ref hash)
            if !payload.is_empty() && valid_sha256(hash) && sha256_hex(payload) == *hash =>
        {
            Some(payload.to_vec())
        }
        None if payload.is_empty() => None,
        _ => return Err(StoreError::Corrupt),
    };
    if header.binding != *binding {
        return Ok(None);
    }
    Ok(Some(BrowserDiskCacheValue {
        ciphertext,
        verified_at_unix_ms: header.verified_at_unix_ms,
        remote_updated_at_unix_ms: header.remote_updated_at_unix_ms,
    }))
}

pub(crate) fn remove_profile(
    root: &Path,
    plugin_id: &str,
    profile_id: &str,
) -> Result<(), StoreError> {
    validate_identifier(plugin_id)?;
    validate_identifier(profile_id)?;
    let Some(plugin) = plugin_directory(root, plugin_id, false)? else {
        return Ok(());
    };
    let profile_name = sha256_hex(profile_id.as_bytes());
    remove_child_directory(&plugin, &profile_name)
}

pub(crate) fn remove_plugin(root: &Path, plugin_id: &str) -> Result<(), StoreError> {
    validate_identifier(plugin_id)?;
    let Some(base) = base_directory(root, false)? else {
        return Ok(());
    };
    let plugin_name = sha256_hex(plugin_id.as_bytes());
    remove_child_directory(&base, &plugin_name)
}

fn cache_directory(
    root: &Path,
    binding: &BrowserDiskCacheBinding,
    create: bool,
) -> Result<Option<Dir>, StoreError> {
    let Some(plugin) = plugin_directory(root, &binding.plugin_id, create)? else {
        return Ok(None);
    };
    child_directory(&plugin, &sha256_hex(binding.profile_id.as_bytes()), create)
}

fn plugin_directory(root: &Path, plugin_id: &str, create: bool) -> Result<Option<Dir>, StoreError> {
    let Some(base) = base_directory(root, create)? else {
        return Ok(None);
    };
    child_directory(&base, &sha256_hex(plugin_id.as_bytes()), create)
}

fn base_directory(root: &Path, create: bool) -> Result<Option<Dir>, StoreError> {
    let root = Dir::open_ambient_dir(root, ambient_authority()).map_err(|_| StoreError::Io)?;
    child_directory(&root, DIRECTORY, create)
}

fn child_directory(parent: &Dir, name: &str, create: bool) -> Result<Option<Dir>, StoreError> {
    if create {
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        match parent.create_dir_with(name, &builder) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(StoreError::Io),
        }
    }
    let directory = match open_directory_nofollow(parent, name) {
        Ok(directory) => directory,
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(StoreError::UnsafePath),
    };
    #[cfg(unix)]
    if create {
        use std::os::unix::fs::PermissionsExt as _;
        directory
            .try_clone()
            .map_err(|_| StoreError::Io)?
            .into_std_file()
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|_| StoreError::Io)?;
    }
    Ok(Some(directory))
}

fn open_directory_nofollow(parent: &Dir, name: &str) -> io::Result<Dir> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(windows)]
    options.custom_flags(0x0200_0000 | 0x0020_0000); // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
    let file = parent.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(io::Error::other("unsafe cache directory"));
    }
    Ok(Dir::from_std_file(file.into_std()))
}

fn open_optional_regular(dir: &Dir, name: &str) -> Result<Option<cap_std::fs::File>, StoreError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(windows)]
    options.custom_flags(0x0020_0000); // OPEN_REPARSE_POINT
    let file = match dir.open_with(name, &options) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(StoreError::Io),
    };
    let metadata = file.metadata().map_err(|_| StoreError::Io)?;
    if !metadata.is_file() || is_reparse(&metadata) {
        return Err(StoreError::UnsafePath);
    }
    Ok(Some(file))
}

fn ensure_regular_or_missing(dir: &Dir, name: &str) -> Result<(), StoreError> {
    match dir.symlink_metadata(name) {
        Ok(metadata) if metadata.is_file() && !is_reparse(&metadata) => Ok(()),
        Ok(_) => Err(StoreError::UnsafePath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StoreError::Io),
    }
}

#[cfg(windows)]
fn is_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt as _;
    metadata.file_attributes() & 0x0000_0400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.is_symlink()
}

#[cfg(not(windows))]
fn replace_file(
    _: &Path,
    _: &BrowserDiskCacheBinding,
    directory: &Dir,
    temporary: &str,
) -> Result<(), StoreError> {
    directory
        .rename(temporary, directory, FILE_NAME)
        .map_err(|_| StoreError::Io)
}

#[cfg(windows)]
fn replace_file(
    root: &Path,
    binding: &BrowserDiskCacheBinding,
    _: &Dir,
    temporary: &str,
) -> Result<(), StoreError> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let parent = root
        .join(DIRECTORY)
        .join(sha256_hex(binding.plugin_id.as_bytes()))
        .join(sha256_hex(binding.profile_id.as_bytes()));
    let source = extended_windows_path(&parent.join(temporary))?;
    let destination = extended_windows_path(&parent.join(FILE_NAME))?;
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        eprintln!(
            "verified SSH sync browser cache file replace failed: {}",
            io::Error::last_os_error()
        );
        Err(StoreError::Io)
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn extended_windows_path(path: &Path) -> Result<Vec<u16>, StoreError> {
    use std::os::windows::ffi::OsStrExt as _;

    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    let verbatim = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    let unc = [b'\\' as u16, b'\\' as u16];
    let mut wide = if units.starts_with(&verbatim) {
        units
    } else if units.starts_with(&unc) {
        let mut wide: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
        wide.extend_from_slice(&units[2..]);
        wide
    } else if path.is_absolute()
        && units.len() >= 3
        && units[1] == b':' as u16
        && units[2] == b'\\' as u16
    {
        let mut wide = verbatim.to_vec();
        wide.extend_from_slice(&units);
        wide
    } else {
        return Err(StoreError::Io);
    };
    wide.push(0);
    Ok(wide)
}

#[cfg(unix)]
fn sync_directory(directory: &Dir) -> Result<(), StoreError> {
    directory
        .open(".")
        .and_then(|file| file.sync_all())
        .map_err(|_| StoreError::Io)
}

#[cfg(not(unix))]
fn sync_directory(_: &Dir) -> Result<(), StoreError> {
    Ok(())
}

fn remove_child_directory(parent: &Dir, name: &str) -> Result<(), StoreError> {
    let child = match open_directory_nofollow(parent, name) {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(StoreError::UnsafePath),
    };
    // No unknown descendants may be traversed during cleanup.
    for entry in child.read_dir(".").map_err(|_| StoreError::Io)? {
        let entry = entry.map_err(|_| StoreError::Io)?;
        let name = entry.file_name();
        let metadata = child.symlink_metadata(&name).map_err(|_| StoreError::Io)?;
        if metadata.is_dir() && !is_reparse(&metadata) {
            let name = name.to_str().ok_or(StoreError::UnsafePath)?;
            remove_child_directory(&child, name)?;
        } else if metadata.is_file() && !is_reparse(&metadata) {
            child.remove_file(&name).map_err(|_| StoreError::Io)?;
        } else {
            return Err(StoreError::UnsafePath);
        }
    }
    drop(child);
    parent.remove_dir(name).map_err(|_| StoreError::Io)?;
    sync_directory(parent)
}

fn validate_binding(binding: &BrowserDiskCacheBinding) -> Result<(), StoreError> {
    validate_identifier(&binding.plugin_id)?;
    validate_identifier(&binding.profile_id)?;
    if !valid_sha256(&binding.signer_sha256)
        || !valid_sha256(&binding.oauth_configuration_sha256)
        || binding.oauth_session_id.is_empty()
        || binding.oauth_session_id.len() > 256
        || binding.oauth_session_id.chars().any(char::is_control)
        || binding.canonical_source_url.is_empty()
        || binding.canonical_source_url.len() > 4096
        || binding.canonical_source_url.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidBinding);
    }
    let url = reqwest::Url::parse(&binding.canonical_source_url)
        .map_err(|_| StoreError::InvalidBinding)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.as_str() != binding.canonical_source_url
    {
        return Err(StoreError::InvalidBinding);
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        Err(StoreError::InvalidBinding)
    } else {
        Ok(())
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn new_mac(key: &[u8; 32]) -> Result<Hmac<Sha256>, StoreError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| StoreError::Corrupt)?;
    mac.update(MAC_DOMAIN);
    Ok(mac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const KEY: [u8; 32] = [7; 32];

    fn binding() -> BrowserDiskCacheBinding {
        BrowserDiskCacheBinding {
            plugin_id: "org.example.sync".into(),
            signer_sha256: "a".repeat(64),
            profile_id: "primary".into(),
            oauth_configuration_sha256: "b".repeat(64),
            oauth_session_id: "account-1".into(),
            canonical_source_url: "https://sync.example.test/v1/exchange".into(),
        }
    }

    fn path(root: &Path, binding: &BrowserDiskCacheBinding) -> std::path::PathBuf {
        root.join(DIRECTORY)
            .join(sha256_hex(binding.plugin_id.as_bytes()))
            .join(sha256_hex(binding.profile_id.as_bytes()))
            .join(FILE_NAME)
    }

    fn forge_header(file: &Path, change: impl FnOnce(&mut Header)) {
        let bytes = fs::read(file).unwrap();
        let old_len =
            u32::from_le_bytes(bytes[MAGIC.len()..MAGIC.len() + 4].try_into().unwrap()) as usize;
        let mut header: Header =
            serde_json::from_slice(&bytes[MAGIC.len() + 4..MAGIC.len() + 4 + old_len]).unwrap();
        change(&mut header);
        let encoded = serde_json::to_vec(&header).unwrap();
        let mut forged = Vec::new();
        forged.extend_from_slice(MAGIC);
        forged.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
        forged.extend_from_slice(&encoded);
        forged.extend_from_slice(&bytes[MAGIC.len() + 4 + old_len..]);
        fs::write(file, forged).unwrap();
    }

    #[test]
    fn overwrites_one_file_and_binds_every_identity_field() {
        let root = tempfile::tempdir().unwrap();
        let owner = binding();
        save(
            root.path(),
            &owner,
            Some(b"encrypted-first"),
            Some(41),
            &KEY,
        )
        .unwrap();
        save(
            root.path(),
            &owner,
            Some(b"encrypted-second"),
            Some(42),
            &KEY,
        )
        .unwrap();
        let value = load(root.path(), &owner, &KEY).unwrap().unwrap();
        assert_eq!(
            value.ciphertext.as_deref(),
            Some(b"encrypted-second".as_slice())
        );
        assert_eq!(value.remote_updated_at_unix_ms, Some(42));
        assert!(value.verified_at_unix_ms > 0);
        assert_eq!(
            fs::read_dir(path(root.path(), &owner).parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        for change in [
            Box::new(|b: &mut BrowserDiskCacheBinding| b.plugin_id = "other.plugin".into())
                as Box<dyn Fn(&mut BrowserDiskCacheBinding)>,
            Box::new(|b: &mut BrowserDiskCacheBinding| b.signer_sha256 = "c".repeat(64)),
            Box::new(|b: &mut BrowserDiskCacheBinding| b.profile_id = "other".into()),
            Box::new(|b: &mut BrowserDiskCacheBinding| {
                b.oauth_configuration_sha256 = "d".repeat(64)
            }),
            Box::new(|b: &mut BrowserDiskCacheBinding| b.oauth_session_id = "account-2".into()),
            Box::new(|b: &mut BrowserDiskCacheBinding| {
                b.canonical_source_url = "https://other.test/v1/exchange".into()
            }),
        ] {
            let mut other = owner.clone();
            change(&mut other);
            assert_eq!(load(root.path(), &other, &KEY).unwrap(), None);
        }
    }

    #[cfg(windows)]
    #[test]
    fn saves_and_replaces_cache_beyond_legacy_win32_path_limit() {
        use std::os::windows::ffi::OsStrExt as _;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("long-cache-root-".repeat(4));
        fs::create_dir(&root).unwrap();
        let owner = binding();
        assert!(path(&root, &owner).as_os_str().encode_wide().count() > 260);
        save(&root, &owner, Some(b"first"), None, &KEY).unwrap();
        save(&root, &owner, Some(b"second"), None, &KEY).unwrap();
        assert_eq!(
            load(&root, &owner, &KEY).unwrap().unwrap().ciphertext,
            Some(b"second".to_vec())
        );
    }

    #[test]
    fn detects_corruption_and_truncation() {
        let root = tempfile::tempdir().unwrap();
        let owner = binding();
        save(root.path(), &owner, Some(b"ciphertext"), None, &KEY).unwrap();
        let file = path(root.path(), &owner);
        let pristine = fs::read(&file).unwrap();
        let mut bytes = pristine.clone();
        bytes[pristine.len() - MAC_BYTES - 1] ^= 1;
        fs::write(&file, &bytes).unwrap();
        assert_eq!(load(root.path(), &owner, &KEY), Err(StoreError::Corrupt));
        let mut bytes = pristine;
        bytes.truncate(bytes.len() - 4);
        fs::write(&file, &bytes).unwrap();
        assert_eq!(load(root.path(), &owner, &KEY), Err(StoreError::Corrupt));
    }

    #[test]
    fn authenticates_header_and_rejects_a_different_key() {
        let root = tempfile::tempdir().unwrap();
        let owner = binding();
        save(root.path(), &owner, Some(b"ciphertext"), Some(41), &KEY).unwrap();
        let file = path(root.path(), &owner);
        assert_eq!(
            load(root.path(), &owner, &[8; 32]),
            Err(StoreError::Corrupt)
        );
        forge_header(&file, |header| {
            header.binding.oauth_session_id = "account-2".into()
        });
        assert_eq!(load(root.path(), &owner, &KEY), Err(StoreError::Corrupt));
        save(root.path(), &owner, Some(b"ciphertext"), Some(41), &KEY).unwrap();
        forge_header(&file, |header| {
            header.binding.canonical_source_url = "https://other.example.test/v1/exchange".into()
        });
        assert_eq!(load(root.path(), &owner, &KEY), Err(StoreError::Corrupt));
        save(root.path(), &owner, Some(b"ciphertext"), Some(41), &KEY).unwrap();
        forge_header(&file, |header| header.remote_updated_at_unix_ms = Some(42));
        assert_eq!(load(root.path(), &owner, &KEY), Err(StoreError::Corrupt));
    }

    #[test]
    fn authenticates_the_empty_marker() {
        let root = tempfile::tempdir().unwrap();
        let owner = binding();
        save(root.path(), &owner, None, None, &KEY).unwrap();
        let file = path(root.path(), &owner);
        forge_header(&file, |header| {
            header.ciphertext_sha256 = Some(sha256_hex(b"fake"));
            header.ciphertext_len = 4;
        });
        assert_eq!(load(root.path(), &owner, &KEY), Err(StoreError::Corrupt));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_cache_paths() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let owner = binding();
        save(root.path(), &owner, Some(b"ciphertext"), None, &KEY).unwrap();
        let file = path(root.path(), &owner);
        let original = file.with_extension("original");
        fs::rename(&file, &original).unwrap();
        symlink(&original, &file).unwrap();
        assert!(load(root.path(), &owner, &KEY).is_err());
        assert_eq!(
            save(root.path(), &owner, None, None, &KEY),
            Err(StoreError::UnsafePath)
        );
        fs::remove_file(&file).unwrap();
        let profile = file.parent().unwrap();
        let original_profile = profile.with_extension("original");
        fs::rename(profile, &original_profile).unwrap();
        symlink(&original_profile, profile).unwrap();
        assert!(load(root.path(), &owner, &KEY).is_err());
        assert!(save(root.path(), &owner, None, None, &KEY).is_err());
    }

    #[test]
    fn verified_absence_is_distinct_from_missing_cache() {
        let root = tempfile::tempdir().unwrap();
        let owner = binding();
        assert_eq!(load(root.path(), &owner, &KEY).unwrap(), None);
        save(root.path(), &owner, None, None, &KEY).unwrap();
        let value = load(root.path(), &owner, &KEY).unwrap().unwrap();
        assert_eq!(value.ciphertext, None);
        remove_profile(root.path(), &owner.plugin_id, &owner.profile_id).unwrap();
        assert_eq!(load(root.path(), &owner, &KEY).unwrap(), None);
        save(root.path(), &owner, None, None, &KEY).unwrap();
        remove_plugin(root.path(), &owner.plugin_id).unwrap();
        assert_eq!(load(root.path(), &owner, &KEY).unwrap(), None);
    }
}
