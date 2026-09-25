//! The only host-owned background SSH sync writer. Plugin code cannot start or
//! retain this scheduler, and every operation is fenced by fresh Core state.

use std::{
    net::IpAddr,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use norishell_core_api::{
    PluginCapability, PluginId, PluginPackageKind, PluginSettingValue, PluginSettingsValues,
    PluginSshSyncConflictPolicy, PluginSshSyncCredentialProfile, PluginSshSyncDownloadSource,
    PluginSshSyncHttpMethod, PluginSshSyncRequest, PluginSshSyncUploadTarget, RequestId,
    WireSequence,
};
use reqwest::Url;

use super::PluginService;
use crate::ssh_sync_exchange::{ActionFence, SshSyncActionRevision, SshSyncInvocationBinding};

const PLUGIN_ID: &str = "com.norishell.self-host-sync";
const PROFILE_ID: &str = "primary";
const POLL_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, PartialEq, Eq)]
struct AutoSyncConfig {
    origin: String,
    enabled: bool,
    interval: Duration,
    check_on_startup: bool,
    conflict_policy: PluginSshSyncConflictPolicy,
    deletion_policy: PluginSshSyncConflictPolicy,
}

impl AutoSyncConfig {
    fn from_values(values: &PluginSettingsValues) -> Option<Self> {
        let server = match values.get("serverUrl")? {
            PluginSettingValue::String(value) => value,
            _ => return None,
        };
        let enabled = matches!(
            values.get("autoSyncEnabled"),
            Some(PluginSettingValue::Boolean(true))
        );
        let check_on_startup = matches!(
            values.get("checkOnStartup"),
            Some(PluginSettingValue::Boolean(true))
        );
        if !enabled && !check_on_startup {
            return None;
        }
        let minutes = match values.get("autoSyncIntervalMinutes") {
            Some(PluginSettingValue::String(value)) => match value.as_str() {
                "5" => 5,
                "15" => 15,
                "30" => 30,
                "60" => 60,
                _ => return None,
            },
            _ => return None,
        };
        Some(Self {
            origin: canonical_origin(server)?,
            enabled,
            interval: Duration::from_secs(minutes * 60),
            check_on_startup,
            conflict_policy: setting_policy(values, "conflictPolicy")?,
            deletion_policy: setting_policy(values, "deletionPolicy")?,
        })
    }
}

fn setting_policy(values: &PluginSettingsValues, key: &str) -> Option<PluginSshSyncConflictPolicy> {
    match values.get(key) {
        Some(PluginSettingValue::String(value)) if value == "prompt" => {
            Some(PluginSshSyncConflictPolicy::Prompt)
        }
        Some(PluginSettingValue::String(value)) if value == "newest" => {
            Some(PluginSshSyncConflictPolicy::Newest)
        }
        None => Some(PluginSshSyncConflictPolicy::Newest),
        _ => None,
    }
}

