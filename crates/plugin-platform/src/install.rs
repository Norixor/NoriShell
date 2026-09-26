use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PLUGIN_THEME_PROTOCOL_MINOR, PluginId,
    PluginOperationId, PluginPackageKind, ThemeDefinition,
};
use semver::Version;
use sha2::{Digest, Sha256};

use crate::{
    InspectedPackage, InspectedPluginProtocols, InspectedPluginWorkflows, PackageLimits,
    PluginPlatformError, Result, extract_package, inspect_plugin_protocols,
    inspect_plugin_workflows, lower_hex,
};

const ACTIVE_POINTER: &str = "active";
const HASH_MARKER: &str = ".package-sha256";
const WASM_HASH_MARKER: &str = ".plugin-wasm-sha256";
const THEME_HASH_MARKER: &str = ".theme-json-sha256";

#[derive(Debug, Clone)]
pub struct PluginInstaller {
    root: PathBuf,
    limits: PackageLimits,
}

#[derive(Debug)]
pub struct StagedPlugin {
    pub plugin_id: PluginId,
    pub version: String,
    pub package_sha256: [u8; 32],
    path: PathBuf,
}

impl StagedPlugin {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationResult {
    pub plugin_id: PluginId,
    pub previous_version: Option<String>,
    pub active_version: String,
    pub version_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallResult {
    pub plugin_id: PluginId,
    pub removed_version: String,
}

impl PluginInstaller {
    pub fn new(root: impl AsRef<Path>, limits: PackageLimits) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        reject_directory(&root)?;
        sync_directory(&root)?;
        Ok(Self { root, limits })
    }

