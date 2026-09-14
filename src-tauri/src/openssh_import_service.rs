use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use norishell_app_persistence::{HostBatchCreateInput, HostBatchJumpHostInput};
use norishell_core_api::{
    CoreApiError, ErrorCategory, HostSummary, OpenSshConfigCommitRequest,
    OpenSshConfigCommitResponse, OpenSshConfigPreviewRequest, OpenSshConfigPreviewResponse,
    OpenSshImportCandidate, OpenSshImportDiagnostic, OpenSshImportDiagnosticSeverity,
    OpenSshImportEndpoint, OpenSshImportJumpHop, OpenSshImportRoutePreview, ProxyDnsMode,
    ProxyEndpoint, RequestId, RetryStrategy, RouteIngress,
};
use norishell_ssh_domain::openssh_config::{
    OpenSshDiagnostic, OpenSshDiagnosticSeverity, OpenSshHostCandidate, OpenSshRoutePreview,
    parse_openssh_config_preview,
};
use tauri::State;
use uuid::Uuid;

use crate::{core_api_error::core_error, host_service::HostService, time::unix_time_ms};

const SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_SNAPSHOTS: usize = 8;
type CoreResult<T> = Result<T, Box<CoreApiError>>;

#[derive(Clone)]
struct StoredCandidate {
    input: HostBatchCreateInput,
}

struct StoredSnapshot {
    expires_at_unix_ms: i64,
    candidates: BTreeMap<String, StoredCandidate>,
    committed: Option<(Vec<String>, Vec<HostSummary>)>,
}

#[derive(Clone, Default)]
pub struct OpenSshImportService {
    snapshots: Arc<Mutex<BTreeMap<String, StoredSnapshot>>>,
}

#[tauri::command]
pub fn openssh_config_preview(
    request: OpenSshConfigPreviewRequest,
    service: State<'_, OpenSshImportService>,
) -> CoreResult<OpenSshConfigPreviewResponse> {
    service.preview(request)
}

#[tauri::command]
pub fn openssh_config_commit(
    request: OpenSshConfigCommitRequest,
    service: State<'_, OpenSshImportService>,
    hosts: State<'_, HostService>,
) -> CoreResult<OpenSshConfigCommitResponse> {
    service.commit(request, &hosts)
}

impl OpenSshImportService {
    fn preview(
        &self,
        request: OpenSshConfigPreviewRequest,
    ) -> CoreResult<OpenSshConfigPreviewResponse> {
        let parsed = parse_openssh_config_preview(&request.config_text);
        let now = unix_time_ms();
        let expires_at_unix_ms = now.saturating_add(duration_millis(SNAPSHOT_TTL));
        let snapshot_id = Uuid::now_v7().to_string();
        let mut stored_candidates = BTreeMap::new();
        let candidates = parsed
            .candidates
            .into_iter()
            .map(|candidate| map_candidate(candidate, &mut stored_candidates))
            .collect();
        let response = OpenSshConfigPreviewResponse {
            snapshot_id: snapshot_id.clone(),
            expires_at_unix_ms,
            candidates,
            diagnostics: parsed.diagnostics.into_iter().map(map_diagnostic).collect(),
        };
        let mut snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshots.retain(|_, snapshot| snapshot.expires_at_unix_ms > now);
        while snapshots.len() >= MAX_SNAPSHOTS {
            let Some(oldest) = snapshots.keys().next().cloned() else {
                break;
            };
            snapshots.remove(&oldest);
        }
        snapshots.insert(
            snapshot_id,
            StoredSnapshot {
                expires_at_unix_ms,
                candidates: stored_candidates,
                committed: None,
            },
        );
        Ok(response)
    }

