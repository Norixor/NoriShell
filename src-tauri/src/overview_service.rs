use std::collections::BTreeMap;

use norishell_core_api::{
    ForwardSessionState, ForwardSessionSummary, HostCatalogEntry, MetricsSessionState,
    MetricsSessionSummary, MonitoringPolicySummary, ServerOverviewCard,
    ServerOverviewConnectionState, ServerOverviewSnapshot, ServerOverviewSnapshotRequest,
    ServerResourceCounts, SftpSessionState, SftpSessionSummary, SshSessionSnapshot,
    SshSessionState, SshSessionTarget, WireSequence,
};
use tauri::State;

use crate::{
    forward_session_service::ForwardSessionService, host_service::HostService,
    metrics_session_service::MetricsSessionService, sftp_session_service::SftpSessionService,
    ssh_session_service::SshSessionService,
};

type CoreResult<T> = Result<T, Box<norishell_core_api::CoreApiError>>;

#[tauri::command]
pub async fn server_overview_snapshot(
    request: ServerOverviewSnapshotRequest,
    hosts: State<'_, HostService>,
    terminals: State<'_, SshSessionService>,
    metrics: State<'_, MetricsSessionService>,
    sftp: State<'_, SftpSessionService>,
    forwards: State<'_, ForwardSessionService>,
) -> CoreResult<ServerOverviewSnapshot> {
    let request_id = request.meta.request_id;
    let host_sources = hosts.list_overview_sources(request_id.clone())?;
    let terminal_snapshot = terminals.snapshot(request_id.clone()).await?;
    let metrics_sessions = metrics.summaries(request_id).await?;
    let sftp_sessions = sftp.wire_summaries().await.map_err(|_| {
        Box::new(norishell_core_api::CoreApiError {
            code: "overview.sftp_snapshot_failed".to_owned(),
            category: norishell_core_api::ErrorCategory::Unavailable,
            retry_strategy: norishell_core_api::RetryStrategy::RefreshSnapshot,
            message_key: "errors.overview.resourceSnapshotFailed".to_owned(),
            params: BTreeMap::new(),
            request_id: None,
            diagnostic_id: None,
            conflict: None,
        })
    })?;
    let forward_sessions = forwards.wire_summaries().map_err(|_| {
        Box::new(norishell_core_api::CoreApiError {
            code: "overview.forward_snapshot_failed".to_owned(),
            category: norishell_core_api::ErrorCategory::Unavailable,
            retry_strategy: norishell_core_api::RetryStrategy::RefreshSnapshot,
            message_key: "errors.overview.resourceSnapshotFailed".to_owned(),
            params: BTreeMap::new(),
            request_id: None,
            diagnostic_id: None,
            conflict: None,
        })
    })?;
    Ok(project_overview(
        host_sources,
        &terminal_snapshot,
        &metrics_sessions,
        &sftp_sessions,
        &forward_sessions,
    ))
}