    pub fn stage(
        &self,
        package_path: &Path,
        inspected: &InspectedPackage,
        operation_id: &PluginOperationId,
    ) -> Result<StagedPlugin> {
        let plugin_id = inspected.manifest.plugin_id.clone();
        let version = Version::parse(&inspected.manifest.version)
            .map_err(|_| PluginPlatformError::ManifestMismatch)?
            .to_string();
        reject_directory(&self.root)?;
        let staging_root = self.root.join(".staging");
        fs::create_dir_all(&staging_root)?;
        reject_directory(&staging_root)?;
        let path = staging_root.join(format!(
            "{}-{}-{}",
            plugin_id.as_str(),
            version,
            operation_id.as_str()
        ));
        fs::create_dir(&path)?;
        let result = (|| {
            extract_package(package_path, &path, inspected, self.limits)?;
            write_new_synced(
                &path.join(HASH_MARKER),
                lower_hex(&inspected.package_sha256).as_bytes(),
            )?;
            match inspected.package_kind {
                PluginPackageKind::Wasm => {
                    if inspected.theme_definition.is_some()
                        || inspected.theme_definition_sha256.is_some()
                    {
                        return Err(PluginPlatformError::InvalidArchive);
                    }
                    let wasm = read_regular_file_snapshot(
                        &path.join("plugin.wasm"),
                        self.limits.max_single_file_bytes,
                    )?;
                    let wasm_sha256: [u8; 32] = Sha256::digest(&wasm).into();
                    write_new_synced(
                        &path.join(WASM_HASH_MARKER),
                        lower_hex(&wasm_sha256).as_bytes(),
                    )?;
                }
                PluginPackageKind::Theme => {
                    let expected_theme_sha256 = inspected
                        .theme_definition_sha256
                        .ok_or(PluginPlatformError::InvalidArchive)?;
                    if inspected.theme_definition.is_none() {
                        return Err(PluginPlatformError::InvalidArchive);
                    }
                    let theme = read_regular_file_snapshot(
                        &path.join("assets/theme.json"),
                        self.limits.max_theme_definition_bytes,
                    )?;
                    let actual_theme_sha256: [u8; 32] = Sha256::digest(&theme).into();
                    let parsed_theme = ThemeDefinition::parse_json(&theme)
                        .map_err(|_| PluginPlatformError::InvalidArchive)?;
                    if actual_theme_sha256 != expected_theme_sha256
                        || inspected.theme_definition.as_ref() != Some(&parsed_theme)
                    {
                        return Err(PluginPlatformError::InvalidArchive);
                    }
                    write_new_synced(
                        &path.join(THEME_HASH_MARKER),
                        lower_hex(&actual_theme_sha256).as_bytes(),
                    )?;
                }
            }
            sync_directory_tree(&path)?;
            sync_directory(&staging_root)?;
            Ok(StagedPlugin {
                plugin_id,
                version,
                package_sha256: inspected.package_sha256,
                path: path.clone(),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&path);
        }
        result
    }

    /// Makes the immutable version durable and switches the small active pointer with CAS.
    /// A post-replace directory-sync failure returns `InstallCommitUncertain`; callers must
    /// preserve their durable operation phase and reconcile the pointer before changing SQLite.
    pub fn activate(
        &self,
        staged: StagedPlugin,
        expected_active_version: Option<&str>,
        operation_id: &PluginOperationId,
    ) -> Result<ActivationResult> {
        let plugin_root = self.root.join(staged.plugin_id.as_str());
        fs::create_dir_all(&plugin_root)?;
        reject_symlink(&plugin_root)?;
        let versions = plugin_root.join("versions");
        fs::create_dir_all(&versions)?;
        reject_symlink(&versions)?;
        let active_path = plugin_root.join(ACTIVE_POINTER);
        let previous_version = read_optional_pointer(&active_path)?;
        if previous_version.as_deref() != expected_active_version {
            return Err(PluginPlatformError::InstallConflict);
        }

        let version_directory = versions.join(&staged.version);
        if version_directory.exists() {
            let marker = read_bounded_text(&version_directory.join(HASH_MARKER), 64)?;
            if marker != lower_hex(&staged.package_sha256) {
                return Err(PluginPlatformError::InstallConflict);
            }
            if staged.path.exists() {
                fs::remove_dir_all(&staged.path)?;
            }
        } else {
            fs::rename(&staged.path, &version_directory)?;
            if sync_directory(&versions).is_err() {
                return Err(PluginPlatformError::InstallCommitUncertain);
            }
        }

        let temporary_pointer = plugin_root.join(format!(".active-{}.tmp", operation_id.as_str()));
        write_new_synced(&temporary_pointer, staged.version.as_bytes())?;
        fs::rename(&temporary_pointer, &active_path)?;
        if sync_directory(&plugin_root).is_err() {
            return Err(PluginPlatformError::InstallCommitUncertain);
        }
        Ok(ActivationResult {
            plugin_id: staged.plugin_id,
            previous_version,
            active_version: staged.version,
            version_directory,
        })
    }

    pub fn read_active_version(&self, plugin_id: &PluginId) -> Result<Option<String>> {
        read_optional_pointer(&self.root.join(plugin_id.as_str()).join(ACTIVE_POINTER))
    }

    pub fn read_active_package_sha256(&self, plugin_id: &PluginId) -> Result<Option<String>> {
        reject_directory(&self.root)?;
        let plugin_root = self.root.join(plugin_id.as_str());
        match fs::symlink_metadata(&plugin_root) {
            Ok(_) => {
                reject_directory(&plugin_root)?;
                reject_directory(&plugin_root.join("versions"))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let Some(version) = self.read_active_version(plugin_id)? else {
            return Ok(None);
        };
        let version_root = plugin_root.join("versions").join(version);
        match fs::symlink_metadata(&version_root) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        reject_tree_symlinks(&version_root)?;
        read_lower_hex_marker(&version_root.join(HASH_MARKER)).map(Some)
    }

    fn replacement_backup(
        &self,
        plugin_id: &PluginId,
        operation_id: &PluginOperationId,
    ) -> PathBuf {
        self.root.join(".replaced").join(format!(
            "{}-{}",
            plugin_id.as_str(),
            operation_id.as_str()
        ))
    }

    pub fn replacement_backup_exists(
        &self,
        plugin_id: &PluginId,
        operation_id: &PluginOperationId,
    ) -> Result<bool> {
        reject_directory(&self.root)?;
        reject_optional_directory(&self.root.join(".replaced"))?;
        let backup = self.replacement_backup(plugin_id, operation_id);
        match fs::symlink_metadata(&backup) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        reject_symlink(&backup)?;
        Ok(true)
    }

    /// A same-version replacement keeps the old immutable directory until the
    /// SQLite commit is known. The operation id makes crash recovery unambiguous.
    pub fn activate_replacement(
        &self,
        staged: StagedPlugin,
        expected_old_hash: &str,
        operation_id: &PluginOperationId,
    ) -> Result<ActivationResult> {
        reject_directory(&self.root)?;
        let staging_root = self.root.join(".staging");
        reject_directory(&staging_root)?;
        if staged.path.parent() != Some(staging_root.as_path()) {
            return Err(PluginPlatformError::InstallConflict);
        }
        let plugin_root = self.root.join(staged.plugin_id.as_str());
        reject_directory(&plugin_root)?;
        let versions = plugin_root.join("versions");
        reject_directory(&versions)?;
        if !lower_hex_64(expected_old_hash)
            || self.read_active_version(&staged.plugin_id)?.as_deref()
                != Some(staged.version.as_str())
            || self
                .read_active_package_sha256(&staged.plugin_id)?
                .as_deref()
                != Some(expected_old_hash)
            || expected_old_hash == lower_hex(&staged.package_sha256)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        reject_tree_symlinks(&staged.path)?;
        if read_lower_hex_marker(&staged.path.join(HASH_MARKER))?
            != lower_hex(&staged.package_sha256)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let version_directory = versions.join(&staged.version);
        let backup_root = self.root.join(".replaced");
        fs::create_dir_all(&backup_root)?;
        reject_directory(&backup_root)?;
        let backup = self.replacement_backup(&staged.plugin_id, operation_id);
        if path_exists_no_follow(&backup)? {
            return Err(PluginPlatformError::InstallConflict);
        }
        fs::rename(&version_directory, &backup)?;
        if sync_directory(&versions).is_err() || sync_directory(&backup_root).is_err() {
            return Err(PluginPlatformError::InstallCommitUncertain);
        }
        fs::rename(&staged.path, &version_directory)?;
        if sync_directory(&versions).is_err() {
            return Err(PluginPlatformError::InstallCommitUncertain);
        }
        Ok(ActivationResult {
            plugin_id: staged.plugin_id,
            previous_version: Some(staged.version.clone()),
            active_version: staged.version,
            version_directory,
        })
    }

    pub fn restore_replacement(
        &self,
        plugin_id: &PluginId,
        version: &str,
        old_hash: &str,
        candidate_hash: &str,
        operation_id: &PluginOperationId,
    ) -> Result<()> {
        reject_directory(&self.root)?;
        reject_directory(&self.root.join(plugin_id.as_str()))?;
        let versions = self.root.join(plugin_id.as_str()).join("versions");
        reject_directory(&versions)?;
        reject_optional_directory(&self.root.join(".replaced"))?;
        if !lower_hex_64(old_hash)
            || !lower_hex_64(candidate_hash)
            || self.read_active_version(plugin_id)?.as_deref() != Some(version)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let live = versions.join(version);
        let backup = self.replacement_backup(plugin_id, operation_id);
        let discarded = self
            .root
            .join(".replaced")
            .join(format!("{}-discard", operation_id.as_str()));
        if !path_exists_no_follow(&backup)? {
            if self.read_active_package_sha256(plugin_id)?.as_deref() != Some(old_hash) {
                return Err(PluginPlatformError::InstallConflict);
            }
            if path_exists_no_follow(&discarded)? {
                reject_tree_symlinks(&discarded)?;
                if read_lower_hex_marker(&discarded.join(HASH_MARKER))? != candidate_hash {
                    return Err(PluginPlatformError::InstallConflict);
                }
                fs::remove_dir_all(&discarded)?;
                sync_directory(&self.root.join(".replaced"))
                    .map_err(|_| PluginPlatformError::InstallCommitUncertain)?;
            }
            return Ok(());
        }
        reject_tree_symlinks(&backup)?;
        if read_lower_hex_marker(&backup.join(HASH_MARKER))? != old_hash {
            return Err(PluginPlatformError::InstallConflict);
        }
        if path_exists_no_follow(&live)? {
            reject_tree_symlinks(&live)?;
            if read_lower_hex_marker(&live.join(HASH_MARKER))? != candidate_hash {
                return Err(PluginPlatformError::InstallConflict);
            }
            if path_exists_no_follow(&discarded)? {
                return Err(PluginPlatformError::InstallConflict);
            }
            fs::rename(&live, &discarded)?;
            sync_directory(&versions).map_err(|_| PluginPlatformError::InstallCommitUncertain)?;
        }
        fs::rename(&backup, &live)?;
        sync_directory(&versions).map_err(|_| PluginPlatformError::InstallCommitUncertain)?;
        sync_directory(&self.root.join(".replaced"))
            .map_err(|_| PluginPlatformError::InstallCommitUncertain)?;
        if path_exists_no_follow(&discarded)? {
            reject_tree_symlinks(&discarded)?;
            fs::remove_dir_all(&discarded)?;
            sync_directory(&self.root.join(".replaced"))
                .map_err(|_| PluginPlatformError::InstallCommitUncertain)?;
        }
        Ok(())
    }

    pub fn finalize_replacement(
        &self,
        plugin_id: &PluginId,
        version: &str,
        candidate_hash: &str,
        operation_id: &PluginOperationId,
    ) -> Result<()> {
        reject_directory(&self.root)?;
        reject_directory(&self.root.join(plugin_id.as_str()))?;
        reject_directory(&self.root.join(plugin_id.as_str()).join("versions"))?;
        reject_optional_directory(&self.root.join(".replaced"))?;
        if self.read_active_version(plugin_id)?.as_deref() != Some(version)
            || self.read_active_package_sha256(plugin_id)?.as_deref() != Some(candidate_hash)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let backup = self.replacement_backup(plugin_id, operation_id);
        if path_exists_no_follow(&backup)? {
            reject_tree_symlinks(&backup)?;
            fs::remove_dir_all(&backup)?;
            sync_directory(&self.root.join(".replaced"))
                .map_err(|_| PluginPlatformError::InstallCommitUncertain)?;
        }
        Ok(())
    }

    /// Restores (or clears) the active pointer with compare-and-swap semantics.
    /// This is intentionally narrower than exposing installation paths to the
    /// application service and is used to reconcile a filesystem-first saga.
    pub fn restore_active_version(
        &self,
        plugin_id: &PluginId,
        expected_current_version: &str,
        restore_version: Option<&str>,
        operation_id: &PluginOperationId,
    ) -> Result<()> {
        Version::parse(expected_current_version)
            .map_err(|_| PluginPlatformError::InstallConflict)?;
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        let active_path = plugin_root.join(ACTIVE_POINTER);
        let current = read_optional_pointer(&active_path)?;
        let clear_tombstone =
            plugin_root.join(format!(".inactive-restore-{}", operation_id.as_str()));
        if restore_version.is_none() && current.is_none() && clear_tombstone.exists() {
            reject_symlink(&clear_tombstone)?;
            if read_bounded_text(&clear_tombstone, 80)? != expected_current_version {
                return Err(PluginPlatformError::InstallConflict);
            }
            fs::remove_file(&clear_tombstone)?;
            return sync_directory(&plugin_root)
                .map_err(|_| PluginPlatformError::InstallCommitUncertain);
        }
        if current.as_deref() != Some(expected_current_version) {
            return Err(PluginPlatformError::InstallConflict);
        }
        if let Some(version) = restore_version {
            Version::parse(version).map_err(|_| PluginPlatformError::InstallConflict)?;
            let versions_root = plugin_root.join("versions");
            reject_symlink(&versions_root)?;
            let version_root = versions_root.join(version);
            reject_symlink(&version_root)?;
            reject_tree_symlinks(&version_root)?;
            let marker = read_bounded_text(&version_root.join(HASH_MARKER), 64)?;
            if marker.len() != 64 || !marker.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(PluginPlatformError::InstallConflict);
            }
            let temporary_pointer =
                plugin_root.join(format!(".active-restore-{}.tmp", operation_id.as_str()));
            if temporary_pointer.exists() {
                reject_symlink(&temporary_pointer)?;
                if read_bounded_text(&temporary_pointer, 80)? != version {
                    return Err(PluginPlatformError::InstallConflict);
                }
            } else {
                write_new_synced(&temporary_pointer, version.as_bytes())?;
            }
            fs::rename(&temporary_pointer, &active_path)?;
        } else {
            if clear_tombstone.exists() {
                return Err(PluginPlatformError::InstallConflict);
            }
            fs::rename(&active_path, &clear_tombstone)?;
            if sync_directory(&plugin_root).is_err() {
                return Err(PluginPlatformError::InstallCommitUncertain);
            }
            fs::remove_file(&clear_tombstone)?;
        }
        sync_directory(&plugin_root).map_err(|_| PluginPlatformError::InstallCommitUncertain)
    }

    pub fn read_active_module(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>> {
        Version::parse(expected_version).map_err(|_| PluginPlatformError::InstallConflict)?;
        if expected_package_sha256.len() != 64
            || !expected_package_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        if read_optional_pointer(&plugin_root.join(ACTIVE_POINTER))?.as_deref()
            != Some(expected_version)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let version_root = plugin_root.join("versions").join(expected_version);
        reject_symlink(&version_root)?;
        if read_bounded_text(&version_root.join(HASH_MARKER), 64)?
            != expected_package_sha256.to_ascii_lowercase()
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let module_path = version_root.join("plugin.wasm");
        let expected_wasm_hash = read_bounded_text(&version_root.join(WASM_HASH_MARKER), 64)?;
        let module = read_regular_file_snapshot(&module_path, max_bytes)?;
        let actual_wasm_hash: [u8; 32] = Sha256::digest(&module).into();
        if lower_hex(&actual_wasm_hash) != expected_wasm_hash {
            return Err(PluginPlatformError::InstallConflict);
        }
        Ok(module)
    }

    pub fn read_active_manifest(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
    ) -> Result<crate::PluginManifest> {
        Version::parse(expected_version).map_err(|_| PluginPlatformError::InstallConflict)?;
        if expected_package_sha256.len() != 64
            || !expected_package_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        if read_optional_pointer(&plugin_root.join(ACTIVE_POINTER))?.as_deref()
            != Some(expected_version)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let version_root = plugin_root.join("versions").join(expected_version);
        reject_symlink(&version_root)?;
        if read_bounded_text(&version_root.join(HASH_MARKER), 64)?
            != expected_package_sha256.to_ascii_lowercase()
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let bytes = read_regular_file_snapshot(
            &version_root.join("manifest.json"),
            self.limits.max_manifest_bytes,
        )?;
        let manifest: crate::PluginManifest =
            serde_json::from_slice(&bytes).map_err(|_| PluginPlatformError::InstallConflict)?;
        if manifest.plugin_id != *plugin_id || manifest.version != expected_version {
            return Err(PluginPlatformError::InstallConflict);
        }
        Ok(manifest)
    }

    /// Determines the package branch from the package-bound active manifest.
    /// Theme classification performs the full fixed-asset verification so a
    /// damaged or mixed installation can never be treated as executable Wasm.
    pub fn active_package_kind(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
    ) -> Result<PluginPackageKind> {
        let manifest =
            self.read_active_manifest(plugin_id, expected_version, expected_package_sha256)?;
        if manifest.protocol_major == PLUGIN_PROTOCOL_MAJOR
            && manifest.protocol_minor == PLUGIN_THEME_PROTOCOL_MINOR
            && manifest.capabilities.is_empty()
        {
            self.read_active_theme(plugin_id, expected_version, expected_package_sha256)?;
            return Ok(PluginPackageKind::Theme);
        }
        if manifest.protocol_major == PLUGIN_PROTOCOL_MAJOR
            && manifest.protocol_minor == PLUGIN_PROTOCOL_MINOR
        {
            let version_root =
                self.active_version_root(plugin_id, expected_version, expected_package_sha256)?;
            if active_wasm_has_theme_artifacts(&version_root)? {
                return Err(PluginPlatformError::InstallConflict);
            }
            return Ok(PluginPackageKind::Wasm);
        }
        Err(PluginPlatformError::InstallConflict)
    }

    /// Reads only the fixed theme asset from an active package after binding it
    /// to the active pointer, installed version and package hash. No caller can
    /// select a package path, and every read recomputes the staged asset hash.
    pub fn read_active_theme(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
    ) -> Result<ThemeDefinition> {
        let version_root =
            self.active_version_root(plugin_id, expected_version, expected_package_sha256)?;
        let manifest_bytes = read_regular_file_snapshot(
            &version_root.join("manifest.json"),
            self.limits.max_manifest_bytes,
        )?;
        let manifest: crate::PluginManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| PluginPlatformError::InstallConflict)?;
        if manifest.plugin_id != *plugin_id
            || manifest.version != expected_version
            || manifest.protocol_major != PLUGIN_PROTOCOL_MAJOR
            || manifest.protocol_minor != PLUGIN_THEME_PROTOCOL_MINOR
            || !manifest.capabilities.is_empty()
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        validate_active_theme_layout(&version_root)?;
        let expected_theme_hash = read_lower_hex_marker(&version_root.join(THEME_HASH_MARKER))?;
        let theme_bytes = read_regular_file_snapshot(
            &version_root.join("assets/theme.json"),
            self.limits.max_theme_definition_bytes,
        )?;
        let actual_theme_hash: [u8; 32] = Sha256::digest(&theme_bytes).into();
        if lower_hex(&actual_theme_hash) != expected_theme_hash {
            return Err(PluginPlatformError::InstallConflict);
        }
        ThemeDefinition::parse_json(&theme_bytes).map_err(|_| PluginPlatformError::InstallConflict)
    }

    fn active_version_root(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
    ) -> Result<PathBuf> {
        Version::parse(expected_version).map_err(|_| PluginPlatformError::InstallConflict)?;
        if !lower_hex_64(expected_package_sha256) {
            return Err(PluginPlatformError::InstallConflict);
        }
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        if read_optional_pointer(&plugin_root.join(ACTIVE_POINTER))?.as_deref()
            != Some(expected_version)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let version_root = plugin_root.join("versions").join(expected_version);
        reject_tree_symlinks(&version_root)?;
        if read_lower_hex_marker(&version_root.join(HASH_MARKER))? != expected_package_sha256 {
            return Err(PluginPlatformError::InstallConflict);
        }
        Ok(version_root)
    }

    /// Reads one immutable, package-bound HTML asset for a sandboxed plugin
    /// surface. The identifier is mapped to a fixed private path and never
    /// accepted as a caller-supplied filesystem path.
    pub fn read_active_isolated_surface(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
        surface_id: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>> {
        if surface_id.is_empty()
            || surface_id.len() > 80
            || !surface_id.is_ascii()
            || !surface_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        Version::parse(expected_version).map_err(|_| PluginPlatformError::InstallConflict)?;
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        if read_optional_pointer(&plugin_root.join(ACTIVE_POINTER))?.as_deref()
            != Some(expected_version)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let version_root = plugin_root.join("versions").join(expected_version);
        reject_symlink(&version_root)?;
        if read_bounded_text(&version_root.join(HASH_MARKER), 64)?
            != expected_package_sha256.to_ascii_lowercase()
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let assets_root = version_root.join("assets");
        let isolated_root = assets_root.join("isolated");
        reject_symlink(&assets_root)?;
        reject_symlink(&isolated_root)?;
        read_regular_file_snapshot(&isolated_root.join(format!("{surface_id}.html")), max_bytes)
    }

    /// Reads the fixed, validated provider catalog from one active immutable
    /// package. Missing metadata is an empty declaration, never a fallback
    /// grant. The caller cannot select an arbitrary asset path.
    pub fn read_protocol_catalog(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
    ) -> Result<Option<InspectedPluginProtocols>> {
        Version::parse(expected_version).map_err(|_| PluginPlatformError::InstallConflict)?;
        if expected_package_sha256.len() != 64
            || !expected_package_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        if read_optional_pointer(&plugin_root.join(ACTIVE_POINTER))?.as_deref()
            != Some(expected_version)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let version_root = plugin_root.join("versions").join(expected_version);
        reject_symlink(&version_root)?;
        if read_bounded_text(&version_root.join(HASH_MARKER), 64)?
            != expected_package_sha256.to_ascii_lowercase()
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let assets_root = version_root.join("assets");
        reject_symlink(&assets_root)?;
        let catalog_path = assets_root.join("protocols.json");
        if !catalog_path.try_exists()? {
            return Ok(None);
        }
        let bytes = read_regular_file_snapshot(
            &catalog_path,
            crate::MAX_PLUGIN_PROTOCOL_CATALOG_BYTES as u64,
        )?;
        inspect_plugin_protocols(&bytes).map(Some)
    }

    /// Reads the fixed workflow catalog from one active immutable package.
    /// The caller never supplies an arbitrary package-relative path.
    pub fn read_workflow_catalog(
        &self,
        plugin_id: &PluginId,
        expected_version: &str,
        expected_package_sha256: &str,
    ) -> Result<Option<InspectedPluginWorkflows>> {
        Version::parse(expected_version).map_err(|_| PluginPlatformError::InstallConflict)?;
        if expected_package_sha256.len() != 64
            || !expected_package_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        if read_optional_pointer(&plugin_root.join(ACTIVE_POINTER))?.as_deref()
            != Some(expected_version)
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let version_root = plugin_root.join("versions").join(expected_version);
        reject_symlink(&version_root)?;
        if read_bounded_text(&version_root.join(HASH_MARKER), 64)?
            != expected_package_sha256.to_ascii_lowercase()
        {
            return Err(PluginPlatformError::InstallConflict);
        }
        let assets_root = version_root.join("assets");
        reject_symlink(&assets_root)?;
        let catalog_path = assets_root.join("workflows.json");
        if !catalog_path.try_exists()? {
            return Ok(None);
        }
        let bytes = read_regular_file_snapshot(
            &catalog_path,
            crate::MAX_PLUGIN_WORKFLOW_CATALOG_BYTES as u64,
        )?;
        inspect_plugin_workflows(&bytes).map(Some)
    }

    /// Moves the active pointer and immutable versions into operation-scoped
    /// tombstones without deleting either. The caller must commit the database
    /// half before calling `finalize_uninstall`, or call `restore_uninstall`.
    pub fn prepare_uninstall(
        &self,
        plugin_id: &PluginId,
        expected_active_version: &str,
        operation_id: &PluginOperationId,
    ) -> Result<UninstallResult> {
        Version::parse(expected_active_version)
            .map_err(|_| PluginPlatformError::InstallConflict)?;
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        let active_path = plugin_root.join(ACTIVE_POINTER);
        if read_optional_pointer(&active_path)?.as_deref() != Some(expected_active_version) {
            return Err(PluginPlatformError::InstallConflict);
        }
        let versions = plugin_root.join("versions");
        reject_tree_symlinks(&versions)?;
        let version_directory = versions.join(expected_active_version);
        if !version_directory.is_dir() {
            return Err(PluginPlatformError::InstallConflict);
        }
        let trash = self.root.join(".trash");
        fs::create_dir_all(&trash)?;
        reject_symlink(&trash)?;
        let pointer_tombstone = plugin_root.join(format!(".inactive-{}", operation_id.as_str()));
        let version_tombstone = trash.join(format!(
            "{}-{}-{}",
            plugin_id.as_str(),
            expected_active_version,
            operation_id.as_str()
        ));
        if pointer_tombstone.exists() || version_tombstone.exists() {
            return Err(PluginPlatformError::InstallConflict);
        }
        fs::rename(&active_path, &pointer_tombstone)?;
        if sync_directory(&plugin_root).is_err() {
            return Err(PluginPlatformError::InstallCommitUncertain);
        }
        if fs::rename(&versions, &version_tombstone).is_err()
            || sync_directory(&plugin_root).is_err()
            || sync_directory(&trash).is_err()
        {
            return Err(PluginPlatformError::InstallCommitUncertain);
        }
        Ok(UninstallResult {
            plugin_id: plugin_id.clone(),
            removed_version: expected_active_version.to_owned(),
        })
    }

    /// Restores a prepared uninstall. The operation fence prevents one
    /// uninstall from consuming another operation's tombstones.
    pub fn restore_uninstall(
        &self,
        plugin_id: &PluginId,
        expected_active_version: &str,
        operation_id: &PluginOperationId,
    ) -> Result<()> {
        Version::parse(expected_active_version)
            .map_err(|_| PluginPlatformError::InstallConflict)?;
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        let active_path = plugin_root.join(ACTIVE_POINTER);
        if read_optional_pointer(&active_path)?.is_some() {
            return Err(PluginPlatformError::InstallConflict);
        }
        let pointer_tombstone = plugin_root.join(format!(".inactive-{}", operation_id.as_str()));
        reject_symlink(&pointer_tombstone)?;
        if read_bounded_text(&pointer_tombstone, 80)? != expected_active_version {
            return Err(PluginPlatformError::InstallConflict);
        }
        let versions = plugin_root.join("versions");
        let version_tombstone = self.root.join(".trash").join(format!(
            "{}-{}-{}",
            plugin_id.as_str(),
            expected_active_version,
            operation_id.as_str()
        ));
        if version_tombstone.exists() {
            if versions.exists() {
                return Err(PluginPlatformError::InstallConflict);
            }
            reject_tree_symlinks(&version_tombstone)?;
            fs::rename(&version_tombstone, &versions)?;
            if sync_directory(&plugin_root).is_err()
                || sync_directory(&self.root.join(".trash")).is_err()
            {
                return Err(PluginPlatformError::InstallCommitUncertain);
            }
        } else {
            reject_tree_symlinks(&versions)?;
        }
        fs::rename(&pointer_tombstone, &active_path)?;
        sync_directory(&plugin_root).map_err(|_| PluginPlatformError::InstallCommitUncertain)
    }

    /// Permanently removes operation-scoped tombstones after the database no
    /// longer advertises the installation. This operation is retry-safe.
    pub fn finalize_uninstall(
        &self,
        plugin_id: &PluginId,
        expected_active_version: &str,
        operation_id: &PluginOperationId,
    ) -> Result<()> {
        Version::parse(expected_active_version)
            .map_err(|_| PluginPlatformError::InstallConflict)?;
        let plugin_root = self.root.join(plugin_id.as_str());
        reject_symlink(&plugin_root)?;
        if read_optional_pointer(&plugin_root.join(ACTIVE_POINTER))?.is_some() {
            return Err(PluginPlatformError::InstallConflict);
        }
        let trash = self.root.join(".trash");
        reject_symlink(&trash)?;
        let version_tombstone = trash.join(format!(
            "{}-{}-{}",
            plugin_id.as_str(),
            expected_active_version,
            operation_id.as_str()
        ));
        if version_tombstone.exists() {
            reject_tree_symlinks(&version_tombstone)?;
            fs::remove_dir_all(&version_tombstone)?;
            sync_directory(&trash)?;
        }
        let pointer_tombstone = plugin_root.join(format!(".inactive-{}", operation_id.as_str()));
        if pointer_tombstone.exists() {
            reject_symlink(&pointer_tombstone)?;
            if read_bounded_text(&pointer_tombstone, 80)? != expected_active_version {
                return Err(PluginPlatformError::InstallConflict);
            }
            fs::remove_file(&pointer_tombstone)?;
            sync_directory(&plugin_root)?;
        }
        Ok(())
    }

    pub fn discard_stage(&self, staged: StagedPlugin) -> Result<()> {
        if staged.path.starts_with(self.root.join(".staging")) && staged.path.exists() {
            fs::remove_dir_all(staged.path)?;
            sync_directory(&self.root.join(".staging"))?;
        }
        Ok(())
    }
}

fn read_optional_pointer(path: &Path) -> Result<Option<String>> {
    match path.try_exists() {
        Ok(false) => Ok(None),
        Ok(true) => {
            reject_symlink(path)?;
            let value = read_bounded_text(path, 80)?;
            Version::parse(&value).map_err(|_| PluginPlatformError::InstallConflict)?;
            Ok(Some(value))
        }
        Err(error) => Err(error.into()),
    }
}

fn read_bounded_text(path: &Path, maximum: u64) -> Result<String> {
    let file = File::open(path)?;
    if file.metadata()?.len() > maximum {
        return Err(PluginPlatformError::InstallConflict);
    }
    let mut value = String::new();
    file.take(maximum + 1).read_to_string(&mut value)?;
    if value.len() as u64 > maximum || value.contains(['\r', '\n', '\0']) {
        return Err(PluginPlatformError::InstallConflict);
    }
    Ok(value)
}

fn read_lower_hex_marker(path: &Path) -> Result<String> {
    let value = read_bounded_text(path, 64)?;
    if !lower_hex_64(&value) {
        return Err(PluginPlatformError::InstallConflict);
    }
    Ok(value)
}

fn lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_active_theme_layout(version_root: &Path) -> Result<()> {
    reject_tree_symlinks(version_root)?;
    let mut manifest_seen = false;
    let mut package_marker_seen = false;
    let mut theme_marker_seen = false;
    let mut assets_seen = false;
    for entry in fs::read_dir(version_root)? {
        let entry = entry?;
        reject_reparse(&entry.path())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| PluginPlatformError::InstallConflict)?;
        let file_type = entry.file_type()?;
        if file_type.is_file() {
            match name.as_str() {
                "manifest.json" if !manifest_seen => manifest_seen = true,
                HASH_MARKER if !package_marker_seen => package_marker_seen = true,
                THEME_HASH_MARKER if !theme_marker_seen => theme_marker_seen = true,
                _ => return Err(PluginPlatformError::InstallConflict),
            }
        } else if file_type.is_dir() && name == "assets" && !assets_seen {
            assets_seen = true;
            validate_active_theme_assets(&entry.path())?;
        } else {
            return Err(PluginPlatformError::InstallConflict);
        }
    }
    if !manifest_seen || !package_marker_seen || !theme_marker_seen || !assets_seen {
        return Err(PluginPlatformError::InstallConflict);
    }
    Ok(())
}

fn validate_active_theme_assets(assets_root: &Path) -> Result<()> {
    reject_tree_symlinks(assets_root)?;
    let mut theme_seen = false;
    for entry in fs::read_dir(assets_root)? {
        let entry = entry?;
        reject_reparse(&entry.path())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| PluginPlatformError::InstallConflict)?;
        if !entry.file_type()?.is_file() || name != "theme.json" || theme_seen {
            return Err(PluginPlatformError::InstallConflict);
        }
        theme_seen = true;
    }
    if !theme_seen {
        return Err(PluginPlatformError::InstallConflict);
    }
    Ok(())
}

/// Legacy Wasm installations may retain arbitrary assets, but a theme asset or
/// theme hash marker is conclusive evidence that the tree is not executable
/// Wasm. This closes the recovery path for a damaged mixed package.
fn active_wasm_has_theme_artifacts(version_root: &Path) -> Result<bool> {
    let marker = version_root.join(THEME_HASH_MARKER);
    if path_exists_without_reparse(&marker)? {
        return Ok(true);
    }
    let assets_root = version_root.join("assets");
    match fs::symlink_metadata(&assets_root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(PluginPlatformError::InstallConflict);
            }
            reject_reparse(&assets_root)?;
            if !metadata.is_dir() {
                return Ok(false);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    path_exists_without_reparse(&assets_root.join("theme.json"))
}

fn path_exists_without_reparse(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(PluginPlatformError::InstallConflict);
            }
            reject_reparse(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn write_new_synced(path: &Path, value: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(value)?;
    file.sync_all()?;
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(PluginPlatformError::InstallConflict);
    }
    Ok(())
}

fn reject_directory(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    reject_reparse(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(PluginPlatformError::InstallConflict);
    }
    Ok(())
}

fn reject_optional_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => reject_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn path_exists_no_follow(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn reject_reparse(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(PluginPlatformError::InstallConflict);
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

fn reject_tree_symlinks(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    reject_reparse(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(PluginPlatformError::InstallConflict);
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(PluginPlatformError::InstallConflict);
        }
        reject_reparse(&entry.path())?;
        if file_type.is_dir() {
            reject_tree_symlinks(&entry.path())?;
        } else if !file_type.is_file() {
            return Err(PluginPlatformError::InstallConflict);
        }
    }
    Ok(())
}

fn read_regular_file_snapshot(path: &Path, maximum: u64) -> Result<Vec<u8>> {
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
            return Err(PluginPlatformError::InstallConflict);
        }
    }
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(PluginPlatformError::InstallConflict);
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len()).map_err(|_| PluginPlatformError::PackageTooLarge)?,
    );
    source
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(PluginPlatformError::InstallConflict);
    }
    Ok(bytes)
}

