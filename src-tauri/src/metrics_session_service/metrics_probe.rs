use std::{sync::Arc, time::Duration};

use norishell_core_api::{MetricsSessionFailureCode, MetricsSessionState, MonitoringPolicy};
use norishell_server_metrics::{
    LINUX_CPU_FOLLOW_UP_COMMAND, LINUX_PROBE_COMMAND, LinuxMetricSample, LinuxMetricsProvider,
    LinuxRawSample, MAX_LINUX_PROBE_BYTES, MAX_PLATFORM_BYTES, MAX_PROC_STAT_BYTES,
    MetricUnavailableReason, MetricValue,
};
use norishell_ssh_transport::RemoteExecTransport;
use tokio::sync::{Semaphore, watch};

use super::{
    ActorMetricsHostKeyVerifier, ActorMetricsInteraction, MetricsWorkerIdentity, WorkerFailure,
    map_connection_failure, map_exec_failure, map_parse_failure, send_worker_state, wait_for_stop,
};
use crate::{
    connection_profile::ResolvedSshConnectionBase, ssh_agent_service::SshAgentService,
    ssh_connection_orchestrator::SshConnectionOrchestrator,
    transient_credential_service::TransientCredentialService, vault_service::VaultService,
};

type MetricsExecTransport = RemoteExecTransport<ActorMetricsHostKeyVerifier>;

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

/// Runs exactly one bounded metrics probe. Once authentication succeeds, every
/// success, failure, and cooperative-cancellation path converges on an explicit
/// transport disconnect before returning to the scheduler.
pub(super) async fn run_metrics_probe(
    identity: &MetricsWorkerIdentity,
    runtime: &MetricsProbeRuntime,
    policy: &MonitoringPolicy,
    provider: &mut LinuxMetricsProvider,
    started: tokio::time::Instant,
    stop: &mut watch::Receiver<bool>,
) -> MetricsProbeOutcome {
    let _permit = tokio::select! {
        biased;
        _ = wait_for_stop(stop) => return MetricsProbeOutcome::Stopped,
        permit = runtime.connect_limit.clone().acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => return MetricsProbeOutcome::Failed(WorkerFailure {
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
        _ = wait_for_stop(stop) => return MetricsProbeOutcome::Stopped,
        connection = orchestrator.connect(
            runtime.connection.clone(),
            verifier,
            &mut interaction,
            None,
        ) => {
            match connection {
                Ok(connection) => connection,
                Err(failure) => {
                    return MetricsProbeOutcome::Failed(map_connection_failure(failure.error));
                }
            }
        }
    };
    let mut transport = connection.transport.into_exec_transport();
    let result = tokio::select! {
        biased;
        _ = wait_for_stop(stop) => None,
        result = async {
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
                + Duration::from_secs(u64::from(policy.sample_timeout_seconds));
            collect_linux_sample(&mut transport, policy, provider, started, deadline).await
        } => Some(result),
    };
    let cleanup = transport.disconnect().await.map_err(map_exec_failure);
    match (result, cleanup) {
        (None, _) => MetricsProbeOutcome::Stopped,
        (Some(Err(failure)), _) => MetricsProbeOutcome::Failed(failure),
        (Some(Ok(_)), Err(failure)) => MetricsProbeOutcome::Failed(failure),
        (Some(Ok(sample)), Ok(())) => MetricsProbeOutcome::Sample(sample),
    }
}

async fn collect_linux_sample(
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
