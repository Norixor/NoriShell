use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use norishell_app_persistence::KnownHostObservation;
use norishell_core_api::{
    CoreApiError, CpuMetric, DiskMetric, DiskResourceId, ErrorCategory, HostCatalogEntry, HostId,
    IdentityId, MetricByteCount, MetricFieldState, MetricSnapshot, MetricsAuthenticationReason,
    MetricsHostKeyChallenge, MetricsHostKeyChallengeId, MetricsHostKeyDecision,
    MetricsHostKeyDecisionRequest, MetricsKeyboardInteractiveAnswerPrepareRequest,
    MetricsKeyboardInteractiveAnswerPrepareResponse, MetricsKeyboardInteractiveChallenge,
    MetricsKeyboardInteractiveChallengeId, MetricsKeyboardInteractivePrompt,
    MetricsKeyboardInteractiveRespondRequest, MetricsPlatform, MetricsReconcileRequest,
    MetricsRetryRequest, MetricsSessionFailureCode, MetricsSessionId, MetricsSessionState,
    MetricsSessionSummary, MetricsStopRequest, MonitoringPolicy, MonitoringPolicySummary,
    NetworkResourceId, RequestId, RetryStrategy, SshSessionRouteStage, WireSequence,
};
use norishell_server_metrics::{
    LinuxMetricSample, LinuxMetricsProvider, MetricUnavailableReason, MetricValue, ParseError,
};
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    HostKeyDecision, HostKeyVerifier, ObservedHostKey, TransportError, VerifyFuture,
};
use tauri::State;
use tokio::sync::{Semaphore, mpsc, oneshot, watch};
use zeroize::Zeroizing;

mod metrics_probe;

use self::metrics_probe::{MetricsProbeOutcome, MetricsProbeRuntime, run_metrics_probe};
use crate::{
    connection_profile::{
        ConnectionProfileError, ResolvedMetricsConnectionProfile, connection_has_vault_credentials,
        connection_requires_vault, resolve_metrics_connection_profile,
    },
    core_api_error::core_error,
    host_service::HostService,
    ssh_agent_service::SshAgentService,
    ssh_connection_orchestrator::{
        ConnectionPhase, ConnectionRouteStage, KeyboardInteractiveRequest, SshConnectionInteraction,
    },
    time::unix_time_ms,
    transient_credential_service::TransientCredentialService,
    vault_service::VaultService,
};

type CoreResult<T> = Result<T, Box<CoreApiError>>;
type ActorResult<T> = Result<T, Box<CoreApiError>>;
const ACTOR_MAILBOX_CAPACITY: usize = 256;
const MAX_CONCURRENT_CONNECTS: usize = 4;
const KEYBOARD_INTERACTIVE_ROUND_TIMEOUT: Duration = Duration::from_secs(120);
const KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES: usize = 64 * 1024;
const MAX_BACKOFF_SECONDS: u64 = 300;
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct MetricsSessionService {
    tx: mpsc::Sender<Message>,
    hosts: HostService,
}

impl MetricsSessionService {
    pub fn start(
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: SshAgentService,
    ) -> Self {
        let (tx, rx) = mpsc::channel(ACTOR_MAILBOX_CAPACITY);
        let service = Self {
            tx: tx.clone(),
            hosts: hosts.clone(),
        };
        tauri::async_runtime::spawn(run_actor(
            Actor::new(tx, hosts, vault, transient_credentials, ssh_agent),
            rx,
        ));
        service
    }

    async fn request<T>(
        &self,
        request_id: RequestId,
        build: impl FnOnce(oneshot::Sender<ActorResult<T>>) -> Message,
    ) -> ActorResult<T> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(build(reply))
            .await
            .map_err(|_| unavailable_error(request_id.clone()))?;
        response.await.map_err(|_| unavailable_error(request_id))?
    }

    pub(crate) async fn summaries(
        &self,
        request_id: RequestId,
    ) -> ActorResult<Vec<MetricsSessionSummary>> {
        self.request(request_id.clone(), |reply| Message::Snapshot { reply })
            .await
    }

    pub(crate) async fn reconcile_hosts(
        &self,
        request_id: RequestId,
    ) -> ActorResult<Vec<MetricsSessionSummary>> {
        let sources = self.hosts.list_overview_sources(request_id.clone())?;
        self.request(request_id.clone(), |reply| Message::Reconcile {
            request_id,
            sources,
            reply,
        })
        .await
    }

    pub(crate) async fn disable_host(
        &self,
        request_id: RequestId,
        host_id: HostId,
    ) -> ActorResult<()> {
        self.request(request_id.clone(), |reply| Message::DisableHost {
            request_id,
            host_id,
            reply,
        })
        .await
    }

    pub(crate) async fn recheck_hosts_referencing_identity(
        &self,
        request_id: RequestId,
        identity_id: IdentityId,
    ) -> ActorResult<Vec<MetricsSessionSummary>> {
        let host_ids = self
            .hosts
            .host_ids_referencing_identity(&identity_id)
            .map_err(|_| unavailable_error(request_id.clone()))?;
        self.request(request_id.clone(), |reply| Message::RetryHosts {
            request_id,
            host_ids,
            reply,
        })
        .await
    }

    async fn reconcile(
        &self,
        request: MetricsReconcileRequest,
    ) -> ActorResult<Vec<MetricsSessionSummary>> {
        let request_id = request.meta.request_id;
        self.reconcile_hosts(request_id).await
    }

    async fn retry(&self, request: MetricsRetryRequest) -> ActorResult<MetricsSessionSummary> {
        let request_id = request.meta.request_id.clone();
        self.request(request_id, |reply| Message::Retry { request, reply })
            .await
    }

    async fn stop(&self, request: MetricsStopRequest) -> ActorResult<MetricsSessionSummary> {
        let request_id = request.meta.request_id.clone();
        self.request(request_id, |reply| Message::Stop { request, reply })
            .await
    }

    async fn decide_host_key(
        &self,
        request: MetricsHostKeyDecisionRequest,
    ) -> ActorResult<MetricsSessionSummary> {
        let request_id = request.meta.request_id.clone();
        self.request(request_id, |reply| Message::HostKeyDecide {
            request,
            reply,
        })
        .await
    }

    async fn prepare_keyboard_interactive_answer(
        &self,
        request: MetricsKeyboardInteractiveAnswerPrepareRequest,
    ) -> ActorResult<MetricsKeyboardInteractiveAnswerPrepareResponse> {
        let request_id = request.meta.request_id.clone();
        self.request(request_id, |reply| Message::KeyboardAnswerPrepare {
            request,
            reply,
        })
        .await
    }

    async fn respond_keyboard_interactive(
        &self,
        request: MetricsKeyboardInteractiveRespondRequest,
    ) -> ActorResult<MetricsSessionSummary> {
        let request_id = request.meta.request_id.clone();
        self.request(request_id, |reply| Message::KeyboardRespond {
            request,
            reply,
        })
        .await
    }

    pub(crate) async fn cancel_keyboard_challenge(
        &self,
        request: MetricsKeyboardInteractiveRespondRequest,
    ) -> ActorResult<()> {
        self.request(request.meta.request_id.clone(), |reply| {
            Message::KeyboardCancel { request, reply }
        })
        .await
    }

    pub(crate) async fn shutdown_all(&self, request_id: RequestId) -> ActorResult<()> {
        self.request(request_id.clone(), |reply| Message::ShutdownAll {
            request_id,
            reply,
        })
        .await
    }
}

#[tauri::command]
pub async fn metrics_reconcile(
    request: MetricsReconcileRequest,
    service: State<'_, MetricsSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<Vec<MetricsSessionSummary>> {
    let _creation_permit = lifecycle.acquire_resource_creation(request.meta.request_id.clone())?;
    service.reconcile(request).await
}

#[tauri::command]
pub async fn metrics_retry(
    request: MetricsRetryRequest,
    service: State<'_, MetricsSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<MetricsSessionSummary> {
    let _creation_permit = lifecycle.acquire_resource_creation(request.meta.request_id.clone())?;
    service.retry(request).await
}

#[tauri::command]
pub async fn metrics_stop(
    request: MetricsStopRequest,
    service: State<'_, MetricsSessionService>,
) -> CoreResult<MetricsSessionSummary> {
    service.stop(request).await
}

#[tauri::command]
pub async fn metrics_host_key_decide(
    request: MetricsHostKeyDecisionRequest,
    service: State<'_, MetricsSessionService>,
) -> CoreResult<MetricsSessionSummary> {
    service.decide_host_key(request).await
}

#[tauri::command]
pub async fn metrics_keyboard_interactive_answer_prepare(
    request: MetricsKeyboardInteractiveAnswerPrepareRequest,
    service: State<'_, MetricsSessionService>,
) -> CoreResult<MetricsKeyboardInteractiveAnswerPrepareResponse> {
    service.prepare_keyboard_interactive_answer(request).await
}

#[tauri::command]
pub async fn metrics_keyboard_interactive_respond(
    request: MetricsKeyboardInteractiveRespondRequest,
    service: State<'_, MetricsSessionService>,
) -> CoreResult<MetricsSessionSummary> {
    service.respond_keyboard_interactive(request).await
}

struct Actor {
    tx: mpsc::Sender<Message>,
    hosts: HostService,
    vault: VaultService,
    transient_credentials: TransientCredentialService,
    ssh_agent: SshAgentService,
    sessions: BTreeMap<String, SessionRecord>,
    connect_limit: Arc<Semaphore>,
}

struct SessionRecord {
    summary: MetricsSessionSummary,
    host_state_version: WireSequence,
    profile_revision_token: Option<String>,
    connection_revision_token: Option<String>,
    has_vault_credentials: bool,
    monitoring_policy: MonitoringPolicy,
    route_stages: BTreeMap<String, SshSessionRouteStage>,
    task: Option<ConnectTask>,
    stop: Option<watch::Sender<bool>>,
    policy_updates: Option<watch::Sender<MonitoringPolicy>>,
    active_host_key_challenge: Option<ActiveHostKeyChallenge>,
    active_keyboard_challenge: Option<ActiveKeyboardChallenge>,
    prepared_answers: BTreeMap<String, PreparedAnswer>,
    consecutive_failures: u32,
}

