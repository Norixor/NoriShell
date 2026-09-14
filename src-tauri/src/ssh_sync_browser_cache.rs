//! Bounded, memory-only projection of a Core-verified remote SSH-sync bundle.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use norishell_core_api::{
    PluginSshSyncBrowserCredential, PluginSshSyncBrowserCredentialMaterialKind,
    PluginSshSyncBrowserDesktopProfile, PluginSshSyncBrowserHost, PluginSshSyncBrowserSnapshot,
    PluginSshSyncBrowserState, WireSequence,
};
use norishell_ssh_profile_sync::{
    PortableBundleV1, PortableCredentialMaterial, PortableDesktopProtocol, PortableObjectId,
    RouteIngress,
};
use uuid::Uuid;

const MAX_HOST_ROWS: usize = 1_000;
const MAX_CREDENTIAL_ROWS: usize = 1_000;
const MAX_DESKTOP_PROFILE_ROWS: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshSyncBrowserCacheBinding {
    pub(crate) plugin_id: String,
    pub(crate) signer_fingerprint_sha256: String,
    pub(crate) profile_id: String,
    pub(crate) package_sha256: String,
    pub(crate) instance_generation: WireSequence,
    pub(crate) authorization_epoch: WireSequence,
    pub(crate) account_epoch: u64,
    pub(crate) account_configuration_sha256: String,
}

#[derive(Clone)]
struct BrowserCacheEntry {
    binding: SshSyncBrowserCacheBinding,
    snapshot: PluginSshSyncBrowserSnapshot,
}

#[derive(Default)]
struct BrowserCacheState {
    revision: u64,
    entries: BTreeMap<String, BrowserCacheEntry>,
}

#[derive(Clone, Default)]
pub(crate) struct SshSyncBrowserCache {
    state: Arc<Mutex<BrowserCacheState>>,
}

impl SshSyncBrowserCache {
    pub(crate) fn publish(
        &self,
        binding: &SshSyncBrowserCacheBinding,
        bundle: &PortableBundleV1,
        remote_updated_at_unix_ms: Option<i64>,
        fence: &dyn Fn() -> bool,
    ) {
        if !fence() {
            return;
        }
        let (hosts, credentials, desktop_profiles) = project_rows(bundle);
        let host_count = bounded_count(bundle.objects.hosts.len());
        let credential_count = bounded_count(bundle.objects.credentials.len());
        let desktop_profile_count = bounded_count(bundle.objects.desktop_profiles.len());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revision = state.revision.saturating_add(1).max(1);
        let cache_revision = WireSequence::new(state.revision);
        state.entries.insert(
            namespace(binding),
            BrowserCacheEntry {
                binding: binding.clone(),
                snapshot: PluginSshSyncBrowserSnapshot {
                    state: if host_count == 0 && credential_count == 0 && desktop_profile_count == 0
                    {
                        PluginSshSyncBrowserState::Empty
                    } else {
                        PluginSshSyncBrowserState::Ready
                    },
                    profile_id: binding.profile_id.clone(),
                    cache_revision,
                    host_count,
                    credential_count,
                    desktop_profile_count,
                    host_rows_omitted: host_count.saturating_sub(bounded_count(hosts.len())),
                    credential_rows_omitted: credential_count
                        .saturating_sub(bounded_count(credentials.len())),
                    desktop_profile_rows_omitted: desktop_profile_count
                        .saturating_sub(bounded_count(desktop_profiles.len())),
                    remote_updated_at_unix_ms,
                    hosts,
                    credentials,
                    desktop_profiles,
                },
            },
        );
        drop(state);
        if !fence() {
            self.invalidate_profile(&binding.plugin_id, &binding.profile_id);
        }
    }

