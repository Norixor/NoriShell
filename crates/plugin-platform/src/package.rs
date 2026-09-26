use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::{Cursor, Read, Seek},
    path::{Path, PathBuf},
};

use norishell_core_api::{
    CoreApiVersion, PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PLUGIN_THEME_PROTOCOL_MINOR,
    PluginCapability, PluginId, PluginPackageKind, ThemeDefinition,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive};

use crate::{
    InspectedPluginProtocols, InspectedPluginSettings, InspectedPluginWorkflows,
    PLUGIN_PROTOCOLS_ASSET_PATH, PLUGIN_SETTINGS_ASSET_PATH, PLUGIN_WORKFLOWS_ASSET_PATH,
    PluginPlatformError, Result, inspect_plugin_protocols, inspect_plugin_settings,
    inspect_plugin_workflows,
};

#[derive(Debug, Clone, Copy)]
pub struct PackageLimits {
    pub max_archive_bytes: u64,
    pub max_files: usize,
    pub max_single_file_bytes: u64,
    pub max_uncompressed_bytes: u64,
    pub max_manifest_bytes: u64,
    pub max_theme_definition_bytes: u64,
}

impl Default for PackageLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 64 * 1024 * 1024,
            max_files: 512,
            max_single_file_bytes: 32 * 1024 * 1024,
            max_uncompressed_bytes: 128 * 1024 * 1024,
            max_manifest_bytes: 64 * 1024,
            max_theme_definition_bytes: norishell_core_api::MAX_THEME_DEFINITION_BYTES as u64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManifest {
    pub plugin_id: PluginId,
    pub name: String,
    pub publisher: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_signature: Option<String>,
    pub version: String,
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub platform: String,
    pub architectures: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_url: Option<String>,
    pub capabilities: Vec<PluginCapability>,
    pub minimum_app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_core_api_version: Option<CoreApiVersion>,
}

pub fn plugin_core_api_compatible(manifest: &PluginManifest) -> bool {
    // The former sshSync broker is no longer an authority for a plugin-owned
    // exchange. A sync package must explicitly declare the Core data API it uses.
    if manifest.capabilities.contains(&PluginCapability::SshSync)
        && manifest.minimum_core_api_version.is_none()
    {
        return false;
    }
    manifest.minimum_core_api_version.is_none_or(|minimum| {
        minimum.major == CoreApiVersion::current().major
            && minimum.minor <= CoreApiVersion::current().minor
    })
}

#[derive(Debug, Clone)]
pub struct InspectedPackage {
    pub manifest: PluginManifest,
    pub package_size: u64,
    pub package_sha256: [u8; 32],
    pub files: Vec<InspectedFile>,
    pub package_kind: PluginPackageKind,
    pub theme_definition: Option<ThemeDefinition>,
    pub theme_definition_sha256: Option<[u8; 32]>,
    pub settings: Option<InspectedPluginSettings>,
    pub protocols: Option<InspectedPluginProtocols>,
    pub workflows: Option<InspectedPluginWorkflows>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedFile {
    pub relative_path: PathBuf,
    pub uncompressed_size: u64,
}

/// Inspects a package explicitly selected by the local user. This path does not
/// assert a publisher identity; it only establishes archive integrity, manifest
/// compatibility and a content hash that is rechecked during extraction.
pub fn inspect_local_package(
    package_path: &Path,
    current_app_version: &Version,
    current_architecture: &str,
    limits: PackageLimits,
) -> Result<InspectedPackage> {
    let mut file = open_regular_package(package_path)?;
    let metadata = file.metadata()?;
    if metadata.len() == 0 || metadata.len() > limits.max_archive_bytes {
        return Err(PluginPlatformError::PackageTooLarge);
    }
    let package_bytes = read_package_snapshot(&mut file, limits.max_archive_bytes)?;
    let package_sha256: [u8; 32] = Sha256::digest(&package_bytes).into();
    let package_size = package_bytes.len() as u64;
    let mut archive = ZipArchive::new(Cursor::new(package_bytes))?;
    let files = validate_archive(&mut archive, limits)?;
    let manifest = read_archive_manifest(&mut archive, limits)?;
    validate_local_manifest(&manifest, current_app_version, current_architecture)?;
    let (package_kind, theme_definition, theme_definition_sha256) =
        inspect_package_kind(&mut archive, &files, &manifest, limits)?;
    let (settings, protocols, workflows) = if package_kind == PluginPackageKind::Wasm {
        (
            inspect_settings_asset(&mut archive)?,
            inspect_protocols_asset(&mut archive)?,
            inspect_workflows_asset(&mut archive)?,
        )
    } else {
        (None, None, None)
    };
    Ok(InspectedPackage {
        manifest,
        package_size,
        package_sha256,
        files,
        package_kind,
        theme_definition,
        theme_definition_sha256,
        settings,
        protocols,
        workflows,
    })
}

fn read_archive_manifest<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    limits: PackageLimits,
) -> Result<PluginManifest> {
    let mut manifest_file = archive
        .by_name("manifest.json")
        .map_err(|_| PluginPlatformError::InvalidArchive)?;
    if manifest_file.is_dir() || manifest_file.size() == 0 {
        return Err(PluginPlatformError::InvalidArchive);
    }
    if manifest_file.size() > limits.max_manifest_bytes {
        return Err(PluginPlatformError::ExtractionLimitExceeded);
    }
    serde_json::from_reader(&mut manifest_file).map_err(|_| PluginPlatformError::InvalidArchive)
}