struct ConnectTask {
    generation: u64,
    handle: tauri::async_runtime::JoinHandle<()>,
}

struct ActiveHostKeyChallenge {
    challenge: MetricsHostKeyChallenge,
    endpoint: Endpoint,
    observed: ObservedHostKey,
}

struct ActiveKeyboardChallenge {
    challenge: MetricsKeyboardInteractiveChallenge,
    answers: oneshot::Sender<Result<Vec<Zeroizing<String>>, TransportError>>,
}

struct PreparedAnswer {
    challenge_id: MetricsKeyboardInteractiveChallengeId,
    generation: WireSequence,
    round_index: u8,
    prompt_index: u8,
    expires_at_unix_ms: i64,
    value: Zeroizing<String>,
}

enum Message {
    Snapshot {
        reply: oneshot::Sender<ActorResult<Vec<MetricsSessionSummary>>>,
    },
    Reconcile {
        request_id: RequestId,
        sources: Vec<(HostCatalogEntry, MonitoringPolicySummary)>,
        reply: oneshot::Sender<ActorResult<Vec<MetricsSessionSummary>>>,
    },
    DisableHost {
        request_id: RequestId,
        host_id: HostId,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    RetryHosts {
        request_id: RequestId,
        host_ids: Vec<HostId>,
        reply: oneshot::Sender<ActorResult<Vec<MetricsSessionSummary>>>,
    },
    Retry {
        request: MetricsRetryRequest,
        reply: oneshot::Sender<ActorResult<MetricsSessionSummary>>,
    },
    Stop {
        request: MetricsStopRequest,
        reply: oneshot::Sender<ActorResult<MetricsSessionSummary>>,
    },
    HostKeyDecide {
        request: MetricsHostKeyDecisionRequest,
        reply: oneshot::Sender<ActorResult<MetricsSessionSummary>>,
    },
    KeyboardAnswerPrepare {
        request: MetricsKeyboardInteractiveAnswerPrepareRequest,
        reply: oneshot::Sender<ActorResult<MetricsKeyboardInteractiveAnswerPrepareResponse>>,
    },
    KeyboardRespond {
        request: MetricsKeyboardInteractiveRespondRequest,
        reply: oneshot::Sender<ActorResult<MetricsSessionSummary>>,
    },
    KeyboardCancel {
        request: MetricsKeyboardInteractiveRespondRequest,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    ShutdownAll {
        request_id: RequestId,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    PhaseChanged {
        host_id: String,
        generation: u64,
        phase: ConnectionPhase,
    },
    HostKeyObserved {
        host_id: String,
        generation: u64,
        endpoint: Endpoint,
        observed: ObservedHostKey,
        reply: oneshot::Sender<Result<HostKeyDecision, TransportError>>,
    },
    KeyboardChallengeObserved {
        host_id: String,
        generation: u64,
        request: KeyboardInteractiveRequest,
        reply: oneshot::Sender<Result<Vec<Zeroizing<String>>, TransportError>>,
    },
    WorkerState {
        host_id: String,
        generation: u64,
        state: MetricsSessionState,
    },
    SampleReady {
        host_id: String,
        generation: u64,
        snapshot: MetricSnapshot,
    },
    WorkerFailed {
        host_id: String,
        generation: u64,
        failure: WorkerFailure,
    },
    RetryDue {
        host_id: String,
        generation: u64,
    },
}

#[derive(Debug, Clone, Copy)]
struct WorkerFailure {
    code: MetricsSessionFailureCode,
    recoverable: bool,
}

impl Actor {
    fn new(
        tx: mpsc::Sender<Message>,
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: SshAgentService,
    ) -> Self {
        Self {
            tx,
            hosts,
            vault,
            transient_credentials,
            ssh_agent,
            sessions: BTreeMap::new(),
            connect_limit: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTS)),
        }
    }

    fn summaries(&self) -> Vec<MetricsSessionSummary> {
        self.sessions
            .values()
            .map(|record| record.summary.clone())
            .collect()
    }

    async fn reconcile(
        &mut self,
        request_id: RequestId,
        sources: Vec<(HostCatalogEntry, MonitoringPolicySummary)>,
    ) -> ActorResult<Vec<MetricsSessionSummary>> {
        for (_, policy) in &sources {
            validate_supported_selection(&policy.policy)
                .map_err(|()| validation_error(request_id.clone()))?;
        }
        let enabled = sources
            .iter()
            .filter(|(_, policy)| policy.policy.enabled)
            .map(|(entry, _)| entry.host.host_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let disabled = self
            .sessions
            .keys()
            .filter(|host_id| !enabled.contains(*host_id))
            .cloned()
            .collect::<Vec<_>>();
        for host_id in disabled {
            if let Some(mut record) = self.sessions.remove(&host_id)
                && !stop_record_and_wait(&mut record).await
            {
                self.sessions.insert(host_id, record);
                return Err(unavailable_error(request_id));
            }
        }
        for (entry, policy) in sources {
            if policy.policy.enabled {
                self.ensure_started(
                    request_id.clone(),
                    entry.host.host_id,
                    entry.host.state_version,
                    policy.policy,
                    false,
                )
                .await?;
            }
        }
        Ok(self.summaries())
    }

    async fn disable_host(&mut self, request_id: RequestId, host_id: &HostId) -> ActorResult<()> {
        if let Some(mut record) = self.sessions.remove(host_id.as_str())
            && !stop_record_and_wait(&mut record).await
        {
            self.sessions.insert(host_id.as_str().to_owned(), record);
            return Err(unavailable_error(request_id));
        }
        Ok(())
    }

    async fn retry_hosts(
        &mut self,
        request_id: RequestId,
        host_ids: Vec<HostId>,
    ) -> ActorResult<Vec<MetricsSessionSummary>> {
        for host_id in host_ids {
            let snapshot = self
                .hosts
                .get_connection_snapshot(&host_id)
                .map_err(|_| unavailable_error(request_id.clone()))?;
            let policy = snapshot.config.monitoring_policy.policy;
            if policy.enabled {
                self.ensure_started(
                    request_id.clone(),
                    host_id,
                    snapshot.host.state_version,
                    policy,
                    true,
                )
                .await?;
            }
        }
        Ok(self.summaries())
    }

    async fn ensure_started(
        &mut self,
        request_id: RequestId,
        host_id: HostId,
        host_state_version: WireSequence,
        monitoring_policy: MonitoringPolicy,
        force: bool,
    ) -> ActorResult<MetricsSessionSummary> {
        if !monitoring_policy.enabled || validate_supported_selection(&monitoring_policy).is_err() {
            return Err(validation_error(request_id));
        }
        let key = host_id.as_str().to_owned();
        if !force
            && self.sessions.get(&key).is_some_and(|record| {
                paused_session_can_be_reused(
                    record,
                    host_state_version,
                    &monitoring_policy,
                    self.vault.is_unlocked(),
                )
            })
        {
            return Ok(self.sessions[&key].summary.clone());
        }
        let profile = resolve_metrics_connection_profile(&self.hosts, &host_id, host_state_version);
        let profile = match profile {
            Ok(profile) => profile,
            Err(error) => {
                let authentication_reason = (error
                    == ConnectionProfileError::CredentialUnavailable)
                    .then_some(MetricsAuthenticationReason::CredentialUnavailable);
                let record = self.sessions.entry(key).or_insert_with(|| {
                    new_record(
                        host_id.clone(),
                        host_state_version,
                        monitoring_policy.clone(),
                    )
                });
                record.host_state_version = host_state_version;
                record.monitoring_policy = monitoring_policy;
                if let Some(reason) = authentication_reason {
                    transition(
                        record,
                        MetricsSessionState::NeedsAuthentication,
                        Some(reason),
                        None,
                    );
                    return Ok(record.summary.clone());
                }
                transition(
                    record,
                    MetricsSessionState::Failed,
                    None,
                    Some(map_profile_failure(error)),
                );
                return Ok(record.summary.clone());
            }
        };
        validate_supported_selection(&profile.monitoring_policy.policy)
            .map_err(|()| validation_error(request_id.clone()))?;
        if !self.vault.is_unlocked() && profile_requires_vault(&profile) {
            let record = self.sessions.entry(key).or_insert_with(|| {
                new_record(host_id, host_state_version, monitoring_policy.clone())
            });
            if !stop_record_and_wait(record).await {
                return Err(unavailable_error(request_id));
            }
            record.host_state_version = host_state_version;
            record.monitoring_policy = monitoring_policy;
            transition(
                record,
                MetricsSessionState::NeedsAuthentication,
                Some(MetricsAuthenticationReason::VaultLocked),
                None,
            );
            return Ok(record.summary.clone());
        }
        if !force
            && let Some(record) = self.sessions.get_mut(&key)
            && record.task.is_some()
            && record.connection_revision_token.as_deref()
                == Some(profile.connection.revision_token.as_str())
        {
            record.monitoring_policy = profile.monitoring_policy.policy.clone();
            record.profile_revision_token = Some(profile.revision_token.clone());
            if let Some(updates) = &record.policy_updates {
                updates.send_replace(profile.monitoring_policy.policy);
            }
            return Ok(record.summary.clone());
        }
        if let Some(record) = self.sessions.get(&key)
            && !force
            && record.profile_revision_token.as_deref() == Some(profile.revision_token.as_str())
            && record.task.is_some()
        {
            return Ok(record.summary.clone());
        }
        self.start_profile(request_id, host_id, host_state_version, profile)
            .await
    }

    async fn start_profile(
        &mut self,
        request_id: RequestId,
        host_id: HostId,
        host_state_version: WireSequence,
        profile: ResolvedMetricsConnectionProfile,
    ) -> ActorResult<MetricsSessionSummary> {
        let key = host_id.as_str().to_owned();
        let record = self.sessions.entry(key.clone()).or_insert_with(|| {
            new_record(
                host_id.clone(),
                host_state_version,
                profile.monitoring_policy.policy.clone(),
            )
        });
        if !stop_record_and_wait(record).await {
            return Err(unavailable_error(request_id));
        }
        record.host_state_version = host_state_version;
        record.monitoring_policy = profile.monitoring_policy.policy.clone();
        record.profile_revision_token = Some(profile.revision_token.clone());
        record.connection_revision_token = Some(profile.connection.revision_token.clone());
        record.has_vault_credentials = profile_has_any_vault_credentials(&profile);
        record.route_stages = route_stage_map(&profile);
        record.summary.generation =
            WireSequence::new(record.summary.generation.get().saturating_add(1).max(1));
        record.summary.host_key_challenge = None;
        record.summary.keyboard_interactive_challenge = None;
        record.active_host_key_challenge = None;
        clear_keyboard_challenge(record, TransportError::AuthenticationRejected);
        transition(record, MetricsSessionState::Connecting, None, None);
        let generation = record.summary.generation.get();
        let (stop, stop_rx) = watch::channel(false);
        let (policy_updates, policy_rx) = watch::channel(record.monitoring_policy.clone());
        record.stop = Some(stop);
        record.policy_updates = Some(policy_updates);

        let worker_tx = self.tx.clone();
        let vault = self.vault.clone();
        let transient_credentials = self.transient_credentials.clone();
        let ssh_agent = self.ssh_agent.clone();
        let connect_limit = self.connect_limit.clone();
        let session_id = record.summary.metrics_session_id.clone();
        let connection = profile.connection;
        let task = tauri::async_runtime::spawn(async move {
            run_metrics_worker(
                MetricsWorkerIdentity {
                    tx: worker_tx,
                    host_key: key,
                    host_id,
                    metrics_session_id: session_id,
                    generation,
                },
                MetricsProbeRuntime {
                    connection,
                    vault,
                    transient_credentials,
                    ssh_agent,
                    connect_limit,
                },
                policy_rx,
                stop_rx,
            )
            .await;
        });
        record.task = Some(ConnectTask {
            generation,
            handle: task,
        });
        Ok(record.summary.clone())
    }

    async fn cancel_keyboard_challenge(
        &mut self,
        request: MetricsKeyboardInteractiveRespondRequest,
    ) -> ActorResult<()> {
        let record = self
            .sessions
            .get(request.host_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        let current = record
            .active_keyboard_challenge
            .as_ref()
            .map(|active| &active.challenge);
        if record.summary.metrics_session_id != request.metrics_session_id
            || record.summary.generation != request.expected_generation
            || current.is_none_or(|challenge| {
                challenge.challenge_id != request.challenge_id
                    || challenge.generation != request.expected_generation
                    || challenge.round_index != request.round_index
            })
        {
            return Err(conflict_error(request.meta.request_id));
        }
        // The actor cannot process another response between this exact challenge check and stop.
        self.stop(MetricsStopRequest {
            meta: request.meta,
            host_id: request.host_id,
            expected_generation: Some(request.expected_generation),
        })
        .await?;
        Ok(())
    }

    async fn stop(&mut self, request: MetricsStopRequest) -> ActorResult<MetricsSessionSummary> {
        let record = self
            .sessions
            .get_mut(request.host_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        if request
            .expected_generation
            .is_some_and(|generation| generation != record.summary.generation)
        {
            return Err(conflict_error(request.meta.request_id));
        }
        transition(record, MetricsSessionState::Disconnecting, None, None);
        if !stop_record_and_wait(record).await {
            transition(
                record,
                MetricsSessionState::Failed,
                None,
                Some(MetricsSessionFailureCode::ConnectionUnavailable),
            );
            return Err(unavailable_error(request.meta.request_id));
        }
        record.active_host_key_challenge = None;
        record.summary.host_key_challenge = None;
        transition(record, MetricsSessionState::Closed, None, None);
        Ok(record.summary.clone())
    }

    async fn retry(&mut self, request: MetricsRetryRequest) -> ActorResult<MetricsSessionSummary> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        if let Some(record) = self.sessions.get(request.host_id.as_str())
            && request
                .expected_generation
                .is_some_and(|generation| generation != record.summary.generation)
        {
            return Err(conflict_error(request.meta.request_id));
        }
        let snapshot = self
            .hosts
            .get_connection_snapshot(&request.host_id)
            .map_err(|_| unavailable_error(request.meta.request_id.clone()))?;
        self.ensure_started(
            request.meta.request_id,
            request.host_id,
            snapshot.host.state_version,
            snapshot.config.monitoring_policy.policy,
            true,
        )
        .await
    }

    async fn decide_host_key(
        &mut self,
        request: MetricsHostKeyDecisionRequest,
    ) -> ActorResult<MetricsSessionSummary> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let record = self
            .sessions
            .get_mut(request.host_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        if record.summary.metrics_session_id != request.metrics_session_id
            || record.summary.generation != request.expected_generation
        {
            return Err(conflict_error(request.meta.request_id));
        }
        let active = record
            .active_host_key_challenge
            .take()
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        if active.challenge.challenge_id != request.challenge_id {
            record.active_host_key_challenge = Some(active);
            return Err(conflict_error(request.meta.request_id));
        }
        record.summary.host_key_challenge = None;
        if request.decision == MetricsHostKeyDecision::Reject {
            transition(
                record,
                MetricsSessionState::Failed,
                None,
                Some(MetricsSessionFailureCode::ConnectionUnavailable),
            );
            return Ok(record.summary.clone());
        }
        self.hosts
            .trust_known_host(
                active.endpoint.normalized_address(),
                active.endpoint.port(),
                &active.observed.algorithm,
                &active.observed.public_key_blob,
            )
            .map_err(|_| conflict_error(request.meta.request_id.clone()))?;
        let host_id = record.summary.host_id.clone();
        let host_state_version = record.host_state_version;
        let policy = record.monitoring_policy.clone();
        self.ensure_started(
            request.meta.request_id,
            host_id,
            host_state_version,
            policy,
            true,
        )
        .await
    }

    fn prepare_answer(
        &mut self,
        mut request: MetricsKeyboardInteractiveAnswerPrepareRequest,
    ) -> ActorResult<MetricsKeyboardInteractiveAnswerPrepareResponse> {
        if request.value.len() > KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES {
            return Err(validation_error(request.meta.request_id));
        }
        let now = unix_time_ms();
        let record = self
            .sessions
            .values_mut()
            .find(|record| record.summary.metrics_session_id == request.metrics_session_id)
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        let challenge = record
            .active_keyboard_challenge
            .as_ref()
            .map(|active| &active.challenge)
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        if record.summary.generation != request.expected_generation
            || challenge.challenge_id != request.challenge_id
            || challenge.round_index != request.round_index
            || challenge.expires_at_unix_ms <= now
            || challenge
                .prompts
                .get(usize::from(request.prompt_index))
                .is_none_or(|prompt| prompt.prompt_index != request.prompt_index)
            || record
                .prepared_answers
                .contains_key(request.answer_ref_id.as_str())
        {
            return Err(conflict_error(request.meta.request_id));
        }
        record.prepared_answers.insert(
            request.answer_ref_id.as_str().to_owned(),
            PreparedAnswer {
                challenge_id: request.challenge_id,
                generation: request.expected_generation,
                round_index: request.round_index,
                prompt_index: request.prompt_index,
                expires_at_unix_ms: challenge.expires_at_unix_ms,
                value: Zeroizing::new(std::mem::take(&mut request.value)),
            },
        );
        Ok(MetricsKeyboardInteractiveAnswerPrepareResponse {
            answer_ref_id: request.answer_ref_id,
            expires_at_unix_ms: challenge.expires_at_unix_ms,
        })
    }

    fn respond_keyboard_interactive(
        &mut self,
        request: MetricsKeyboardInteractiveRespondRequest,
    ) -> ActorResult<MetricsSessionSummary> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let now = unix_time_ms();
        let record = self
            .sessions
            .get_mut(request.host_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        if record.summary.metrics_session_id != request.metrics_session_id
            || record.summary.generation != request.expected_generation
        {
            return Err(conflict_error(request.meta.request_id));
        }
        let active = record
            .active_keyboard_challenge
            .as_ref()
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        let challenge = &active.challenge;
        if challenge.challenge_id != request.challenge_id
            || challenge.round_index != request.round_index
            || challenge.expires_at_unix_ms <= now
            || request.answer_ref_ids.len() != challenge.prompts.len()
        {
            return Err(conflict_error(request.meta.request_id));
        }
        let mut unique = BTreeSet::new();
        for (prompt_index, answer_ref_id) in request.answer_ref_ids.iter().enumerate() {
            if !unique.insert(answer_ref_id.as_str()) {
                return Err(conflict_error(request.meta.request_id.clone()));
            }
            let Some(answer) = record.prepared_answers.get(answer_ref_id.as_str()) else {
                return Err(conflict_error(request.meta.request_id.clone()));
            };
            if answer.challenge_id != request.challenge_id
                || answer.generation != request.expected_generation
                || answer.round_index != request.round_index
                || answer.prompt_index != u8::try_from(prompt_index).unwrap_or(u8::MAX)
                || answer.expires_at_unix_ms <= now
            {
                return Err(conflict_error(request.meta.request_id.clone()));
            }
        }
        let answers = request
            .answer_ref_ids
            .iter()
            .map(|reference| {
                record
                    .prepared_answers
                    .remove(reference.as_str())
                    .expect("prepared answer was validated")
                    .value
            })
            .collect::<Vec<_>>();
        let active = record
            .active_keyboard_challenge
            .take()
            .expect("keyboard challenge was validated");
        record.prepared_answers.clear();
        record.summary.keyboard_interactive_challenge = None;
        transition(record, MetricsSessionState::Authenticating, None, None);
        let _ = active.answers.send(Ok(answers));
        Ok(record.summary.clone())
    }

    fn phase_changed(&mut self, host_id: &str, generation: u64, phase: ConnectionPhase) {
        let Some(record) = self.sessions.get_mut(host_id) else {
            return;
        };
        if !is_current(record, generation) {
            return;
        }
        if record
            .summary
            .latest_snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.stale)
        {
            return;
        }
        let state = match phase {
            ConnectionPhase::Connecting => MetricsSessionState::Connecting,
            ConnectionPhase::Authenticating => MetricsSessionState::Authenticating,
        };
        transition(record, state, None, None);
    }

    fn host_key_observed(
        &mut self,
        host_id: &str,
        generation: u64,
        endpoint: Endpoint,
        observed: ObservedHostKey,
        reply: oneshot::Sender<Result<HostKeyDecision, TransportError>>,
    ) {
        let Some(record) = self.sessions.get_mut(host_id) else {
            let _ = reply.send(Err(TransportError::HostKeyVerificationFailed));
            return;
        };
        if !is_current(record, generation) {
            let _ = reply.send(Err(TransportError::HostKeyVerificationFailed));
            return;
        }
        if !record
            .summary
            .latest_snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.stale)
        {
            transition(record, MetricsSessionState::VerifyingHostKey, None, None);
        }
        let observation = self.hosts.observe_known_host(
            endpoint.normalized_address(),
            endpoint.port(),
            &observed.algorithm,
            &observed.public_key_blob,
        );
        match observation {
            Ok(KnownHostObservation::Trusted(_)) => {
                let result = self.hosts.record_known_host_verified(
                    endpoint.normalized_address(),
                    endpoint.port(),
                    &observed.algorithm,
                    &observed.public_key_blob,
                );
                let _ = reply.send(if result.is_ok() {
                    Ok(HostKeyDecision::Trusted)
                } else {
                    Err(TransportError::HostKeyVerificationFailed)
                });
            }
            Ok(KnownHostObservation::Unknown(_)) => {
                let route_stage = record
                    .route_stages
                    .get(&endpoint_key(&endpoint))
                    .cloned()
                    .unwrap_or(SshSessionRouteStage::Target);
                let challenge = MetricsHostKeyChallenge {
                    challenge_id: MetricsHostKeyChallengeId::new(),
                    metrics_session_id: record.summary.metrics_session_id.clone(),
                    host_id: record.summary.host_id.clone(),
                    generation: record.summary.generation,
                    route_stage,
                    algorithm: observed.algorithm.clone(),
                    fingerprint_sha256: observed.fingerprint_sha256.clone(),
                    trusted_fingerprint_sha256: None,
                };
                record.active_host_key_challenge = Some(ActiveHostKeyChallenge {
                    challenge: challenge.clone(),
                    endpoint,
                    observed,
                });
                record.summary.host_key_challenge = Some(challenge);
                transition(record, MetricsSessionState::NeedsHostKeyReview, None, None);
                // A background reconcile never keeps an unauthenticated socket open.
                let _ = reply.send(Ok(HostKeyDecision::Rejected));
            }
            Ok(KnownHostObservation::Mismatch { trusted, .. })
            | Ok(KnownHostObservation::AlgorithmChanged { trusted, .. }) => {
                transition(
                    record,
                    MetricsSessionState::Failed,
                    None,
                    Some(MetricsSessionFailureCode::HostKeyMismatch),
                );
                let _ = reply.send(Ok(HostKeyDecision::Mismatch {
                    trusted_fingerprint: trusted.fingerprint_sha256,
                }));
            }
            Err(_) => {
                let _ = reply.send(Err(TransportError::HostKeyVerificationFailed));
            }
        }
    }

    fn keyboard_challenge_observed(
        &mut self,
        host_id: &str,
        generation: u64,
        request: KeyboardInteractiveRequest,
        reply: oneshot::Sender<Result<Vec<Zeroizing<String>>, TransportError>>,
    ) {
        let Some(record) = self.sessions.get_mut(host_id) else {
            let _ = reply.send(Err(TransportError::AuthenticationRejected));
            return;
        };
        if !is_current(record, generation) || record.active_keyboard_challenge.is_some() {
            let _ = reply.send(Err(TransportError::AuthenticationRejected));
            return;
        }
        record.prepared_answers.clear();
        let expires_at_unix_ms = unix_time_ms().saturating_add(
            i64::try_from(KEYBOARD_INTERACTIVE_ROUND_TIMEOUT.as_millis()).unwrap_or(i64::MAX),
        );
        let challenge = MetricsKeyboardInteractiveChallenge {
            challenge_id: MetricsKeyboardInteractiveChallengeId::new(),
            metrics_session_id: record.summary.metrics_session_id.clone(),
            host_id: record.summary.host_id.clone(),
            generation: record.summary.generation,
            route_stage: connection_route_stage_to_wire(request.route_stage),
            credential_ref_id: request.credential_ref_id,
            attempt_index: request.attempt_index,
            round_index: request.round_index,
            name: request.challenge.name,
            instruction: request.challenge.instructions,
            prompts: request
                .challenge
                .prompts
                .into_iter()
                .enumerate()
                .map(|(index, prompt)| MetricsKeyboardInteractivePrompt {
                    prompt_index: u8::try_from(index).unwrap_or(u8::MAX),
                    label: prompt.text,
                    echo: prompt.echo,
                })
                .collect(),
            expires_at_unix_ms,
        };
        record.summary.keyboard_interactive_challenge = Some(challenge.clone());
        record.active_keyboard_challenge = Some(ActiveKeyboardChallenge {
            challenge,
            answers: reply,
        });
        transition(
            record,
            MetricsSessionState::NeedsAuthentication,
            Some(MetricsAuthenticationReason::KeyboardInteractive),
            None,
        );
    }

    fn worker_failed(&mut self, host_id: &str, generation: u64, failure: WorkerFailure) {
        let Some(record) = self.sessions.get_mut(host_id) else {
            return;
        };
        if !is_current(record, generation) {
            return;
        }
        mark_snapshot_stale(record);
        if let Some(task) = record.task.take() {
            task.handle.abort();
        }
        record.stop = None;
        if record.active_host_key_challenge.is_some()
            && failure.code == MetricsSessionFailureCode::ConnectionUnavailable
        {
            transition(record, MetricsSessionState::NeedsHostKeyReview, None, None);
            return;
        }
        clear_keyboard_challenge(record, TransportError::AuthenticationRejected);
        if failure.code == MetricsSessionFailureCode::AuthenticationRejected {
            let reason = if record.has_vault_credentials && !self.vault.is_unlocked() {
                MetricsAuthenticationReason::VaultLocked
            } else {
                MetricsAuthenticationReason::CredentialUnavailable
            };
            transition(
                record,
                MetricsSessionState::NeedsAuthentication,
                Some(reason),
                Some(failure.code),
            );
            return;
        }
        if !failure.recoverable {
            transition(
                record,
                MetricsSessionState::Failed,
                None,
                Some(failure.code),
            );
            return;
        }
        record.consecutive_failures = record.consecutive_failures.saturating_add(1);
        let delay = backoff_delay(
            record.consecutive_failures,
            record.monitoring_policy.sample_interval_seconds,
        );
        let next_retry_at_unix_ms =
            unix_time_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
        transition(
            record,
            MetricsSessionState::Backoff,
            None,
            Some(failure.code),
        );
        record.summary.next_retry_at_unix_ms = Some(next_retry_at_unix_ms);
        let tx = self.tx.clone();
        let host_id = host_id.to_owned();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx
                .send(Message::RetryDue {
                    host_id,
                    generation,
                })
                .await;
        });
    }

    async fn retry_due(&mut self, host_id: &str, generation: u64) {
        let Some(record) = self.sessions.get(host_id) else {
            return;
        };
        if !generation_matches(record, generation)
            || record.task.is_some()
            || record.summary.state != MetricsSessionState::Backoff
        {
            return;
        }
        let host_id = record.summary.host_id.clone();
        let version = record.host_state_version;
        let policy = record.monitoring_policy.clone();
        if policy.enabled {
            let _ = self
                .ensure_started(RequestId::new(), host_id, version, policy, true)
                .await;
        }
    }

    fn sample_ready(&mut self, host_id: &str, generation: u64, snapshot: MetricSnapshot) {
        let Some(record) = self.sessions.get_mut(host_id) else {
            return;
        };
        if !is_current(record, generation)
            || snapshot.generation != record.summary.generation
            || snapshot.metrics_session_id != record.summary.metrics_session_id
        {
            return;
        }
        let previous_sequence = record
            .summary
            .latest_snapshot
            .as_ref()
            .filter(|value| {
                value.generation == snapshot.generation
                    && value.metrics_session_id == snapshot.metrics_session_id
            })
            .map(|value| value.sample_sequence.get())
            .unwrap_or(0);
        if snapshot.sample_sequence.get() <= previous_sequence {
            return;
        }
        record.summary.latest_snapshot = Some(snapshot);
        record.consecutive_failures = 0;
        transition(record, MetricsSessionState::Ready, None, None);
    }

    async fn shutdown_all(&mut self, request_id: RequestId) -> ActorResult<()> {
        let tasks = self
            .sessions
            .iter_mut()
            .filter_map(|(host_id, record)| {
                take_record_task(record).map(|task| (host_id.clone(), task))
            })
            .collect::<Vec<_>>();
        let deadline = tokio::time::Instant::now() + WORKER_STOP_TIMEOUT;
        let mut failed_hosts = BTreeSet::new();
        for (host_id, task) in tasks {
            if !wait_record_task_until(task, deadline).await {
                failed_hosts.insert(host_id);
            }
        }
        for (host_id, record) in &mut self.sessions {
            if failed_hosts.contains(host_id) {
                transition(
                    record,
                    MetricsSessionState::Failed,
                    None,
                    Some(MetricsSessionFailureCode::ConnectionUnavailable),
                );
            } else {
                transition(record, MetricsSessionState::Closed, None, None);
            }
        }
        if failed_hosts.is_empty() {
            Ok(())
        } else {
            Err(unavailable_error(request_id))
        }
    }

    async fn handle(&mut self, message: Message) {
        match message {
            Message::Snapshot { reply } => {
                let _ = reply.send(Ok(self.summaries()));
            }
            Message::Reconcile {
                request_id,
                sources,
                reply,
            } => {
                let _ = reply.send(self.reconcile(request_id, sources).await);
            }
            Message::DisableHost {
                request_id,
                host_id,
                reply,
            } => {
                let _ = reply.send(self.disable_host(request_id, &host_id).await);
            }
            Message::RetryHosts {
                request_id,
                host_ids,
                reply,
            } => {
                let _ = reply.send(self.retry_hosts(request_id, host_ids).await);
            }
            Message::Retry { request, reply } => {
                let _ = reply.send(self.retry(request).await);
            }
            Message::Stop { request, reply } => {
                let _ = reply.send(self.stop(request).await);
            }
            Message::HostKeyDecide { request, reply } => {
                let _ = reply.send(self.decide_host_key(request).await);
            }
            Message::KeyboardAnswerPrepare { request, reply } => {
                let _ = reply.send(self.prepare_answer(request));
            }
            Message::KeyboardRespond { request, reply } => {
                let _ = reply.send(self.respond_keyboard_interactive(request));
            }
            Message::KeyboardCancel { request, reply } => {
                let _ = reply.send(self.cancel_keyboard_challenge(request).await);
            }
            Message::ShutdownAll { request_id, reply } => {
                let _ = reply.send(self.shutdown_all(request_id).await);
            }
            Message::PhaseChanged {
                host_id,
                generation,
                phase,
            } => self.phase_changed(&host_id, generation, phase),
            Message::HostKeyObserved {
                host_id,
                generation,
                endpoint,
                observed,
                reply,
            } => self.host_key_observed(&host_id, generation, endpoint, observed, reply),
            Message::KeyboardChallengeObserved {
                host_id,
                generation,
                request,
                reply,
            } => self.keyboard_challenge_observed(&host_id, generation, request, reply),
            Message::WorkerState {
                host_id,
                generation,
                state,
            } => {
                if let Some(record) = self.sessions.get_mut(&host_id)
                    && is_current(record, generation)
                    && !record
                        .summary
                        .latest_snapshot
                        .as_ref()
                        .is_some_and(|snapshot| !snapshot.stale)
                {
                    transition(record, state, None, None);
                }
            }
            Message::SampleReady {
                host_id,
                generation,
                snapshot,
            } => self.sample_ready(&host_id, generation, snapshot),
            Message::WorkerFailed {
                host_id,
                generation,
                failure,
            } => self.worker_failed(&host_id, generation, failure),
            Message::RetryDue {
                host_id,
                generation,
            } => self.retry_due(&host_id, generation).await,
        }
    }
}

