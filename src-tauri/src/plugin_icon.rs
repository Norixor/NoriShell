//! Bounded, host-owned cache for the fixed Norixor plugin icon endpoint.
//!
//! Icons are mutable display material. This module never derives publisher,
//! package, permission, or runtime authority from an icon response.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::{PluginIconReadResponse, PluginIconReadScope, PluginIconSource, PluginId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::norixor_marketplace::PluginIconFetch;

pub(crate) const ICON_MAX_BYTES: usize = 64 * 1024;
const ICON_TTL_MILLIS: i64 = 300_000;
const ICON_CACHE_MAX_ENTRIES: usize = 256;
const ICON_CACHE_MAX_BYTES: u64 = 24 * 1024 * 1024;
const ICON_CACHE_ORIGIN: &str = "https://api.norixor.org";
const ICON_FETCH_CONCURRENCY: usize = 4;

#[derive(Clone)]
pub(crate) struct PluginIconCache {
    root: Arc<PathBuf>,
    key_locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
    network: Arc<NetworkLimit>,
}

struct NetworkLimit {
    available: Mutex<usize>,
    wake: Condvar,
}

struct NetworkPermit {
    limit: Arc<NetworkLimit>,
}

impl Drop for NetworkPermit {
    fn drop(&mut self) {
        let mut available = self
            .limit
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *available = available.saturating_add(1).min(ICON_FETCH_CONCURRENCY);
        self.limit.wake.notify_one();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IconFetchOutcome {
    Image {
        mime_type: String,
        etag: String,
        bytes: Vec<u8>,
    },
    NotModified,
    NotFound,
    Unavailable,
}

/// Public package identity facts retained by the installer. They are only
/// used to keep a cached display icon bound to the exact installed artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InstalledIconProvenance {
    pub(crate) version: String,
    pub(crate) package_sha256: String,
    pub(crate) publisher_key_base64: String,
    pub(crate) publisher_signature_base64: String,
}

impl From<Result<PluginIconFetch, crate::norixor_marketplace::MarketplaceFetchError>>
    for IconFetchOutcome
{
    fn from(
        value: Result<PluginIconFetch, crate::norixor_marketplace::MarketplaceFetchError>,
    ) -> Self {
        match value {
            Ok(PluginIconFetch::Image {
                mime_type,
                etag,
                bytes,
            }) => Self::Image {
                mime_type,
                etag,
                bytes,
            },
            Ok(PluginIconFetch::NotModified) => Self::NotModified,
            Ok(PluginIconFetch::NotFound) => Self::NotFound,
            Err(_) => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IconMetadata {
    format_version: u8,
    origin: String,
    scope: PluginIconReadScope,
    plugin_id: PluginId,
    mime_type: String,
    etag: String,
    sha256: String,
    fetched_at_unix_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    installed_provenance: Option<InstalledIconProvenance>,
}

#[derive(Debug, Clone)]
struct CachedIcon {
    metadata: IconMetadata,
    bytes: Vec<u8>,
}

impl PluginIconCache {
    pub(crate) fn new(app_data_directory: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = app_data_directory.as_ref().join("plugin-icon-cache");
        ensure_private_cache_directory(&root)?;
        Ok(Self {
            root: Arc::new(root),
            key_locks: Arc::new(Mutex::new(BTreeMap::new())),
            network: Arc::new(NetworkLimit {
                available: Mutex::new(ICON_FETCH_CONCURRENCY),
                wake: Condvar::new(),
            }),
        })
    }

    pub(crate) fn read_or_fetch<F>(
        &self,
        plugin_id: PluginId,
        scope: PluginIconReadScope,
        installed_provenance: Option<InstalledIconProvenance>,
        allow_network: bool,
        refresh: bool,
        fetch: F,
    ) -> PluginIconReadResponse
    where
        F: FnOnce(Option<&str>) -> IconFetchOutcome,
    {
        let key = cache_key(&plugin_id, scope);
        let key_lock = {
            let mut locks = self
                .key_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _key_guard = key_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cached = self
            .load(&key, &plugin_id, scope, installed_provenance.as_ref())
            .ok()
            .flatten();
        if cached
            .as_ref()
            .is_some_and(|entry| is_fresh(entry, now_unix_ms()))
            && !refresh
        {
            return response_from_cached(plugin_id, cached.expect("checked cache"));
        }
        if !allow_network {
            return cached
                .map(|entry| response_from_cached(plugin_id.clone(), entry))
                .unwrap_or_else(|| empty_response(plugin_id));
        }

        let _network = self.acquire_network_permit();
        match fetch(cached.as_ref().map(|entry| entry.metadata.etag.as_str())) {
            IconFetchOutcome::Image {
                mime_type,
                etag,
                bytes,
            } => match CachedIcon::new(
                plugin_id.clone(),
                scope,
                installed_provenance,
                mime_type,
                etag,
                bytes,
            ) {
                Ok(entry) => {
                    if self.store(&key, &entry).is_ok() {
                        self.trim();
                    }
                    response_from_cached_with_source(plugin_id, entry, PluginIconSource::Network)
                }
                Err(()) => cached
                    .map(|entry| response_from_cached(plugin_id.clone(), entry))
                    .unwrap_or_else(|| empty_response(plugin_id)),
            },
            IconFetchOutcome::NotModified => cached
                .map(|mut entry| {
                    entry.metadata.fetched_at_unix_ms = now_unix_ms();
                    let _ = self.store(&key, &entry);
                    response_from_cached(plugin_id.clone(), entry)
                })
                .unwrap_or_else(|| empty_response(plugin_id)),
            IconFetchOutcome::NotFound => {
                self.remove(&key);
                empty_response(plugin_id)
            }
            IconFetchOutcome::Unavailable => cached
                .map(|entry| response_from_cached(plugin_id.clone(), entry))
                .unwrap_or_else(|| empty_response(plugin_id)),
        }
    }

    fn acquire_network_permit(&self) -> NetworkPermit {
        let mut available = self
            .network
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *available == 0 {
            available = self
                .network
                .wake
                .wait(available)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *available -= 1;
        NetworkPermit {
            limit: self.network.clone(),
        }
    }

    fn paths(&self, key: &str) -> (PathBuf, PathBuf) {
        (
            self.root.join(format!("{key}.bin")),
            self.root.join(format!("{key}.json")),
        )
    }

    fn load(
        &self,
        key: &str,
        plugin_id: &PluginId,
        scope: PluginIconReadScope,
        installed_provenance: Option<&InstalledIconProvenance>,
    ) -> std::io::Result<Option<CachedIcon>> {
        ensure_private_cache_directory(&self.root)?;
        let (data_path, metadata_path) = self.paths(key);
        let metadata_bytes = match read_regular_bounded(&metadata_path, 4 * 1024)? {
            Some(value) => value,
            None => return Ok(None),
        };
        let metadata = match serde_json::from_slice::<IconMetadata>(&metadata_bytes) {
            Ok(metadata)
                if metadata.format_version == 1
                    && metadata.origin == ICON_CACHE_ORIGIN
                    && metadata.scope == scope
                    && metadata.plugin_id == *plugin_id
                    && metadata.installed_provenance.as_ref() == installed_provenance
                    && valid_mime_type(&metadata.mime_type)
                    && valid_etag(&metadata.etag)
                    && valid_digest(&metadata.sha256)
                    && metadata.fetched_at_unix_ms > 0 =>
            {
                metadata
            }
            _ => return Ok(None),
        };
        let Some(bytes) = read_regular_bounded(&data_path, ICON_MAX_BYTES)? else {
            return Ok(None);
        };
        if !valid_image(&metadata.mime_type, &bytes)
            || hex::encode(Sha256::digest(&bytes)) != metadata.sha256
        {
            return Ok(None);
        }
        Ok(Some(CachedIcon { metadata, bytes }))
    }

    fn store(&self, key: &str, entry: &CachedIcon) -> std::io::Result<()> {
        ensure_private_cache_directory(&self.root)?;
        let (data_path, metadata_path) = self.paths(key);
        let metadata = serde_json::to_vec(&entry.metadata)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        write_atomic_private(&data_path, &entry.bytes)?;
        write_atomic_private(&metadata_path, &metadata)
    }

    fn remove(&self, key: &str) {
        let (data_path, metadata_path) = self.paths(key);
        remove_regular_if_present(&data_path);
        remove_regular_if_present(&metadata_path);
    }

    fn trim(&self) {
        let Ok(entries) = fs::read_dir(&*self.root) else {
            return;
        };
        let mut data_files = Vec::new();
        let mut total = 0_u64;
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(key) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(cache_key_from_data_name)
            else {
                continue;
            };
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                continue;
            }
            total = total.saturating_add(metadata.len());
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            data_files.push((modified, metadata.len(), key.to_owned()));
        }
        data_files.sort_by_key(|(modified, _, _)| *modified);
        while data_files.len() > ICON_CACHE_MAX_ENTRIES || total > ICON_CACHE_MAX_BYTES {
            let Some((_, size, key)) = data_files.first().cloned() else {
                break;
            };
            data_files.remove(0);
            total = total.saturating_sub(size);
            self.remove(&key);
        }
    }
}

impl CachedIcon {
    fn new(
        plugin_id: PluginId,
        scope: PluginIconReadScope,
        installed_provenance: Option<InstalledIconProvenance>,
        mime_type: String,
        etag: String,
        bytes: Vec<u8>,
    ) -> Result<Self, ()> {
        if !valid_mime_type(&mime_type) || !valid_etag(&etag) || !valid_image(&mime_type, &bytes) {
            return Err(());
        }
        Ok(Self {
            metadata: IconMetadata {
                format_version: 1,
                origin: ICON_CACHE_ORIGIN.to_owned(),
                scope,
                plugin_id,
                mime_type,
                etag,
                sha256: hex::encode(Sha256::digest(&bytes)),
                fetched_at_unix_ms: now_unix_ms(),
                installed_provenance,
            },
            bytes,
        })
    }
}

fn cache_key(plugin_id: &PluginId, scope: PluginIconReadScope) -> String {
    let scope = match scope {
        PluginIconReadScope::Catalog => "catalog",
        PluginIconReadScope::Installed => "installed",
    };
    hex::encode(Sha256::digest(format!(
        "{ICON_CACHE_ORIGIN}\0{scope}\0{}",
        plugin_id.as_str()
    )))
}

fn cache_key_from_data_name(name: &str) -> Option<&str> {
    let key = name.strip_suffix(".bin")?;
    (key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(key)
}

fn response_from_cached(plugin_id: PluginId, entry: CachedIcon) -> PluginIconReadResponse {
    response_from_cached_with_source(plugin_id, entry, PluginIconSource::Cache)
}

fn response_from_cached_with_source(
    plugin_id: PluginId,
    entry: CachedIcon,
    source: PluginIconSource,
) -> PluginIconReadResponse {
    PluginIconReadResponse {
        plugin_id,
        data_url: Some(format!(
            "data:{};base64,{}",
            entry.metadata.mime_type,
            BASE64.encode(entry.bytes)
        )),
        sha256: Some(entry.metadata.sha256),
        source: Some(source),
    }
}

fn empty_response(plugin_id: PluginId) -> PluginIconReadResponse {
    PluginIconReadResponse {
        plugin_id,
        data_url: None,
        sha256: None,
        source: None,
    }
}

fn valid_mime_type(value: &str) -> bool {
    matches!(value, "image/png" | "image/jpeg" | "image/webp")
}

fn valid_etag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.is_ascii()
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_image(mime_type: &str, bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > ICON_MAX_BYTES || !valid_mime_type(mime_type) {
        return false;
    }
    let detected = match mime_type {
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => true,
        "image/jpeg" if bytes.starts_with(&[0xff, 0xd8]) => true,
        "image/webp" if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => true,
        _ => false,
    };
    detected
        && imagesize::blob_size(bytes)
            .is_ok_and(|size| (1..=1024).contains(&size.width) && (1..=1024).contains(&size.height))
        && fully_decodes(mime_type, bytes)
}

fn fully_decodes(mime_type: &str, bytes: &[u8]) -> bool {
    let format = match mime_type {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        "image/webp" => image::ImageFormat::WebP,
        _ => return false,
    };
    let reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.decode().is_ok_and(|image| {
        (1..=1024).contains(&image.width()) && (1..=1024).contains(&image.height())
    })
}

fn is_fresh(entry: &CachedIcon, now: i64) -> bool {
    now.saturating_sub(entry.metadata.fetched_at_unix_ms) <= ICON_TTL_MILLIS
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn ensure_private_cache_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other("invalid plugin icon cache directory"));
    }
    Ok(())
}

fn read_regular_bounded(path: &Path, maximum_bytes: usize) -> std::io::Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > maximum_bytes as u64
    {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let actual = file.metadata()?;
    if !actual.is_file() || actual.len() > maximum_bytes as u64 {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(actual.len()).unwrap_or(0));
    std::io::Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    (bytes.len() <= maximum_bytes)
        .then_some(bytes)
        .ok_or_else(|| std::io::Error::other("plugin icon cache entry exceeds its byte limit"))
        .map(Some)
}

fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("plugin icon cache has no parent"))?;
    ensure_private_cache_directory(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("icon"),
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        remove_regular_if_present(&temporary);
    }
    result
}

fn remove_regular_if_present(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_file()
        && !metadata.file_type().is_symlink()
    {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ICON_MAX_BYTES, IconFetchOutcome, InstalledIconProvenance, PluginIconCache, cache_key,
        empty_response,
    };
    use norishell_core_api::{PluginIconReadScope, PluginIconSource, PluginId};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    fn plugin_id() -> PluginId {
        PluginId::parse("com.norishell.fixture").expect("fixture id")
    }

    fn png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode fixture png");
        bytes.into_inner()
    }