    fn commit(
        &self,
        request: OpenSshConfigCommitRequest,
        hosts: &HostService,
    ) -> CoreResult<OpenSshConfigCommitResponse> {
        let request_id = request.meta.request_id.clone();
        let now = unix_time_ms();
        if request.candidate_ids.is_empty() || request.candidate_ids.len() > 128 {
            return Err(validation_error(request_id));
        }
        let selected = request
            .candidate_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if selected.len() != request.candidate_ids.len() {
            return Err(validation_error(request_id));
        }
        let mut snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshots.retain(|_, snapshot| snapshot.expires_at_unix_ms > now);
        let snapshot = snapshots
            .get_mut(&request.snapshot_id)
            .ok_or_else(|| expired_error(request_id.clone()))?;
        if let Some((committed_ids, committed_hosts)) = &snapshot.committed {
            return if committed_ids == &request.candidate_ids {
                Ok(OpenSshConfigCommitResponse {
                    hosts: committed_hosts.clone(),
                })
            } else {
                Err(conflict_error(request_id))
            };
        }
        let inputs = request
            .candidate_ids
            .iter()
            .map(|candidate_id| {
                snapshot
                    .candidates
                    .get(candidate_id)
                    .map(|candidate| candidate.input.clone())
                    .ok_or_else(|| validation_error(request_id.clone()))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let imported = hosts.create_hosts_atomically(&inputs, request_id)?;
        snapshot.committed = Some((request.candidate_ids, imported.clone()));
        Ok(OpenSshConfigCommitResponse { hosts: imported })
    }
}

fn map_candidate(
    candidate: OpenSshHostCandidate,
    stored: &mut BTreeMap<String, StoredCandidate>,
) -> OpenSshImportCandidate {
    let candidate_id = Uuid::now_v7().to_string();
    let endpoint = candidate
        .endpoint
        .as_ref()
        .map(|endpoint| OpenSshImportEndpoint {
            address: endpoint.address.clone(),
            port: endpoint.port,
        });
    let (ingress, jump_hosts) = stored_route(&candidate.route);
    let route = map_route(candidate.route);
    let importable = candidate.importable && endpoint.is_some();
    let diagnostics = candidate
        .diagnostics
        .into_iter()
        .map(map_diagnostic)
        .collect::<Vec<_>>();
    if importable {
        let endpoint_value = endpoint.as_ref().expect("importable endpoint");
        stored.insert(
            candidate_id.clone(),
            StoredCandidate {
                input: HostBatchCreateInput {
                    label: candidate.alias.clone(),
                    address: endpoint_value.address.clone(),
                    port: endpoint_value.port,
                    username: candidate.user.clone(),
                    ingress,
                    jump_hosts,
                },
            },
        );
    }
    OpenSshImportCandidate {
        candidate_id,
        alias: candidate.alias,
        endpoint,
        username: candidate.user,
        identity_file_hints: candidate.identity_file_hints,
        route,
        diagnostics,
        importable,
    }
}

fn stored_route(route: &OpenSshRoutePreview) -> (RouteIngress, Vec<HostBatchJumpHostInput>) {
    match route {
        OpenSshRoutePreview::Direct => (RouteIngress::DirectTcp, Vec::new()),
        OpenSshRoutePreview::JumpChain(hops) => (
            RouteIngress::DirectTcp,
            hops.iter()
                .map(|hop| HostBatchJumpHostInput {
                    address: hop.endpoint.address.clone(),
                    port: hop.endpoint.port,
                    username: hop.user.clone(),
                })
                .collect(),
        ),
        OpenSshRoutePreview::HttpConnect { proxy } => (
            RouteIngress::HttpConnectProxy {
                endpoint: proxy_endpoint(proxy),
                proxy_auth_credential_ref_id: None,
            },
            Vec::new(),
        ),
        OpenSshRoutePreview::Socks5 { proxy } => (
            RouteIngress::Socks5Proxy {
                endpoint: proxy_endpoint(proxy),
                dns_mode: ProxyDnsMode::Proxy,
                proxy_auth_credential_ref_id: None,
            },
            Vec::new(),
        ),
    }
}

fn proxy_endpoint(
    endpoint: &norishell_ssh_domain::openssh_config::OpenSshEndpointPreview,
) -> ProxyEndpoint {
    ProxyEndpoint {
        address: endpoint.address.clone(),
        normalized_address: endpoint.address.clone(),
        port: endpoint.port,
    }
}

fn map_route(route: OpenSshRoutePreview) -> OpenSshImportRoutePreview {
    match route {
        OpenSshRoutePreview::Direct => OpenSshImportRoutePreview::Direct,
        OpenSshRoutePreview::JumpChain(hops) => OpenSshImportRoutePreview::JumpChain {
            hops: hops
                .into_iter()
                .map(|hop| OpenSshImportJumpHop {
                    endpoint: OpenSshImportEndpoint {
                        address: hop.endpoint.address,
                        port: hop.endpoint.port,
                    },
                    username: hop.user,
                })
                .collect(),
        },
        OpenSshRoutePreview::HttpConnect { proxy } => OpenSshImportRoutePreview::HttpConnect {
            proxy: OpenSshImportEndpoint {
                address: proxy.address,
                port: proxy.port,
            },
        },
        OpenSshRoutePreview::Socks5 { proxy } => OpenSshImportRoutePreview::Socks5 {
            proxy: OpenSshImportEndpoint {
                address: proxy.address,
                port: proxy.port,
            },
        },
    }
}

fn map_diagnostic(diagnostic: OpenSshDiagnostic) -> OpenSshImportDiagnostic {
    OpenSshImportDiagnostic {
        code: format!("{:?}", diagnostic.code),
        severity: match diagnostic.severity {
            OpenSshDiagnosticSeverity::Warning => OpenSshImportDiagnosticSeverity::Warning,
            OpenSshDiagnosticSeverity::Error => OpenSshImportDiagnosticSeverity::Error,
        },
        line: diagnostic.line.and_then(|line| u32::try_from(line).ok()),
        message: diagnostic.message,
        blocking: diagnostic.blocking,
    }
}

fn validation_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "openssh_import.invalid_selection",
        ErrorCategory::Validation,
        RetryStrategy::Never,
        "errors.opensshImport.invalidSelection",
    )
}