impl Drop for Actor {
    fn drop(&mut self) {
        for record in self.sessions.values_mut() {
            abort_record_now(record);
        }
    }
}

async fn run_actor(mut actor: Actor, mut rx: mpsc::Receiver<Message>) {
    while let Some(message) = rx.recv().await {
        actor.handle(message).await;
    }
}

#[derive(Clone)]
struct ActorMetricsHostKeyVerifier {
    tx: mpsc::Sender<Message>,
    host_id: String,
    generation: u64,
}

impl HostKeyVerifier for ActorMetricsHostKeyVerifier {
    fn verify(&self, endpoint: Endpoint, observed: ObservedHostKey) -> VerifyFuture {
        let tx = self.tx.clone();
        let host_id = self.host_id.clone();
        let generation = self.generation;
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            tx.send(Message::HostKeyObserved {
                host_id,
                generation,
                endpoint,
                observed,
                reply,
            })
            .await
            .map_err(|_| TransportError::HostKeyVerificationFailed)?;
            response
                .await
                .map_err(|_| TransportError::HostKeyVerificationFailed)?
        })
    }
}

struct ActorMetricsInteraction {
    tx: mpsc::Sender<Message>,
    host_id: String,
    generation: u64,
}

impl SshConnectionInteraction for ActorMetricsInteraction {
    async fn phase_changed(
        &mut self,
        phase: ConnectionPhase,
        _route_stage: ConnectionRouteStage,
    ) -> Result<(), TransportError> {
        self.tx
            .send(Message::PhaseChanged {
                host_id: self.host_id.clone(),
                generation: self.generation,
                phase,
            })
            .await
            .map_err(|_| TransportError::ConnectionLost)
    }