fn sync_directory_tree(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directory_tree(&entry.path())?;
        }
    }
    sync_directory(path)
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        // Windows directories require BACKUP_SEMANTICS, and FlushFileBuffers requires write access.
        // Validate the same handle and reject reparse points; the existing commit boundary still handles sync failures.
        let directory = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(PluginPlatformError::InstallConflict);
        }
        directory.sync_all()?;
    }
    #[cfg(not(windows))]
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_directory_tests {
    #[test]
    fn directory_sync_uses_a_writable_directory_handle() {
        let directory = tempfile::tempdir().unwrap();
        super::sync_directory(directory.path()).unwrap();
        let file = directory.path().join("not-a-directory");
        std::fs::write(&file, b"fixture").unwrap();
        assert!(super::sync_directory(&file).is_err());
    }

    #[test]
    fn staging_junction_is_not_an_install_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("plugins");
        let external = directory.path().join("external");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&external).unwrap();
        let junction = root.join(".staging");
        let created = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&external)
            .output()
            .unwrap();
        assert!(created.status.success(), "junction fixture creation failed");
        assert!(super::reject_directory(&root).is_ok());
        assert!(super::reject_directory(&junction).is_err());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::Write as _,
        path::{Path, PathBuf},
    };

    use norishell_core_api::{
        PLUGIN_PROTOCOL_MINOR, PluginCapability, PluginId, PluginOperationId, PluginPackageKind,
    };
    use sha2::{Digest, Sha256};
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::{ACTIVE_POINTER, HASH_MARKER, PluginInstaller, StagedPlugin};
    use crate::{
        InspectedFile, InspectedPackage, PackageLimits, PluginManifest, inspect_local_package,
        inspect_plugin_protocols,
    };

    #[test]
    fn same_version_replacement_keeps_recoverable_old_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PackageLimits::default())
                .expect("installer");
        let plugin_id = PluginId::parse("com.norishell.replacement").expect("plugin id");
        let plugin_root = installer.root.join(plugin_id.as_str());
        let versions = plugin_root.join("versions");
        std::fs::create_dir_all(&versions).expect("versions");
        std::fs::write(plugin_root.join(ACTIVE_POINTER), b"1.0.0").expect("pointer");
        let old_hash = "a".repeat(64);
        let new_hash = "b".repeat(64);
        let live = versions.join("1.0.0");
        std::fs::create_dir(&live).expect("old version");
        std::fs::write(live.join(HASH_MARKER), &old_hash).expect("old hash");
        std::fs::write(live.join("plugin.wasm"), b"old").expect("old code");

        let operation = PluginOperationId::new();
        let staging_root = installer.root.join(".staging");
        std::fs::create_dir(&staging_root).expect("staging root");
        let staged_path = staging_root.join("candidate");
        std::fs::create_dir(&staged_path).expect("candidate");
        std::fs::write(staged_path.join(HASH_MARKER), &new_hash).expect("new hash");
        std::fs::write(staged_path.join("plugin.wasm"), b"new").expect("new code");
        let staged = StagedPlugin {
            plugin_id: plugin_id.clone(),
            version: "1.0.0".to_owned(),
            package_sha256: [0xbb; 32],
            path: staged_path,
        };
        installer
            .activate_replacement(staged, &old_hash, &operation)
            .expect("replace");
        assert_eq!(
            installer.read_active_package_sha256(&plugin_id).unwrap(),
            Some(new_hash.clone())
        );
        assert!(
            installer
                .replacement_backup_exists(&plugin_id, &operation)
                .unwrap()
        );
        installer
            .restore_replacement(&plugin_id, "1.0.0", &old_hash, &new_hash, &operation)
            .expect("restore after failed database commit");
        assert_eq!(
            installer.read_active_package_sha256(&plugin_id).unwrap(),
            Some(old_hash)
        );
        assert_eq!(std::fs::read(live.join("plugin.wasm")).unwrap(), b"old");
        assert!(
            !installer
                .replacement_backup_exists(&plugin_id, &operation)
                .unwrap()
        );

        let interrupted_operation = PluginOperationId::new();
        let backup = installer.replacement_backup(&plugin_id, &interrupted_operation);
        std::fs::rename(&live, &backup).expect("simulate interrupted directory switch");
        installer
            .restore_replacement(
                &plugin_id,
                "1.0.0",
                &"a".repeat(64),
                &new_hash,
                &interrupted_operation,
            )
            .expect("restore when candidate directory was not installed");
        assert_eq!(std::fs::read(live.join("plugin.wasm")).unwrap(), b"old");

        let final_operation = PluginOperationId::new();
        let final_staged_path = staging_root.join("final-candidate");
        std::fs::create_dir(&final_staged_path).expect("final candidate");
        std::fs::write(final_staged_path.join(HASH_MARKER), &new_hash).expect("hash");
        let final_staged = StagedPlugin {
            plugin_id: plugin_id.clone(),
            version: "1.0.0".to_owned(),
            package_sha256: [0xbb; 32],
            path: final_staged_path,
        };
        installer
            .activate_replacement(final_staged, &"a".repeat(64), &final_operation)
            .expect("activate final candidate");
        installer
            .finalize_replacement(&plugin_id, "1.0.0", &new_hash, &final_operation)
            .expect("finalize committed replacement");
        assert!(
            !installer
                .replacement_backup_exists(&plugin_id, &final_operation)
                .unwrap()
        );
        assert_eq!(
            installer.read_active_package_sha256(&plugin_id).unwrap(),
            Some(new_hash)
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacement_rejects_linked_ancestors_before_touching_external_tree() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("tempdir");
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PackageLimits::default())
                .expect("installer");
        let plugin_id = PluginId::parse("com.norishell.linked").expect("plugin id");
        let external = directory.path().join("external");
        std::fs::create_dir(&external).expect("external");
        std::fs::write(external.join("sentinel"), b"unchanged").expect("sentinel");
        let plugin_root = installer.root.join(plugin_id.as_str());
        symlink(&external, &plugin_root).expect("linked plugin root");
        assert!(installer.read_active_package_sha256(&plugin_id).is_err());
        std::fs::remove_file(&plugin_root).expect("remove link");

        let versions = plugin_root.join("versions");
        std::fs::create_dir_all(&versions).expect("versions");
        std::fs::write(plugin_root.join(ACTIVE_POINTER), b"1.0.0").expect("pointer");
        let live = versions.join("1.0.0");
        std::fs::create_dir(&live).expect("live");
        std::fs::write(live.join(HASH_MARKER), "a".repeat(64)).expect("old hash");
        let staging_root = installer.root.join(".staging");
        symlink(&external, &staging_root).expect("linked staging root");
        let staging = staging_root.join("candidate");
        assert!(
            installer
                .activate_replacement(
                    StagedPlugin {
                        plugin_id: plugin_id.clone(),
                        version: "1.0.0".to_owned(),
                        package_sha256: [0xbb; 32],
                        path: staging.clone(),
                    },
                    &"a".repeat(64),
                    &PluginOperationId::new(),
                )
                .is_err()
        );
        std::fs::remove_file(&staging_root).expect("remove staging link");
        std::fs::create_dir(&staging_root).expect("staging root");
        std::fs::create_dir(&staging).expect("staging");
        std::fs::write(staging.join(HASH_MARKER), "b".repeat(64)).expect("new hash");
        symlink(&external, installer.root.join(".replaced")).expect("linked backup root");
        let operation = PluginOperationId::new();
        let staged = StagedPlugin {
            plugin_id: plugin_id.clone(),
            version: "1.0.0".to_owned(),
            package_sha256: [0xbb; 32],
            path: staging,
        };
        assert!(
            installer
                .activate_replacement(staged, &"a".repeat(64), &operation)
                .is_err()
        );
        assert!(
            installer
                .restore_replacement(
                    &plugin_id,
                    "1.0.0",
                    &"a".repeat(64),
                    &"b".repeat(64),
                    &operation,
                )
                .is_err()
        );
        assert!(
            installer
                .finalize_replacement(&plugin_id, "1.0.0", &"a".repeat(64), &operation,)
                .is_err()
        );
        assert_eq!(
            std::fs::read(external.join("sentinel")).unwrap(),
            b"unchanged"
        );
        assert_eq!(
            std::fs::read(live.join(HASH_MARKER)).unwrap(),
            "a".repeat(64).as_bytes()
        );
    }

    #[test]
    fn immutable_version_and_active_pointer_use_compare_and_swap() {
        let directory = tempfile::tempdir().expect("tempdir");
        let package_path = directory.path().join("fixture.zip");
        let manifest = PluginManifest {
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
            capabilities: vec![
                PluginCapability::UiPanel,
                PluginCapability::UiWebviewIsolated,
            ],
            minimum_app_version: "0.1.0".to_owned(),
            minimum_core_api_version: None,
        };
        let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest");
        let file = File::create(&package_path).expect("package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive
            .start_file("manifest.json", options)
            .expect("manifest");
        archive.write_all(&manifest_bytes).expect("manifest bytes");
        archive.start_file("plugin.wasm", options).expect("wasm");
        archive.write_all(b"\0asm\x01\0\0\0").expect("wasm bytes");
        archive
            .start_file("assets/isolated/panel.html", options)
            .expect("isolated surface");
        archive
            .write_all(b"<main>Isolated</main>")
            .expect("isolated surface bytes");
        let protocols = r#"{"schemaVersion":1,"providers":[{"id":"telnet","label":{"en":"Telnet","zh-CN":"Telnet"},"configuration":{"schemaVersion":1,"fields":[{"type":"number","key":"port","label":{"en":"Port","zh-CN":"端口"},"default":23,"min":1,"max":65535}]},"features":{"terminal":true,"resize":"supported","reconnect":true},"resources":["tcp"]}]}"#;
        archive
            .start_file("assets/protocols.json", options)
            .expect("protocol catalog");
        archive
            .write_all(protocols.as_bytes())
            .expect("protocol catalog bytes");
        archive.finish().expect("finish");
        let package = std::fs::read(&package_path).expect("package bytes");
        let inspected = InspectedPackage {
            manifest,
            package_size: package.len() as u64,
            package_sha256: Sha256::digest(&package).into(),
            files: vec![
                InspectedFile {
                    relative_path: PathBuf::from("assets/isolated/panel.html"),
                    uncompressed_size: 21,
                },
                InspectedFile {
                    relative_path: PathBuf::from("assets/protocols.json"),
                    uncompressed_size: protocols.len() as u64,
                },
                InspectedFile {
                    relative_path: PathBuf::from("manifest.json"),
                    uncompressed_size: manifest_bytes.len() as u64,
                },
                InspectedFile {
                    relative_path: PathBuf::from("plugin.wasm"),
                    uncompressed_size: 8,
                },
            ],
            package_kind: PluginPackageKind::Wasm,
            theme_definition: None,
            theme_definition_sha256: None,
            settings: None,
            protocols: Some(
                inspect_plugin_protocols(protocols.as_bytes()).expect("protocol catalog"),
            ),
            workflows: None,
        };
        let installer =
            PluginInstaller::new(directory.path().join("plugins"), PackageLimits::default())
                .expect("installer");
        let operation_id = PluginOperationId::new();
        let staged = installer
            .stage(&package_path, &inspected, &operation_id)
            .expect("stage");
        let activated = installer
            .activate(staged, None, &operation_id)
            .expect("activate");
        assert_eq!(activated.active_version, "1.0.0");
        assert_eq!(
            installer
                .read_active_version(&inspected.manifest.plugin_id)
                .expect("active"),
            Some("1.0.0".to_owned())
        );
        assert_eq!(
            installer
                .read_active_module(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    &crate::lower_hex(&inspected.package_sha256),
                    1024,
                )
                .expect("verified active wasm"),
            b"\0asm\x01\0\0\0"
        );
        assert_eq!(
            installer
                .read_active_isolated_surface(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    &crate::lower_hex(&inspected.package_sha256),
                    "panel",
                    1024,
                )
                .expect("verified isolated surface"),
            b"<main>Isolated</main>"
        );
        assert!(
            installer
                .read_active_isolated_surface(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    &crate::lower_hex(&inspected.package_sha256),
                    "../panel",
                    1024,
                )
                .is_err()
        );
        assert_eq!(
            installer
                .read_protocol_catalog(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    &crate::lower_hex(&inspected.package_sha256),
                )
                .expect("protocol catalog")
                .expect("declared catalog")
                .catalog
                .providers[0]
                .id,
            "telnet"
        );

        let active_directory = &activated.version_directory;
        std::fs::write(active_directory.join("assets/theme.json"), b"{}")
            .expect("legacy mixed theme asset");
        assert!(
            installer
                .active_package_kind(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    &crate::lower_hex(&inspected.package_sha256),
                )
                .is_err()
        );
        std::fs::remove_file(active_directory.join("assets/theme.json"))
            .expect("remove legacy mixed theme asset");
        std::fs::write(
            active_directory.join(super::THEME_HASH_MARKER),
            "0".repeat(64),
        )
        .expect("legacy theme hash marker");
        assert!(
            installer
                .active_package_kind(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    &crate::lower_hex(&inspected.package_sha256),
                )
                .is_err()
        );
        std::fs::remove_file(active_directory.join(super::THEME_HASH_MARKER))
            .expect("remove legacy theme hash marker");

        let replay_stage = installer
            .stage(&package_path, &inspected, &PluginOperationId::new())
            .expect("stage replay");
        assert!(
            installer
                .activate(replay_stage, None, &PluginOperationId::new())
                .is_err()
        );
        let inactive_version = directory
            .path()
            .join("plugins/com.norishell.fixture/versions/0.9.0");
        std::fs::create_dir(&inactive_version).expect("inactive immutable version fixture");
        std::fs::write(inactive_version.join("plugin.wasm"), b"old").expect("inactive fixture");
        std::fs::write(inactive_version.join(".package-sha256"), "a".repeat(64))
            .expect("inactive hash fixture");
        installer
            .restore_active_version(
                &inspected.manifest.plugin_id,
                "1.0.0",
                Some("0.9.0"),
                &PluginOperationId::new(),
            )
            .expect("restore old pointer");
        assert!(
            installer
                .restore_active_version(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    None,
                    &PluginOperationId::new(),
                )
                .is_err(),
            "stale expected pointer must fail closed"
        );
        installer
            .restore_active_version(
                &inspected.manifest.plugin_id,
                "0.9.0",
                Some("1.0.0"),
                &PluginOperationId::new(),
            )
            .expect("restore candidate pointer for uninstall");
        let uninstall_operation = PluginOperationId::new();
        let removed = installer
            .prepare_uninstall(&inspected.manifest.plugin_id, "1.0.0", &uninstall_operation)
            .expect("prepare uninstall with pointer CAS");
        assert_eq!(removed.removed_version, "1.0.0");
        assert!(
            !directory
                .path()
                .join("plugins/com.norishell.fixture/versions")
                .exists()
        );
        assert_eq!(
            installer
                .read_active_version(&inspected.manifest.plugin_id)
                .expect("no active version"),
            None
        );
        installer
            .restore_uninstall(&inspected.manifest.plugin_id, "1.0.0", &uninstall_operation)
            .expect("restore prepared uninstall");
        assert_eq!(
            installer
                .read_active_version(&inspected.manifest.plugin_id)
                .expect("restored active version"),
            Some("1.0.0".to_owned())
        );
        installer
            .prepare_uninstall(&inspected.manifest.plugin_id, "1.0.0", &uninstall_operation)
            .expect("prepare uninstall again");
        installer
            .finalize_uninstall(&inspected.manifest.plugin_id, "1.0.0", &uninstall_operation)
            .expect("finalize prepared uninstall");
        assert!(
            installer
                .prepare_uninstall(
                    &inspected.manifest.plugin_id,
                    "1.0.0",
                    &PluginOperationId::new(),
                )
                .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let external = directory.path().join("external-staging");
            std::fs::create_dir(&external).expect("external staging");
            std::fs::write(external.join("sentinel"), b"unchanged").expect("sentinel");
            let staging_root = installer.root.join(".staging");
            std::fs::remove_dir_all(&staging_root).expect("clear staging");
            symlink(&external, &staging_root).expect("linked staging");
            assert!(
                installer
                    .stage(&package_path, &inspected, &PluginOperationId::new())
                    .is_err()
            );
            assert_eq!(
                std::fs::read(external.join("sentinel")).expect("untouched sentinel"),
                b"unchanged"
            );
        }
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn theme_example_package(
        root: &Path,
        temporary: &Path,
        slug: &str,
        file_name: &str,
    ) -> PathBuf {
        let generated = root.join("output/theme-plugins").join(file_name);
        if generated.is_file() {
            return generated;
        }
        let source = root.join("examples/theme-plugins").join(slug);
        let package = temporary.join(file_name);
        let file = File::create(&package).expect("theme package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for relative in ["manifest.json", "assets/theme.json"] {
            archive.start_file(relative, options).expect("theme entry");
            archive
                .write_all(&std::fs::read(source.join(relative)).expect("theme source"))
                .expect("theme bytes");
        }
        archive.finish().expect("finish theme package");
        package
    }

    #[test]
    fn pure_data_theme_examples_stage_and_fail_closed_after_asset_tampering() {
        let directory = tempfile::tempdir().expect("directory");
        let root = workspace_root();
        for (slug, file_name) in [
            ("clear", "NoriShell-Theme-Clear-1.0.0.zip"),
            ("midnight", "NoriShell-Theme-Midnight-1.0.0.zip"),
            ("sand", "NoriShell-Theme-Sand-1.0.0.zip"),
        ] {
            let package = theme_example_package(&root, directory.path(), slug, file_name);
            let inspected = inspect_local_package(
                &package,
                &semver::Version::parse("0.1.0").expect("app version"),
                std::env::consts::ARCH,
                PackageLimits::default(),
            )
            .expect("inspect theme package");
            assert_eq!(inspected.package_kind, PluginPackageKind::Theme);
            assert_eq!(
                inspected
                    .theme_definition
                    .as_ref()
                    .expect("theme summary")
                    .id,
                slug
            );
            let installer = PluginInstaller::new(
                directory.path().join(format!("plugins-{slug}")),
                PackageLimits::default(),
            )
            .expect("installer");
            let operation_id = PluginOperationId::new();
            let staged = installer
                .stage(&package, &inspected, &operation_id)
                .expect("stage theme");
            let activated = installer
                .activate(staged, None, &operation_id)
                .expect("activate theme");
            let package_hash = crate::lower_hex(&inspected.package_sha256);
            assert_eq!(
                installer
                    .active_package_kind(
                        &inspected.manifest.plugin_id,
                        &activated.active_version,
                        &package_hash,
                    )
                    .expect("verified theme kind"),
                PluginPackageKind::Theme
            );
            assert_eq!(
                installer
                    .read_active_theme(
                        &inspected.manifest.plugin_id,
                        &activated.active_version,
                        &package_hash,
                    )
                    .expect("verified theme")
                    .id,
                slug
            );
            assert!(
                !activated
                    .version_directory
                    .join(".plugin-wasm-sha256")
                    .exists()
            );
            std::fs::write(activated.version_directory.join("assets/theme.json"), b"{}")
                .expect("tamper theme asset");
            assert!(
                installer
                    .read_active_theme(
                        &inspected.manifest.plugin_id,
                        &activated.active_version,
                        &package_hash,
                    )
                    .is_err()
            );
        }
    }
}
