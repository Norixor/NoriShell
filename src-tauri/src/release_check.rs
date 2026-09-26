use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{Manager, WebviewWindow};

const GITHUB_RELEASES_API: &str =
    "https://api.github.com/repos/Norixor/NoriShell/releases?per_page=100";
pub const GITHUB_RELEASES_PAGE: &str = "https://github.com/Norixor/NoriShell/releases";
const MAIN_WINDOW_LABEL: &str = "main";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct GithubRelease {
    draft: bool,
    prerelease: bool,
    tag_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum ReleaseCheckStatus {
    UpToDate,
    UpdateAvailable,
    NoRelease,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseCheckResponse {
    current_version: String,
    status: ReleaseCheckStatus,
    latest_version: Option<String>,
    release_url: &'static str,
    supports_auto_install: bool,
}

fn supports_auto_install() -> bool {
    #[cfg(windows)]
    {
        // The direct-run ZIP contains only norishell.exe. NSIS creates its
        // uninstaller beside the installed executable.
        return std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("uninstall.exe")))
            .is_some_and(|path| path.is_file());
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum ReleaseCheckFailureCode {
    WindowNotAllowed,
    RequestFailed,
    HttpRejected,
    ResponseTooLarge,
    InvalidResponse,
    InvalidCurrentVersion,
    Internal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseCheckFailure {
    code: ReleaseCheckFailureCode,
    message_key: &'static str,
    diagnostic_id: Option<String>,
}

impl ReleaseCheckFailure {
    const fn new(code: ReleaseCheckFailureCode) -> Self {
        let message_key = match code {
            ReleaseCheckFailureCode::WindowNotAllowed => "releases.checkErrors.windowNotAllowed",
            ReleaseCheckFailureCode::RequestFailed => "releases.checkErrors.requestFailed",
            ReleaseCheckFailureCode::HttpRejected => "releases.checkErrors.httpRejected",
            ReleaseCheckFailureCode::ResponseTooLarge => "releases.checkErrors.responseTooLarge",
            ReleaseCheckFailureCode::InvalidResponse => "releases.checkErrors.invalidResponse",
            ReleaseCheckFailureCode::InvalidCurrentVersion => {
                "releases.checkErrors.invalidCurrentVersion"
            }
            ReleaseCheckFailureCode::Internal => "releases.checkErrors.internal",
        };
        Self {
            code,
            message_key,
            diagnostic_id: None,
        }
    }

    fn internal() -> Self {
        let diagnostic_id = uuid::Uuid::new_v4().to_string();
        eprintln!("release_check internal failure: diagnostic_id={diagnostic_id}");
        let mut failure = Self::new(ReleaseCheckFailureCode::Internal);
        failure.diagnostic_id = Some(diagnostic_id);
        failure
    }
}

fn release_version(tag_name: &str) -> Option<Version> {
    Version::parse(tag_name.strip_prefix('v').unwrap_or(tag_name)).ok()
}

fn is_eligible_release(current: &Version, release: &GithubRelease) -> Option<Version> {
    if release.draft {
        return None;
    }

    let version = release_version(&release.tag_name)?;
    // A stable installation never offers prerelease builds, even if a release
    // was mistakenly published without GitHub's prerelease flag.
    if current.pre.is_empty() && (release.prerelease || !version.pre.is_empty()) {
        return None;
    }
    Some(version)
}

fn evaluate_releases(current: Version, releases: Vec<GithubRelease>) -> ReleaseCheckResponse {
    let latest = releases
        .iter()
        .filter_map(|release| is_eligible_release(&current, release))
        .max_by(|left, right| left.cmp_precedence(right));

    let (status, latest_version, stable_release) = match latest {
        None => (ReleaseCheckStatus::NoRelease, None, false),
        Some(version) if version.cmp_precedence(&current).is_gt() => (
            ReleaseCheckStatus::UpdateAvailable,
            Some(version.to_string()),
            version.pre.is_empty(),
        ),
        Some(_) => (ReleaseCheckStatus::UpToDate, None, false),
    };

    ReleaseCheckResponse {
        current_version: current.to_string(),
        status,
        latest_version,
        release_url: GITHUB_RELEASES_PAGE,
        // GitHub's Latest endpoint never points at a prerelease.
        supports_auto_install: stable_release && supports_auto_install(),
    }
}

async fn read_bounded_response(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, ReleaseCheckFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ReleaseCheckFailure::new(
            ReleaseCheckFailureCode::ResponseTooLarge,
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ReleaseCheckFailure::new(ReleaseCheckFailureCode::RequestFailed))?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(ReleaseCheckFailure::new(
                ReleaseCheckFailureCode::ResponseTooLarge,
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[tauri::command]
pub(crate) async fn release_check<R: tauri::Runtime>(
    window: WebviewWindow<R>,
) -> Result<ReleaseCheckResponse, ReleaseCheckFailure> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err(ReleaseCheckFailure::new(
            ReleaseCheckFailureCode::WindowNotAllowed,
        ));
    }

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("NoriShell/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| ReleaseCheckFailure::internal())?;
    let response = client
        .get(GITHUB_RELEASES_API)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| ReleaseCheckFailure::new(ReleaseCheckFailureCode::RequestFailed))?;
    if !response.status().is_success() {
        return Err(ReleaseCheckFailure::new(
            ReleaseCheckFailureCode::HttpRejected,
        ));
    }

    let body = read_bounded_response(response).await?;
    let releases = serde_json::from_slice::<Vec<GithubRelease>>(&body)
        .map_err(|_| ReleaseCheckFailure::new(ReleaseCheckFailureCode::InvalidResponse))?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| ReleaseCheckFailure::new(ReleaseCheckFailureCode::InvalidCurrentVersion))?;
    Ok(evaluate_releases(current, releases))
}

#[tauri::command]
pub(crate) fn release_update_readiness<R: tauri::Runtime>(
    window: WebviewWindow<R>,
) -> Result<norishell_core_api::ExitReadiness, ReleaseCheckFailure> {
    if window.label() != MAIN_WINDOW_LABEL {
        return Err(ReleaseCheckFailure::new(
            ReleaseCheckFailureCode::WindowNotAllowed,
        ));
    }
    Ok(crate::lifecycle::current_update_readiness(
        window.app_handle(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag_name: &str, draft: bool, prerelease: bool) -> GithubRelease {
        GithubRelease {
            draft,
            prerelease,
            tag_name: tag_name.into(),
        }
    }

    #[test]
    fn stable_versions_do_not_offer_prereleases_or_drafts() {
        let result = evaluate_releases(
            Version::parse("0.1.0").unwrap(),
            vec![
                release("v0.2.0-beta.1", false, true),
                release("v0.3.0", true, false),
                release("v0.1.1", false, false),
            ],
        );

        assert_eq!(result.status, ReleaseCheckStatus::UpdateAvailable);
        assert_eq!(result.latest_version.as_deref(), Some("0.1.1"));
        assert_eq!(result.release_url, GITHUB_RELEASES_PAGE);
        #[cfg(not(windows))]
        assert!(result.supports_auto_install);
    }

    #[test]
    fn beta_versions_can_advance_to_a_newer_beta_or_the_final_release() {
        let result = evaluate_releases(
            Version::parse("0.1.0-beta.1").unwrap(),
            vec![
                release("v0.1.0-beta.2", false, true),
                release("v0.1.0", false, false),
            ],
        );

        assert_eq!(result.status, ReleaseCheckStatus::UpdateAvailable);
        assert_eq!(result.latest_version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn prerelease_numeric_identifiers_and_release_precedence_follow_semver() {
        let result = evaluate_releases(
            Version::parse("0.1.0-beta.2").unwrap(),
            vec![
                release("v0.1.0-beta.10", false, true),
                release("v0.1.0-beta.1", false, true),
            ],
        );

        assert_eq!(result.status, ReleaseCheckStatus::UpdateAvailable);
        assert_eq!(result.latest_version.as_deref(), Some("0.1.0-beta.10"));
        assert!(!result.supports_auto_install);
    }

    #[test]
    fn equal_precedence_build_metadata_and_older_releases_are_not_updates() {
        let result = evaluate_releases(
            Version::parse("0.1.0+local.1").unwrap(),
            vec![
                release("v0.1.0+build.2", false, false),
                release("v0.0.9", false, false),
            ],
        );

        assert_eq!(result.status, ReleaseCheckStatus::UpToDate);
        assert_eq!(result.latest_version, None);
    }

    #[test]
    fn an_empty_or_unparseable_published_list_reports_no_release() {
        let empty = evaluate_releases(Version::parse("0.1.0").unwrap(), vec![]);
        let invalid = evaluate_releases(
            Version::parse("0.1.0").unwrap(),
            vec![release("release-candidate", false, false)],
        );

        assert_eq!(empty.status, ReleaseCheckStatus::NoRelease);
        assert_eq!(invalid.status, ReleaseCheckStatus::NoRelease);
    }
}