    async fn answer_keyboard_interactive(
        &mut self,
        request: KeyboardInteractiveRequest,
    ) -> Result<Vec<Zeroizing<String>>, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(Message::KeyboardChallengeObserved {
                host_id: self.host_id.clone(),
                generation: self.generation,
                request,
                reply,
            })
            .await
            .map_err(|_| TransportError::ConnectionLost)?;
        tokio::time::timeout(KEYBOARD_INTERACTIVE_ROUND_TIMEOUT, response)
            .await
            .map_err(|_| TransportError::AuthenticationTimeout)?
            .map_err(|_| TransportError::ConnectionLost)?
    }
}

struct MetricsWorkerIdentity {
    tx: mpsc::Sender<Message>,
    host_key: String,
    host_id: HostId,
    metrics_session_id: MetricsSessionId,
    generation: u64,
}

async fn run_metrics_worker(
    identity: MetricsWorkerIdentity,
    runtime: MetricsProbeRuntime,
    policy_updates: watch::Receiver<MonitoringPolicy>,
    mut stop: watch::Receiver<bool>,
) {
    let started = tokio::time::Instant::now();
    let mut provider = LinuxMetricsProvider::new();
    let mut sample_sequence = 0_u64;
    loop {
        let cycle_started = tokio::time::Instant::now();
        let policy = policy_updates.borrow().clone();
        let sample_started_at_unix_ms = unix_time_ms();
        let sample = match run_metrics_probe(
            &identity,
            &runtime,
            &policy,
            &mut provider,
            started,
            &mut stop,
        )
        .await
        {
            MetricsProbeOutcome::Sample(sample) => sample,
            MetricsProbeOutcome::Stopped => return,
            MetricsProbeOutcome::Failed(failure) => {
                let _ = identity
                    .tx
                    .send(Message::WorkerFailed {
                        host_id: identity.host_key,
                        generation: identity.generation,
                        failure,
                    })
                    .await;
                return;
            }
        };
        sample_sequence = sample_sequence.saturating_add(1);
        let snapshot = metric_snapshot_from(
            identity.host_id.clone(),
            identity.metrics_session_id.clone(),
            identity.generation,
            sample_sequence,
            sample_started_at_unix_ms,
            unix_time_ms(),
            sample,
        );
        if identity
            .tx
            .send(Message::SampleReady {
                host_id: identity.host_key.clone(),
                generation: identity.generation,
                snapshot,
            })
            .await
            .is_err()
        {
            return;
        }
        let interval = Duration::from_secs(u64::from(policy.sample_interval_seconds));
        let next_due = cycle_started + interval;
        tokio::select! {
            biased;
            _ = wait_for_stop(&mut stop) => return,
            _ = tokio::time::sleep_until(next_due) => {}
        }
    }
}