fn canonical_origin(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let value = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        let http_candidate = Url::parse(&format!("http://{trimmed}")).ok()?;
        let local_or_ip = http_candidate.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok() || host == "localhost"
        });
        format!("{}://{trimmed}", if local_or_ip { "http" } else { "https" })
    };
    let url = Url::parse(&value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

#[derive(Clone, PartialEq, Eq)]
struct AutoSyncLease {
    config: AutoSyncConfig,
    signer: String,
    package_sha256: String,
    installed_state_version: WireSequence,
    settings_revision: WireSequence,
    instance_generation: WireSequence,
    contribution_revision: WireSequence,
    grant_epoch: WireSequence,
}

impl AutoSyncLease {
    fn auth(&self) -> PluginSshSyncCredentialProfile {
        let endpoint = |path: &str| format!("{}{path}", self.config.origin);
        PluginSshSyncCredentialProfile {
            login_url: endpoint("/auth/login"),
            registration_url: endpoint("/auth/register"),
            email_verification_url: endpoint("/auth/email/verify"),
            mfa_url: endpoint("/auth/mfa"),
            token_url: endpoint("/auth/token"),
            revoke_url: endpoint("/auth/revoke"),
            client_id: "norishell-self-host".to_owned(),
            scopes: vec!["ssh.sync".to_owned()],
            resource_origins: vec![self.config.origin.clone()],
        }
    }

    fn request(&self, automatic: bool) -> PluginSshSyncRequest {
        let url = format!("{}/exchange", self.config.origin);
        let source = PluginSshSyncDownloadSource {
            url: url.clone(),
            use_oauth: true,
        };
        if automatic {
            PluginSshSyncRequest::Sync {
                profile_id: PROFILE_ID.to_owned(),
                auth: self.auth(),
                source,
                destination: PluginSshSyncUploadTarget {
                    url,
                    method: PluginSshSyncHttpMethod::Put,
                    use_oauth: true,
                    if_match: None,
                },
                conflict_policy: self.config.conflict_policy,
                deletion_policy: self.config.deletion_policy,
            }
        } else {
            PluginSshSyncRequest::Refresh {
                profile_id: PROFILE_ID.to_owned(),
                auth: self.auth(),
                source,
            }
        }
    }
}

impl PluginService {
    pub(crate) fn start_auto_sync(&self) {
        if self.ssh_sync.is_none() || self.safe_mode_active {
            return;
        }
        let mut task = self
            .auto_sync_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if task.is_some() {
            return;
        }
        self.auto_sync_stopped.store(false, Ordering::Release);
        let service = self.clone();
        *task = Some(tauri::async_runtime::spawn(async move {
            service.auto_sync_loop().await;
        }));
    }

    pub(super) async fn stop_auto_sync(&self) {
        self.auto_sync_stopped.store(true, Ordering::Release);
        let task = self
            .auto_sync_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }

    async fn auto_sync_loop(&self) {
        let mut current: Option<AutoSyncLease> = None;
        let mut next_sync: Option<Instant> = None;
        let mut startup_checked = false;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if self.auto_sync_stopped.load(Ordering::Acquire) {
                break;
            }
            let Some(lease) = self.auto_sync_lease() else {
                current = None;
                next_sync = None;
                continue;
            };
            if current.as_ref() != Some(&lease) {
                next_sync = lease
                    .config
                    .enabled
                    .then(|| Instant::now() + lease.config.interval);
                current = Some(lease.clone());
            }
            if !startup_checked {
                startup_checked = true;
                if lease.config.check_on_startup {
                    self.run_auto_sync_action(&lease, false).await;
                }
            }
            let Some(broker) = &self.ssh_sync else {
                continue;
            };
            if !broker
                .has_established_baseline(PLUGIN_ID, &lease.signer, PROFILE_ID)
                .await
                || !self.auto_sync_lease_current(&lease)
            {
                continue;
            }
            if next_sync.is_some_and(|deadline| Instant::now() >= deadline) {
                next_sync = Some(Instant::now() + lease.config.interval);
                self.run_auto_sync_action(&lease, true).await;
            }
        }
    }

    fn auto_sync_lease(&self) -> Option<AutoSyncLease> {
        if self.safe_mode_active || self.auto_sync_stopped.load(Ordering::Acquire) {
            return None;
        }
        let broker = self.ssh_sync.as_ref()?;
        if !broker.vault_unlocked() {
            return None;
        }
        let plugin_id = PluginId::parse(PLUGIN_ID).ok()?;
        let request_id = RequestId::new();
        let installed = self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_installation(&plugin_id))
            .ok()?;
        let installed = self
            .has_capability(
                request_id.clone(),
                &plugin_id,
                &installed.signer_fingerprint_sha256,
                PluginCapability::SshSync,
            )
            .ok()?;
        if !matches!(
            self.installer.active_package_kind(
                &plugin_id,
                &installed.active_version,
                &installed.package_sha256,
            ),
            Ok(PluginPackageKind::Wasm)
        ) {
            return None;
        }
        let grant_epoch = self
            .capability_grant_epoch_for_record(
                request_id.clone(),
                &installed,
                PluginCapability::SshSync,
            )
            .ok()?;
        let settings = self
            .settings_snapshot_for_installation(request_id, &installed)
            .ok()??;
        let config = AutoSyncConfig::from_values(&settings.values)?;
        let instance = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get(PLUGIN_ID)
            .cloned()?;
        if instance.operation_authority_revoked
            || !instance.api_authority.load(Ordering::Acquire)
            || instance.signer_fingerprint_sha256 != installed.signer_fingerprint_sha256
            || instance.package_sha256 != installed.package_sha256
            || instance.state_version != installed.state_version
            || instance.settings_revision != Some(settings.revision)
        {
            return None;
        }
        Some(AutoSyncLease {
            config,
            signer: installed.signer_fingerprint_sha256,
            package_sha256: installed.package_sha256,
            installed_state_version: installed.state_version,
            settings_revision: settings.revision,
            instance_generation: instance.instance_generation,
            contribution_revision: instance.contribution_revision,
            grant_epoch,
        })
    }

    fn auto_sync_lease_current(&self, lease: &AutoSyncLease) -> bool {
        self.auto_sync_lease().as_ref() == Some(lease)
    }

    async fn run_auto_sync_action(&self, lease: &AutoSyncLease, automatic: bool) {
        if !self.auto_sync_lease_current(lease) {
            return;
        }
        let Some(broker) = &self.ssh_sync else {
            return;
        };
        let service = self.clone();
        let expected = lease.clone();
        let fence: ActionFence = Arc::new(move || service.auto_sync_lease_current(&expected));
        let _ = broker
            .clone()
            .for_automatic_sync()
            .invoke(
                PLUGIN_ID,
                &lease.signer,
                lease.request(automatic),
                &[],
                SshSyncActionRevision {
                    authorization: lease.installed_state_version,
                    configuration: lease.contribution_revision,
                },
                SshSyncInvocationBinding {
                    package_sha256: lease.package_sha256.clone(),
                    instance_generation: lease.instance_generation,
                    authorization_epoch: lease.grant_epoch,
                },
                true,
                fence,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PluginSettingValue, PluginSettingsValues, PluginSshSyncHttpMethod, PluginSshSyncRequest,
        WireSequence,
    };

    use super::{AutoSyncConfig, AutoSyncLease, canonical_origin};

    fn values(server: &str, enabled: bool, interval: &str) -> PluginSettingsValues {
        PluginSettingsValues::from([
            (
                "serverUrl".to_owned(),
                PluginSettingValue::String(server.to_owned()),
            ),
            (
                "autoSyncEnabled".to_owned(),
                PluginSettingValue::Boolean(enabled),
            ),
            (
                "autoSyncIntervalMinutes".to_owned(),
                PluginSettingValue::String(interval.to_owned()),
            ),
        ])
    }

    #[test]
    fn auto_sync_requires_explicit_opt_in_and_supported_interval() {
        assert!(AutoSyncConfig::from_values(&values("sync.example", false, "15")).is_none());
        assert!(AutoSyncConfig::from_values(&values("sync.example", true, "1")).is_none());
        let config = AutoSyncConfig::from_values(&values("sync.example", true, "5"))
            .expect("explicit opt-in");
        assert_eq!(config.origin, "https://sync.example");
        assert_eq!(config.interval.as_secs(), 300);
    }

    #[test]
    fn background_endpoint_accepts_http_and_https_origins() {
        for invalid in [
            "ftp://sync.example",
            "https://user:pass@sync.example",
            "https://sync.example/path",
            "https://sync.example?token=secret",
            "https://sync.example/#fragment",
        ] {
            assert!(canonical_origin(invalid).is_none(), "{invalid}");
        }
        assert_eq!(
            canonical_origin("[::1]:8443"),
            Some("http://[::1]:8443".to_owned())
        );
        assert_eq!(
            canonical_origin("http://sync.example:8787"),
            Some("http://sync.example:8787".to_owned())
        );
        assert_eq!(
            canonical_origin("192.168.1.10:8787"),
            Some("http://192.168.1.10:8787".to_owned())
        );
    }

    #[test]
    fn startup_check_is_read_only_and_auto_upload_has_core_cas_inputs() {
        let mut settings = values("https://sync.example", false, "15");
        settings.insert(
            "checkOnStartup".to_owned(),
            PluginSettingValue::Boolean(true),
        );
        let config = AutoSyncConfig::from_values(&settings).expect("startup check opt-in");
        assert!(!config.enabled);
        let lease = AutoSyncLease {
            config,
            signer: "a".repeat(64),
            package_sha256: "b".repeat(64),
            installed_state_version: WireSequence::new(1),
            settings_revision: WireSequence::new(2),
            instance_generation: WireSequence::new(3),
            contribution_revision: WireSequence::new(4),
            grant_epoch: WireSequence::new(5),
        };
        assert!(matches!(
            lease.request(false),
            PluginSshSyncRequest::Refresh { .. }
        ));
        let PluginSshSyncRequest::Sync {
            auth,
            source,
            destination,
            ..
        } = lease.request(true)
        else {
            panic!("automatic request must be Core sync");
        };
        assert_eq!(auth.login_url, "https://sync.example/auth/login");
        assert_eq!(auth.client_id, "norishell-self-host");
        assert_eq!(auth.scopes, ["ssh.sync"]);
        assert_eq!(source.url, "https://sync.example/exchange");
        assert_eq!(source.url, destination.url);
        assert!(source.use_oauth && destination.use_oauth);
        assert_eq!(destination.method, PluginSshSyncHttpMethod::Put);
        assert!(destination.if_match.is_none());
    }
}