fn inspect_package_kind<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    files: &[InspectedFile],
    manifest: &PluginManifest,
    limits: PackageLimits,
) -> Result<(PluginPackageKind, Option<ThemeDefinition>, Option<[u8; 32]>)> {
    let contains = |path: &str| {
        files
            .iter()
            .any(|file| file.relative_path == Path::new(path))
    };
    let has_wasm = contains("plugin.wasm");
    let has_theme = contains("assets/theme.json");
    if has_theme {
        if has_wasm
            || files.len() != 2
            || !contains("manifest.json")
            || manifest.protocol_major != PLUGIN_PROTOCOL_MAJOR
            || manifest.protocol_minor != PLUGIN_THEME_PROTOCOL_MINOR
            || !manifest.capabilities.is_empty()
        {
            return Err(PluginPlatformError::InvalidArchive);
        }
        let (definition, sha256) = inspect_theme_asset(archive, limits)?;
        return Ok((PluginPackageKind::Theme, Some(definition), Some(sha256)));
    }
    if !has_wasm
        || manifest.protocol_major != PLUGIN_PROTOCOL_MAJOR
        || manifest.protocol_minor != PLUGIN_PROTOCOL_MINOR
    {
        return Err(PluginPlatformError::InvalidArchive);
    }
    Ok((PluginPackageKind::Wasm, None, None))
}