fn metric_snapshot_from(
    host_id: HostId,
    metrics_session_id: MetricsSessionId,
    generation: u64,
    sample_sequence: u64,
    sample_started_at_unix_ms: i64,
    sample_completed_at_unix_ms: i64,
    sample: LinuxMetricSample,
) -> MetricSnapshot {
    MetricSnapshot {
        host_id,
        metrics_session_id,
        generation: WireSequence::new(generation),
        sample_sequence: WireSequence::new(sample_sequence),
        provider_id: sample.provider_id.to_owned(),
        provider_version: sample.provider_version,
        platform: MetricsPlatform::Linux,
        sample_started_at_unix_ms,
        sample_completed_at_unix_ms,
        stale: false,
        cpu: match sample.cpu {
            MetricValue::Available(value) => CpuMetric {
                state: MetricFieldState::Available,
                basis_points: Some(value.basis_points),
            },
            MetricValue::Unavailable(reason) => CpuMetric {
                state: unavailable_state(reason),
                basis_points: None,
            },
        },
        memory: norishell_core_api::MemoryMetric {
            state: MetricFieldState::Available,
            used_bytes: Some(MetricByteCount::new(sample.memory.used_bytes)),
            available_bytes: Some(MetricByteCount::new(sample.memory.available_bytes)),
            total_bytes: Some(MetricByteCount::new(sample.memory.total_bytes)),
        },
        network: match sample.network {
            MetricValue::Available(value) => norishell_core_api::NetworkMetric {
                resource_id: NetworkResourceId::AggregateNonLoopback,
                state: MetricFieldState::Available,
                receive_bytes_per_second: Some(MetricByteCount::new(
                    value.receive_bytes_per_second,
                )),
                transmit_bytes_per_second: Some(MetricByteCount::new(
                    value.transmit_bytes_per_second,
                )),
            },
            MetricValue::Unavailable(reason) => norishell_core_api::NetworkMetric {
                resource_id: NetworkResourceId::AggregateNonLoopback,
                state: unavailable_state(reason),
                receive_bytes_per_second: None,
                transmit_bytes_per_second: None,
            },
        },
        disks: vec![DiskMetric {
            resource_id: DiskResourceId::Root,
            state: MetricFieldState::Available,
            filesystem_id: sample.root_disk.filesystem_id,
            mount: sample.root_disk.mount,
            used_bytes: Some(MetricByteCount::new(sample.root_disk.used_bytes)),
            available_bytes: Some(MetricByteCount::new(sample.root_disk.available_bytes)),
            total_bytes: Some(MetricByteCount::new(sample.root_disk.total_bytes)),
        }],
    }
}