pub(crate) fn project_overview(
    host_sources: Vec<(HostCatalogEntry, MonitoringPolicySummary)>,
    terminal_snapshot: &SshSessionSnapshot,
    metrics_sessions: &[MetricsSessionSummary],
    sftp_sessions: &[SftpSessionSummary],
    forward_sessions: &[ForwardSessionSummary],
) -> ServerOverviewSnapshot {
    let mut terminal_by_host = BTreeMap::<String, Vec<_>>::new();
    for session in &terminal_snapshot.sessions {
        let SshSessionTarget::Host { host_id, .. } = &session.target else {
            continue;
        };
        terminal_by_host
            .entry(host_id.as_str().to_owned())
            .or_default()
            .push(session);
    }
    let metrics_by_host = metrics_sessions
        .iter()
        .map(|session| (session.host_id.as_str(), session))
        .collect::<BTreeMap<_, _>>();
    let mut sftp_by_host = BTreeMap::<String, Vec<_>>::new();
    for session in sftp_sessions {
        let Some(host_id) = session.host_id.as_ref() else {
            continue;
        };
        sftp_by_host
            .entry(host_id.as_str().to_owned())
            .or_default()
            .push(session);
    }
    let mut forward_by_host = BTreeMap::<String, Vec<_>>::new();
    for session in forward_sessions {
        forward_by_host
            .entry(session.host_id.as_str().to_owned())
            .or_default()
            .push(session);
    }

    let cards = host_sources
        .into_iter()
        .map(|(catalog_entry, monitoring_policy)| {
            let host_id = catalog_entry.host.host_id.as_str();
            let terminals = terminal_by_host.remove(host_id).unwrap_or_default();
            let visible_terminals = terminals
                .into_iter()
                .filter(|session| {
                    session.attachment_count > 0 && session.state != SshSessionState::Closed
                })
                .collect::<Vec<_>>();
            let terminal_counts = visible_terminals.iter().fold(
                ServerResourceCounts::default(),
                |mut counts, session| {
                    match session.state {
                        SshSessionState::Resolving
                        | SshSessionState::Connecting
                        | SshSessionState::VerifyingHostKey
                        | SshSessionState::AwaitingHostKeyDecision
                        | SshSessionState::Authenticating
                        | SshSessionState::OpeningChannel
                        | SshSessionState::AutomatingLogin => counts.connecting += 1,
                        SshSessionState::Running => counts.running += 1,
                        SshSessionState::Disconnecting => counts.lost += 1,
                        SshSessionState::Closed => {}
                        SshSessionState::Failed => counts.failed += 1,
                    }
                    counts
                },
            );
            let metrics_session = metrics_by_host.get(host_id).copied().cloned();
            let metrics_counts = metrics_session
                .as_ref()
                .map(metrics_resource_counts)
                .unwrap_or_default();
            let sftp_counts = sftp_by_host
                .remove(host_id)
                .unwrap_or_default()
                .into_iter()
                .fold(ServerResourceCounts::default(), |mut counts, session| {
                    match session.state {
                        SftpSessionState::Connecting
                        | SftpSessionState::VerifyingHostKey
                        | SftpSessionState::Authenticating
                        | SftpSessionState::NeedsAuthentication
                        | SftpSessionState::OpeningSubsystem => counts.connecting += 1,
                        SftpSessionState::Ready => counts.running += 1,
                        SftpSessionState::Disconnecting => counts.lost += 1,
                        SftpSessionState::Closed => {}
                        SftpSessionState::Failed => counts.failed += 1,
                    }
                    counts
                });
            let forward_counts = forward_by_host
                .remove(host_id)
                .unwrap_or_default()
                .into_iter()
                .fold(ServerResourceCounts::default(), |mut counts, session| {
                    match session.state {
                        ForwardSessionState::Starting => counts.connecting += 1,
                        ForwardSessionState::Running => counts.running += 1,
                        ForwardSessionState::Stopping => counts.lost += 1,
                        ForwardSessionState::Stopped => {}
                        ForwardSessionState::Failed => counts.failed += 1,
                    }
                    counts
                });
            let connection_state = aggregate_connection_state(
                &terminal_counts,
                &sftp_counts,
                &forward_counts,
                &metrics_counts,
                metrics_session
                    .as_ref()
                    .and_then(|session| session.latest_snapshot.as_ref())
                    .is_some(),
            );
            ServerOverviewCard {
                catalog_entry,
                monitoring_policy,
                connection_state,
                terminal_counts,
                sftp_counts,
                forward_counts,
                metrics_counts,
                terminal_session_ids: visible_terminals
                    .into_iter()
                    .map(|session| session.session_id.clone())
                    .collect(),
                metrics_session,
            }
        })
        .collect();
    let metrics_revision = metrics_sessions
        .iter()
        .map(|session| session.state_revision.get())
        .max()
        .unwrap_or(0);
    let sftp_revision = sftp_sessions
        .iter()
        .map(|session| session.state_revision.get())
        .max()
        .unwrap_or(0);
    let forward_revision = forward_sessions
        .iter()
        .map(|session| session.state_revision.get())
        .max()
        .unwrap_or(0);
    ServerOverviewSnapshot {
        snapshot_revision: WireSequence::new(
            terminal_snapshot
                .snapshot_revision
                .get()
                .saturating_add(metrics_revision)
                .saturating_add(sftp_revision)
                .saturating_add(forward_revision),
        ),
        cards,
    }
}

fn metrics_resource_counts(session: &MetricsSessionSummary) -> ServerResourceCounts {
    let mut counts = ServerResourceCounts::default();
    match session.state {
        MetricsSessionState::Idle
        | MetricsSessionState::Connecting
        | MetricsSessionState::VerifyingHostKey
        | MetricsSessionState::Authenticating
        | MetricsSessionState::DetectingPlatform
        | MetricsSessionState::Sampling => counts.connecting = 1,
        MetricsSessionState::Ready => counts.running = 1,
        MetricsSessionState::NeedsHostKeyReview
        | MetricsSessionState::NeedsAuthentication
        | MetricsSessionState::Backoff
        | MetricsSessionState::Disconnecting
        | MetricsSessionState::Closed => counts.lost = 1,
        MetricsSessionState::Failed => counts.failed = 1,
    }
    counts
}

