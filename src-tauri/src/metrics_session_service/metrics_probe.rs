use std::{sync::Arc, time::Duration};

use norishell_core_api::{MetricsSessionFailureCode, MetricsSessionState, MonitoringPolicy};
use norishell_server_metrics::{
    LINUX_CPU_FOLLOW_UP_COMMAND, LINUX_PROBE_COMMAND, LinuxMetricSample, LinuxMetricsProvider,
    LinuxRawSample, MAX_LINUX_PROBE_BYTES, MAX_PLATFORM_BYTES, MAX_PROC_STAT_BYTES,
    MetricUnavailableReason, MetricValue,
};
use norishell_ssh_transport::RemoteExecTransport;
use tokio::sync::{Semaphore, oneshot, watch};

use super::{
    ActorMetricsHostKeyVerifier, ActorMetricsInteraction, Message, MetricsWorkerIdentity,
    WorkerFailure, map_connection_failure, map_exec_failure, map_parse_failure, send_worker_state,
    wait_for_stop,
};
use crate::{
    connection_profile::ResolvedSshConnectionBase, ssh_agent_service::SshAgentService,
    ssh_connection_orchestrator::SshConnectionOrchestrator,
    transient_credential_service::TransientCredentialService, vault_service::VaultService,
};

pub(super) type MetricsExecTransport = RemoteExecTransport<ActorMetricsHostKeyVerifier>;

const INITIAL_CPU_WINDOW: Duration = Duration::from_millis(250);

pub(super) struct MetricsProbeRuntime {
    pub(super) connection: ResolvedSshConnectionBase,
    pub(super) vault: VaultService,
    pub(super) transient_credentials: TransientCredentialService,
    pub(super) ssh_agent: SshAgentService,
    pub(super) connect_limit: Arc<Semaphore>,
}

pub(super) enum MetricsProbeOutcome {
    Sample(LinuxMetricSample),
    Stopped,
    Failed(WorkerFailure),
}

pub(super) enum MetricsConnectOutcome {
    Connected(MetricsExecTransport),
    Stopped,
    Failed(WorkerFailure),
}

/// The permit limits simultaneous handshakes, not the number of monitored hosts.
pub(super) async fn connect_metrics_transport(
    identity: &MetricsWorkerIdentity,
    runtime: &MetricsProbeRuntime,
    stop: &mut watch::Receiver<bool>,
) -> MetricsConnectOutcome {
    let _permit = tokio::select! {
        biased;
        _ = wait_for_stop(stop) => return MetricsConnectOutcome::Stopped,
        permit = runtime.connect_limit.clone().acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => return MetricsConnectOutcome::Failed(WorkerFailure {
                code: MetricsSessionFailureCode::ConnectionUnavailable,
                recoverable: true,
            }),
        },
    };
    let verifier = Arc::new(ActorMetricsHostKeyVerifier {
        tx: identity.tx.clone(),
        host_id: identity.host_key.clone(),
        generation: identity.generation,
    });
    let mut interaction = ActorMetricsInteraction {
        tx: identity.tx.clone(),
        host_id: identity.host_key.clone(),
        generation: identity.generation,
    };
    let orchestrator = SshConnectionOrchestrator::new(
        &runtime.vault,
        &runtime.transient_credentials,
        &runtime.ssh_agent,
    );
    let connection = tokio::select! {
        biased;
        _ = wait_for_stop(stop) => return MetricsConnectOutcome::Stopped,
        connection = orchestrator.connect(
            runtime.connection.clone(),
            verifier,
            &mut interaction,
            None,
        ) => {
            match connection {
                Ok(connection) => connection,
                Err(failure) => {
                    return MetricsConnectOutcome::Failed(map_connection_failure(failure.error));
                }
            }
        }
    };
    MetricsConnectOutcome::Connected(connection.transport.into_exec_transport())
}

/// One fixed-command sample opens and closes exec channels on the worker's SSH transport.
pub(super) async fn run_metrics_probe(
    identity: &MetricsWorkerIdentity,
    transport: &mut MetricsExecTransport,
    policy: &MonitoringPolicy,
    provider: &mut LinuxMetricsProvider,
    started: tokio::time::Instant,
    stop: &mut watch::Receiver<bool>,
) -> MetricsProbeOutcome {
    let result = tokio::select! {
        biased;
        _ = wait_for_stop(stop) => None,
        result = async {
            validate_current_trust(identity).await?;
            send_worker_state(
                &identity.tx,
                &identity.host_key,
                identity.generation,
                MetricsSessionState::DetectingPlatform,
            )
            .await
            .map_err(|()| WorkerFailure {
                code: MetricsSessionFailureCode::ConnectionUnavailable,
                recoverable: true,
            })?;
            send_worker_state(
                &identity.tx,
                &identity.host_key,
                identity.generation,
                MetricsSessionState::Sampling,
            )
            .await
            .map_err(|()| WorkerFailure {
                code: MetricsSessionFailureCode::ConnectionUnavailable,
                recoverable: true,
            })?;
            let deadline = tokio::time::Instant::now()
                + Duration::from_millis(u64::from(policy.sample_timeout_millis));
            collect_linux_sample(identity, transport, policy, provider, started, deadline).await
        } => Some(result),
    };
    match result {
        None => MetricsProbeOutcome::Stopped,
        Some(Err(failure)) => MetricsProbeOutcome::Failed(failure),
        Some(Ok(sample)) => MetricsProbeOutcome::Sample(sample),
    }
}