fn unavailable_state(reason: MetricUnavailableReason) -> MetricFieldState {
    match reason {
        MetricUnavailableReason::InitialBaseline => MetricFieldState::InitialBaseline,
        MetricUnavailableReason::CounterReset => MetricFieldState::CounterReset,
        MetricUnavailableReason::CounterSetChanged => MetricFieldState::CounterSetChanged,
        MetricUnavailableReason::NoCounterProgress => MetricFieldState::NoCounterProgress,
    }
}

fn validate_supported_selection(policy: &MonitoringPolicy) -> Result<(), ()> {
    let bounds_supported = (5..=300).contains(&policy.sample_interval_seconds)
        && (2..=30).contains(&policy.sample_timeout_seconds)
        && policy.sample_timeout_seconds < policy.sample_interval_seconds;
    let disk_supported = policy.disk_mount_ids.as_slice() == [DiskResourceId::Root];
    let network_supported =
        policy.network_interface_ids.as_slice() == [NetworkResourceId::AggregateNonLoopback];
    (bounds_supported && disk_supported && network_supported)
        .then_some(())
        .ok_or(())
}

fn profile_requires_vault(profile: &ResolvedMetricsConnectionProfile) -> bool {
    connection_requires_vault(&profile.connection)
}

fn profile_has_any_vault_credentials(profile: &ResolvedMetricsConnectionProfile) -> bool {
    connection_has_vault_credentials(&profile.connection)
}

fn map_exec_failure(error: TransportError) -> WorkerFailure {
    match error {
        TransportError::RemoteExecTimeout => WorkerFailure {
            code: MetricsSessionFailureCode::SampleTimedOut,
            recoverable: true,
        },
        TransportError::RemoteExecOutputTooLarge => WorkerFailure {
            code: MetricsSessionFailureCode::OutputLimitExceeded,
            recoverable: true,
        },
        TransportError::ConnectionLost | TransportError::Protocol => WorkerFailure {
            code: MetricsSessionFailureCode::TransportLost,
            recoverable: true,
        },
        _ => WorkerFailure {
            code: MetricsSessionFailureCode::ConnectionUnavailable,
            recoverable: true,
        },
    }
}

fn map_parse_failure(_error: ParseError) -> WorkerFailure {
    WorkerFailure {
        code: MetricsSessionFailureCode::MalformedOutput,
        recoverable: true,
    }
}

fn map_connection_failure(error: TransportError) -> WorkerFailure {
    match error {
        TransportError::HostKeyMismatch { .. } => WorkerFailure {
            code: MetricsSessionFailureCode::HostKeyMismatch,
            recoverable: false,
        },
        TransportError::HostKeyRejected => WorkerFailure {
            code: MetricsSessionFailureCode::ConnectionUnavailable,
            recoverable: false,
        },
        TransportError::AuthenticationRejected
        | TransportError::AuthenticationIncomplete
        | TransportError::InvalidKeyboardInteractiveResponse
        | TransportError::AuthenticationTimeout
        | TransportError::InvalidPrivateKey
        | TransportError::SshAgentKeyUnavailable
        | TransportError::SshAgentUnavailable
        | TransportError::InsecureRsaSignatureOnly => WorkerFailure {
            code: MetricsSessionFailureCode::AuthenticationRejected,
            recoverable: false,
        },
        _ => WorkerFailure {
            code: MetricsSessionFailureCode::ConnectionUnavailable,
            recoverable: true,
        },
    }
}

fn map_profile_failure(error: ConnectionProfileError) -> MetricsSessionFailureCode {
    match error {
        ConnectionProfileError::CredentialUnavailable => {
            MetricsSessionFailureCode::AuthenticationRejected
        }
        ConnectionProfileError::InvalidTarget
        | ConnectionProfileError::UnsupportedConfiguration => {
            MetricsSessionFailureCode::ProviderUnsupported
        }
        ConnectionProfileError::StaleHost | ConnectionProfileError::PersistenceUnavailable => {
            MetricsSessionFailureCode::ConnectionUnavailable
        }
    }
}

fn new_record(
    host_id: HostId,
    host_state_version: WireSequence,
    monitoring_policy: MonitoringPolicy,
) -> SessionRecord {
    SessionRecord {
        summary: MetricsSessionSummary {
            metrics_session_id: MetricsSessionId::new(),
            host_id,
            generation: WireSequence::new(0),
            state_revision: WireSequence::new(0),
            state: MetricsSessionState::Idle,
            authentication_reason: None,
            failure_code: None,
            next_retry_at_unix_ms: None,
            latest_snapshot: None,
            host_key_challenge: None,
            keyboard_interactive_challenge: None,
        },
        host_state_version,
        profile_revision_token: None,
        connection_revision_token: None,
        has_vault_credentials: false,
        monitoring_policy,
        route_stages: BTreeMap::new(),
        task: None,
        stop: None,
        policy_updates: None,
        active_host_key_challenge: None,
        active_keyboard_challenge: None,
        prepared_answers: BTreeMap::new(),
        consecutive_failures: 0,
    }
}

fn transition(
    record: &mut SessionRecord,
    state: MetricsSessionState,
    authentication_reason: Option<MetricsAuthenticationReason>,
    failure_code: Option<MetricsSessionFailureCode>,
) {
    record.summary.state = state;
    record.summary.authentication_reason = authentication_reason;
    record.summary.failure_code = failure_code;
    record.summary.next_retry_at_unix_ms = None;
    record.summary.state_revision =
        WireSequence::new(record.summary.state_revision.get().saturating_add(1));
}

fn take_record_task(record: &mut SessionRecord) -> Option<ConnectTask> {
    if let Some(stop) = record.stop.take() {
        let _ = stop.send(true);
    }
    let task = record.task.take();
    record.policy_updates = None;
    clear_keyboard_challenge(record, TransportError::AuthenticationRejected);
    record.prepared_answers.clear();
    task
}

async fn stop_record_and_wait(record: &mut SessionRecord) -> bool {
    let Some(task) = take_record_task(record) else {
        return true;
    };
    wait_record_task_until(task, tokio::time::Instant::now() + WORKER_STOP_TIMEOUT).await
}

async fn wait_record_task_until(task: ConnectTask, deadline: tokio::time::Instant) -> bool {
    let mut handle = task.handle;
    match tokio::time::timeout_at(deadline, &mut handle).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            handle.abort();
            let _ = handle.await;
            false
        }
    }
}

fn abort_record_now(record: &mut SessionRecord) {
    if let Some(task) = take_record_task(record) {
        task.handle.abort();
    }
}

fn clear_keyboard_challenge(record: &mut SessionRecord, error: TransportError) {
    if let Some(active) = record.active_keyboard_challenge.take() {
        let _ = active.answers.send(Err(error));
    }
    record.summary.keyboard_interactive_challenge = None;
    record.prepared_answers.clear();
}

fn is_current(record: &SessionRecord, generation: u64) -> bool {
    generation_matches(record, generation)
        && record
            .task
            .as_ref()
            .is_some_and(|task| task.generation == generation)
}

fn generation_matches(record: &SessionRecord, generation: u64) -> bool {
    record.summary.generation.get() == generation
}

fn paused_session_can_be_reused(
    record: &SessionRecord,
    host_state_version: WireSequence,
    monitoring_policy: &MonitoringPolicy,
    vault_is_unlocked: bool,
) -> bool {
    record.host_state_version == host_state_version
        && &record.monitoring_policy == monitoring_policy
        && record.task.is_none()
        && !(vault_is_unlocked
            && record.summary.state == MetricsSessionState::NeedsAuthentication
            && record.summary.authentication_reason
                == Some(MetricsAuthenticationReason::VaultLocked))
        && matches!(
            record.summary.state,
            MetricsSessionState::NeedsHostKeyReview
                | MetricsSessionState::NeedsAuthentication
                | MetricsSessionState::Backoff
        )
}

fn mark_snapshot_stale(record: &mut SessionRecord) {
    if let Some(snapshot) = record.summary.latest_snapshot.as_mut() {
        snapshot.stale = true;
    }
}

fn backoff_delay(consecutive_failures: u32, sample_interval_seconds: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(8);
    let base = u64::from(sample_interval_seconds).max(5);
    Duration::from_secs(
        base.saturating_mul(1_u64 << exponent)
            .min(MAX_BACKOFF_SECONDS),
    )
}

fn route_stage_map(
    profile: &ResolvedMetricsConnectionProfile,
) -> BTreeMap<String, SshSessionRouteStage> {
    let mut stages = BTreeMap::new();
    for (index, jump) in profile.connection.jump_hosts.iter().enumerate() {
        if let Ok(endpoint) = Endpoint::parse(&jump.endpoint.address, jump.endpoint.port) {
            stages.insert(
                endpoint_key(&endpoint),
                SshSessionRouteStage::JumpHost {
                    hop_index: u8::try_from(index).unwrap_or(u8::MAX),
                    host_id: jump.host_id.clone(),
                    endpoint: jump.endpoint.clone(),
                },
            );
        }
    }
    if let Ok(endpoint) = Endpoint::parse(
        &profile.connection.endpoint.address,
        profile.connection.endpoint.port,
    ) {
        stages.insert(endpoint_key(&endpoint), SshSessionRouteStage::Target);
    }
    stages
}

fn endpoint_key(endpoint: &Endpoint) -> String {
    format!("{}:{}", endpoint.normalized_address(), endpoint.port())
}

fn connection_route_stage_to_wire(stage: ConnectionRouteStage) -> SshSessionRouteStage {
    match stage {
        ConnectionRouteStage::Ingress => SshSessionRouteStage::Ingress,
        ConnectionRouteStage::JumpHost {
            hop_index,
            host_id,
            endpoint,
        } => SshSessionRouteStage::JumpHost {
            hop_index,
            host_id,
            endpoint,
        },
        ConnectionRouteStage::Target => SshSessionRouteStage::Target,
    }
}

async fn wait_for_stop(stop: &mut watch::Receiver<bool>) {
    if *stop.borrow() {
        return;
    }
    while stop.changed().await.is_ok() {
        if *stop.borrow() {
            return;
        }
    }
}