    fn image() -> IconFetchOutcome {
        IconFetchOutcome::Image {
            mime_type: "image/png".to_owned(),
            etag: "\"fixture\"".to_owned(),
            bytes: png(),
        }
    }

    fn installed_provenance(package_sha256: &str) -> InstalledIconProvenance {
        InstalledIconProvenance {
            version: "1.0.0".to_owned(),
            package_sha256: package_sha256.to_owned(),
            publisher_key_base64: "publisher-key".to_owned(),
            publisher_signature_base64: "publisher-signature".to_owned(),
        }
    }

    #[test]
    fn fetches_200_and_revalidates_304_from_private_cache() {
        let directory = tempfile::tempdir().expect("directory");
        let cache = PluginIconCache::new(directory.path()).expect("cache");
        let first = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            false,
            |_| image(),
        );
        assert_eq!(first.source, Some(PluginIconSource::Network));
        let revalidated = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            true,
            |etag| {
                assert_eq!(etag, Some("\"fixture\""));
                IconFetchOutcome::NotModified
            },
        );
        assert_eq!(revalidated.source, Some(PluginIconSource::Cache));
        assert_eq!(first.sha256, revalidated.sha256);
    }

    #[test]
    fn not_found_evicts_network_cache() {
        let directory = tempfile::tempdir().expect("directory");
        let cache = PluginIconCache::new(directory.path()).expect("cache");
        let _ = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            false,
            |_| image(),
        );
        let missing = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            true,
            |_| IconFetchOutcome::NotFound,
        );
        assert_eq!(missing, empty_response(plugin_id()));
        let fallback = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            false,
            |_| IconFetchOutcome::Unavailable,
        );
        assert_eq!(fallback, empty_response(plugin_id()));
    }

    #[test]
    fn invalid_or_oversized_network_images_do_not_replace_a_valid_cache() {
        let directory = tempfile::tempdir().expect("directory");
        let cache = PluginIconCache::new(directory.path()).expect("cache");
        let accepted = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            false,
            |_| image(),
        );
        for outcome in [
            IconFetchOutcome::Image {
                mime_type: "text/html".to_owned(),
                etag: "\"next\"".to_owned(),
                bytes: png(),
            },
            IconFetchOutcome::Image {
                mime_type: "image/png".to_owned(),
                etag: "\"next\"".to_owned(),
                bytes: vec![0; ICON_MAX_BYTES + 1],
            },
            IconFetchOutcome::Image {
                mime_type: "image/png".to_owned(),
                etag: "\"next\"".to_owned(),
                bytes: b"not an image".to_vec(),
            },
            IconFetchOutcome::Image {
                mime_type: "image/png".to_owned(),
                etag: "\"next\"".to_owned(),
                // A PNG IHDR supplies dimensions before any pixel data. It
                // must still fail the full decode and preserve the old icon.
                bytes: png()[..24].to_vec(),
            },
        ] {
            let fallback = cache.read_or_fetch(
                plugin_id(),
                PluginIconReadScope::Catalog,
                None,
                true,
                true,
                |_| outcome,
            );
            assert_eq!(fallback.sha256, accepted.sha256);
            assert_eq!(fallback.source, Some(PluginIconSource::Cache));
        }
    }

    #[test]
    fn network_failure_and_restart_keep_an_offline_cache() {
        let directory = tempfile::tempdir().expect("directory");
        let first = PluginIconCache::new(directory.path()).expect("cache");
        let expected = first.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            false,
            |_| image(),
        );
        let restarted = PluginIconCache::new(directory.path()).expect("restarted cache");
        let cached = restarted.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            true,
            |_| IconFetchOutcome::Unavailable,
        );
        assert_eq!(cached.sha256, expected.sha256);
        assert_eq!(cached.source, Some(PluginIconSource::Cache));
    }

    #[test]
    fn historical_installed_cache_requires_the_exact_artifact_provenance() {
        let directory = tempfile::tempdir().expect("directory");
        let provenance = installed_provenance(&"a".repeat(64));
        let cache = PluginIconCache::new(directory.path()).expect("cache");
        let expected = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Installed,
            Some(provenance.clone()),
            true,
            false,
            |_| image(),
        );
        let offline = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Installed,
            Some(provenance),
            false,
            true,
            |_| panic!("historical cache must not make a network request"),
        );
        assert_eq!(offline.sha256, expected.sha256);
        let changed_artifact = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Installed,
            Some(installed_provenance(&"b".repeat(64))),
            false,
            false,
            |_| panic!("mismatched provenance must not make a network request"),
        );
        assert_eq!(changed_artifact, empty_response(plugin_id()));
    }

    #[test]
    fn scope_is_part_of_the_cache_identity() {
        assert_ne!(
            cache_key(&plugin_id(), PluginIconReadScope::Catalog),
            cache_key(&plugin_id(), PluginIconReadScope::Installed)
        );
    }

    #[test]
    fn concurrent_same_key_reads_share_the_fresh_result() {
        let directory = tempfile::tempdir().expect("directory");
        let cache = PluginIconCache::new(directory.path()).expect("cache");
        let calls = Arc::new(AtomicUsize::new(0));
        let first_cache = cache.clone();
        let first_calls = calls.clone();
        let first = thread::spawn(move || {
            first_cache.read_or_fetch(
                plugin_id(),
                PluginIconReadScope::Catalog,
                None,
                true,
                false,
                |_| {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(30));
                    image()
                },
            )
        });
        let second_cache = cache.clone();
        let second_calls = calls.clone();
        let second = thread::spawn(move || {
            second_cache.read_or_fetch(
                plugin_id(),
                PluginIconReadScope::Catalog,
                None,
                true,
                false,
                |_| {
                    second_calls.fetch_add(1, Ordering::SeqCst);
                    image()
                },
            )
        });
        assert_eq!(
            first.join().expect("first read").sha256,
            second.join().expect("second read").sha256
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_data_is_rejected() {
        use std::{fs, os::unix::fs::symlink};

        let directory = tempfile::tempdir().expect("directory");
        let cache = PluginIconCache::new(directory.path()).expect("cache");
        let _ = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            false,
            |_| image(),
        );
        let key = cache_key(&plugin_id(), PluginIconReadScope::Catalog);
        let (data, _) = cache.paths(&key);
        let target = directory.path().join("outside-icon");
        fs::write(&target, png()).expect("outside data");
        fs::remove_file(&data).expect("remove cache data");
        symlink(&target, &data).expect("replace with symlink");
        let rejected = cache.read_or_fetch(
            plugin_id(),
            PluginIconReadScope::Catalog,
            None,
            true,
            false,
            |_| IconFetchOutcome::Unavailable,
        );
        assert_eq!(rejected, empty_response(plugin_id()));
    }
}