async fn collect_linux_sample(
    identity: &MetricsWorkerIdentity,
    transport: &mut MetricsExecTransport,
    policy: &MonitoringPolicy,
    provider: &mut LinuxMetricsProvider,
    started: tokio::time::Instant,
    deadline: tokio::time::Instant,
) -> Result<LinuxMetricSample, WorkerFailure> {
    super::validate_supported_selection(policy).map_err(|()| WorkerFailure {
        code: MetricsSessionFailureCode::ProviderUnsupported,
        recoverable: false,
    })?;
    let output = transport
        .execute_capture(
            LINUX_PROBE_COMMAND,
            remaining_until(deadline)?,
            MAX_LINUX_PROBE_BYTES,
        )
        .await
        .map_err(map_exec_failure)?;
    if output.exit_status != Some(0) {
        return Err(WorkerFailure {
            code: MetricsSessionFailureCode::PermissionDenied,
            recoverable: true,
        });
    }
    let Some(raw) = parse_linux_probe_output(&output.stdout)? else {
        return Err(WorkerFailure {
            code: MetricsSessionFailureCode::ProviderUnsupported,
            recoverable: false,
        });
    };
    let mut sample = provider
        .sample(
            started.elapsed(),
            LinuxRawSample {
                proc_stat: raw.proc_stat,
                proc_meminfo: raw.proc_meminfo,
                proc_net_dev: raw.proc_net_dev,
                root_df: raw.root_df,
            },
        )
        .map_err(map_parse_failure)?;
    if matches!(
        sample.cpu,
        MetricValue::Unavailable(MetricUnavailableReason::InitialBaseline)
    ) {
        tokio::time::sleep(INITIAL_CPU_WINDOW).await;
        validate_current_trust(identity).await?;
        let follow_up = transport
            .execute_capture(
                LINUX_CPU_FOLLOW_UP_COMMAND,
                remaining_until(deadline)?,
                MAX_PROC_STAT_BYTES,
            )
            .await
            .map_err(map_exec_failure)?;
        if follow_up.exit_status != Some(0) {
            return Err(WorkerFailure {
                code: MetricsSessionFailureCode::PermissionDenied,
                recoverable: true,
            });
        }
        sample.cpu = provider
            .sample_cpu_follow_up(&follow_up.stdout)
            .map_err(map_parse_failure)?;
    }
    Ok(sample)
}

async fn validate_current_trust(identity: &MetricsWorkerIdentity) -> Result<(), WorkerFailure> {
    let (reply, response) = oneshot::channel();
    identity
        .tx
        .send(Message::ValidateTrust {
            host_id: identity.host_key.clone(),
            generation: identity.generation,
            reply,
        })
        .await
        .map_err(|_| WorkerFailure {
            code: MetricsSessionFailureCode::ConnectionUnavailable,
            recoverable: true,
        })?;
    response.await.unwrap_or(Err(WorkerFailure {
        code: MetricsSessionFailureCode::ConnectionUnavailable,
        recoverable: true,
    }))
}

struct LinuxProbeOutput<'a> {
    proc_stat: &'a [u8],
    proc_meminfo: &'a [u8],
    proc_net_dev: &'a [u8],
    root_df: &'a [u8],
}

fn parse_linux_probe_output(output: &[u8]) -> Result<Option<LinuxProbeOutput<'_>>, WorkerFailure> {
    let sections = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let Some(platform) = sections.first().copied() else {
        return Err(malformed_output());
    };
    let platform = platform.strip_suffix(b"\n").unwrap_or(platform);
    let platform = platform.strip_suffix(b"\r").unwrap_or(platform);
    if platform.is_empty()
        || platform.len() > MAX_PLATFORM_BYTES
        || !platform
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(malformed_output());
    }
    if platform != b"Linux" {
        return (sections.len() == 2 && sections[1].is_empty())
            .then_some(None)
            .ok_or_else(malformed_output);
    }
    if sections.len() != 5 {
        return Err(malformed_output());
    }
    Ok(Some(LinuxProbeOutput {
        proc_stat: sections[1],
        proc_meminfo: sections[2],
        proc_net_dev: sections[3],
        root_df: sections[4],
    }))
}

fn remaining_until(deadline: tokio::time::Instant) -> Result<Duration, WorkerFailure> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    (!remaining.is_zero())
        .then_some(remaining)
        .ok_or(WorkerFailure {
            code: MetricsSessionFailureCode::SampleTimedOut,
            recoverable: true,
        })
}

const fn malformed_output() -> WorkerFailure {
    WorkerFailure {
        code: MetricsSessionFailureCode::MalformedOutput,
        recoverable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_probe_requires_exact_linux_sections_and_accepts_unsupported_platforms() {
        let output = b"Linux\n\0cpu 1 0 1 8 0 0 0 0\n\0MemTotal: 10 kB\nMemAvailable: 5 kB\n\0Inter-| Receive | Transmit\n\0Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/root 10 5 5 50% /\n";
        let parsed = parse_linux_probe_output(output).unwrap().unwrap();
        assert_eq!(parsed.proc_stat, b"cpu 1 0 1 8 0 0 0 0\n");
        assert!(parse_linux_probe_output(b"Darwin\n\0").unwrap().is_none());
        assert!(parse_linux_probe_output(b"Linux\n\0cpu only").is_err());
        assert!(parse_linux_probe_output(b"Darwin\n\0unexpected").is_err());
        assert!(parse_linux_probe_output(b"\xff\0").is_err());
    }
}