async fn send_worker_state(
    tx: &mpsc::Sender<Message>,
    host_id: &str,
    generation: u64,
    state: MetricsSessionState,
) -> Result<(), ()> {
    tx.send(Message::WorkerState {
        host_id: host_id.to_owned(),
        generation,
        state,
    })
    .await
    .map_err(|_| ())
}

fn validation_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "metrics.invalid_request",
        ErrorCategory::Validation,
        RetryStrategy::Never,
        "errors.metrics.invalidRequest",
    )
}

fn conflict_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "metrics.stale_fence",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.metrics.staleFence",
    )
}

fn unavailable_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "metrics.unavailable",
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.metrics.unavailable",
    )
}

fn not_found_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "metrics.not_found",
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.metrics.notFound",
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use norishell_core_api::{
        CpuMetric, DiskResourceId, HostId, MetricByteCount, MetricFieldState, MetricSnapshot,
        MetricsKeyboardInteractiveAnswerPrepareRequest, MetricsKeyboardInteractiveAnswerRefId,
        MetricsKeyboardInteractiveChallengeId, MetricsKeyboardInteractiveRespondRequest,
        MetricsPlatform, MetricsSessionFailureCode, MetricsSessionState, MetricsStopRequest,
        MonitoringPolicy, NetworkResourceId, RequestId, RequestMeta, SshSessionRouteStage,
        WireSequence,
    };
    use norishell_server_metrics::{
        CpuUsage, DiskUsage, LinuxMetricSample, MemoryUsage, MetricValue, NetworkRate,
    };
    use norishell_ssh_domain::Endpoint;
    use norishell_ssh_transport::{
        HostKeyDecision, KeyboardInteractiveChallenge, KeyboardInteractivePrompt, ObservedHostKey,
    };
    use tempfile::TempDir;
    use tokio::sync::{mpsc, oneshot, watch};

    use super::{
        Actor, ConnectTask, KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES, KeyboardInteractiveRequest,
        MAX_BACKOFF_SECONDS, WorkerFailure, backoff_delay, generation_matches, mark_snapshot_stale,
        metric_snapshot_from, new_record, paused_session_can_be_reused,
        validate_supported_selection, wait_for_stop,
    };
    use crate::{
        host_service::HostService, ssh_agent_service::SshAgentService,
        transient_credential_service::TransientCredentialService, vault_service::VaultService,
    };

    fn policy(enabled: bool) -> MonitoringPolicy {
        MonitoringPolicy {
            enabled,
            sample_interval_seconds: 15,
            sample_timeout_seconds: 5,
            disk_mount_ids: vec![DiskResourceId::Root],
            network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
        }
    }

    fn available_sample() -> LinuxMetricSample {
        LinuxMetricSample {
            provider_id: "linux-procfs",
            provider_version: 1,
            sampled_at: Duration::from_secs(1),
            cpu: MetricValue::Available(CpuUsage { basis_points: 1 }),
            memory: MemoryUsage {
                used_bytes: 1,
                available_bytes: 2,
                total_bytes: 3,
            },
            network: MetricValue::Available(NetworkRate {
                receive_bytes_per_second: 4,
                transmit_bytes_per_second: 5,
            }),
            root_disk: DiskUsage {
                filesystem_id: "/dev/disk0".to_owned(),
                mount: "/".to_owned(),
                used_bytes: 6,
                available_bytes: 7,
                total_bytes: 13,
            },
        }
    }

    fn actor() -> (TempDir, Actor) {
        let directory = tempfile::tempdir().expect("temp directory");
        let hosts = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        let (tx, _rx) = mpsc::channel(16);
        (
            directory,
            Actor::new(
                tx,
                hosts,
                vault,
                TransientCredentialService::default(),
                SshAgentService::default(),
            ),
        )
    }

    fn install_active_record(actor: &mut Actor, host_id: HostId) {
        let mut record = new_record(host_id.clone(), WireSequence::new(1), policy(true));
        record.summary.generation = WireSequence::new(1);
        let (stop, mut stop_rx) = watch::channel(false);
        record.stop = Some(stop);
        record.task = Some(ConnectTask {
            generation: 1,
            handle: tauri::async_runtime::spawn(async move {
                wait_for_stop(&mut stop_rx).await;
            }),
        });
        actor.sessions.insert(host_id.as_str().to_owned(), record);
    }

    struct CancellationProbe(Option<oneshot::Sender<()>>);

    impl Drop for CancellationProbe {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn targeted_disable_waits_for_worker_exit_without_catalog_reconcile() {
        let (_directory, mut actor) = actor();
        let host_id = HostId::new();
        let mut record = new_record(host_id.clone(), WireSequence::new(1), policy(true));
        let (stopped, stopped_rx) = oneshot::channel();
        let probe = CancellationProbe(Some(stopped));
        let (stop, mut stop_rx) = watch::channel(false);
        record.stop = Some(stop);
        record.task = Some(ConnectTask {
            generation: 1,
            handle: tauri::async_runtime::spawn(async move {
                let _probe = probe;
                wait_for_stop(&mut stop_rx).await;
            }),
        });
        actor.sessions.insert(host_id.as_str().to_owned(), record);

        actor
            .disable_host(RequestId::new(), &host_id)
            .await
            .expect("disable host");

        assert!(!actor.sessions.contains_key(host_id.as_str()));
        tokio::time::timeout(Duration::from_secs(1), stopped_rx)
            .await
            .expect("worker cancellation completed before disable ack")
            .expect("cancellation probe");
    }

    #[test]
    fn snapshot_binds_canonical_resource_ids_to_display_metrics() {
        let snapshot = metric_snapshot_from(
            HostId::new(),
            norishell_core_api::MetricsSessionId::new(),
            1,
            1,
            1,
            2,
            available_sample(),
        );

        assert_eq!(
            snapshot.network.resource_id,
            NetworkResourceId::AggregateNonLoopback
        );
        assert_eq!(snapshot.disks[0].resource_id, DiskResourceId::Root);
        assert_eq!(snapshot.disks[0].filesystem_id, "/dev/disk0");
        assert_eq!(snapshot.disks[0].mount, "/");
    }

    #[test]
    fn disabled_policy_is_rejected_before_any_worker_can_start() {
        assert!(validate_supported_selection(&policy(false)).is_ok());
        let record = new_record(HostId::new(), WireSequence::new(1), policy(false));
        assert!(!record.monitoring_policy.enabled);
        assert!(record.task.is_none());
    }

    #[test]
    fn unsupported_provider_selections_fail_closed() {
        let mut selected = policy(true);
        selected.disk_mount_ids = vec![DiskResourceId::Root, DiskResourceId::Root];
        assert!(validate_supported_selection(&selected).is_err());
        selected.disk_mount_ids = vec![DiskResourceId::Root];
        selected.network_interface_ids.clear();
        assert!(validate_supported_selection(&selected).is_err());
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_delay(1, 15).as_secs(), 15);
        assert_eq!(backoff_delay(5, 15).as_secs(), 240);
        assert_eq!(backoff_delay(u32::MAX, 15).as_secs(), MAX_BACKOFF_SECONDS);
    }

    #[test]
    fn old_generation_cannot_update_a_new_record() {
        let mut record = new_record(HostId::new(), WireSequence::new(1), policy(true));
        record.summary.generation = WireSequence::new(4);
        assert!(!generation_matches(&record, 3));
        assert!(generation_matches(&record, 4));
    }

    #[tokio::test]
    async fn first_sample_of_a_new_generation_replaces_an_old_high_sequence_snapshot() {
        let (_directory, mut actor) = actor();
        let host_id = HostId::new();
        install_active_record(&mut actor, host_id.clone());
        let record = actor.sessions.get_mut(host_id.as_str()).expect("record");
        record.summary.generation = WireSequence::new(2);
        record.task.as_mut().expect("task").generation = 2;
        record.summary.latest_snapshot = Some(metric_snapshot_from(
            host_id.clone(),
            record.summary.metrics_session_id.clone(),
            1,
            100,
            1,
            2,
            available_sample(),
        ));
        let next = metric_snapshot_from(
            host_id.clone(),
            record.summary.metrics_session_id.clone(),
            2,
            1,
            3,
            4,
            available_sample(),
        );

        actor.sample_ready(host_id.as_str(), 2, next);

        let snapshot = actor.sessions[host_id.as_str()]
            .summary
            .latest_snapshot
            .as_ref()
            .expect("new generation snapshot");
        assert_eq!(snapshot.generation, WireSequence::new(2));
        assert_eq!(snapshot.sample_sequence, WireSequence::new(1));
    }

    #[test]
    fn reconcile_reuses_a_paused_authentication_session_until_its_blocker_changes() {
        let monitoring = policy(true);
        let mut record = new_record(HostId::new(), WireSequence::new(3), monitoring.clone());
        record.summary.state = MetricsSessionState::NeedsAuthentication;
        assert!(paused_session_can_be_reused(
            &record,
            WireSequence::new(3),
            &monitoring,
            false,
        ));
        assert!(!paused_session_can_be_reused(
            &record,
            WireSequence::new(4),
            &monitoring,
            false,
        ));

        record.summary.authentication_reason =
            Some(norishell_core_api::MetricsAuthenticationReason::VaultLocked);
        assert!(paused_session_can_be_reused(
            &record,
            WireSequence::new(3),
            &monitoring,
            false,
        ));
        assert!(!paused_session_can_be_reused(
            &record,
            WireSequence::new(3),
            &monitoring,
            true,
        ));
    }

    #[test]
    fn failures_preserve_last_values_but_mark_them_stale() {
        let mut record = new_record(HostId::new(), WireSequence::new(1), policy(true));
        record.summary.generation = WireSequence::new(1);
        record.summary.latest_snapshot = Some(MetricSnapshot {
            host_id: record.summary.host_id.clone(),
            metrics_session_id: record.summary.metrics_session_id.clone(),
            generation: WireSequence::new(1),
            sample_sequence: WireSequence::new(1),
            provider_id: "linux-procfs".to_owned(),
            provider_version: 1,
            platform: MetricsPlatform::Linux,
            sample_started_at_unix_ms: 1,
            sample_completed_at_unix_ms: 2,
            stale: false,
            cpu: CpuMetric {
                state: MetricFieldState::Available,
                basis_points: Some(0),
            },
            memory: norishell_core_api::MemoryMetric {
                state: MetricFieldState::Available,
                used_bytes: Some(MetricByteCount::new(0)),
                available_bytes: Some(MetricByteCount::new(1)),
                total_bytes: Some(MetricByteCount::new(1)),
            },
            network: norishell_core_api::NetworkMetric {
                resource_id: NetworkResourceId::AggregateNonLoopback,
                state: MetricFieldState::Available,
                receive_bytes_per_second: Some(MetricByteCount::new(0)),
                transmit_bytes_per_second: Some(MetricByteCount::new(0)),
            },
            disks: Vec::new(),
        });
        mark_snapshot_stale(&mut record);
        assert!(
            record
                .summary
                .latest_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.stale)
        );
    }

    #[tokio::test]
    async fn authentication_failure_marks_the_previous_snapshot_stale_before_pausing() {
        let (_directory, mut actor) = actor();
        let host_id = HostId::new();
        install_active_record(&mut actor, host_id.clone());
        let record = actor.sessions.get_mut(host_id.as_str()).expect("record");
        record.summary.latest_snapshot = Some(metric_snapshot_from(
            host_id.clone(),
            record.summary.metrics_session_id.clone(),
            1,
            1,
            1,
            2,
            available_sample(),
        ));

        actor.worker_failed(
            host_id.as_str(),
            1,
            WorkerFailure {
                code: MetricsSessionFailureCode::AuthenticationRejected,
                recoverable: false,
            },
        );

        let record = &actor.sessions[host_id.as_str()];
        assert_eq!(
            record.summary.state,
            MetricsSessionState::NeedsAuthentication
        );
        assert!(
            record
                .summary
                .latest_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.stale)
        );
    }

    #[tokio::test]
    async fn unknown_host_key_becomes_a_review_challenge_and_rejects_background_socket() {
        let (_directory, mut actor) = actor();
        let host_id = HostId::new();
        install_active_record(&mut actor, host_id.clone());
        let endpoint = Endpoint::parse("metrics.example", 22).expect("endpoint");
        actor
            .sessions
            .get_mut(host_id.as_str())
            .expect("record")
            .route_stages
            .insert(
                "metrics.example:22".to_owned(),
                SshSessionRouteStage::Target,
            );
        let (decision, response) = oneshot::channel();
        actor.host_key_observed(
            host_id.as_str(),
            1,
            endpoint,
            ObservedHostKey {
                algorithm: "ssh-ed25519".to_owned(),
                public_key_blob: vec![1, 2, 3],
                fingerprint_sha256: "SHA256:test".to_owned(),
            },
            decision,
        );
        assert!(matches!(
            response.await.expect("decision"),
            Ok(HostKeyDecision::Rejected)
        ));
        let summary = &actor.sessions[host_id.as_str()].summary;
        assert_eq!(summary.state, MetricsSessionState::NeedsHostKeyReview);
        assert!(summary.host_key_challenge.is_some());
    }

    #[tokio::test]
    async fn keyboard_answer_is_redacted_bounded_and_expires_with_the_challenge() {
        let (_directory, mut actor) = actor();
        let host_id = HostId::new();
        install_active_record(&mut actor, host_id.clone());
        let (answers, _response) = oneshot::channel();
        actor.keyboard_challenge_observed(
            host_id.as_str(),
            1,
            KeyboardInteractiveRequest {
                route_stage: super::ConnectionRouteStage::Target,
                credential_ref_id: norishell_core_api::CredentialRefId::new(),
                attempt_index: 0,
                round_index: 1,
                challenge: KeyboardInteractiveChallenge {
                    name: "login".to_owned(),
                    instructions: "answer".to_owned(),
                    prompts: vec![KeyboardInteractivePrompt {
                        text: "Password:".to_owned(),
                        echo: false,
                    }],
                },
            },
            answers,
        );
        let record = actor.sessions.get(host_id.as_str()).expect("record");
        let challenge = record
            .summary
            .keyboard_interactive_challenge
            .clone()
            .expect("challenge");
        let request = MetricsKeyboardInteractiveAnswerPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            answer_ref_id: MetricsKeyboardInteractiveAnswerRefId::new(),
            metrics_session_id: record.summary.metrics_session_id.clone(),
            expected_generation: WireSequence::new(1),
            challenge_id: challenge.challenge_id.clone(),
            round_index: 1,
            prompt_index: 0,
            value: "top-secret".to_owned(),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("top-secret"));
        assert!(actor.prepare_answer(request).is_ok());

        let record = actor.sessions.get_mut(host_id.as_str()).expect("record");
        record
            .active_keyboard_challenge
            .as_mut()
            .expect("active")
            .challenge
            .expires_at_unix_ms = 0;
        let expired = MetricsKeyboardInteractiveAnswerPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            answer_ref_id: MetricsKeyboardInteractiveAnswerRefId::new(),
            metrics_session_id: record.summary.metrics_session_id.clone(),
            expected_generation: WireSequence::new(1),
            challenge_id: challenge.challenge_id,
            round_index: 1,
            prompt_index: 0,
            value: "too-late".to_owned(),
        };
        assert!(actor.prepare_answer(expired).is_err());

        let record = actor.sessions.get(host_id.as_str()).expect("record");
        let oversized = MetricsKeyboardInteractiveAnswerPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            answer_ref_id: MetricsKeyboardInteractiveAnswerRefId::new(),
            metrics_session_id: record.summary.metrics_session_id.clone(),
            expected_generation: WireSequence::new(1),
            challenge_id: MetricsKeyboardInteractiveChallengeId::new(),
            round_index: 1,
            prompt_index: 0,
            value: "x".repeat(KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES + 1),
        };
        assert!(actor.prepare_answer(oversized).is_err());
    }

    #[tokio::test]
    async fn secure_keyboard_cancel_rejects_wrong_round_and_preserves_other_sessions() {
        let (_directory, mut actor) = actor();
        let host_id = HostId::new();
        install_active_record(&mut actor, host_id.clone());
        let (answers, _response) = oneshot::channel();
        actor.keyboard_challenge_observed(
            host_id.as_str(),
            1,
            KeyboardInteractiveRequest {
                route_stage: super::ConnectionRouteStage::Target,
                credential_ref_id: norishell_core_api::CredentialRefId::new(),
                attempt_index: 0,
                round_index: 1,
                challenge: KeyboardInteractiveChallenge {
                    name: "login".to_owned(),
                    instructions: "answer".to_owned(),
                    prompts: vec![KeyboardInteractivePrompt {
                        text: "Password:".to_owned(),
                        echo: false,
                    }],
                },
            },
            answers,
        );
        let record = actor.sessions.get(host_id.as_str()).expect("record");
        let challenge = record
            .summary
            .keyboard_interactive_challenge
            .clone()
            .expect("challenge");
        let request = MetricsKeyboardInteractiveRespondRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: norishell_core_api::OperationId::new(),
            idempotency_key: "secure-cancel".into(),
            metrics_session_id: challenge.metrics_session_id.clone(),
            host_id: host_id.clone(),
            expected_generation: challenge.generation,
            challenge_id: challenge.challenge_id.clone(),
            round_index: challenge.round_index,
            answer_ref_ids: vec![],
        };
        let other = HostId::new();
        install_active_record(&mut actor, other.clone());
        let mut stale = request.clone();
        stale.round_index += 1;
        assert!(actor.cancel_keyboard_challenge(stale).await.is_err());
        assert!(
            actor.sessions[host_id.as_str()]
                .active_keyboard_challenge
                .is_some()
        );
        let mut stale = request.clone();
        stale.expected_generation = WireSequence::new(999);
        assert!(actor.cancel_keyboard_challenge(stale).await.is_err());
        let mut stale = request.clone();
        stale.challenge_id = MetricsKeyboardInteractiveChallengeId::new();
        assert!(actor.cancel_keyboard_challenge(stale).await.is_err());
        actor
            .cancel_keyboard_challenge(request.clone())
            .await
            .expect("exact cancel");
        assert_eq!(
            actor.sessions[host_id.as_str()].summary.state,
            MetricsSessionState::Closed
        );
        assert!(
            actor.sessions[host_id.as_str()]
                .active_keyboard_challenge
                .is_none()
        );
        assert!(actor.sessions[other.as_str()].task.is_some());
        assert!(actor.cancel_keyboard_challenge(request).await.is_err());
        actor.shutdown_all(RequestId::new()).await.expect("cleanup");
    }

    #[tokio::test]
    async fn stop_waits_for_the_owned_probe_worker_and_closes_the_session() {
        let (_directory, mut actor) = actor();
        let host_id = HostId::new();
        install_active_record(&mut actor, host_id.clone());
        let summary = actor
            .stop(MetricsStopRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                host_id: host_id.clone(),
                expected_generation: Some(WireSequence::new(1)),
            })
            .await
            .expect("stop");
        assert_eq!(summary.state, MetricsSessionState::Closed);
        let record = actor.sessions.get(host_id.as_str()).expect("record");
        assert!(record.task.is_none());
        assert!(record.stop.is_none());
    }

    #[tokio::test]
    async fn shutdown_cancels_all_probe_workers_before_reporting_closed() {
        let (_directory, mut actor) = actor();
        let first = HostId::new();
        let second = HostId::new();
        install_active_record(&mut actor, first.clone());
        install_active_record(&mut actor, second.clone());

        actor
            .shutdown_all(RequestId::new())
            .await
            .expect("shutdown all");

        for host_id in [first, second] {
            let record = &actor.sessions[host_id.as_str()];
            assert_eq!(record.summary.state, MetricsSessionState::Closed);
            assert!(record.task.is_none());
            assert!(record.stop.is_none());
        }
    }
}