fn inspect_theme_asset<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    limits: PackageLimits,
) -> Result<(ThemeDefinition, [u8; 32])> {
    let asset = archive
        .by_name("assets/theme.json")
        .map_err(|_| PluginPlatformError::InvalidArchive)?;
    if asset.is_dir() || asset.size() == 0 || asset.size() > limits.max_theme_definition_bytes {
        return Err(PluginPlatformError::InvalidArchive);
    }
    let expected_size = asset.size();
    let mut bytes = Vec::with_capacity(
        usize::try_from(expected_size).map_err(|_| PluginPlatformError::ExtractionLimitExceeded)?,
    );
    asset
        .take(limits.max_theme_definition_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected_size {
        return Err(PluginPlatformError::InvalidArchive);
    }
    let definition =
        ThemeDefinition::parse_json(&bytes).map_err(|_| PluginPlatformError::InvalidArchive)?;
    Ok((definition, Sha256::digest(&bytes).into()))
}

fn inspect_settings_asset<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Option<InspectedPluginSettings>> {
    let asset = match archive.by_name(PLUGIN_SETTINGS_ASSET_PATH) {
        Ok(asset) => asset,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if asset.is_dir()
        || asset.size() == 0
        || asset.size() > crate::MAX_PLUGIN_SETTINGS_SCHEMA_BYTES as u64
    {
        return Err(PluginPlatformError::InvalidSettingsSchema);
    }
    let mut bytes = Vec::new();
    asset
        .take(crate::MAX_PLUGIN_SETTINGS_SCHEMA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    inspect_plugin_settings(&bytes).map(Some)
}

/// Reads settings from an already verified, immutable package byte snapshot.
pub fn inspect_settings_asset_from_package_snapshot(
    package_bytes: &[u8],
) -> Result<Option<InspectedPluginSettings>> {
    let mut archive = ZipArchive::new(Cursor::new(package_bytes))?;
    inspect_settings_asset(&mut archive)
}

fn inspect_protocols_asset<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Option<InspectedPluginProtocols>> {
    let asset = match archive.by_name(PLUGIN_PROTOCOLS_ASSET_PATH) {
        Ok(asset) => asset,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if asset.is_dir()
        || asset.size() == 0
        || asset.size() > crate::MAX_PLUGIN_PROTOCOL_CATALOG_BYTES as u64
    {
        return Err(PluginPlatformError::InvalidProtocolCatalog);
    }
    let mut bytes = Vec::new();
    asset
        .take(crate::MAX_PLUGIN_PROTOCOL_CATALOG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    inspect_plugin_protocols(&bytes).map(Some)
}

/// Reads protocol declarations from an already verified, immutable package byte
/// snapshot. Absent declarations represent an empty catalog, not permission.
pub fn inspect_protocols_asset_from_package_snapshot(
    package_bytes: &[u8],
) -> Result<Option<InspectedPluginProtocols>> {
    let mut archive = ZipArchive::new(Cursor::new(package_bytes))?;
    inspect_protocols_asset(&mut archive)
}

fn inspect_workflows_asset<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<Option<InspectedPluginWorkflows>> {
    let asset = match archive.by_name(PLUGIN_WORKFLOWS_ASSET_PATH) {
        Ok(asset) => asset,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if asset.is_dir()
        || asset.size() == 0
        || asset.size() > crate::MAX_PLUGIN_WORKFLOW_CATALOG_BYTES as u64
    {
        return Err(PluginPlatformError::InvalidWorkflowCatalog);
    }
    let mut bytes = Vec::new();
    asset
        .take(crate::MAX_PLUGIN_WORKFLOW_CATALOG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    inspect_plugin_workflows(&bytes).map(Some)
}

/// Reads workflow declarations only from an already verified immutable package
/// byte snapshot. The catalog is metadata, never an executable plan.
pub fn inspect_workflows_asset_from_package_snapshot(
    package_bytes: &[u8],
) -> Result<Option<InspectedPluginWorkflows>> {
    let mut archive = ZipArchive::new(Cursor::new(package_bytes))?;
    inspect_workflows_asset(&mut archive)
}

pub(crate) fn extract_package(
    package_path: &Path,
    destination: &Path,
    expected: &InspectedPackage,
    limits: PackageLimits,
) -> Result<()> {
    let mut file = open_regular_package(package_path)?;
    let metadata = file.metadata()?;
    if metadata.len() != expected.package_size {
        return Err(PluginPlatformError::PackageHashMismatch);
    }
    let package_bytes = read_package_snapshot(&mut file, limits.max_archive_bytes)?;
    if package_bytes.len() as u64 != expected.package_size
        || <[u8; 32]>::from(Sha256::digest(&package_bytes)) != expected.package_sha256
    {
        return Err(PluginPlatformError::PackageHashMismatch);
    }
    let mut archive = ZipArchive::new(Cursor::new(package_bytes))?;
    let current_files = validate_archive(&mut archive, limits)?;
    if current_files != expected.files {
        return Err(PluginPlatformError::InvalidArchive);
    }
    let manifest = read_archive_manifest(&mut archive, limits)?;
    if manifest != expected.manifest {
        return Err(PluginPlatformError::ManifestMismatch);
    }
    let (package_kind, theme_definition, theme_definition_sha256) =
        inspect_package_kind(&mut archive, &current_files, &manifest, limits)?;
    if package_kind != expected.package_kind
        || theme_definition != expected.theme_definition
        || theme_definition_sha256 != expected.theme_definition_sha256
    {
        return Err(PluginPlatformError::InvalidArchive);
    }
    for index in 0..archive.len() {
        let source = archive.by_index(index)?;
        let relative = validate_path(source.name(), source.is_dir())?;
        let output = destination.join(&relative);
        if source.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        let parent = output
            .parent()
            .ok_or(PluginPlatformError::RejectedArchivePath)?;
        std::fs::create_dir_all(parent)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut target = options.open(&output)?;
        let expected_size = source.size();
        let copied = std::io::copy(
            &mut source.take(limits.max_single_file_bytes + 1),
            &mut target,
        )?;
        if copied != expected_size || copied > limits.max_single_file_bytes {
            return Err(PluginPlatformError::ExtractionLimitExceeded);
        }
        target.sync_all()?;
    }
    Ok(())
}

fn validate_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    limits: PackageLimits,
) -> Result<Vec<InspectedFile>> {
    if archive.is_empty() || archive.len() > limits.max_files {
        return Err(PluginPlatformError::ExtractionLimitExceeded);
    }
    let mut names = BTreeSet::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    let mut manifest_count = 0;
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index)?;
        if file.encrypted()
            || file.is_symlink()
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
            || file.size() > limits.max_single_file_bytes
        {
            return Err(PluginPlatformError::InvalidArchive);
        }
        let relative = validate_path(file.name(), file.is_dir())?;
        let collision_key = file.name().to_ascii_lowercase();
        if !names.insert(collision_key) {
            return Err(PluginPlatformError::RejectedArchivePath);
        }
        total = total
            .checked_add(file.size())
            .ok_or(PluginPlatformError::ExtractionLimitExceeded)?;
        if total > limits.max_uncompressed_bytes {
            return Err(PluginPlatformError::ExtractionLimitExceeded);
        }
        if !file.is_dir() {
            if relative == Path::new("manifest.json") {
                manifest_count += 1;
            }
            files.push(InspectedFile {
                relative_path: relative,
                uncompressed_size: file.size(),
            });
        }
    }
    if manifest_count != 1 {
        return Err(PluginPlatformError::InvalidArchive);
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn validate_path(name: &str, is_directory: bool) -> Result<PathBuf> {
    if name.is_empty()
        || !name.is_ascii()
        || name.contains(['\\', '\0'])
        || name.starts_with('/')
        || name
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || (is_directory != name.ends_with('/'))
    {
        return Err(PluginPlatformError::RejectedArchivePath);
    }
    let trimmed = name.trim_end_matches('/');
    let allowed = trimmed == "manifest.json"
        || trimmed == "plugin.wasm"
        || trimmed == "assets"
        || trimmed.starts_with("assets/");
    if !allowed
        || (is_directory && matches!(trimmed, "manifest.json" | "plugin.wasm"))
        || (!is_directory && trimmed == "assets")
    {
        return Err(PluginPlatformError::RejectedArchivePath);
    }
    Ok(PathBuf::from(trimmed))
}

fn validate_local_manifest(
    manifest: &PluginManifest,
    current_app_version: &Version,
    current_architecture: &str,
) -> Result<()> {
    if !norishell_core_api::plugin_package_protocol_is_compatible(
        manifest.protocol_major,
        manifest.protocol_minor,
    ) || manifest.capabilities.iter().any(|capability| {
        norishell_core_api::plugin_capability_min_protocol_minor(*capability)
            > manifest.protocol_minor
    }) {
        return Err(PluginPlatformError::InvalidProtocolCatalog);
    }
    let version =
        Version::parse(&manifest.version).map_err(|_| PluginPlatformError::ManifestMismatch)?;
    let minimum = Version::parse(&manifest.minimum_app_version)
        .map_err(|_| PluginPlatformError::ManifestMismatch)?;
    if current_app_version < &minimum {
        return Err(PluginPlatformError::AppVersionIncompatible);
    }
    if !plugin_core_api_compatible(manifest) {
        return Err(PluginPlatformError::CoreApiIncompatible);
    }
    let valid_text = |value: &str, maximum: usize| {
        !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
    };
    let unique_architectures = manifest.architectures.iter().collect::<BTreeSet<_>>();
    let unique_capabilities = manifest.capabilities.iter().collect::<BTreeSet<_>>();
    if !valid_text(&manifest.name, 160)
        || !valid_text(&manifest.publisher, 160)
        || version.to_string() != manifest.version
        || manifest.platform != "desktop"
        || manifest.architectures.is_empty()
        || manifest.architectures.len() > 8
        || unique_architectures.len() != manifest.architectures.len()
        || manifest
            .architectures
            .iter()
            .any(|value| !valid_text(value, 32))
        || (!manifest
            .architectures
            .iter()
            .any(|value| value == "universal")
            && !manifest
                .architectures
                .iter()
                .any(|value| value == current_architecture))
        || manifest.capabilities.len() > 32
        || unique_capabilities.len() != manifest.capabilities.len()
        || manifest
            .package_url
            .as_ref()
            .is_some_and(|value| value.len() > 2_048)
    {
        return Err(PluginPlatformError::ManifestMismatch);
    }
    Ok(())
}

fn open_regular_package(path: &Path) -> Result<File> {
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
    let source = options.open(path)?;
    let metadata = source.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(PluginPlatformError::RejectedArchivePath);
        }
    }
    if !metadata.is_file() {
        return Err(PluginPlatformError::RejectedArchivePath);
    }
    Ok(source)
}

fn read_package_snapshot(source: &mut File, max_archive_bytes: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source
        .take(max_archive_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_archive_bytes {
        return Err(PluginPlatformError::PackageTooLarge);
    }
    Ok(bytes)
}

pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write as _};

    use norishell_core_api::{
        PLUGIN_PROTOCOL_MINOR, PLUGIN_THEME_PROTOCOL_MINOR, PluginCapability, PluginId,
        PluginPackageKind,
    };
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::{PackageLimits, PluginManifest, inspect_local_package, validate_local_manifest};

    fn manifest() -> PluginManifest {
        PluginManifest {
            plugin_id: PluginId::parse("com.norishell.fixture").expect("plugin id"),
            name: "Fixture".to_owned(),
            publisher: "NoriShell".to_owned(),
            publisher_key_id: None,
            publisher_signature: None,
            version: "1.0.0".to_owned(),
            protocol_major: 1,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            platform: "desktop".to_owned(),
            architectures: vec!["universal".to_owned()],
            package_url: Some("https://plugins.example.test/fixture.zip".to_owned()),
            capabilities: vec![PluginCapability::UiPanel],
            minimum_app_version: "0.1.0".to_owned(),
            minimum_core_api_version: None,
        }
    }

    #[test]
    fn core_api_requirement_rejects_newer_or_different_core() {
        let app = semver::Version::parse("0.1.4").expect("version");
        let mut plugin = manifest();
        let current = norishell_core_api::CoreApiVersion::current();
        plugin.minimum_core_api_version = Some(current);
        assert!(validate_local_manifest(&plugin, &app, "universal").is_ok());
        plugin.minimum_core_api_version = Some(norishell_core_api::CoreApiVersion {
            major: current.major,
            minor: current.minor + 1,
        });
        assert!(matches!(
            validate_local_manifest(&plugin, &app, "universal"),
            Err(crate::PluginPlatformError::CoreApiIncompatible)
        ));
        plugin.capabilities.push(PluginCapability::SshSync);
        plugin.minimum_core_api_version = None;
        assert!(matches!(
            validate_local_manifest(&plugin, &app, "universal"),
            Err(crate::PluginPlatformError::CoreApiIncompatible)
        ));
        plugin.minimum_core_api_version = Some(norishell_core_api::CoreApiVersion {
            major: current.major + 1,
            minor: 0,
        });
        assert!(matches!(
            validate_local_manifest(&plugin, &app, "universal"),
            Err(crate::PluginPlatformError::CoreApiIncompatible)
        ));
    }

    fn write_package(path: &std::path::Path, extra_names: &[&str]) {
        let file = File::create(path).expect("package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive
            .start_file("manifest.json", options)
            .expect("manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest()).expect("manifest json"))
            .expect("manifest bytes");
        archive.start_file("plugin.wasm", options).expect("wasm");
        archive.write_all(b"\0asm\x01\0\0\0").expect("wasm bytes");
        for name in extra_names {
            archive.start_file(*name, options).expect("asset");
            archive.write_all(b"asset").expect("asset bytes");
        }
        archive.finish().expect("finish package");
    }

    fn theme_manifest() -> PluginManifest {
        let mut value = manifest();
        value.protocol_minor = PLUGIN_THEME_PROTOCOL_MINOR;
        value.capabilities.clear();
        value
    }

    fn write_theme_package(path: &std::path::Path, include_wasm: bool, extra_names: &[&str]) {
        let file = File::create(path).expect("theme package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive
            .start_file("manifest.json", options)
            .expect("manifest");
        archive
            .write_all(&serde_json::to_vec(&theme_manifest()).expect("manifest json"))
            .expect("manifest bytes");
        if include_wasm {
            archive.start_file("plugin.wasm", options).expect("wasm");
            archive.write_all(b"\0asm\x01\0\0\0").expect("wasm bytes");
        }
        archive
            .start_file("assets/theme.json", options)
            .expect("theme asset");
        archive
            .write_all(include_bytes!(
                "../../../examples/theme-plugins/clear/assets/theme.json"
            ))
            .expect("theme bytes");
        for name in extra_names {
            archive.start_file(*name, options).expect("extra asset");
            archive.write_all(b"extra").expect("extra asset bytes");
        }
        archive.finish().expect("finish theme package");
    }

    fn write_settings_package(path: &std::path::Path, protocol_minor: u16, settings: &[u8]) {
        let mut package_manifest = manifest();
        package_manifest.protocol_minor = protocol_minor;
        let file = File::create(path).expect("package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive
            .start_file("manifest.json", options)
            .expect("manifest");
        archive
            .write_all(&serde_json::to_vec(&package_manifest).expect("manifest json"))
            .expect("manifest bytes");
        archive.start_file("plugin.wasm", options).expect("wasm");
        archive.write_all(b"\0asm\x01\0\0\0").expect("wasm bytes");
        archive
            .start_file("assets/settings.json", options)
            .expect("settings");
        archive.write_all(settings).expect("settings bytes");
        archive.finish().expect("finish package");
    }

    fn write_protocols_package(path: &std::path::Path, protocols: &[u8]) {
        let file = File::create(path).expect("package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive
            .start_file("manifest.json", options)
            .expect("manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest()).expect("manifest json"))
            .expect("manifest bytes");
        archive.start_file("plugin.wasm", options).expect("wasm");
        archive.write_all(b"\0asm\x01\0\0\0").expect("wasm bytes");
        archive
            .start_file("assets/protocols.json", options)
            .expect("protocol catalog");
        archive
            .write_all(protocols)
            .expect("protocol catalog bytes");
        archive.finish().expect("finish package");
    }

    #[test]
    fn local_inspection_accepts_a_bounded_compatible_zip_without_publisher_authentication() {
        let directory = tempfile::tempdir().expect("tempdir");
        let package = directory.path().join("fixture.zip");
        write_package(&package, &[]);

        let inspected = inspect_local_package(
            &package,
            &semver::Version::parse("0.1.0").expect("app version"),
            std::env::consts::ARCH,
            PackageLimits::default(),
        )
        .expect("inspect local package");

        assert_eq!(
            inspected.manifest.plugin_id.as_str(),
            "com.norishell.fixture"
        );
        assert_eq!(inspected.files.len(), 2);
        assert_eq!(
            inspected.package_size,
            std::fs::metadata(package).unwrap().len()
        );
    }

    #[test]
    fn theme_packages_require_the_exact_data_only_archive_shape() {
        let directory = tempfile::tempdir().expect("tempdir");
        let pure = directory.path().join("theme.zip");
        write_theme_package(&pure, false, &[]);
        let inspected = inspect_local_package(
            &pure,
            &semver::Version::parse("0.1.0").expect("app version"),
            std::env::consts::ARCH,
            PackageLimits::default(),
        )
        .expect("inspect pure theme");
        assert_eq!(inspected.package_kind, PluginPackageKind::Theme);

        let mixed = directory.path().join("mixed-theme.zip");
        write_theme_package(&mixed, true, &[]);
        assert!(
            inspect_local_package(
                &mixed,
                &semver::Version::parse("0.1.0").expect("app version"),
                std::env::consts::ARCH,
                PackageLimits::default(),
            )
            .is_err()
        );

        let extra = directory.path().join("extra-theme.zip");
        write_theme_package(&extra, false, &["assets/extra.json"]);
        assert!(
            inspect_local_package(
                &extra,
                &semver::Version::parse("0.1.0").expect("app version"),
                std::env::consts::ARCH,
                PackageLimits::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn settings_asset_requires_current_protocol_and_is_parsed_from_the_verified_zip_snapshot() {
        let directory = tempfile::tempdir().expect("tempdir");
        let schema = r#"{"schemaVersion":1,"fields":[{"type":"boolean","key":"showTerminalStatus","label":{"en":"Show status","zh-CN":"显示状态"},"default":true}],"targetVisibility":[{"targetId":"terminal.footer","settingKey":"showTerminalStatus"}]}"#;
        let current = directory.path().join("settings.zip");
        write_settings_package(&current, PLUGIN_PROTOCOL_MINOR, schema.as_bytes());
        let inspected = inspect_local_package(
            &current,
            &semver::Version::parse("0.1.0").expect("app version"),
            std::env::consts::ARCH,
            PackageLimits::default(),
        )
        .expect("inspect settings package");
        let settings = inspected.settings.expect("settings snapshot");
        assert_eq!(settings.schema.fields.len(), 1);
        assert_eq!(
            settings.default_values_json,
            r#"{"showTerminalStatus":true}"#
        );

        let legacy = directory.path().join("legacy-settings.zip");
        write_settings_package(&legacy, PLUGIN_PROTOCOL_MINOR - 1, schema.as_bytes());
        assert!(
            inspect_local_package(
                &legacy,
                &semver::Version::parse("0.1.0").expect("app version"),
                std::env::consts::ARCH,
                PackageLimits::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn protocols_asset_is_read_from_the_hashed_package_snapshot() {
        let directory = tempfile::tempdir().expect("tempdir");
        let catalog = r#"{"schemaVersion":1,"providers":[{"id":"telnet","label":{"en":"Telnet","zh-CN":"Telnet"},"configuration":{"schemaVersion":1,"fields":[{"type":"number","key":"port","label":{"en":"Port","zh-CN":"端口"},"default":23,"min":1,"max":65535}]},"features":{"terminal":true,"resize":"supported","reconnect":true},"resources":["tcp"]}]}"#;
        let package = directory.path().join("protocols.zip");
        write_protocols_package(&package, catalog.as_bytes());
        let inspected = inspect_local_package(
            &package,
            &semver::Version::parse("0.1.0").expect("app version"),
            std::env::consts::ARCH,
            PackageLimits::default(),
        )
        .expect("inspect protocol package");
        assert_eq!(
            inspected
                .protocols
                .expect("protocol catalog")
                .catalog
                .providers[0]
                .id,
            "telnet"
        );

        let malformed = directory.path().join("malformed-protocols.zip");
        write_protocols_package(
            &malformed,
            br#"{"schemaVersion":1,"providers":[],"grant":"network"}"#,
        );
        assert!(
            inspect_local_package(
                &malformed,
                &semver::Version::parse("0.1.0").expect("app version"),
                std::env::consts::ARCH,
                PackageLimits::default(),
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_inspection_rejects_a_symlinked_package_source() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("tempdir");
        let package = directory.path().join("fixture.zip");
        let link = directory.path().join("fixture-link.zip");
        write_package(&package, &[]);
        symlink(&package, &link).expect("package symlink");

        assert!(
            inspect_local_package(
                &link,
                &semver::Version::parse("0.1.0").expect("app version"),
                std::env::consts::ARCH,
                PackageLimits::default(),
            )
            .is_err()
        );
    }

    #[test]
    fn paths_with_unicode_parent_segments_or_backslashes_are_rejected() {
        assert!(super::validate_path("assets/../plugin.wasm", false).is_err());
        assert!(super::validate_path("assets\\evil", false).is_err());
        assert!(super::validate_path("assets/é.png", false).is_err());
        assert!(super::validate_path("postinstall", false).is_err());
    }
}