fn aggregate_connection_state(
    terminal: &ServerResourceCounts,
    sftp: &ServerResourceCounts,
    forward: &ServerResourceCounts,
    metrics: &ServerResourceCounts,
    has_metric_snapshot: bool,
) -> ServerOverviewConnectionState {
    let interactive_running = terminal
        .running
        .saturating_add(sftp.running)
        .saturating_add(forward.running);
    let any_running = interactive_running.saturating_add(metrics.running);
    let any_connecting = terminal
        .connecting
        .saturating_add(sftp.connecting)
        .saturating_add(forward.connecting)
        .saturating_add(metrics.connecting);
    let any_lost = terminal
        .lost
        .saturating_add(sftp.lost)
        .saturating_add(forward.lost)
        .saturating_add(metrics.lost);
    let any_failed = terminal
        .failed
        .saturating_add(sftp.failed)
        .saturating_add(forward.failed)
        .saturating_add(metrics.failed);

    if any_running > 0 && any_lost.saturating_add(any_failed) > 0 {
        ServerOverviewConnectionState::Degraded
    } else if interactive_running > 0 {
        ServerOverviewConnectionState::Connected
    } else if metrics.running > 0 {
        ServerOverviewConnectionState::MonitoringOnly
    } else if any_connecting > 0 {
        ServerOverviewConnectionState::Connecting
    } else if has_metric_snapshot && any_lost.saturating_add(any_failed) > 0 {
        ServerOverviewConnectionState::Degraded
    } else if any_failed > 0 {
        ServerOverviewConnectionState::Failed
    } else {
        ServerOverviewConnectionState::Disconnected
    }
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        CleanupStepState, DiskResourceId, ForwardCleanupFacts, ForwardSessionId, HostId,
        HostSummary, MetricSnapshot, MetricsPlatform, MetricsSessionId, MonitoringPolicy,
        MonitoringPolicySummary, NetworkResourceId, SftpSessionId, SshOpenAttemptId,
        SshSessionEndpoint, SshSessionId, SshSessionSummary,
    };

    use super::*;

    fn source(label: &str) -> (HostCatalogEntry, MonitoringPolicySummary) {
        let host_id = HostId::new();
        (
            HostCatalogEntry {
                host: HostSummary {
                    host_id: host_id.clone(),
                    label: label.to_owned(),
                    address: format!("{}.example", label.to_lowercase()),
                    normalized_address: format!("{}.example", label.to_lowercase()),
                    port: 22,
                    username: Some("tester".to_owned()),
                    identity_id: None,
                    favorite: false,
                    has_ready_credential: false,
                    state_version: WireSequence::new(1),
                },
                group: None,
                tags: Vec::new(),
                recent_connection: None,
            },
            MonitoringPolicySummary {
                host_id,
                revision: WireSequence::new(1),
                policy: MonitoringPolicy {
                    enabled: false,
                    sample_interval_millis: 1500,
                    sample_timeout_millis: 5000,
                    disk_mount_ids: vec![DiskResourceId::Root],
                    network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
                },
            },
        )
    }

    fn terminal(host: &HostCatalogEntry, state: SshSessionState) -> SshSessionSummary {
        SshSessionSummary {
            session_id: SshSessionId::new(),
            open_attempt_id: SshOpenAttemptId::new(),
            target: SshSessionTarget::Host {
                host_id: host.host.host_id.clone(),
                expected_host_state_version: host.host.state_version,
            },
            credential_ref_id: None,
            endpoint: Some(SshSessionEndpoint {
                address: host.host.address.clone(),
                port: host.host.port,
                username: host.host.username.clone(),
            }),
            generation: WireSequence::new(1),
            state_revision: WireSequence::new(1),
            attachment_revision: WireSequence::new(1),
            event_seq: WireSequence::new(0),
            channel_id: None,
            negotiated_algorithms: Vec::new(),
            state,
            close_reason: None,
            failure_reason: None,
            attachment_count: 1,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        }
    }

    #[test]
    fn projects_exactly_one_card_per_saved_host_and_ignores_quick_connect() {
        let first = source("Alpha");
        let second = source("Beta");
        let sessions = vec![
            terminal(&first.0, SshSessionState::Running),
            terminal(&first.0, SshSessionState::Connecting),
            SshSessionSummary {
                target: SshSessionTarget::QuickConnect {
                    endpoint: SshSessionEndpoint {
                        address: "temporary.example".to_owned(),
                        port: 22,
                        username: Some("tester".to_owned()),
                    },
                },
                ..terminal(&second.0, SshSessionState::Running)
            },
        ];
        let snapshot = project_overview(
            vec![first, second],
            &SshSessionSnapshot {
                snapshot_revision: WireSequence::new(7),
                sessions,
            },
            &[],
            &[],
            &[],
        );

        assert_eq!(snapshot.cards.len(), 2);
        assert_eq!(snapshot.cards[0].terminal_counts.running, 1);
        assert_eq!(snapshot.cards[0].terminal_counts.connecting, 1);
        assert_eq!(
            snapshot.cards[0].connection_state,
            ServerOverviewConnectionState::Connected
        );
        assert_eq!(
            snapshot.cards[1].connection_state,
            ServerOverviewConnectionState::Disconnected
        );
    }

    #[test]
    fn distinguishes_monitoring_only_and_degraded_from_terminal_connected() {
        let monitored = source("Monitored");
        let metrics_session_id = MetricsSessionId::new();
        let metrics = MetricsSessionSummary {
            metrics_session_id: metrics_session_id.clone(),
            host_id: monitored.0.host.host_id.clone(),
            generation: WireSequence::new(1),
            state_revision: WireSequence::new(3),
            state: MetricsSessionState::Ready,
            authentication_reason: None,
            failure_code: None,
            next_retry_at_unix_ms: None,
            latest_snapshot: None,
            host_key_challenge: None,
            keyboard_interactive_challenge: None,
        };
        let ready = project_overview(
            vec![monitored.clone()],
            &SshSessionSnapshot {
                snapshot_revision: WireSequence::new(1),
                sessions: Vec::new(),
            },
            std::slice::from_ref(&metrics),
            &[],
            &[],
        );
        assert_eq!(
            ready.cards[0].connection_state,
            ServerOverviewConnectionState::MonitoringOnly
        );

        let mut backoff = metrics;
        backoff.state = MetricsSessionState::Backoff;
        backoff.latest_snapshot = Some(MetricSnapshot {
            host_id: monitored.0.host.host_id.clone(),
            metrics_session_id,
            generation: WireSequence::new(1),
            sample_sequence: WireSequence::new(1),
            provider_id: "linux-procfs".to_owned(),
            provider_version: 1,
            platform: MetricsPlatform::Linux,
            sample_started_at_unix_ms: 1,
            sample_completed_at_unix_ms: 2,
            stale: true,
            cpu: norishell_core_api::CpuMetric {
                state: norishell_core_api::MetricFieldState::InitialBaseline,
                basis_points: None,
            },
            memory: norishell_core_api::MemoryMetric {
                state: norishell_core_api::MetricFieldState::Error,
                used_bytes: None,
                available_bytes: None,
                total_bytes: None,
            },
            network: norishell_core_api::NetworkMetric {
                resource_id: NetworkResourceId::AggregateNonLoopback,
                state: norishell_core_api::MetricFieldState::InitialBaseline,
                receive_bytes_per_second: None,
                transmit_bytes_per_second: None,
            },
            disks: Vec::new(),
        });
        let stale = project_overview(
            vec![monitored],
            &SshSessionSnapshot {
                snapshot_revision: WireSequence::new(1),
                sessions: Vec::new(),
            },
            &[backoff],
            &[],
            &[],
        );
        assert_eq!(
            stale.cards[0].connection_state,
            ServerOverviewConnectionState::Degraded
        );
    }

    #[test]
    fn paused_metrics_authentication_does_not_look_like_an_active_connection() {
        let monitored = source("Locked");
        let metrics = MetricsSessionSummary {
            metrics_session_id: MetricsSessionId::new(),
            host_id: monitored.0.host.host_id.clone(),
            generation: WireSequence::new(1),
            state_revision: WireSequence::new(2),
            state: MetricsSessionState::NeedsAuthentication,
            authentication_reason: Some(
                norishell_core_api::MetricsAuthenticationReason::VaultLocked,
            ),
            failure_code: None,
            next_retry_at_unix_ms: None,
            latest_snapshot: None,
            host_key_challenge: None,
            keyboard_interactive_challenge: None,
        };

        let snapshot = project_overview(
            vec![monitored],
            &SshSessionSnapshot {
                snapshot_revision: WireSequence::new(1),
                sessions: Vec::new(),
            },
            &[metrics],
            &[],
            &[],
        );

        assert_eq!(snapshot.cards[0].metrics_counts.connecting, 0);
        assert_eq!(snapshot.cards[0].metrics_counts.lost, 1);
        assert_eq!(
            snapshot.cards[0].connection_state,
            ServerOverviewConnectionState::Disconnected
        );
    }

    #[test]
    fn aggregates_independent_sftp_and_forward_resources_by_saved_host() {
        let host = source("Resources");
        let host_id = host.0.host.host_id.clone();
        let sftp = SftpSessionSummary {
            session_id: SftpSessionId::new(),
            host_id: Some(host_id.clone()),
            parent_ssh_session: None,
            generation: WireSequence::new(2),
            state_revision: WireSequence::new(4),
            state: SftpSessionState::Ready,
            transfer_count: 1,
            active_transfer_count: 1,
            failure: None,
        };
        let forward = ForwardSessionSummary {
            session_id: ForwardSessionId::new(),
            host_id: host_id.clone(),
            generation: WireSequence::new(1),
            state_revision: WireSequence::new(3),
            state: ForwardSessionState::Starting,
            rule_id: None,
            rule_revision: None,
            rule_snapshot: norishell_core_api::PortForwardRule::Dynamic {
                host_id,
                local_bind_address: "127.0.0.1".to_owned(),
                local_listen_port: 1080,
            },
            started_at_unix_ms: 1,
            actual_bind: None,
            child_count: 0,
            listener_to_target_bytes: WireSequence::new(0),
            target_to_listener_bytes: WireSequence::new(0),
            failure: None,
            last_child_failure: None,
            cleanup: ForwardCleanupFacts {
                listener_closed_or_remote_cancelled: CleanupStepState::NotRequired,
                children_cleared: CleanupStepState::NotRequired,
                transport_disconnected: CleanupStepState::NotRequired,
                abandoned_child_count: 0,
                uncertain: false,
            },
        };
        let snapshot = project_overview(
            vec![host],
            &SshSessionSnapshot {
                snapshot_revision: WireSequence::new(1),
                sessions: Vec::new(),
            },
            &[],
            &[sftp],
            &[forward],
        );

        assert_eq!(snapshot.cards[0].sftp_counts.running, 1);
        assert_eq!(snapshot.cards[0].forward_counts.connecting, 1);
        assert_eq!(
            snapshot.cards[0].connection_state,
            ServerOverviewConnectionState::Connected
        );
        assert_eq!(snapshot.snapshot_revision, WireSequence::new(8));
    }

    #[test]
    fn normal_sftp_history_does_not_degrade_a_new_ready_session() {
        let host = source("SftpHistory");
        let host_id = host.0.host.host_id.clone();
        let ready = SftpSessionSummary {
            session_id: SftpSessionId::new(),
            host_id: Some(host_id.clone()),
            parent_ssh_session: None,
            generation: WireSequence::new(2),
            state_revision: WireSequence::new(5),
            state: SftpSessionState::Ready,
            transfer_count: 0,
            active_transfer_count: 0,
            failure: None,
        };
        let closed = SftpSessionSummary {
            session_id: SftpSessionId::new(),
            host_id: Some(host_id),
            parent_ssh_session: None,
            generation: WireSequence::new(1),
            state_revision: WireSequence::new(4),
            state: SftpSessionState::Closed,
            transfer_count: 0,
            active_transfer_count: 0,
            failure: None,
        };

        let snapshot = project_overview(
            vec![host],
            &SshSessionSnapshot {
                snapshot_revision: WireSequence::new(1),
                sessions: Vec::new(),
            },
            &[],
            &[closed, ready],
            &[],
        );

        assert_eq!(snapshot.cards[0].sftp_counts.running, 1);
        assert_eq!(snapshot.cards[0].sftp_counts.lost, 0);
        assert_eq!(
            snapshot.cards[0].connection_state,
            ServerOverviewConnectionState::Connected
        );
    }

    #[test]
    fn closed_or_detached_terminal_history_is_not_exposed_as_a_focusable_resource() {
        let host = source("TerminalHistory");
        let running = terminal(&host.0, SshSessionState::Running);
        let running_session_id = running.session_id.clone();
        let closed = terminal(&host.0, SshSessionState::Closed);
        let mut detached_failure = terminal(&host.0, SshSessionState::Failed);
        detached_failure.attachment_count = 0;

        let snapshot = project_overview(
            vec![host],
            &SshSessionSnapshot {
                snapshot_revision: WireSequence::new(3),
                sessions: vec![closed, detached_failure, running],
            },
            &[],
            &[],
            &[],
        );

        let card = &snapshot.cards[0];
        assert_eq!(card.terminal_counts.running, 1);
        assert_eq!(card.terminal_counts.lost, 0);
        assert_eq!(card.terminal_counts.failed, 0);
        assert_eq!(card.terminal_session_ids, vec![running_session_id]);
        assert_eq!(
            card.connection_state,
            ServerOverviewConnectionState::Connected
        );
    }
}
