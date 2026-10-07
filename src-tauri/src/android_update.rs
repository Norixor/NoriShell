//! Android in-app update: the desktop updater plugin is not registered on
//! Android, so this module repeats its trust rules for an APK.
//!
//! Flow (driven by `src/stores/appUpdate.ts`):
//! 1. `android_update_prepare` reads `latest.json`, requires the announced
//!    version to equal the version the release check reported, downloads the
//!    APK from the fixed GitHub release origins into the app cache and verifies
//!    the minisign signature with the configured updater public key, including
//!    the signed-version rule (`requireSignedVersion`).
//! 2. The frontend runs the shared resource fence (`release_update_readiness`
//!    and `release_update_prepare_install`).
//! 3. `android_update_install` re-verifies the cached file and hands it to the
//!    Kotlin `ApkInstaller` (PackageInstaller session, system confirmation).
//!    `android_update_install_result` reports the session outcome.
//!
//! Every failure carries a stable `android_update.*` code; messages are never
//! used for control flow.

// The verification and manifest logic is portable so desktop tests cover it;
// only the bridge-facing commands are Android specific.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{Manager, ipc::Channel};

const MANIFEST_URL: &str =
    "https://github.com/Norixor/NoriShell/releases/latest/download/latest.json";
const RELEASE_DOWNLOAD_PREFIX: &str = "https://github.com/Norixor/NoriShell/releases/download/";
/// Hosts a GitHub release asset download may redirect through.
const REDIRECT_HOSTS: [&str; 4] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];
const MAX_REDIRECTS: usize = 5;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_APK_BYTES: u64 = 400 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(15);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(20 * 60);
const VERIFY_BUFFER: usize = 256 * 1024;
const UPDATE_DIRECTORY: &str = "update";

#[cfg(target_arch = "aarch64")]
const PLATFORM_KEY: &str = "android-aarch64";
#[cfg(target_arch = "aarch64")]
const ARTIFACT_ARCH: &str = "arm64";
#[cfg(not(target_arch = "aarch64"))]
const PLATFORM_KEY: &str = "android-x86_64";
#[cfg(not(target_arch = "aarch64"))]
const ARTIFACT_ARCH: &str = "x64";

/// Stable machine-readable outcome codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Code {
    WindowNotAllowed,
    NoUpdate,
    PackageUnavailable,
    ManifestInvalid,
    RequestFailed,
    DownloadFailed,
    SizeExceeded,
    SignatureInvalid,
    VersionMismatch,
    PermissionRequired,
    NotPrepared,
    UserCancelled,
    InstallBlocked,
    InstallFailed,
    BridgeUnavailable,
    Internal,
}