    pub(crate) fn mark_failed(
        &self,
        binding: &SshSyncBrowserCacheBinding,
        fence: &dyn Fn() -> bool,
    ) {
        if !fence() {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revision = state.revision.saturating_add(1).max(1);
        let cache_revision = WireSequence::new(state.revision);
        state.entries.insert(
            namespace(binding),
            BrowserCacheEntry {
                binding: binding.clone(),
                snapshot: empty_snapshot(
                    PluginSshSyncBrowserState::Failed,
                    binding.profile_id.clone(),
                    cache_revision,
                ),
            },
        );
    }

    pub(crate) fn read(
        &self,
        binding: &SshSyncBrowserCacheBinding,
        fence: &dyn Fn() -> bool,
    ) -> PluginSshSyncBrowserSnapshot {
        if !fence() {
            return empty_snapshot(
                PluginSshSyncBrowserState::NotLoaded,
                binding.profile_id.clone(),
                WireSequence::new(0),
            );
        }
        let snapshot = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(&namespace(binding))
            .filter(|entry| entry.binding == *binding)
            .map(|entry| entry.snapshot.clone())
            .unwrap_or_else(|| {
                empty_snapshot(
                    PluginSshSyncBrowserState::NotLoaded,
                    binding.profile_id.clone(),
                    WireSequence::new(0),
                )
            });
        if fence() {
            snapshot
        } else {
            empty_snapshot(
                PluginSshSyncBrowserState::NotLoaded,
                binding.profile_id.clone(),
                WireSequence::new(0),
            )
        }
    }

    pub(crate) fn invalidate_profile(&self, plugin_id: &str, profile_id: &str) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .retain(|_, entry| {
                entry.binding.plugin_id != plugin_id || entry.binding.profile_id != profile_id
            });
    }

    pub(crate) fn invalidate_plugin(&self, plugin_id: &str) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .retain(|_, entry| entry.binding.plugin_id != plugin_id);
    }

    pub(crate) fn invalidate_all(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .clear();
    }
}

pub(crate) fn empty_snapshot(
    state: PluginSshSyncBrowserState,
    profile_id: String,
    cache_revision: WireSequence,
) -> PluginSshSyncBrowserSnapshot {
    PluginSshSyncBrowserSnapshot {
        state,
        profile_id,
        cache_revision,
        host_count: 0,
        credential_count: 0,
        desktop_profile_count: 0,
        host_rows_omitted: 0,
        credential_rows_omitted: 0,
        desktop_profile_rows_omitted: 0,
        remote_updated_at_unix_ms: None,
        hosts: Vec::new(),
        credentials: Vec::new(),
        desktop_profiles: Vec::new(),
    }
}