fn expired_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "openssh_import.snapshot_expired",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.opensshImport.snapshotExpired",
    )
}

fn conflict_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "openssh_import.already_committed",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.opensshImport.alreadyCommitted",
    )
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use norishell_core_api::{
        OpenSshConfigCommitRequest, OpenSshConfigPreviewRequest, ProxyDnsMode, RequestId,
        RequestMeta, RouteIngress,
    };
    use norishell_ssh_domain::openssh_config::parse_openssh_config_preview;

    use super::{OpenSshImportRoutePreview, OpenSshImportService, map_candidate};
    use crate::host_service::HostService;

    #[test]
    fn safely_mapped_literal_routes_enter_the_commit_snapshot() {
        let preview = parse_openssh_config_preview(
            "Host direct\n  HostName direct.example.test\n  User deploy\n\
             Host jumped\n  HostName jumped.example.test\n  ProxyJump jump-user@bastion.example.test:2222\n\
             Host http\n  HostName http.example.test\n  ProxyCommand /usr/bin/nc -X connect -x proxy.example.test:8080 %h %p\n\
             Host socks\n  HostName socks.example.test\n  ProxyCommand /usr/bin/nc -X 5 -x socks.example.test:1080 %h %p\n",
        );
        let mut stored = BTreeMap::new();
        let candidates = preview
            .candidates
            .into_iter()
            .map(|candidate| map_candidate(candidate, &mut stored))
            .collect::<Vec<_>>();
        assert_eq!(candidates.len(), 4);
        assert!(candidates.iter().all(|candidate| candidate.importable));
        let jumped = candidates
            .iter()
            .find(|candidate| candidate.alias == "jumped")
            .expect("jump preview");
        let OpenSshImportRoutePreview::JumpChain { hops } = &jumped.route else {
            panic!("expected jump route")
        };
        assert_eq!(hops[0].username.as_deref(), Some("jump-user"));
        assert_eq!(stored.len(), 4);
        assert!(stored.values().any(|candidate| {
            candidate.input.jump_hosts.len() == 1
                && candidate.input.jump_hosts[0].username.as_deref() == Some("jump-user")
        }));
        assert!(stored.values().any(|candidate| matches!(
            candidate.input.ingress,
            RouteIngress::HttpConnectProxy {
                proxy_auth_credential_ref_id: None,
                ..
            }
        )));
        assert!(stored.values().any(|candidate| matches!(
            candidate.input.ingress,
            RouteIngress::Socks5Proxy {
                dns_mode: ProxyDnsMode::Proxy,
                proxy_auth_credential_ref_id: None,
                ..
            }
        )));
    }

    #[test]
    fn unsafe_or_ambiguous_routes_never_enter_the_commit_snapshot() {
        for config in [
            "Host bad\nProxyCommand /bin/sh -c 'nc %h %p'",
            "Host bad\nInclude ~/.ssh/conf.d/*",
            "Host *.example.test\nHostName server.example.test",
            "Match exec true\nHost bad\nHostName bad.example.test",
            "Host bad\nProxyJump a,b,c,d,e,f",
        ] {
            let preview = parse_openssh_config_preview(config);
            let mut stored = BTreeMap::new();
            let candidates = preview
                .candidates
                .into_iter()
                .map(|candidate| map_candidate(candidate, &mut stored))
                .collect::<Vec<_>>();
            assert!(
                stored.is_empty(),
                "unsafe config entered snapshot: {config}"
            );
            assert!(
                candidates.is_empty() || candidates.iter().all(|candidate| !candidate.importable),
                "unsafe config became importable: {config}"
            );
        }
    }

    #[test]
    fn non_direct_commit_is_atomic_and_exact_replay_is_idempotent() {
        let directory = tempfile::tempdir().expect("tempdir");
        let hosts = HostService::start(directory.path()).expect("start host service");
        let importer = OpenSshImportService::default();
        let preview = importer
            .preview(OpenSshConfigPreviewRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                config_text: "Host jump\n  HostName jump.example.test\n  User jump-user\n  Port 2222\n\
                              Host target\n  HostName target.example.test\n  User deploy\n  IdentityFile /path/that/must/not/be/read\n  ProxyJump jump-user@jump.example.test:2222\n\
                              Host proxy\n  HostName proxy-target.example.test\n  ProxyCommand /usr/bin/nc -X connect -x proxy.example.test:8080 %h %p\n\
                              Host socks\n  HostName socks-target.example.test\n  ProxyCommand /usr/bin/nc -X 5 -x socks.example.test:1080 %h %p\n"
                    .to_owned(),
            })
            .expect("preview supported routes");
        assert!(
            preview
                .candidates
                .iter()
                .all(|candidate| candidate.importable)
        );
        let jump_candidate = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "jump")
            .expect("selected jump candidate");
        let target_candidate = preview
            .candidates
            .iter()
            .find(|candidate| candidate.alias == "target")
            .expect("selected route target");
        let OpenSshImportRoutePreview::JumpChain { hops } = &target_candidate.route else {
            panic!("expected frozen jump route")
        };
        assert_eq!(jump_candidate.endpoint.as_ref(), Some(&hops[0].endpoint));
        assert_eq!(jump_candidate.username, hops[0].username);
        let candidate_ids = preview
            .candidates
            .iter()
            .map(|candidate| candidate.candidate_id.clone())
            .collect::<Vec<_>>();
        let request = OpenSshConfigCommitRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            snapshot_id: preview.snapshot_id,
            candidate_ids,
        };
        let first = importer
            .commit(request.clone(), &hosts)
            .expect("commit non-direct routes");
        assert_eq!(
            first.hosts.len(),
            4,
            "four selected targets with no duplicate hop"
        );
        let replay = importer
            .commit(request.clone(), &hosts)
            .expect("replay exact selection");
        assert_eq!(replay, first);
        assert_eq!(
            hosts
                .with_plugin_repository(|repository| repository.list_hosts())
                .expect("list imported hosts")
                .len(),
            4,
            "exact replay must not create another target or jump host"
        );

        for host in &first.hosts {
            let snapshot = hosts
                .get_connection_snapshot(&host.host_id)
                .expect("imported connection snapshot");
            assert!(snapshot.credentials.is_empty());
            assert!(
                snapshot
                    .config
                    .authentication_plan
                    .credential_ref_ids
                    .is_empty()
            );
        }
        let target = first
            .hosts
            .iter()
            .find(|host| host.label == "target")
            .expect("jump target");
        let selected_jump = first
            .hosts
            .iter()
            .find(|host| host.label == "jump")
            .expect("selected jump target");
        let target_route = hosts
            .get_connection_snapshot(&target.host_id)
            .expect("jump target snapshot")
            .config
            .route_plan;
        assert_eq!(target_route.jump_host_ids.len(), 1);
        assert_eq!(target_route.jump_host_ids[0], selected_jump.host_id);

        let mut changed = request;
        changed.candidate_ids.pop();
        assert!(importer.commit(changed, &hosts).is_err());
        assert_eq!(
            hosts
                .with_plugin_repository(|repository| repository.list_hosts())
                .expect("list after conflicting replay")
                .len(),
            4
        );
    }
}