impl Code {
    const fn as_str(self) -> &'static str {
        match self {
            Self::WindowNotAllowed => "android_update.window_not_allowed",
            Self::NoUpdate => "android_update.no_update",
            Self::PackageUnavailable => "android_update.package_unavailable",
            Self::ManifestInvalid => "android_update.manifest_invalid",
            Self::RequestFailed => "android_update.request_failed",
            Self::DownloadFailed => "android_update.download_failed",
            Self::SizeExceeded => "android_update.size_exceeded",
            Self::SignatureInvalid => "android_update.signature_invalid",
            Self::VersionMismatch => "android_update.version_mismatch",
            Self::PermissionRequired => "android_update.permission_required",
            Self::NotPrepared => "android_update.not_prepared",
            Self::UserCancelled => "android_update.user_cancelled",
            Self::InstallBlocked => "android_update.install_blocked",
            Self::InstallFailed => "android_update.install_failed",
            Self::BridgeUnavailable => "android_update.bridge_unavailable",
            Self::Internal => "android_update.internal",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Failure {
    code: &'static str,
    /// Platform-specific cause (for example the Kotlin installer code); never a message.
    detail: Option<String>,
    diagnostic_id: Option<String>,
}

impl Failure {
    fn new(code: Code) -> Self {
        Self {
            code: code.as_str(),
            detail: None,
            diagnostic_id: None,
        }
    }

    fn with_detail(code: Code, detail: impl Into<String>) -> Self {
        let mut failure = Self::new(code);
        failure.detail = Some(detail.into());
        failure
    }

    fn internal() -> Self {
        let diagnostic_id = uuid::Uuid::new_v4().to_string();
        eprintln!("android_update internal failure: diagnostic_id={diagnostic_id}");
        let mut failure = Self::new(Code::Internal);
        failure.diagnostic_id = Some(diagnostic_id);
        failure
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    platforms: BTreeMap<String, ManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    url: String,
    signature: String,
}

/// Where the manifest comes from and which key signs it. Release builds always
/// use the fixed endpoint and the `plugins.updater.pubkey` from tauri.conf.json.
struct Source {
    manifest_url: String,
    pubkey: String,
    /// Debug-only test override: allows plain HTTP and arbitrary artifact URLs.
    relaxed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DebugOverride {
    manifest_url: String,
    pubkey: String,
}

fn resolve_source<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<Source, Failure> {
    let pubkey = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .ok_or_else(Failure::internal)?;
    let source = Source {
        manifest_url: MANIFEST_URL.to_owned(),
        pubkey,
        relaxed: false,
    };
    // Compiled out of release builds: lets a debug APK be tested against a
    // local server and a throwaway key. The file lives in the app-private data
    // directory, so only `adb run-as` on a debuggable build can create it.
    #[cfg(debug_assertions)]
    {
        if let Ok(directory) = app.path().app_data_dir()
            && let Ok(bytes) = std::fs::read(directory.join("debug-update-override.json"))
            && let Ok(over) = serde_json::from_slice::<DebugOverride>(&bytes)
        {
            return Ok(Source {
                manifest_url: over.manifest_url,
                pubkey: over.pubkey,
                relaxed: true,
            });
        }
    }
    let _ = &app;
    Ok(source)
}

fn expected_artifact_url(version: &Version) -> String {
    format!("{RELEASE_DOWNLOAD_PREFIX}v{version}/NoriShell_{version}_android_{ARTIFACT_ARCH}.apk")
}

fn allowed_redirect_host(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| REDIRECT_HOSTS.contains(&host))
}

/// Mirrors the vendored updater's `verify_signed_version`: the manifest is not
/// signed, so the version inside the signed trusted comment is what binds the
/// artifact to the announced version.
fn verify_signed_version(trusted_comment: &str, announced: &str) -> Result<(), Code> {
    let signed = trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or(Code::VersionMismatch)?;
    let matches = match (
        Version::parse(signed.trim_start_matches('v')),
        Version::parse(announced.trim_start_matches('v')),
    ) {
        (Ok(signed), Ok(announced)) => signed == announced,
        _ => signed == announced,
    };
    if matches {
        Ok(())
    } else {
        Err(Code::VersionMismatch)
    }
}

fn decode_base64_text(value: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Streams the file through the minisign verifier and checks the signed version.
fn verify_file(path: &Path, signature: &str, pubkey: &str, version: &str) -> Result<(), Code> {
    let public_key = decode_base64_text(pubkey)
        .and_then(|text| PublicKey::decode(&text).ok())
        .ok_or(Code::Internal)?;
    let signature = decode_base64_text(signature)
        .and_then(|text| Signature::decode(&text).ok())
        .ok_or(Code::SignatureInvalid)?;
    let mut verifier = public_key
        .verify_stream(&signature)
        .map_err(|_| Code::SignatureInvalid)?;
    let mut file = std::fs::File::open(path).map_err(|_| Code::Internal)?;
    let mut buffer = vec![0_u8; VERIFY_BUFFER];
    loop {
        let read = file.read(&mut buffer).map_err(|_| Code::Internal)?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    // finalize also verifies the global signature that covers the trusted comment.
    verifier.finalize().map_err(|_| Code::SignatureInvalid)?;
    verify_signed_version(signature.trusted_comment(), version)
}

fn apk_path(directory: &Path, version: &Version) -> PathBuf {
    directory.join(format!("NoriShell_{version}_android.apk"))
}

#[derive(Clone)]
struct Prepared {
    path: PathBuf,
    version: Version,
    signature: String,
    pubkey: String,
}

static PREPARED: Mutex<Option<Prepared>> = Mutex::new(None);

fn client(relaxed: bool) -> Result<reqwest::Client, Failure> {
    let _ = relaxed;
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        // Redirects are followed by hand so every hop is checked against the allowlist.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("NoriShell/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| Failure::internal())
}

/// GET with manual, allowlisted redirects. The first URL is trusted by the caller.
async fn get_following(
    client: &reqwest::Client,
    first: &str,
    relaxed: bool,
    timeout: Option<Duration>,
    failure: Code,
) -> Result<reqwest::Response, Failure> {
    let mut url = reqwest::Url::parse(first).map_err(|_| Failure::new(Code::ManifestInvalid))?;
    for _ in 0..=MAX_REDIRECTS {
        let mut request = client.get(url.clone());
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await.map_err(|_| Failure::new(failure))?;
        if response.status().is_redirection() {
            let next = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| url.join(location).ok())
                .ok_or_else(|| Failure::new(failure))?;
            if !relaxed && !allowed_redirect_host(&next) {
                return Err(Failure::new(failure));
            }
            url = next;
            continue;
        }
        if !response.status().is_success() {
            return Err(Failure::with_detail(
                failure,
                response.status().as_u16().to_string(),
            ));
        }
        return Ok(response);
    }
    Err(Failure::new(failure))
}

async fn fetch_manifest(client: &reqwest::Client, source: &Source) -> Result<Manifest, Failure> {
    if !source.relaxed && source.manifest_url != MANIFEST_URL {
        return Err(Failure::internal());
    }
    let mut response = get_following(
        client,
        &source.manifest_url,
        source.relaxed,
        Some(MANIFEST_TIMEOUT),
        Code::RequestFailed,
    )
    .await?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Failure::new(Code::RequestFailed))?
    {
        if chunk.len() > MAX_MANIFEST_BYTES.saturating_sub(body.len()) {
            return Err(Failure::new(Code::ManifestInvalid));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| Failure::new(Code::ManifestInvalid))
}

/// Decides whether the manifest offers `expected` for this device and returns its entry.
fn select_entry<'a>(
    manifest: &'a Manifest,
    current: &Version,
    expected: &Version,
    relaxed: bool,
) -> Result<(&'a ManifestEntry, Version), Failure> {
    let announced = Version::parse(manifest.version.trim_start_matches('v'))
        .map_err(|_| Failure::new(Code::ManifestInvalid))?;
    // A stable install is never offered a prerelease (same rule as release_check).
    if current.pre.is_empty() && !announced.pre.is_empty() {
        return Err(Failure::new(Code::NoUpdate));
    }
    if !announced.cmp_precedence(current).is_gt() {
        return Err(Failure::new(Code::NoUpdate));
    }
    if announced != *expected {
        return Err(Failure::new(Code::VersionMismatch));
    }
    let entry = manifest
        .platforms
        .get(PLATFORM_KEY)
        .ok_or_else(|| Failure::new(Code::PackageUnavailable))?;
    if !relaxed && entry.url != expected_artifact_url(&announced) {
        return Err(Failure::new(Code::PackageUnavailable));
    }
    Ok((entry, announced))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedUpdate {
    version: String,
    size: u64,
    /// Whether "install unknown apps" is already allowed for NoriShell.
    can_install: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

fn is_main_or_page<R: tauri::Runtime>(webview: &tauri::Webview<R>) -> bool {
    webview
        .app_handle()
        .state::<crate::workspace_windows::WorkspaceWindows>()
        .is_main_or_page(webview)
}

async fn download(
    client: &reqwest::Client,
    source: &Source,
    url: &str,
    target: &Path,
    progress: &Channel<DownloadProgress>,
) -> Result<u64, Failure> {
    use tokio::io::AsyncWriteExt;

    let mut response =
        get_following(client, url, source.relaxed, None, Code::DownloadFailed).await?;
    let total = response.content_length();
    if total.is_some_and(|length| length > MAX_APK_BYTES) {
        return Err(Failure::new(Code::SizeExceeded));
    }
    let partial = target.with_extension("apk.part");
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|_| Failure::internal())?;
    let started = Instant::now();
    let mut downloaded = 0_u64;
    let mut last_percent = None;
    let outcome: Result<(), Failure> = async {
        loop {
            if started.elapsed() > DOWNLOAD_DEADLINE {
                return Err(Failure::new(Code::DownloadFailed));
            }
            let chunk = tokio::time::timeout(CHUNK_TIMEOUT, response.chunk())
                .await
                .map_err(|_| Failure::new(Code::DownloadFailed))?
                .map_err(|_| Failure::new(Code::DownloadFailed))?;
            let Some(chunk) = chunk else { break };
            downloaded += chunk.len() as u64;
            if downloaded > MAX_APK_BYTES {
                return Err(Failure::new(Code::SizeExceeded));
            }
            file.write_all(&chunk)
                .await
                .map_err(|_| Failure::internal())?;
            let percent = total
                .filter(|total| *total > 0)
                .map(|total| downloaded * 100 / total);
            if percent != last_percent {
                last_percent = percent;
                let _ = progress.send(DownloadProgress { downloaded, total });
            }
        }
        file.flush().await.map_err(|_| Failure::internal())?;
        file.sync_all().await.map_err(|_| Failure::internal())?;
        if total.is_some_and(|total| total != downloaded) || downloaded == 0 {
            return Err(Failure::new(Code::DownloadFailed));
        }
        Ok(())
    }
    .await;
    drop(file);
    if let Err(error) = outcome {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(error);
    }
    tokio::fs::rename(&partial, target)
        .await
        .map_err(|_| Failure::internal())?;
    Ok(downloaded)
}

fn update_directory<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, Failure> {
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|_| Failure::internal())?
        .join(UPDATE_DIRECTORY);
    std::fs::create_dir_all(&directory).map_err(|_| Failure::internal())?;
    Ok(directory)
}

/// Removes every cached update file except `keep`.
fn prune_directory(directory: &Path, keep: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if Some(path.as_path()) != keep {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[tauri::command]
pub(crate) async fn android_update_prepare<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
    expected_version: String,
    progress: Channel<DownloadProgress>,
) -> Result<PreparedUpdate, Failure> {
    if !is_main_or_page(&webview) {
        return Err(Failure::new(Code::WindowNotAllowed));
    }
    let app = webview.app_handle();
    let expected = Version::parse(expected_version.trim_start_matches('v'))
        .map_err(|_| Failure::new(Code::VersionMismatch))?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| Failure::internal())?;
    let source = resolve_source(app)?;
    let client = client(source.relaxed)?;
    let manifest = fetch_manifest(&client, &source).await?;
    let (entry, version) = select_entry(&manifest, &current, &expected, source.relaxed)?;
    let directory = update_directory(app)?;
    let target = apk_path(&directory, &version);
    *PREPARED.lock().map_err(|_| Failure::internal())? = None;
    prune_directory(&directory, Some(&target));

    // A previously downloaded, still valid file is reused (retry after the
    // "install unknown apps" detour); anything else is downloaded again.
    let reusable = target.is_file()
        && verify_file(
            &target,
            &entry.signature,
            &source.pubkey,
            &version.to_string(),
        )
        .is_ok();
    if !reusable {
        let _ = std::fs::remove_file(&target);
        download(&client, &source, &entry.url, &target, &progress).await?;
        if let Err(code) = verify_file(
            &target,
            &entry.signature,
            &source.pubkey,
            &version.to_string(),
        ) {
            // Never leave an unverified package where install could pick it up.
            let _ = std::fs::remove_file(&target);
            return Err(Failure::new(code));
        }
    }
    let size = std::fs::metadata(&target)
        .map_err(|_| Failure::internal())?
        .len();
    *PREPARED.lock().map_err(|_| Failure::internal())? = Some(Prepared {
        path: target,
        version: version.clone(),
        signature: entry.signature.clone(),
        pubkey: source.pubkey,
    });
    let can_install = bridge_call::<BridgeStatus>(json_status())
        .await
        .map(|status| status.can_install)
        .unwrap_or(false);
    Ok(PreparedUpdate {
        version: version.to_string(),
        size,
        can_install,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeStatus {
    #[serde(default)]
    can_install: bool,
}

#[derive(Debug, Deserialize)]
struct BridgeResult {
    state: String,
    #[serde(default)]
    code: Option<String>,
}

fn json_status() -> serde_json::Value {
    serde_json::json!({ "action": "status" })
}

/// Runs a Kotlin `apkInstall` action off the async worker (the call blocks).
async fn bridge_call<T: serde::de::DeserializeOwned + Send + 'static>(
    payload: serde_json::Value,
) -> Result<T, Failure> {
    #[cfg(target_os = "android")]
    {
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            tauri_plugin_norishell_android::run_command::<T>("apkInstall", payload)
        })
        .await
        .map_err(|_| Failure::internal())?;
        outcome.map_err(|error| match error {
            tauri_plugin_norishell_android::BridgeError::Rejected { code } => match code.as_str() {
                "update.permission_required" => Failure::new(Code::PermissionRequired),
                _ => Failure::with_detail(Code::InstallFailed, code),
            },
            other => Failure::with_detail(Code::BridgeUnavailable, other.code()),
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = payload;
        Err::<T, _>(Failure::new(Code::BridgeUnavailable))
    }
}

#[tauri::command]
pub(crate) async fn android_update_open_install_settings<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
) -> Result<(), Failure> {
    if !is_main_or_page(&webview) {
        return Err(Failure::new(Code::WindowNotAllowed));
    }
    bridge_call::<serde_json::Value>(serde_json::json!({ "action": "openSettings" }))
        .await
        .map(|_| ())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallStarted {
    version: String,
}

/// Re-verifies the prepared APK and starts the PackageInstaller session. The
/// frontend has already passed the resource fence.
#[tauri::command]
pub(crate) async fn android_update_install<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
) -> Result<InstallStarted, Failure> {
    if !is_main_or_page(&webview) {
        return Err(Failure::new(Code::WindowNotAllowed));
    }
    let prepared = PREPARED
        .lock()
        .map_err(|_| Failure::internal())?
        .clone()
        .ok_or_else(|| Failure::new(Code::NotPrepared))?;
    let version = prepared.version.to_string();
    let check = prepared.clone();
    tauri::async_runtime::spawn_blocking(move || {
        verify_file(
            &check.path,
            &check.signature,
            &check.pubkey,
            &check.version.to_string(),
        )
    })
    .await
    .map_err(|_| Failure::internal())?
    .map_err(Failure::new)?;
    let path = prepared
        .path
        .to_str()
        .ok_or_else(Failure::internal)?
        .to_owned();
    bridge_call::<serde_json::Value>(serde_json::json!({ "action": "install", "path": path }))
        .await?;
    Ok(InstallStarted { version })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallResult {
    /// `none`, `awaitingUser`, `installed`; failures are returned as errors.
    state: &'static str,
}

/// Polled by the frontend after `android_update_install`. A successful install
/// normally replaces the process before this can report `installed`.
#[tauri::command]
pub(crate) async fn android_update_install_result<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
) -> Result<InstallResult, Failure> {
    if !is_main_or_page(&webview) {
        return Err(Failure::new(Code::WindowNotAllowed));
    }
    let result = bridge_call::<BridgeResult>(serde_json::json!({ "action": "result" })).await?;
    match result.state.as_str() {
        "awaiting_user" | "committing" => Ok(InstallResult {
            state: "awaitingUser",
        }),
        "installed" => Ok(InstallResult { state: "installed" }),
        "cancelled" => Err(Failure::new(Code::UserCancelled)),
        "blocked" => Err(Failure::new(Code::InstallBlocked)),
        "failed" => Err(match result.code {
            Some(code) => Failure::with_detail(Code::InstallFailed, code),
            None => Failure::new(Code::InstallFailed),
        }),
        _ => Ok(InstallResult { state: "none" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, url: &str) -> Manifest {
        Manifest {
            version: version.into(),
            platforms: BTreeMap::from([(
                PLATFORM_KEY.to_owned(),
                ManifestEntry {
                    url: url.into(),
                    signature: String::new(),
                },
            )]),
        }
    }

    #[test]
    fn selects_only_the_expected_newer_stable_version() {
        let current = Version::parse("0.1.7").unwrap();
        let next = Version::parse("0.1.8").unwrap();
        let url = expected_artifact_url(&next);
        let good = manifest("0.1.8", &url);
        assert!(select_entry(&good, &current, &next, false).is_ok());

        fn pick(m: &Manifest, c: &Version, n: &Version, relaxed: bool) -> Option<&'static str> {
            select_entry(m, c, n, relaxed)
                .err()
                .map(|failure| failure.code)
        }
        assert_eq!(
            pick(&manifest("0.1.7", &url), &current, &current, false),
            Some("android_update.no_update")
        );
        assert_eq!(
            pick(&manifest("0.1.9", &url), &current, &next, false),
            Some("android_update.version_mismatch")
        );
        assert_eq!(
            pick(&manifest("0.2.0-beta.1", &url), &current, &next, false),
            Some("android_update.no_update")
        );
        assert_eq!(
            pick(
                &manifest("0.1.8", "https://evil.example/a.apk"),
                &current,
                &next,
                false
            ),
            Some("android_update.package_unavailable")
        );
        assert!(
            select_entry(
                &manifest("0.1.8", "http://127.0.0.1/a.apk"),
                &current,
                &next,
                true
            )
            .is_ok()
        );
        let empty = Manifest {
            version: "0.1.8".into(),
            platforms: BTreeMap::new(),
        };
        assert_eq!(
            pick(&empty, &current, &next, false),
            Some("android_update.package_unavailable")
        );
    }

    #[test]
    fn redirects_are_limited_to_github_release_hosts() {
        let allowed = |value: &str| allowed_redirect_host(&reqwest::Url::parse(value).unwrap());
        assert!(allowed("https://objects.githubusercontent.com/x"));
        assert!(allowed("https://release-assets.githubusercontent.com/x"));
        assert!(!allowed("http://objects.githubusercontent.com/x"));
        assert!(!allowed("https://githubusercontent.com.evil.example/x"));
        assert!(!allowed("https://example.com/x"));
    }

    #[test]
    fn signed_version_must_match_the_announced_version() {
        let comment = "timestamp:1\tfile:a.apk\tversion:1.2.3";
        assert!(verify_signed_version(comment, "1.2.3").is_ok());
        assert!(verify_signed_version(comment, "v1.2.3").is_ok());
        assert_eq!(
            verify_signed_version(comment, "1.2.4"),
            Err(Code::VersionMismatch)
        );
        assert_eq!(
            verify_signed_version("timestamp:1\tfile:a.apk", "1.2.3"),
            Err(Code::VersionMismatch)
        );
    }

    #[test]
    fn artifact_url_is_derived_from_the_version() {
        let version = Version::parse("1.2.3").unwrap();
        assert!(
            expected_artifact_url(&version).ends_with("/v1.2.3/NoriShell_1.2.3_android_arm64.apk")
                || expected_artifact_url(&version)
                    .ends_with("/v1.2.3/NoriShell_1.2.3_android_x64.apk")
        );
    }
}