fn project_rows(
    bundle: &PortableBundleV1,
) -> (
    Vec<PluginSshSyncBrowserHost>,
    Vec<PluginSshSyncBrowserCredential>,
    Vec<PluginSshSyncBrowserDesktopProfile>,
) {
    let host_row_ids = bundle
        .objects
        .hosts
        .iter()
        .take(MAX_HOST_ROWS)
        .map(|host| (host.id, Uuid::new_v4().to_string()))
        .collect::<BTreeMap<_, _>>();
    let hosts = bundle
        .objects
        .hosts
        .iter()
        .take(MAX_HOST_ROWS)
        .map(|host| PluginSshSyncBrowserHost {
            row_id: host_row_ids
                .get(&host.id)
                .expect("row identifier was created for every projected Host")
                .clone(),
            label: host.label.clone(),
            address: host.address.clone(),
            port: host.port,
            username: host.username.clone(),
            tags: host.tags.iter().cloned().collect(),
        })
        .collect::<Vec<_>>();

    let desktop_profile_row_ids = bundle
        .objects
        .desktop_profiles
        .iter()
        .take(MAX_DESKTOP_PROFILE_ROWS)
        .map(|profile| (profile.id, Uuid::new_v4().to_string()))
        .collect::<BTreeMap<_, _>>();
    let desktop_profiles = bundle
        .objects
        .desktop_profiles
        .iter()
        .take(MAX_DESKTOP_PROFILE_ROWS)
        .map(|profile| PluginSshSyncBrowserDesktopProfile {
            row_id: desktop_profile_row_ids
                .get(&profile.id)
                .expect("row identifier was created for every projected remote desktop")
                .clone(),
            label: profile.label.clone(),
            protocol: match profile.protocol {
                PortableDesktopProtocol::Rdp => norishell_core_api::DesktopProtocol::Rdp,
                PortableDesktopProtocol::Vnc => norishell_core_api::DesktopProtocol::Vnc,
            },
            address: profile.address.clone(),
            port: profile.port,
            username: profile.username.clone(),
            domain: profile.domain.clone(),
        })
        .collect::<Vec<_>>();

    let authentication_plans = bundle
        .objects
        .authentication_plans
        .iter()
        .map(|plan| (plan.id, &plan.credential_ids))
        .collect::<BTreeMap<_, _>>();
    let identities = bundle
        .objects
        .identities
        .iter()
        .map(|identity| (identity.id, &identity.credential_ids))
        .collect::<BTreeMap<_, _>>();
    let routes = bundle
        .objects
        .routes
        .iter()
        .map(|route| (route.id, &route.ingress))
        .collect::<BTreeMap<_, _>>();
    let mut credential_hosts = BTreeMap::<PortableObjectId, BTreeSet<PortableObjectId>>::new();
    for host in &bundle.objects.hosts {
        if let Some(credentials) = authentication_plans.get(&host.authentication_plan_id) {
            for credential_id in *credentials {
                credential_hosts
                    .entry(*credential_id)
                    .or_default()
                    .insert(host.id);
            }
        }
        if let Some(credentials) = host.identity_id.and_then(|id| identities.get(&id).copied()) {
            for credential_id in credentials {
                credential_hosts
                    .entry(*credential_id)
                    .or_default()
                    .insert(host.id);
            }
        }
        let route_credential = routes
            .get(&host.route_id)
            .and_then(|ingress| match ingress {
                RouteIngress::HttpConnect { credential_id, .. }
                | RouteIngress::Socks5 { credential_id, .. } => *credential_id,
                RouteIngress::Direct => None,
            });
        if let Some(credential_id) = route_credential {
            credential_hosts
                .entry(credential_id)
                .or_default()
                .insert(host.id);
        }
    }

    let mut credential_desktop_profiles =
        BTreeMap::<PortableObjectId, BTreeSet<PortableObjectId>>::new();
    for profile in &bundle.objects.desktop_profiles {
        if let Some(credential_id) = profile.credential_id {
            credential_desktop_profiles
                .entry(credential_id)
                .or_default()
                .insert(profile.id);
        }
    }

    let credentials = bundle
        .objects
        .credentials
        .iter()
        .take(MAX_CREDENTIAL_ROWS)
        .map(|credential| PluginSshSyncBrowserCredential {
            row_id: Uuid::new_v4().to_string(),
            label: credential.label.clone(),
            material_kind: match credential.material {
                PortableCredentialMaterial::Password { .. } => {
                    PluginSshSyncBrowserCredentialMaterialKind::Password
                }
                PortableCredentialMaterial::PrivateKey { .. } => {
                    PluginSshSyncBrowserCredentialMaterialKind::PrivateKey
                }
                PortableCredentialMaterial::Certificate { .. } => {
                    PluginSshSyncBrowserCredentialMaterialKind::Certificate
                }
                PortableCredentialMaterial::KeyboardInteractive { .. } => {
                    PluginSshSyncBrowserCredentialMaterialKind::KeyboardInteractive
                }
            },
            host_row_ids: credential_hosts
                .get(&credential.id)
                .into_iter()
                .flatten()
                .filter_map(|host_id| host_row_ids.get(host_id).cloned())
                .collect(),
            desktop_profile_row_ids: credential_desktop_profiles
                .get(&credential.id)
                .into_iter()
                .flatten()
                .filter_map(|profile_id| desktop_profile_row_ids.get(profile_id).cloned())
                .collect(),
        })
        .collect();
    (hosts, credentials, desktop_profiles)
}

fn namespace(binding: &SshSyncBrowserCacheBinding) -> String {
    format!(
        "{}\0{}\0{}",
        binding.plugin_id, binding.signer_fingerprint_sha256, binding.profile_id
    )
}

fn bounded_count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::PluginSshSyncBrowserState;
    use norishell_ssh_profile_sync::{
        BundleSchema, PortableAuthenticationPlan, PortableCredential, PortableDesktopProfile,
        PortableDesktopProtocol, PortableHost, PortableIdentity, PortableObjects, PortableRoute,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn binding() -> SshSyncBrowserCacheBinding {
        SshSyncBrowserCacheBinding {
            plugin_id: "org.example.sync".to_owned(),
            signer_fingerprint_sha256: "a".repeat(64),
            profile_id: "primary".to_owned(),
            package_sha256: "b".repeat(64),
            instance_generation: WireSequence::new(4),
            authorization_epoch: WireSequence::new(7),
            account_epoch: 9,
            account_configuration_sha256: "c".repeat(64),
        }
    }

    fn bundle(host_total: usize, credential_total: usize) -> PortableBundleV1 {
        let route_id = PortableObjectId::new();
        let authentication_plan_id = PortableObjectId::new();
        let algorithm_policy_id = PortableObjectId::new();
        let heartbeat_policy_id = PortableObjectId::new();
        let monitoring_policy_id = PortableObjectId::new();
        let identity_id = PortableObjectId::new();
        let credentials = (0..credential_total)
            .map(|index| PortableCredential {
                id: PortableObjectId::new(),
                identity_id,
                label: format!("Credential {index}"),
                material: PortableCredentialMaterial::Password {
                    password_secret_id: PortableObjectId::new(),
                },
            })
            .collect::<Vec<_>>();
        let credential_ids = credentials
            .first()
            .map(|credential| vec![credential.id])
            .unwrap_or_default();
        let hosts = (0..host_total)
            .map(|index| PortableHost {
                id: PortableObjectId::new(),
                label: format!("Host {index}"),
                address: format!("server-{index}.example.test"),
                port: 22,
                username: Some("deploy".to_owned()),
                favorite: false,
                tags: BTreeSet::from(["production".to_owned()]),
                identity_id: Some(identity_id),
                route_id,
                authentication_plan_id,
                algorithm_policy_id,
                heartbeat_policy_id,
                monitoring_policy_id,
                login_automation_id: None,
            })
            .collect();
        PortableBundleV1 {
            schema: BundleSchema::V2,
            revision: 3,
            objects: PortableObjects {
                hosts,
                identities: vec![PortableIdentity {
                    id: identity_id,
                    label: "Production".to_owned(),
                    username: Some("deploy".to_owned()),
                    credential_ids: credential_ids.clone(),
                }],
                credentials,
                routes: vec![PortableRoute {
                    id: route_id,
                    ingress: RouteIngress::Direct,
                    jump_hops: Vec::new(),
                }],
                authentication_plans: vec![PortableAuthenticationPlan {
                    id: authentication_plan_id,
                    credential_ids,
                }],
                ..PortableObjects::default()
            },
            secrets: Vec::new(),
            skipped_machine_bound: Vec::new(),
            tombstones: Vec::new(),
        }
    }

    fn bundle_with_desktops(
        host_total: usize,
        credential_total: usize,
        desktop_total: usize,
    ) -> PortableBundleV1 {
        let mut bundle = bundle(host_total, credential_total);
        bundle.schema = BundleSchema::V3;
        let host_id = bundle.objects.hosts.first().map(|host| host.id);
        let credential_id = bundle
            .objects
            .credentials
            .first()
            .map(|credential| credential.id);
        bundle.objects.desktop_profiles = (0..desktop_total)
            .map(|index| PortableDesktopProfile {
                id: PortableObjectId::new(),
                label: format!("Desktop {index}"),
                protocol: PortableDesktopProtocol::Rdp,
                address: format!("desktop-{index}.example.test"),
                port: 3389,
                username: "administrator".to_owned(),
                domain: "EXAMPLE".to_owned(),
                host_id,
                gateway_host_id: None,
                credential_id,
                width: 1280,
                height: 800,
                clipboard_enabled: true,
                audio_playback_enabled: true,
            })
            .collect();
        bundle
    }

    #[test]
    fn projection_is_bounded_and_reports_real_totals() {
        let cache = SshSyncBrowserCache::default();
        let binding = binding();
        cache.publish(
            &binding,
            &bundle_with_desktops(1_001, 1_001, 1_001),
            Some(42),
            &|| true,
        );
        let snapshot = cache.read(&binding, &|| true);
        assert_eq!(snapshot.state, PluginSshSyncBrowserState::Ready);
        assert_eq!(snapshot.host_count, 1_001);
        assert_eq!(snapshot.credential_count, 1_001);
        assert_eq!(snapshot.desktop_profile_count, 1_001);
        assert_eq!(snapshot.hosts.len(), MAX_HOST_ROWS);
        assert_eq!(snapshot.credentials.len(), MAX_CREDENTIAL_ROWS);
        assert_eq!(snapshot.desktop_profiles.len(), MAX_DESKTOP_PROFILE_ROWS);
        assert_eq!(snapshot.host_rows_omitted, 1);
        assert_eq!(snapshot.credential_rows_omitted, 1);
        assert_eq!(snapshot.desktop_profile_rows_omitted, 1);
        assert_eq!(snapshot.remote_updated_at_unix_ms, Some(42));
    }

    #[test]
    fn projection_exposes_only_redacted_metadata_and_opaque_row_ids() {
        let source = bundle_with_desktops(1, 1, 1);
        let portable_host_id = source.objects.hosts[0].id.as_uuid().to_string();
        let portable_credential_id = source.objects.credentials[0].id.as_uuid().to_string();
        let portable_desktop_profile_id =
            source.objects.desktop_profiles[0].id.as_uuid().to_string();
        let PortableCredentialMaterial::Password { password_secret_id } =
            source.objects.credentials[0].material
        else {
            unreachable!("fixture uses a password credential")
        };
        let cache = SshSyncBrowserCache::default();
        let binding = binding();
        cache.publish(&binding, &source, Some(84), &|| true);
        let snapshot = cache.read(&binding, &|| true);
        let serialized = serde_json::to_string(&snapshot).expect("browser projection");
        assert!(!serialized.contains(&portable_host_id));
        assert!(!serialized.contains(&portable_credential_id));
        assert!(!serialized.contains(&portable_desktop_profile_id));
        assert!(!serialized.contains(&password_secret_id.as_uuid().to_string()));
        assert!(!serialized.contains("credentialId"));
        assert!(!serialized.contains("gatewayHostId"));
        assert_eq!(snapshot.credentials[0].host_row_ids.len(), 1);
        assert_eq!(
            snapshot.credentials[0].host_row_ids[0],
            snapshot.hosts[0].row_id
        );
        assert_eq!(snapshot.credentials[0].desktop_profile_row_ids.len(), 1);
        assert_eq!(
            snapshot.credentials[0].desktop_profile_row_ids[0],
            snapshot.desktop_profiles[0].row_id
        );
    }

    #[test]
    fn desktop_only_bundle_is_ready_and_does_not_report_empty() {
        let cache = SshSyncBrowserCache::default();
        let binding = binding();
        cache.publish(&binding, &bundle_with_desktops(0, 0, 1), None, &|| true);
        let snapshot = cache.read(&binding, &|| true);
        assert_eq!(snapshot.state, PluginSshSyncBrowserState::Ready);
        assert_eq!(snapshot.desktop_profile_count, 1);
        assert_eq!(snapshot.desktop_profiles.len(), 1);
    }

    #[test]
    fn binding_fence_and_owner_invalidation_prevent_stale_cache_reuse() {
        let cache = SshSyncBrowserCache::default();
        let binding = binding();
        cache.publish(&binding, &bundle(1, 1), None, &|| true);
        assert_eq!(
            cache.read(&binding, &|| true).state,
            PluginSshSyncBrowserState::Ready
        );

        let mut changed_account = binding.clone();
        changed_account.account_epoch += 1;
        assert_eq!(
            cache.read(&changed_account, &|| true).state,
            PluginSshSyncBrowserState::NotLoaded
        );
        assert_eq!(
            cache.read(&binding, &|| false).state,
            PluginSshSyncBrowserState::NotLoaded
        );

        cache.invalidate_profile(&binding.plugin_id, &binding.profile_id);
        assert_eq!(
            cache.read(&binding, &|| true).state,
            PluginSshSyncBrowserState::NotLoaded
        );
    }

    #[test]
    fn fence_that_expires_during_publish_or_read_never_releases_rows() {
        let cache = SshSyncBrowserCache::default();
        let binding = binding();
        let publish_checks = AtomicUsize::new(0);
        cache.publish(&binding, &bundle(1, 1), None, &|| {
            publish_checks.fetch_add(1, Ordering::SeqCst) == 0
        });
        assert_eq!(
            cache.read(&binding, &|| true).state,
            PluginSshSyncBrowserState::NotLoaded
        );

        cache.publish(&binding, &bundle(1, 1), None, &|| true);
        let read_checks = AtomicUsize::new(0);
        assert_eq!(
            cache
                .read(&binding, &|| {
                    read_checks.fetch_add(1, Ordering::SeqCst) == 0
                })
                .state,
            PluginSshSyncBrowserState::NotLoaded
        );
    }
}
