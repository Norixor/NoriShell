use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[path = "ssh_session_service/local_terminal.rs"]
mod local_terminal;

use norishell_app_persistence::KnownHostObservation;
#[cfg(test)]
use norishell_app_persistence::{CredentialRecord, CredentialRecordDetails};
use norishell_core_api::{
    AlgorithmCategory as WireAlgorithmCategory, CoreApiError, CredentialRefId, ErrorCategory,
    HeartbeatPolicy, LocalSessionAttachRequest, LocalSessionAttachResponse, LocalSessionAttachment,
    LocalSessionAttachmentHeartbeatRequest, LocalSessionDetachRequest, LocalSessionDetachResult,
    LocalSessionDetails, LocalSessionEvent, LocalSessionFailureReason, LocalSessionGetRequest,
    LocalSessionInputLease, LocalSessionInputLeaseRenewRequest, LocalSessionInputRequest,
    LocalSessionOpenRequest, LocalSessionOpenResponse, LocalSessionResizeRequest,
    LocalSessionSnapshot, LocalSessionSnapshotRequest, LocalSessionSummary,
    LocalSessionTerminateRequest, LoginAutomationStepInput, RequestId, RetryStrategy,
    SSH_TERMINAL_EVENT_SCHEMA_VERSION, SSH_TERMINAL_OUTPUT_FRAME_MAX_BYTES,
    ShellHeartbeatLineEnding, SshAlgorithmNegotiationFailure, SshAttachAttemptId, SshAttachmentId,
    SshChannelId, SshHeartbeatMode, SshHostKeyChallenge, SshHostKeyChallengeId, SshHostKeyDecision,
    SshHostKeyDecisionRequest, SshInputLeaseId, SshKeyboardInteractiveAnswerInput,
    SshKeyboardInteractiveAnswerPrepareRequest, SshKeyboardInteractiveAnswerPrepareResponse,
    SshKeyboardInteractiveAnswerRefId, SshKeyboardInteractiveChallenge,
    SshKeyboardInteractiveChallengeId, SshKeyboardInteractivePrompt,
    SshKeyboardInteractiveResponseRequest, SshLoginAutomationFailureCode,
    SshLoginAutomationProgress, SshLoginAutomationStatus, SshLoginAutomationStepKind,
    SshLoginAutomationTakeoverRequest, SshLoginAutomationTakeoverResponse, SshNegotiatedAlgorithms,
    SshSessionAttachRequest, SshSessionAttachResponse, SshSessionAttachment,
    SshSessionAttachmentChange, SshSessionAttachmentHeartbeatRequest, SshSessionCloseReason,
    SshSessionDetachIntent, SshSessionDetachRequest, SshSessionDetachResult, SshSessionDetails,
    SshSessionDisconnectRequest, SshSessionEndpoint, SshSessionEvent, SshSessionEventPayload,
    SshSessionFailureCode, SshSessionFailureReason, SshSessionFailureStage, SshSessionGetRequest,
    SshSessionHeartbeatStatus, SshSessionInputLease, SshSessionInputLeaseChange,
    SshSessionInputLeaseRenewRequest, SshSessionInputRequest, SshSessionLastDetachAction,
    SshSessionOpenRequest, SshSessionOpenResponse, SshSessionOutputFrame, SshSessionOutputGap,
    SshSessionOutputGapReason, SshSessionOutputItem, SshSessionReconnectRequest,
    SshSessionResizeRequest, SshSessionRetryStrategy, SshSessionRouteStage, SshSessionSnapshot,
    SshSessionSnapshotRequest, SshSessionState, SshSessionSummary, SshSessionTarget,
    SshShellHeartbeatSkipReason, SshShellHeartbeatStatus, SshTerminalInputFocusChangeRequest,
    SshTerminalInputFocusChangeResponse, SshTerminalInputFocusSnapshot,
    SshTerminalInputFocusSnapshotRequest, SshTransportHeartbeatStatus,
    TerminalInputFocusChangeRequest, TerminalInputFocusChangeResponse, TerminalInputFocusSnapshot,
    TerminalInputFocusSnapshotRequest, TerminalInputFocusTarget, TerminalInputLease, WireSequence,
};
#[cfg(test)]
use norishell_core_api::{SshSessionInputLeaseAcquireRequest, SshTerminalInputFocusTarget};
use norishell_secret_vault::SecretKind;
use norishell_ssh_domain::Endpoint;
#[cfg(test)]
use norishell_ssh_transport::RouteIngressError;
use norishell_ssh_transport::{
    HostKeyDecision, HostKeyVerifier, IngressFailureKind, IngressStage,
    KeyboardInteractiveChallenge as TransportKeyboardChallenge, ObservedHostKey, PtySize,
    RemoteShell, ShellEvent, TransportError, TransportHeartbeatHandle, VerifyFuture,
};
use tauri::{State, ipc::Channel};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    task::AbortHandle,
};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::connection_profile::{
    ConnectionCredential, ConnectionProfileError, ResolvedAlgorithmPolicy,
    ResolvedConnectionProfile, ResolvedHeartbeatPolicy, ResolvedJumpHost, ResolvedLoginAutomation,
    ResolvedRouteIngress, ResolvedSshConnectionBase, resolve_connection_profile,
    resolve_reconnect_connection_profile,
};
use crate::local_terminal_platform::{LocalPtyExit, LocalPtyMetadata, LocalPtyProcess};
use crate::ssh_connection_orchestrator::{
    ConnectionPhase, ConnectionRouteStage, KeyboardInteractiveRequest, NegotiatedAlgorithmFact,
    RoutedTransportHeartbeat as ConnectionHeartbeatFact, SshConnectionFailure,
    SshConnectionInteraction, SshConnectionOrchestrator,
};
use crate::ssh_operation_ledger::{
    SshOperationFingerprint, SshOperationLedger, SshOperationLookup, StoredSshOperationResult,
};
use crate::{
    core_api_error::core_error,
    host_service::HostService,
    native_terminal::{NativeTerminalService, PreparedNativeTerminalEnable},
    time::unix_time_ms,
    transient_credential_service::TransientCredentialService,
    vault_service::{VaultService, VaultServiceError},
};
use local_terminal::LocalSessions;

type CoreResult<T> = Result<T, Box<CoreApiError>>;
type ActorResult<T> = Result<T, Box<CoreApiError>>;
enum SessionShell {
    Connection(RemoteShell<ActorHostKeyVerifier>),
    Child(
        Box<norishell_ssh_transport::SharedTerminalChannel>,
        Box<crate::plugin_service::ApprovedPluginTerminalStartup>,
    ),
}

impl SessionShell {
    async fn send_input(&self, bytes: Vec<u8>) -> Result<(), TransportError> {
        match self {
            Self::Connection(shell) => shell.send_input(bytes).await,
            Self::Child(shell, startup)
                if startup.parent.is_current() && (startup.is_current)() =>
            {
                shell.send_input(bytes).await
            }
            Self::Child(..) => Err(TransportError::ConnectionLost),
        }
    }
    async fn resize(&self, size: PtySize) -> Result<(), TransportError> {
        match self {
            Self::Connection(shell) => shell.resize(size).await,
            Self::Child(shell, startup)
                if startup.parent.is_current() && (startup.is_current)() =>
            {
                shell.resize(size).await
            }
            Self::Child(..) => Err(TransportError::ConnectionLost),
        }
    }
    async fn next_event(&mut self) -> Result<ShellEvent, TransportError> {
        match self {
            Self::Connection(shell) => shell.next_event().await,
            Self::Child(shell, startup) => {
                let mut stop = startup.parent.cancelled.clone();
                tokio::select! {
                    biased;
                    () = startup.parent.wait_cancelled() => Err(TransportError::ConnectionLost),
                    () = crate::plugin_operations::wait_cancelled(&mut stop, &startup.is_current) => Err(TransportError::RemoteExecRejected),
                    result = shell.next_event() => result,
                }
            }
        }
    }
    async fn disconnect(self) -> Result<(), TransportError> {
        match self {
            Self::Connection(shell) => shell.disconnect().await,
            Self::Child(shell, _) => shell.disconnect().await,
        }
    }
}

/// A channel capability bound to one existing user Session generation.
#[derive(Clone, Debug)]
pub(crate) struct SessionChannelLease {
    pub(crate) session_id: norishell_core_api::SshSessionId,
    pub(crate) generation: WireSequence,
    pub(crate) parent_channel_id: SshChannelId,
    pub(crate) target: SshSessionTarget,
    pub(crate) endpoint: SshSessionEndpoint,
    pub(crate) credential_ref_id: Option<CredentialRefId>,
    pub(crate) channels: norishell_ssh_transport::SharedSessionChannels,
    pub(crate) cancelled: watch::Receiver<bool>,
}
impl SessionChannelLease {
    pub(crate) fn is_current(&self) -> bool {
        !*self.cancelled.borrow() && !self.channels.is_closed()
    }
    pub(crate) async fn wait_cancelled(&self) {
        let mut cancelled = self.cancelled.clone();
        tokio::select! {
            () = self.channels.wait_closed() => {},
            () = async { while !*cancelled.borrow() { if cancelled.changed().await.is_err() { return; } } } => {},
        }
    }
}

const ACTOR_MAILBOX_CAPACITY: usize = 256;
const SHELL_MAILBOX_CAPACITY: usize = 128;
const OUTPUT_RING_MAX_BYTES: usize = 4 * 1024 * 1024;
const INPUT_LEASE_MILLIS: i64 = 15_000;
const ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS: i64 = 45_000;
const ATTACHMENT_REAPER_INTERVAL_MILLIS: u64 = 5_000;
const CONTROL_OPERATION_LEDGER_CAPACITY: usize = 1_024;
const APPLICATION_EXIT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const KEYBOARD_INTERACTIVE_ROUND_TIMEOUT: Duration = Duration::from_secs(120);
const KEYBOARD_INTERACTIVE_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);
const KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES: usize = 64 * 1024;
const LOGIN_AUTOMATION_MATCH_BUFFER_MAX_BYTES: usize = 64 * 1024;
const MANUAL_INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const SHELL_HEARTBEAT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const PLUGIN_INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// A capability-broker-only terminal write. The broker consumes the user's
/// one-shot approval before constructing this value; the Session actor still
/// revalidates every focus and ownership fence at the actual write boundary.
#[derive(Clone)]
pub(crate) struct ApprovedPluginInput {
    pub request_id: RequestId,
    pub session_id: norishell_core_api::SshSessionId,
    pub expected_generation: WireSequence,
    pub channel_id: SshChannelId,
    pub attachment_id: SshAttachmentId,
    pub view_id: norishell_core_api::SshViewId,
    pub focus_epoch: WireSequence,
    pub lease_id: SshInputLeaseId,
    pub input_epoch: WireSequence,
    pub bytes: Vec<u8>,
    pub approval_fence: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApprovedPluginObservationAttach {
    pub request_id: RequestId,
    pub observer_id: Uuid,
    pub focus_epoch: WireSequence,
    pub target: norishell_core_api::SshTerminalInputFocusTarget,
    pub sink: mpsc::Sender<PluginTerminalObservation>,
}

/// Internal metadata source for the subscription broker. It intentionally
/// contains no endpoint, credential, close/failure text, or terminal output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginSessionMetadataEvent {
    Ssh {
        session_id: norishell_core_api::SshSessionId,
        generation: WireSequence,
        state_revision: WireSequence,
        state: SshSessionState,
    },
    Local {
        session_id: norishell_core_api::LocalSessionId,
        generation: WireSequence,
        state_revision: WireSequence,
        state: norishell_core_api::LocalSessionState,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginTerminalObservation {
    Text {
        output_seq: WireSequence,
        text: String,
    },
    Detached,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TerminalTextProjectionState {
    #[default]
    Ground,
    Escape,
    ControlSequence,
    OperatingSystemCommand,
    OperatingSystemCommandEscape,
}

#[derive(Debug)]
struct PluginObserverRecord {
    target: norishell_core_api::SshTerminalInputFocusTarget,
    sink: mpsc::Sender<PluginTerminalObservation>,
    projection_state: TerminalTextProjectionState,
    utf8_pending: Vec<u8>,
}

struct ConnectPlan {
    session_id: String,
    generation: u64,
    endpoint: SshSessionEndpoint,
    username: String,
    ingress: ResolvedRouteIngress,
    jump_hosts: Vec<ResolvedJumpHost>,
    credentials: Vec<ConnectionCredential>,
    algorithm_policy: ResolvedAlgorithmPolicy,
    heartbeat_policy: ResolvedHeartbeatPolicy,
    login_automation: Option<ResolvedLoginAutomation>,
    automation_attachment_id: SshAttachmentId,
    automation_view_id: norishell_core_api::SshViewId,
    profile_revision_token: String,
    rows: u16,
    cols: u16,
}

struct ConnectTask {
    generation: u64,
    abort_handle: AbortHandle,
}

struct RoutedTransportHeartbeat {
    route_stage: SshSessionRouteStage,
    handle: TransportHeartbeatHandle,
    first_due: tokio::time::Instant,
    first_due_at_unix_ms: i64,
}

struct HeartbeatTask {
    handle: tauri::async_runtime::JoinHandle<()>,
}

impl HeartbeatTask {
    fn new(handle: tauri::async_runtime::JoinHandle<()>) -> Self {
        Self { handle }
    }

    fn abort(&self) {
        self.handle.abort();
    }
}

impl Drop for HeartbeatTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct ShellReadyContext {
    channels: norishell_ssh_transport::SharedSessionChannels,
    login_automation: Option<ResolvedLoginAutomation>,
    attachment_id: SshAttachmentId,
    view_id: norishell_core_api::SshViewId,
    heartbeat_policy: ResolvedHeartbeatPolicy,
    transport_heartbeats: Vec<RoutedTransportHeartbeat>,
}

struct TransientCredentialCleanup {
    service: TransientCredentialService,
    credential_ref_ids: Vec<CredentialRefId>,
}

impl Drop for TransientCredentialCleanup {
    fn drop(&mut self) {
        for credential_ref_id in &self.credential_ref_ids {
            self.service.discard(credential_ref_id);
        }
    }
}

#[derive(Clone)]
pub struct SshSessionService {
    tx: mpsc::Sender<Message>,
    focus_broker: crate::terminal_focus_broker::TerminalFocusBroker,
    live_sessions: Arc<Mutex<BTreeSet<String>>>,
    live_local_sessions: Arc<Mutex<BTreeSet<String>>>,
    plugin_metadata_events: broadcast::Sender<PluginSessionMetadataEvent>,
}

impl SshSessionService {
    #[cfg(test)]
    pub fn start(
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
    ) -> Self {
        Self::start_with_agent(
            hosts,
            vault,
            transient_credentials,
            crate::ssh_agent_service::SshAgentService::default(),
        )
    }

    #[cfg(test)]
    pub fn start_with_agent(
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: crate::ssh_agent_service::SshAgentService,
    ) -> Self {
        Self::start_with_agent_and_native(
            hosts,
            vault.clone(),
            transient_credentials,
            ssh_agent,
            NativeTerminalService::detached(vault),
        )
    }

    pub fn start_with_agent_and_native(
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: crate::ssh_agent_service::SshAgentService,
        native_terminal: NativeTerminalService,
    ) -> Self {
        let (tx, rx) = mpsc::channel(ACTOR_MAILBOX_CAPACITY);
        let live_sessions = Arc::new(Mutex::new(BTreeSet::new()));
        let live_local_sessions = Arc::new(Mutex::new(BTreeSet::new()));
        let (plugin_metadata_events, _) = broadcast::channel(64);
        let service = Self {
            tx: tx.clone(),
            focus_broker: crate::terminal_focus_broker::TerminalFocusBroker::default(),
            live_sessions: live_sessions.clone(),
            live_local_sessions: live_local_sessions.clone(),
            plugin_metadata_events: plugin_metadata_events.clone(),
        };
        tauri::async_runtime::spawn(run_actor(
            Actor::new(
                tx,
                hosts,
                vault,
                transient_credentials,
                ssh_agent,
                native_terminal,
                live_sessions,
                live_local_sessions,
                plugin_metadata_events,
            ),
            rx,
        ));
        let reaper_tx = service.tx.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(
                    ATTACHMENT_REAPER_INTERVAL_MILLIS,
                ))
                .await;
                if reaper_tx
                    .send(Message::ReapStaleAttachments {
                        now_unix_ms: unix_time_ms(),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        service
    }

    pub(crate) fn subscribe_plugin_metadata(
        &self,
    ) -> broadcast::Receiver<PluginSessionMetadataEvent> {
        self.plugin_metadata_events.subscribe()
    }

    pub(crate) fn exit_blockers(&self) -> Vec<norishell_core_api::SshSessionId> {
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|value| norishell_core_api::SshSessionId::parse(value).ok())
            .collect()
    }

    pub(crate) fn local_exit_blockers(&self) -> Vec<norishell_core_api::LocalSessionId> {
        self.live_local_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|value| norishell_core_api::LocalSessionId::parse(value).ok())
            .collect()
    }

    pub(crate) async fn plugin_channel_lease(
        &self,
        request_id: RequestId,
        session_id: norishell_core_api::SshSessionId,
        generation: WireSequence,
    ) -> ActorResult<SessionChannelLease> {
        self.request(
            |reply| Message::PluginChannelLease {
                request_id: request_id.clone(),
                session_id,
                generation,
                reply,
            },
            request_id.clone(),
        )
        .await
    }

    pub(crate) async fn snapshot(&self, request_id: RequestId) -> ActorResult<SshSessionSnapshot> {
        self.request(|reply| Message::Snapshot { reply }, request_id)
            .await
    }

    pub(crate) async fn local_snapshot(
        &self,
        request_id: RequestId,
    ) -> ActorResult<LocalSessionSnapshot> {
        self.request(|reply| Message::LocalSnapshot { reply }, request_id)
            .await
    }

    pub(crate) async fn terminal_focus_snapshot_internal(
        &self,
        request_id: RequestId,
    ) -> ActorResult<TerminalInputFocusSnapshot> {
        self.focus_broker
            .linearize(self.terminal_focus_snapshot_unserialized(request_id))
            .await
    }

    pub(crate) async fn terminal_focus_snapshot_unserialized(
        &self,
        request_id: RequestId,
    ) -> ActorResult<TerminalInputFocusSnapshot> {
        self.request(|reply| Message::TerminalFocusSnapshot { reply }, request_id)
            .await
    }

    pub(crate) fn focus_broker(&self) -> crate::terminal_focus_broker::TerminalFocusBroker {
        self.focus_broker.clone()
    }

    pub(crate) async fn clear_plugin_focus_exact_unserialized(
        &self,
        target: norishell_core_api::PluginTerminalInputFocusTarget,
        focus_epoch: u64,
    ) -> ActorResult<TerminalInputFocusSnapshot> {
        self.request(
            |reply| Message::PluginTerminalFocusInvalidateExact {
                target,
                focus_epoch,
                reply,
            },
            RequestId::new(),
        )
        .await
    }

    pub(crate) async fn clear_telnet_focus_exact_unserialized(
        &self,
        invalidation: crate::telnet_session_service::TelnetFocusInvalidation,
    ) -> ActorResult<TerminalInputFocusSnapshot> {
        self.request(
            |reply| Message::TerminalFocusInvalidateExact {
                invalidation,
                reply,
            },
            RequestId::new(),
        )
        .await
    }

    pub(crate) async fn send_approved_plugin_input(
        &self,
        request: ApprovedPluginInput,
    ) -> ActorResult<()> {
        let request_id = request.request_id.clone();
        self.focus_broker
            .linearize(self.request(
                |reply| Message::ApprovedPluginInput { request, reply },
                request_id,
            ))
            .await
    }

    pub(crate) async fn native_terminal_enable_ssh(
        &self,
        request_id: RequestId,
        prepared: PreparedNativeTerminalEnable,
    ) -> ActorResult<()> {
        let message_request_id = request_id.clone();
        self.focus_broker
            .linearize(self.request(
                |reply| Message::NativeTerminalEnableSsh {
                    request_id: message_request_id,
                    prepared,
                    reply,
                },
                request_id,
            ))
            .await
    }

    pub(crate) async fn native_terminal_enable_local(
        &self,
        request_id: RequestId,
        prepared: PreparedNativeTerminalEnable,
    ) -> ActorResult<()> {
        let message_request_id = request_id.clone();
        self.focus_broker
            .linearize(self.request(
                |reply| Message::NativeTerminalEnableLocal {
                    request_id: message_request_id,
                    prepared,
                    reply,
                },
                request_id,
            ))
            .await
    }

    pub(crate) async fn attach_approved_plugin_observer(
        &self,
        request: ApprovedPluginObservationAttach,
    ) -> ActorResult<()> {
        let request_id = request.request_id.clone();
        self.request(
            |reply| Message::PluginObserveAttach { request, reply },
            request_id,
        )
        .await
    }

    pub(crate) async fn detach_plugin_observer(
        &self,
        request_id: RequestId,
        observer_id: Uuid,
    ) -> ActorResult<()> {
        self.request(
            |reply| Message::PluginObserveDetach { observer_id, reply },
            request_id,
        )
        .await
    }

    /// Stops every SSH shell and local PTY owned by this actor, then waits for
    /// their generation-fenced cleanup facts. A timeout is a hard failure: the
    /// caller must keep the application alive instead of relying on process
    /// teardown to hide an uncollected remote channel or process tree.
    pub(crate) async fn shutdown_all(&self, request_id: RequestId) -> ActorResult<()> {
        let command_request_id = request_id.clone();
        self.request(
            |reply| Message::ShutdownAll {
                request_id: command_request_id,
                reply,
            },
            request_id,
        )
        .await
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<ActorResult<T>>) -> Message,
        request_id: RequestId,
    ) -> ActorResult<T> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(build(reply))
            .await
            .map_err(|_| unavailable_error(request_id.clone()))?;
        response.await.map_err(|_| unavailable_error(request_id))?
    }

    async fn linearized_request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<ActorResult<T>>) -> Message,
        request_id: RequestId,
    ) -> ActorResult<T> {
        self.focus_broker
            .linearize(self.request(build, request_id))
            .await
    }
}

struct Actor {
    tx: mpsc::Sender<Message>,
    hosts: HostService,
    vault: VaultService,
    transient_credentials: TransientCredentialService,
    ssh_agent: crate::ssh_agent_service::SshAgentService,
    native_terminal: NativeTerminalService,
    sessions: BTreeMap<String, SessionRecord>,
    connect_tasks: BTreeMap<String, ConnectTask>,
    snapshot_revision: u64,
    open_operations: SshOperationLedger<
        SshOperationFingerprint,
        StoredSshOperationResult<SshSessionOpenResponse>,
    >,
    attach_operations: SshOperationLedger<
        SshOperationFingerprint,
        StoredSshOperationResult<SshSessionAttachResponse>,
    >,
    detach_operations: SshOperationLedger<
        SshOperationFingerprint,
        StoredSshOperationResult<SshSessionDetachResult>,
    >,
    host_key_operations:
        SshOperationLedger<SshOperationFingerprint, StoredSshOperationResult<SshSessionDetails>>,
    #[cfg(test)]
    lease_operations:
        SshOperationLedger<SshOperationFingerprint, StoredSshOperationResult<SshSessionInputLease>>,
    focus_operations: SshOperationLedger<
        SshOperationFingerprint,
        StoredSshOperationResult<TerminalInputFocusChangeResponse>,
    >,
    reconnect_operations:
        SshOperationLedger<SshOperationFingerprint, StoredSshOperationResult<SshSessionDetails>>,
    login_automation_takeover_operations: SshOperationLedger<
        SshOperationFingerprint,
        StoredSshOperationResult<SshLoginAutomationTakeoverResponse>,
    >,
    disconnect_operations:
        SshOperationLedger<SshOperationFingerprint, StoredSshOperationResult<SshSessionDetails>>,
    focus_epoch: u64,
    focused_target: Option<TerminalInputFocusTarget>,
    plugin_observers: BTreeMap<Uuid, PluginObserverRecord>,
    live_sessions: Arc<Mutex<BTreeSet<String>>>,
    local_sessions: LocalSessions,
    shutdown_waiter: Option<ShutdownWaiter>,
    plugin_metadata_events: broadcast::Sender<PluginSessionMetadataEvent>,
}

struct ShutdownWaiter {
    request_id: RequestId,
    token: Uuid,
    reply: oneshot::Sender<ActorResult<()>>,
}

impl Actor {
    // Actor construction owns the complete runtime dependency set. Keeping
    // ownership explicit here prevents a NativeTerminalService from becoming
    // ambient/global state just to satisfy a parameter-count style rule.
    #[allow(clippy::too_many_arguments)]
    fn new(
        tx: mpsc::Sender<Message>,
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: crate::ssh_agent_service::SshAgentService,
        native_terminal: NativeTerminalService,
        live_sessions: Arc<Mutex<BTreeSet<String>>>,
        live_local_sessions: Arc<Mutex<BTreeSet<String>>>,
        plugin_metadata_events: broadcast::Sender<PluginSessionMetadataEvent>,
    ) -> Self {
        Self {
            tx,
            hosts,
            vault,
            transient_credentials,
            ssh_agent,
            native_terminal: native_terminal.clone(),
            sessions: BTreeMap::new(),
            connect_tasks: BTreeMap::new(),
            snapshot_revision: 0,
            open_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            attach_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            detach_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            host_key_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            #[cfg(test)]
            lease_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            focus_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            reconnect_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            login_automation_takeover_operations: SshOperationLedger::new(
                CONTROL_OPERATION_LEDGER_CAPACITY,
            ),
            disconnect_operations: SshOperationLedger::new(CONTROL_OPERATION_LEDGER_CAPACITY),
            focus_epoch: 0,
            focused_target: None,
            plugin_observers: BTreeMap::new(),
            live_sessions,
            local_sessions: LocalSessions::new(
                live_local_sessions,
                native_terminal,
                plugin_metadata_events.clone(),
            ),
            shutdown_waiter: None,
            plugin_metadata_events,
        }
    }
}

fn negotiated_algorithms_to_wire(fact: NegotiatedAlgorithmFact) -> SshNegotiatedAlgorithms {
    let negotiated = fact.negotiated;
    SshNegotiatedAlgorithms {
        route_stage: connection_route_stage_to_wire(fact.route_stage),
        policy_id: fact.policy_id,
        policy_revision: fact.policy_revision,
        policy_catalog_version: fact.policy_catalog_version.to_owned(),
        key_exchange: negotiated.key_exchange,
        host_key: negotiated.host_key,
        cipher_client_to_server: negotiated.cipher_client_to_server,
        cipher_server_to_client: negotiated.cipher_server_to_client,
        mac_client_to_server: negotiated.mac_client_to_server,
        mac_server_to_client: negotiated.mac_server_to_client,
    }
}

fn connection_route_stage_to_wire(route_stage: ConnectionRouteStage) -> SshSessionRouteStage {
    match route_stage {
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

fn routed_transport_heartbeat_to_terminal(
    fact: ConnectionHeartbeatFact,
) -> RoutedTransportHeartbeat {
    RoutedTransportHeartbeat {
        route_stage: connection_route_stage_to_wire(fact.route_stage),
        handle: fact.handle,
        first_due: fact.first_due,
        first_due_at_unix_ms: fact.first_due_at_unix_ms,
    }
}

async fn run_shell(
    tx: mpsc::Sender<Message>,
    session_id: String,
    generation: u64,
    mut shell: SessionShell,
    mut commands: mpsc::Receiver<ShellCommand>,
    input_activity_epoch: Arc<AtomicU64>,
    mut stop: watch::Receiver<Option<ShellStopSignal>>,
) {
    let mut close_reason = None;
    'shell: loop {
        tokio::select! {
            biased;
            changed = stop.changed() => {
                if let ShellStopSignal::Disconnect(reason) = shell_stop_signal(&stop, changed) {
                    close_reason = Some(reason);
                }
                break;
            }
            command = commands.recv() => match command {
                Some(ShellCommand::Input { bytes, deadline, completion }) => {
                    let result = tokio::select! {
                        biased;
                        changed = stop.changed() => Err(shell_stop_signal(&stop, changed)),
                        result = tokio::time::timeout_at(deadline, shell.send_input(bytes)) => Ok(result),
                    };
                    if let Err(signal) = result {
                        let _ = completion.send(false);
                        if let ShellStopSignal::Disconnect(reason) = signal {
                            close_reason = Some(reason);
                        }
                        break 'shell;
                    }
                    let succeeded = matches!(result.expect("stop branch returned early"), Ok(Ok(())));
                    let _ = completion.send(succeeded);
                    if !succeeded {
                        let _ = tx.send(Message::ConnectionFailed {
                            session_id: session_id.clone(), generation,
                            error: TransportError::ConnectionLost,
                            route_stage: None,
                        }).await;
                        break;
                    }
                }
                Some(ShellCommand::ApprovedPluginInput { bytes, deadline, completion }) => {
                    let result = tokio::select! {
                        biased;
                        changed = stop.changed() => Err(shell_stop_signal(&stop, changed)),
                        result = tokio::time::timeout_at(deadline, shell.send_input(bytes)) => Ok(result),
                    };
                    if let Err(signal) = result {
                        let _ = completion.send(false);
                        if let ShellStopSignal::Disconnect(reason) = signal {
                            close_reason = Some(reason);
                        }
                        break 'shell;
                    }
                    let succeeded = matches!(result.expect("stop branch returned early"), Ok(Ok(())));
                    let _ = completion.send(succeeded);
                    if !succeeded {
                        let _ = tx.send(Message::ConnectionFailed {
                            session_id: session_id.clone(), generation,
                            error: TransportError::ConnectionLost,
                            route_stage: None,
                        }).await;
                        break;
                    }
                }
                Some(ShellCommand::AutomationInput { bytes, deadline, completion }) => {
                    let result = tokio::select! {
                        biased;
                        changed = stop.changed() => Err(shell_stop_signal(&stop, changed)),
                        result = tokio::time::timeout_at(
                            deadline,
                            shell.send_input(bytes.as_slice().to_vec()),
                        ) => Ok(result),
                    };
                    if let Err(signal) = result {
                        let _ = completion.send(false);
                        if let ShellStopSignal::Disconnect(reason) = signal {
                            close_reason = Some(reason);
                        }
                        break 'shell;
                    }
                    let succeeded = matches!(result.expect("stop branch returned early"), Ok(Ok(())));
                    let _ = completion.send(succeeded);
                    if !succeeded {
                        let _ = tx.send(Message::ConnectionFailed {
                            session_id: session_id.clone(), generation,
                            error: TransportError::ConnectionLost,
                            route_stage: None,
                        }).await;
                        break;
                    }
                }
                Some(ShellCommand::HeartbeatInput {
                    bytes,
                    expected_input_activity_epoch,
                    deadline,
                    completion,
                }) => {
                    let preflight = shell_heartbeat_preflight(
                        expected_input_activity_epoch,
                        input_activity_epoch.as_ref(),
                        deadline,
                        tokio::time::Instant::now(),
                    );
                    let outcome = match preflight {
                        Err(outcome) => outcome,
                        Ok(()) => {
                            let result = tokio::select! {
                                biased;
                                changed = stop.changed() => Err(shell_stop_signal(&stop, changed)),
                                result = tokio::time::timeout_at(deadline, shell.send_input(bytes)) => Ok(result),
                            };
                            if let Err(signal) = result {
                                let _ = completion.send(ShellHeartbeatWriteOutcome::Failed);
                                if let ShellStopSignal::Disconnect(reason) = signal {
                                    close_reason = Some(reason);
                                }
                                break 'shell;
                            }
                            if matches!(result.expect("stop branch returned early"), Ok(Ok(()))) {
                                ShellHeartbeatWriteOutcome::Sent
                            } else {
                                ShellHeartbeatWriteOutcome::Failed
                            }
                        }
                    };
                    let failed = outcome == ShellHeartbeatWriteOutcome::Failed;
                    let _ = completion.send(outcome);
                    if failed {
                        let _ = tx.send(Message::ConnectionFailed {
                            session_id: session_id.clone(), generation,
                            error: TransportError::ConnectionLost,
                            route_stage: None,
                        }).await;
                        break;
                    }
                }
                Some(ShellCommand::Resize {
                    size,
                    deadline,
                    completion,
                }) => {
                    let result = tokio::select! {
                        biased;
                        changed = stop.changed() => Err(shell_stop_signal(&stop, changed)),
                        result = tokio::time::timeout_at(deadline, shell.resize(size)) => Ok(result),
                    };
                    if let Err(signal) = result {
                        let _ = completion.send(false);
                        if let ShellStopSignal::Disconnect(reason) = signal {
                            close_reason = Some(reason);
                        }
                        break 'shell;
                    }
                    let succeeded =
                        matches!(result.expect("stop branch returned early"), Ok(Ok(())));
                    let _ = completion.send(succeeded);
                    if !succeeded {
                        let _ = tx.send(Message::ConnectionFailed {
                            session_id: session_id.clone(), generation,
                            error: TransportError::ConnectionLost,
                            route_stage: None,
                        }).await;
                        break;
                    }
                }
                Some(ShellCommand::Disconnect(reason)) => {
                    close_reason = Some(reason);
                    break;
                }
                Some(ShellCommand::Abort) => break,
                None => {
                    close_reason = Some(SshSessionCloseReason::ApplicationExit);
                    break;
                }
            },
            event = shell.next_event() => match event {
                Ok(ShellEvent::Data(bytes)) | Ok(ShellEvent::ExtendedData { data: bytes, .. }) => {
                    let output = Message::ShellOutput {
                        session_id: session_id.clone(), generation, bytes: bytes.to_vec(),
                    };
                    tokio::select! {
                        biased;
                        changed = stop.changed() => {
                            if let ShellStopSignal::Disconnect(reason) = shell_stop_signal(&stop, changed) {
                                close_reason = Some(reason);
                            }
                            break;
                        }
                        result = tx.send(output) => {
                            if result.is_err() {
                                break;
                            }
                        }
                    }
                }
                Ok(ShellEvent::ExitStatus(exit_status)) => {
                    close_reason = Some(SshSessionCloseReason::RemoteExitStatus { exit_status });
                    break;
                }
                Ok(ShellEvent::ExitSignal { signal, .. }) => {
                    close_reason = Some(SshSessionCloseReason::RemoteExitSignal { signal_name: signal });
                    break;
                }
                Ok(ShellEvent::Eof | ShellEvent::Closed) => {
                    close_reason = Some(SshSessionCloseReason::RemoteEof);
                    break;
                }
                Ok(ShellEvent::RequestSucceeded | ShellEvent::RequestFailed | ShellEvent::Other) => {}
                Err(error) => {
                    let _ = tx.send(Message::ConnectionFailed {
                        session_id: session_id.clone(), generation, error,
                        route_stage: None,
                    }).await;
                    break;
                }
            }
        }
    }
    let _ = shell.disconnect().await;
    if let Some(reason) = close_reason {
        let _ = tx
            .send(Message::ShellClosed {
                session_id,
                generation,
                reason,
            })
            .await;
    } else {
        let _ = tx
            .send(Message::ShellCleanupCompleted {
                session_id,
                generation,
            })
            .await;
    }
}

fn shell_stop_signal(
    stop: &watch::Receiver<Option<ShellStopSignal>>,
    changed: Result<(), watch::error::RecvError>,
) -> ShellStopSignal {
    if changed.is_err() {
        return ShellStopSignal::Abort;
    }
    stop.borrow().clone().unwrap_or(ShellStopSignal::Abort)
}

fn transition_record(
    record: &mut SessionRecord,
    state: SshSessionState,
    close_reason: Option<SshSessionCloseReason>,
    failure_reason: Option<SshSessionFailureReason>,
) {
    if record.summary.state == state
        && record.summary.close_reason == close_reason
        && record.summary.failure_reason == failure_reason
    {
        return;
    }
    if !matches!(
        state,
        SshSessionState::Running | SshSessionState::AutomatingLogin
    ) {
        record.channel_lease_cancel.send_replace(true);
        record.shared_channels = None;
    }
    let previous_state = record.summary.state;
    record.summary.state = state;
    record.summary.close_reason = close_reason.clone();
    record.summary.failure_reason = failure_reason.clone();
    record.summary.state_revision =
        WireSequence::new(record.summary.state_revision.get().saturating_add(1));
    record.summary.updated_at_unix_ms = unix_time_ms();
    emit_payload(
        record,
        SshSessionEventPayload::StateChanged {
            previous_state,
            state,
            close_reason,
            failure_reason,
        },
    );
}

fn emit_payload(record: &mut SessionRecord, payload: SshSessionEventPayload) {
    let event_seq = record.summary.event_seq.get().saturating_add(1);
    record.summary.event_seq = WireSequence::new(event_seq);
    let event = SshSessionEvent {
        schema_version: SSH_TERMINAL_EVENT_SCHEMA_VERSION,
        session_id: record.summary.session_id.clone(),
        generation: record.summary.generation,
        state_revision: record.summary.state_revision,
        event_seq: WireSequence::new(event_seq),
        occurred_at_unix_ms: unix_time_ms(),
        payload,
    };
    for attachment in record.attachments.values() {
        let _ = attachment.events.send(event.clone());
    }
}

fn trim_output_ring(record: &mut SessionRecord) {
    let mut first_dropped = None;
    let mut last_dropped = None;
    while record.output_ring_bytes > OUTPUT_RING_MAX_BYTES {
        let Some(item) = record.output_ring.pop_front() else {
            break;
        };
        if let SshSessionOutputItem::Frame(frame) = item {
            record.output_ring_bytes = record.output_ring_bytes.saturating_sub(frame.bytes.len());
            first_dropped.get_or_insert(frame.output_seq);
            last_dropped = Some(frame.output_seq);
        }
    }
    if let (Some(dropped_from_output_seq), Some(last)) = (first_dropped, last_dropped) {
        while matches!(
            record.output_ring.front(),
            Some(SshSessionOutputItem::Gap(_))
        ) {
            record.output_ring.pop_front();
        }
        let resumes_at_output_seq = record
            .output_ring
            .front()
            .and_then(|item| match item {
                SshSessionOutputItem::Frame(frame) => Some(frame.output_seq),
                SshSessionOutputItem::Gap(_) => None,
            })
            .unwrap_or(WireSequence::new(last.get().saturating_add(1)));
        let gap = SshSessionOutputGap {
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            channel_id: record.summary.channel_id.clone().expect("running channel"),
            dropped_from_output_seq,
            resumes_at_output_seq,
            reason: SshSessionOutputGapReason::RingBufferOverflow,
        };
        record
            .output_ring
            .push_front(SshSessionOutputItem::Gap(gap.clone()));
        emit_payload(record, SshSessionEventPayload::OutputGap { gap });
    }
}

fn replay_after(record: &SessionRecord, after: Option<WireSequence>) -> Vec<SshSessionOutputItem> {
    let after = after.map_or(0, WireSequence::get);
    record
        .output_ring
        .iter()
        .filter(|item| match item {
            SshSessionOutputItem::Frame(frame) => frame.output_seq.get() > after,
            SshSessionOutputItem::Gap(gap) => gap.resumes_at_output_seq.get() > after,
        })
        .cloned()
        .collect()
}

fn details_from(record: &SessionRecord) -> SshSessionDetails {
    SshSessionDetails {
        session: record.summary.clone(),
        attachments: record
            .attachments
            .values()
            .map(|value| value.summary.clone())
            .collect(),
        active_host_key_challenge: record
            .active_challenge
            .as_ref()
            .map(|value| value.challenge.clone()),
        active_keyboard_interactive_challenge: record
            .active_keyboard_interactive_challenge
            .as_ref()
            .map(|value| value.challenge.clone()),
        active_login_automation: record
            .active_login_automation
            .as_ref()
            .map(|value| value.progress.clone()),
        heartbeat: record.heartbeat.clone(),
        input_lease: record.input_lease.clone(),
    }
}

fn heartbeat_status_from_policy(policy: &ResolvedHeartbeatPolicy) -> SshSessionHeartbeatStatus {
    SshSessionHeartbeatStatus {
        policy_revision: policy.revision,
        mode: match &policy.policy {
            HeartbeatPolicy::Disabled => SshHeartbeatMode::Disabled,
            HeartbeatPolicy::TransportKeepalive { .. } => SshHeartbeatMode::TransportKeepalive,
            HeartbeatPolicy::ShellHeartbeat { .. } => SshHeartbeatMode::ShellHeartbeat,
        },
        transports: Vec::new(),
        shell: None,
    }
}

fn login_automation_progress(
    policy_revision: WireSequence,
    steps: &[LoginAutomationStepInput],
    step_index: usize,
    started_at_unix_ms: i64,
    total_deadline_unix_ms: i64,
    status: SshLoginAutomationStatus,
    failure_code: Option<SshLoginAutomationFailureCode>,
) -> SshLoginAutomationProgress {
    let step = steps
        .get(step_index)
        .expect("enabled login automation has a current step");
    let (step_kind, secret_label, timeout_seconds) = match step {
        LoginAutomationStepInput::Expect {
            timeout_seconds, ..
        } => (SshLoginAutomationStepKind::Expect, None, *timeout_seconds),
        LoginAutomationStepInput::SendText {
            timeout_seconds, ..
        } => (SshLoginAutomationStepKind::SendText, None, *timeout_seconds),
        LoginAutomationStepInput::SendSecret {
            secret_label,
            timeout_seconds,
            ..
        } => (
            SshLoginAutomationStepKind::SendSecret,
            Some(secret_label.clone()),
            *timeout_seconds,
        ),
        LoginAutomationStepInput::PreserveExistingSecret {
            timeout_seconds, ..
        } => (
            SshLoginAutomationStepKind::SendSecret,
            None,
            *timeout_seconds,
        ),
    };
    SshLoginAutomationProgress {
        policy_revision,
        current_step_index: u8::try_from(step_index).expect("automation step index is bounded"),
        total_steps: u8::try_from(steps.len()).expect("automation step count is bounded"),
        step_kind,
        secret_label,
        status,
        failure_code,
        started_at_unix_ms,
        step_deadline_unix_ms: unix_time_ms()
            .saturating_add(i64::from(timeout_seconds).saturating_mul(1_000))
            .min(total_deadline_unix_ms),
    }
}

fn login_automation_step_timeout_duration(step: &LoginAutomationStepInput) -> Duration {
    Duration::from_secs(u64::from(match step {
        LoginAutomationStepInput::Expect {
            timeout_seconds, ..
        }
        | LoginAutomationStepInput::SendText {
            timeout_seconds, ..
        }
        | LoginAutomationStepInput::SendSecret {
            timeout_seconds, ..
        }
        | LoginAutomationStepInput::PreserveExistingSecret {
            timeout_seconds, ..
        } => *timeout_seconds,
    }))
}

fn append_login_automation_matcher(buffer: &mut Vec<u8>, bytes: &[u8]) -> bool {
    if buffer.len().saturating_add(bytes.len()) > LOGIN_AUTOMATION_MATCH_BUFFER_MAX_BYTES {
        return false;
    }
    buffer.extend_from_slice(bytes);
    true
}

fn find_literal_bytes(buffer: &[u8], literal: &[u8]) -> Option<usize> {
    if literal.is_empty() || literal.len() > buffer.len() {
        return None;
    }
    buffer
        .windows(literal.len())
        .position(|candidate| candidate == literal)
}

fn login_automation_payload(text: &[u8], append_enter: bool) -> Zeroizing<Vec<u8>> {
    let mut payload = Zeroizing::new(Vec::with_capacity(text.len() + usize::from(append_enter)));
    payload.extend_from_slice(text);
    if append_enter {
        payload.push(b'\r');
    }
    payload
}

fn clear_keyboard_interactive_challenge(record: &mut SessionRecord, error: TransportError) -> bool {
    record.prepared_keyboard_interactive_answers.clear();
    let Some(active) = record.active_keyboard_interactive_challenge.take() else {
        return false;
    };
    let _ = active.answers.send(Err(error));
    record.summary.state_revision =
        WireSequence::new(record.summary.state_revision.get().saturating_add(1));
    record.summary.updated_at_unix_ms = unix_time_ms();
    emit_payload(
        record,
        SshSessionEventPayload::KeyboardInteractiveChallengeChanged { challenge: None },
    );
    true
}

fn clear_login_automation(record: &mut SessionRecord) -> Option<PendingLoginAutomationTakeover> {
    let mut automation = record.active_login_automation.take()?;
    automation.matcher_buffer.clear();
    automation.steps.clear();
    let pending = automation.pending_takeover.take();
    emit_payload(
        record,
        SshSessionEventPayload::LoginAutomationProgressChanged { progress: None },
    );
    pending
}

fn abort_heartbeat_tasks(record: &mut SessionRecord) {
    let status_changed = record.shell_heartbeat_write_in_flight
        || record
            .heartbeat
            .transports
            .iter()
            .any(|status| status.next_due_at_unix_ms.is_some())
        || record
            .heartbeat
            .shell
            .as_ref()
            .is_some_and(|status| status.next_due_at_unix_ms.is_some());
    record.input_activity_epoch.fetch_add(1, Ordering::AcqRel);
    for task in record.transport_heartbeat_tasks.drain(..) {
        task.abort();
    }
    if let Some(task) = record.shell_heartbeat_task.take() {
        task.abort();
    }
    record.shell_heartbeat_write_in_flight = false;
    for status in &mut record.heartbeat.transports {
        status.next_due_at_unix_ms = None;
    }
    if let Some(status) = record.heartbeat.shell.as_mut() {
        status.next_due_at_unix_ms = None;
    }
    if status_changed {
        let heartbeat = record.heartbeat.clone();
        emit_payload(
            record,
            SshSessionEventPayload::HeartbeatChanged { heartbeat },
        );
    }
}

fn request_shell_stop(record: &mut SessionRecord, signal: ShellStopSignal) -> Result<(), ()> {
    let watch_accepted = record
        .shell_stop
        .take()
        .is_some_and(|stop| stop.send(Some(signal.clone())).is_ok());
    let mailbox_accepted = record.shell.as_ref().is_some_and(|shell| {
        let command = match signal {
            ShellStopSignal::Disconnect(reason) => ShellCommand::Disconnect(reason),
            ShellStopSignal::Abort => ShellCommand::Abort,
        };
        shell.try_send(command).is_ok()
    });
    if watch_accepted || mailbox_accepted {
        Ok(())
    } else {
        Err(())
    }
}

fn shell_heartbeat_payload(payload_text: &str, line_ending: ShellHeartbeatLineEnding) -> Vec<u8> {
    let mut payload = payload_text.as_bytes().to_vec();
    match line_ending {
        ShellHeartbeatLineEnding::None => {}
        ShellHeartbeatLineEnding::Cr => payload.push(b'\r'),
        ShellHeartbeatLineEnding::Lf => payload.push(b'\n'),
        ShellHeartbeatLineEnding::Crlf => payload.extend_from_slice(b"\r\n"),
    }
    payload
}

fn fail_login_automation_record(
    record: &mut SessionRecord,
    failure_code: SshLoginAutomationFailureCode,
) -> bool {
    let Some(automation) = record.active_login_automation.as_mut() else {
        return false;
    };
    automation.progress.status = SshLoginAutomationStatus::Failed;
    automation.progress.failure_code = Some(failure_code);
    automation.matcher_buffer.clear();
    automation.steps.clear();
    automation.timeout_scheduled_for = None;
    let progress = automation.progress.clone();
    emit_payload(
        record,
        SshSessionEventPayload::LoginAutomationProgressChanged {
            progress: Some(progress),
        },
    );
    true
}

fn validate_attachment(
    record: &SessionRecord,
    generation: WireSequence,
    attachment_id: &SshAttachmentId,
    view_id: &norishell_core_api::SshViewId,
    request_id: &RequestId,
) -> ActorResult<()> {
    if record.summary.generation != generation {
        return Err(conflict_error(request_id.clone()));
    }
    let attachment = record
        .attachments
        .get(attachment_id.as_str())
        .ok_or_else(|| not_found_error(request_id.clone()))?;
    if attachment.summary.generation != generation || attachment.summary.view_id != *view_id {
        return Err(conflict_error(request_id.clone()));
    }
    Ok(())
}

fn remove_attachment(
    record: &mut SessionRecord,
    attachment_id: &SshAttachmentId,
) -> Option<SshSessionAttachment> {
    let removed = record.attachments.remove(attachment_id.as_str())?;
    if record
        .active_login_automation
        .as_ref()
        .is_some_and(|automation| {
            automation.owner_attachment_id == removed.summary.attachment_id
                && automation.owner_view_id == removed.summary.view_id
        })
    {
        fail_login_automation_record(record, SshLoginAutomationFailureCode::AttachmentUnavailable);
    }
    if record
        .input_lease
        .as_ref()
        .is_some_and(|lease| lease.attachment_id == *attachment_id)
    {
        record.input_lease = None;
        emit_payload(
            record,
            SshSessionEventPayload::InputLeaseChanged {
                change: SshSessionInputLeaseChange::Released,
                lease: None,
            },
        );
    }
    let next_revision = record.summary.attachment_revision.get().saturating_add(1);
    record.summary.attachment_revision = WireSequence::new(next_revision);
    record.summary.attachment_count = record.attachments.len() as u32;
    emit_payload(
        record,
        SshSessionEventPayload::AttachmentChanged {
            change: SshSessionAttachmentChange::Detached,
            attachment_revision: WireSequence::new(next_revision),
            attachment: removed.summary.clone(),
        },
    );
    Some(removed.summary)
}

#[allow(clippy::too_many_arguments)]
fn validate_lease(
    record: &SessionRecord,
    generation: WireSequence,
    attachment_id: &SshAttachmentId,
    view_id: &norishell_core_api::SshViewId,
    lease_id: &SshInputLeaseId,
    input_epoch: WireSequence,
    request_id: &RequestId,
    require_unexpired: bool,
) -> ActorResult<()> {
    validate_attachment(record, generation, attachment_id, view_id, request_id)?;
    let lease = record
        .input_lease
        .as_ref()
        .ok_or_else(|| conflict_error(request_id.clone()))?;
    if lease.lease_id != *lease_id
        || lease.input_epoch != input_epoch
        || lease.attachment_id != *attachment_id
        || lease.view_id != *view_id
        || (require_unexpired && lease.expires_at_unix_ms <= unix_time_ms())
    {
        return Err(conflict_error(request_id.clone()));
    }
    Ok(())
}

fn failure_from_route(
    error: TransportError,
    route_stage: Option<SshSessionRouteStage>,
) -> SshSessionFailureReason {
    let algorithm_negotiation = match &error {
        TransportError::AlgorithmNegotiationFailed {
            category,
            client_candidates,
            server_candidates,
        } => Some(SshAlgorithmNegotiationFailure {
            category: match category {
                norishell_ssh_transport::AlgorithmCategory::KeyExchange => {
                    WireAlgorithmCategory::KeyExchange
                }
                norishell_ssh_transport::AlgorithmCategory::HostKey => {
                    WireAlgorithmCategory::HostKey
                }
                norishell_ssh_transport::AlgorithmCategory::Cipher => WireAlgorithmCategory::Cipher,
                norishell_ssh_transport::AlgorithmCategory::Mac => WireAlgorithmCategory::Mac,
            },
            client_candidates: client_candidates.clone(),
            server_candidates: server_candidates.clone(),
        }),
        _ => None,
    };
    let (code, stage, retry_strategy, message_key) = match error {
        TransportError::InvalidEndpoint(_) => (
            SshSessionFailureCode::ResolutionFailed,
            SshSessionFailureStage::Resolving,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.resolutionFailed",
        ),
        TransportError::ConnectTimeout => (
            SshSessionFailureCode::ConnectionTimedOut,
            SshSessionFailureStage::Connecting,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.connectionTimedOut",
        ),
        TransportError::ConnectFailed => (
            SshSessionFailureCode::ConnectionRefused,
            SshSessionFailureStage::Connecting,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.connectionFailed",
        ),
        TransportError::RouteIngress(error) => {
            let (code, message_key) = match (error.stage, error.kind) {
                (_, IngressFailureKind::CredentialLocked) => (
                    SshSessionFailureCode::ProxyCredentialLocked,
                    "errors.sshSession.proxyCredentialLocked",
                ),
                (_, IngressFailureKind::CredentialUnavailable) => (
                    SshSessionFailureCode::ProxyCredentialUnavailable,
                    "errors.sshSession.proxyCredentialUnavailable",
                ),
                (_, IngressFailureKind::Rejected) => (
                    SshSessionFailureCode::ProxyRejected,
                    "errors.sshSession.proxyRejected",
                ),
                (IngressStage::ProxyTcpConnect | IngressStage::LocalDnsResolution, _)
                | (_, IngressFailureKind::Timeout | IngressFailureKind::Io) => (
                    SshSessionFailureCode::ProxyConnectionFailed,
                    "errors.sshSession.proxyConnectionFailed",
                ),
                _ => (
                    SshSessionFailureCode::ProxyProtocolError,
                    "errors.sshSession.proxyProtocolError",
                ),
            };
            let retry_strategy = if error.kind == IngressFailureKind::CredentialLocked {
                SshSessionRetryStrategy::UnlockVault
            } else {
                SshSessionRetryStrategy::RetryOpen
            };
            (
                code,
                SshSessionFailureStage::RouteIngress,
                retry_strategy,
                message_key,
            )
        }
        TransportError::Jump(_) => (
            SshSessionFailureCode::JumpChannelFailed,
            SshSessionFailureStage::RouteIngress,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.jumpChannelFailed",
        ),
        TransportError::HostKeyRejected => (
            SshSessionFailureCode::HostKeyRejected,
            SshSessionFailureStage::HostKeyVerification,
            SshSessionRetryStrategy::ReviewHostKey,
            "errors.sshSession.hostKeyRejected",
        ),
        TransportError::HostKeyMismatch { .. } => (
            SshSessionFailureCode::HostKeyMismatch,
            SshSessionFailureStage::HostKeyVerification,
            SshSessionRetryStrategy::ReviewHostKey,
            "errors.sshSession.hostKeyMismatch",
        ),
        TransportError::HostKeyDecisionTimeout => (
            SshSessionFailureCode::HostKeyDecisionExpired,
            SshSessionFailureStage::HostKeyVerification,
            SshSessionRetryStrategy::ReviewHostKey,
            "errors.sshSession.hostKeyDecisionExpired",
        ),
        TransportError::AuthenticationRejected
        | TransportError::InvalidPrivateKey
        | TransportError::InsecureRsaSignatureOnly => (
            SshSessionFailureCode::AuthenticationRejected,
            SshSessionFailureStage::Authentication,
            SshSessionRetryStrategy::ChooseCredential,
            "errors.sshSession.authenticationRejected",
        ),
        TransportError::SshAgentKeyUnavailable => (
            SshSessionFailureCode::SshAgentKeyUnavailable,
            SshSessionFailureStage::Authentication,
            SshSessionRetryStrategy::ChooseCredential,
            "errors.sshSession.sshAgentKeyUnavailable",
        ),
        TransportError::SshAgentUnavailable => (
            SshSessionFailureCode::SshAgentUnavailable,
            SshSessionFailureStage::Authentication,
            SshSessionRetryStrategy::ChooseCredential,
            "errors.sshSession.sshAgentUnavailable",
        ),
        TransportError::AuthenticationTimeout => (
            SshSessionFailureCode::AuthenticationFailed,
            SshSessionFailureStage::Authentication,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.authenticationTimedOut",
        ),
        TransportError::AuthenticationIncomplete
        | TransportError::InvalidKeyboardInteractiveResponse => (
            SshSessionFailureCode::AuthenticationFailed,
            SshSessionFailureStage::Authentication,
            SshSessionRetryStrategy::ChooseCredential,
            "errors.sshSession.authenticationFailed",
        ),
        TransportError::AlgorithmNegotiationFailed { .. } => (
            SshSessionFailureCode::AlgorithmNegotiationFailed,
            SshSessionFailureStage::Connecting,
            SshSessionRetryStrategy::Never,
            "errors.sshSession.algorithmNegotiationFailed",
        ),
        TransportError::ChannelOpenTimeout
        | TransportError::PtyRejected
        | TransportError::PtyRequestTimeout
        | TransportError::ShellRejected
        | TransportError::ShellRequestTimeout => (
            SshSessionFailureCode::ChannelOpenFailed,
            SshSessionFailureStage::ChannelOpen,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.channelOpenFailed",
        ),
        TransportError::ConnectionLost | TransportError::DisconnectTimeout => (
            SshSessionFailureCode::ConnectionLost,
            SshSessionFailureStage::Running,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.connectionLost",
        ),
        _ => (
            SshSessionFailureCode::ProtocolError,
            SshSessionFailureStage::Connecting,
            SshSessionRetryStrategy::RetryOpen,
            "errors.sshSession.protocolError",
        ),
    };
    SshSessionFailureReason {
        code,
        stage,
        route_stage,
        algorithm_negotiation,
        retry_strategy,
        message_key: message_key.to_owned(),
        diagnostic_id: None,
    }
}

fn validation_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "ssh_terminal.invalid_request",
        ErrorCategory::Validation,
        RetryStrategy::Never,
        "errors.sshSession.invalidRequest",
    )
}

fn plugin_authorization_error(request_id: RequestId) -> Box<CoreApiError> {
    crate::core_api_error::core_error(
        request_id,
        "plugin.permission_denied",
        ErrorCategory::Permission,
        RetryStrategy::Never,
        "errors.plugin.capabilityDenied",
    )
}

fn conflict_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "ssh_terminal.stale_fence",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.sshSession.staleFence",
    )
}

fn stale_focus_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "ssh_terminal.stale_focus",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.sshSession.staleFocus",
    )
}

fn credential_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "ssh_terminal.credential_unavailable",
        ErrorCategory::Conflict,
        RetryStrategy::WaitForUser,
        "errors.sshSession.credentialUnavailable",
    )
}

fn map_connection_profile_error(
    request_id: RequestId,
    error: ConnectionProfileError,
) -> Box<CoreApiError> {
    match error {
        ConnectionProfileError::InvalidTarget
        | ConnectionProfileError::UnsupportedConfiguration => validation_error(request_id),
        ConnectionProfileError::LoginAutomationConfirmationRequired => core_error(
            request_id,
            "ssh_terminal.login_automation_confirmation_required",
            ErrorCategory::Conflict,
            RetryStrategy::WaitForUser,
            "errors.sshSession.loginAutomationConfirmationRequired",
        ),
        ConnectionProfileError::StaleHost => conflict_error(request_id),
        ConnectionProfileError::CredentialUnavailable => credential_error(request_id),
        ConnectionProfileError::PersistenceUnavailable => unavailable_error(request_id),
    }
}

fn unavailable_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "ssh_terminal.unavailable",
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.sshSession.unavailable",
    )
}

fn not_found_error(request_id: RequestId) -> Box<CoreApiError> {
    core_error(
        request_id,
        "ssh_terminal.not_found",
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.sshSession.notFound",
    )
}

fn replay_control_operation<Outcome: Clone>(
    ledger: &SshOperationLedger<SshOperationFingerprint, StoredSshOperationResult<Outcome>>,
    operation_id: &str,
    idempotency_key: &str,
    fingerprint: &SshOperationFingerprint,
    request_id: RequestId,
) -> Option<ActorResult<Outcome>> {
    match ledger.lookup(operation_id, idempotency_key, fingerprint) {
        SshOperationLookup::Missing => None,
        SshOperationLookup::Replay(stored) => Some(stored.replay(request_id)),
        SshOperationLookup::Conflict => Some(Err(conflict_error(request_id))),
    }
}

fn record_control_operation<Outcome: Clone>(
    ledger: &mut SshOperationLedger<SshOperationFingerprint, StoredSshOperationResult<Outcome>>,
    operation_id: String,
    idempotency_key: String,
    fingerprint: SshOperationFingerprint,
    request_id: RequestId,
    result: ActorResult<Outcome>,
) -> ActorResult<Outcome> {
    let stored = StoredSshOperationResult::from_actor_result(result);
    match ledger.record(operation_id, idempotency_key, fingerprint, stored) {
        SshOperationLookup::Replay(stored) => stored.replay(request_id),
        SshOperationLookup::Conflict => Err(conflict_error(request_id)),
        SshOperationLookup::Missing => unreachable!("record always returns a terminal lookup"),
    }
}

#[tauri::command]
pub async fn ssh_terminal_open(
    request: SshSessionOpenRequest,
    on_event: Channel<SshSessionEvent>,
    service: State<'_, SshSessionService>,
    plugin_service: State<'_, crate::plugin_service::PluginService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<SshSessionOpenResponse> {
    let request_id = request.meta.request_id.clone();
    let terminal_startup = if let Some(token) = request.plugin_authorization_token.as_ref() {
        plugin_service.consume_terminal_launch_authorization(
            request_id.clone(),
            token,
            &request.target,
        )?
    } else {
        None
    };
    if terminal_startup.is_some() && request.credential_ref_id.is_some() {
        return Err(plugin_authorization_error(request_id));
    }
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .request(
            |reply| Message::Open {
                terminal_startup: terminal_startup.map(Box::new),
                request,
                events: on_event,
                reply,
            },
            request_id.clone(),
        )
        .await
}

#[tauri::command]
pub async fn ssh_terminal_snapshot(
    request: SshSessionSnapshotRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<SshSessionSnapshot> {
    service
        .request(|reply| Message::Snapshot { reply }, request.meta.request_id)
        .await
}

#[tauri::command]
pub async fn ssh_terminal_get(
    request: SshSessionGetRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<SshSessionDetails> {
    let request_id = request.meta.request_id;
    service
        .request(
            |reply| Message::Get {
                session_id: request.session_id.as_str().to_owned(),
                request_id: request_id.clone(),
                reply,
            },
            request_id.clone(),
        )
        .await
}

#[tauri::command]
pub async fn ssh_terminal_input_focus_snapshot(
    request: SshTerminalInputFocusSnapshotRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<SshTerminalInputFocusSnapshot> {
    service
        .linearized_request(
            |reply| Message::FocusSnapshot { reply },
            request.meta.request_id,
        )
        .await
}

#[tauri::command]
pub async fn terminal_input_focus_snapshot(
    request: TerminalInputFocusSnapshotRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<TerminalInputFocusSnapshot> {
    service
        .terminal_focus_snapshot_internal(request.meta.request_id)
        .await
}

#[tauri::command]
pub async fn local_terminal_open(
    request: LocalSessionOpenRequest,
    on_event: Channel<LocalSessionEvent>,
    service: State<'_, SshSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<LocalSessionOpenResponse> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .request(
            |reply| Message::LocalOpen {
                request,
                events: on_event,
                reply,
            },
            request_id,
        )
        .await
}

#[tauri::command]
pub async fn local_terminal_snapshot(
    request: LocalSessionSnapshotRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<LocalSessionSnapshot> {
    service
        .request(
            |reply| Message::LocalSnapshot { reply },
            request.meta.request_id,
        )
        .await
}

#[tauri::command]
pub async fn local_terminal_get(
    request: LocalSessionGetRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<LocalSessionDetails> {
    let request_id = request.meta.request_id.clone();
    service
        .request(|reply| Message::LocalGet { request, reply }, request_id)
        .await
}

#[tauri::command]
pub async fn local_terminal_attach(
    request: LocalSessionAttachRequest,
    on_event: Channel<LocalSessionEvent>,
    service: State<'_, SshSessionService>,
) -> CoreResult<LocalSessionAttachResponse> {
    let request_id = request.meta.request_id.clone();
    service
        .linearized_request(
            |reply| Message::LocalAttach {
                request,
                events: on_event,
                reply,
            },
            request_id,
        )
        .await
}

#[tauri::command]
pub async fn local_terminal_attachment_heartbeat(
    request: LocalSessionAttachmentHeartbeatRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<LocalSessionAttachment> {
    let request_id = request.meta.request_id.clone();
    service
        .request(
            |reply| Message::LocalAttachmentHeartbeat { request, reply },
            request_id,
        )
        .await
}

#[tauri::command]
pub async fn ssh_terminal_attach(
    request: SshSessionAttachRequest,
    on_event: Channel<SshSessionEvent>,
    service: State<'_, SshSessionService>,
) -> CoreResult<SshSessionAttachResponse> {
    let request_id = request.meta.request_id.clone();
    service
        .linearized_request(
            |reply| Message::Attach {
                request,
                events: on_event,
                reply,
            },
            request_id,
        )
        .await
}

#[tauri::command]
pub async fn ssh_terminal_attachment_heartbeat(
    request: SshSessionAttachmentHeartbeatRequest,
    service: State<'_, SshSessionService>,
) -> CoreResult<SshSessionAttachment> {
    let request_id = request.meta.request_id.clone();
    service
        .request(
            |reply| Message::AttachmentHeartbeat { request, reply },
            request_id,
        )
        .await
}

macro_rules! request_command {
    ($name:ident, $request:ty, $response:ty, $variant:ident) => {
        #[tauri::command]
        pub async fn $name(
            request: $request,
            service: State<'_, SshSessionService>,
        ) -> CoreResult<$response> {
            let request_id = request.meta.request_id.clone();
            service
                .request(|reply| Message::$variant { request, reply }, request_id)
                .await
        }
    };
}

macro_rules! linearized_request_command {
    ($name:ident, $request:ty, $response:ty, $variant:ident) => {
        #[tauri::command]
        pub async fn $name(
            request: $request,
            service: State<'_, SshSessionService>,
        ) -> CoreResult<$response> {
            let request_id = request.meta.request_id.clone();
            service
                .linearized_request(|reply| Message::$variant { request, reply }, request_id)
                .await
        }
    };
}

linearized_request_command!(
    ssh_terminal_detach,
    SshSessionDetachRequest,
    SshSessionDetachResult,
    Detach
);
request_command!(
    ssh_terminal_host_key_decide,
    SshHostKeyDecisionRequest,
    SshSessionDetails,
    HostKeyDecide
);
request_command!(
    ssh_terminal_keyboard_interactive_answer_prepare,
    SshKeyboardInteractiveAnswerPrepareRequest,
    SshKeyboardInteractiveAnswerPrepareResponse,
    KeyboardInteractiveAnswerPrepare
);
request_command!(
    ssh_terminal_keyboard_interactive_respond,
    SshKeyboardInteractiveResponseRequest,
    SshSessionDetails,
    KeyboardInteractiveRespond
);
linearized_request_command!(
    ssh_terminal_login_automation_takeover,
    SshLoginAutomationTakeoverRequest,
    SshLoginAutomationTakeoverResponse,
    LoginAutomationTakeover
);
linearized_request_command!(
    ssh_terminal_input_focus_change,
    SshTerminalInputFocusChangeRequest,
    SshTerminalInputFocusChangeResponse,
    FocusChange
);

#[tauri::command]
pub async fn terminal_input_focus_change(
    request: TerminalInputFocusChangeRequest,
    service: State<'_, SshSessionService>,
    telnet: State<'_, crate::telnet_session_service::TelnetSessionService>,
    plugin: State<'_, crate::plugin_terminal_session_service::PluginTerminalSessionService>,
) -> CoreResult<TerminalInputFocusChangeResponse> {
    telnet.ensure_focus_coordinator(service.inner().clone());
    let broker = service.focus_broker();
    let operation_request = request.clone();
    broker
        .linearize_focus_change(
            &request,
            terminal_input_focus_change_once(operation_request, &service, &telnet, Some(&plugin)),
        )
        .await
}

async fn terminal_input_focus_change_once(
    request: TerminalInputFocusChangeRequest,
    service: &SshSessionService,
    telnet: &crate::telnet_session_service::TelnetSessionService,
    plugin: Option<&crate::plugin_terminal_session_service::PluginTerminalSessionService>,
) -> CoreResult<TerminalInputFocusChangeResponse> {
    let request_id = request.meta.request_id.clone();
    let previous = service
        .request(
            |reply| Message::TerminalFocusSnapshot { reply },
            request_id.clone(),
        )
        .await?;
    let mut response = service
        .request(
            |reply| Message::TerminalFocusChange { request, reply },
            request_id.clone(),
        )
        .await?;

    if previous.focus_epoch != response.focus_epoch
        && let Some(TerminalInputFocusTarget::Telnet(previous_target)) = previous.target.clone()
    {
        let _ = telnet
            .revoke_input(
                previous_target.session_id.as_str().to_owned(),
                previous.focus_epoch.get(),
            )
            .await;
    }

    if previous.focus_epoch != response.focus_epoch
        && let (Some(plugin), Some(TerminalInputFocusTarget::Plugin(target))) =
            (plugin, previous.target.as_ref())
        && let Ok(session_id) = uuid::Uuid::parse_str(&target.session_id)
    {
        let _ = plugin
            .revoke_input(session_id, previous.focus_epoch.get())
            .await;
    }
    if let Some(TerminalInputFocusTarget::Plugin(target)) = response.target.as_ref() {
        let grant = async {
            let plugin = plugin.ok_or_else(|| conflict_error(request_id.clone()))?;
            let parse = |value: &str| {
                uuid::Uuid::parse_str(value).map_err(|_| conflict_error(request_id.clone()))
            };
            plugin
                .grant_input(
                    crate::plugin_terminal_session_service::PluginTerminalInputGrantRequest {
                        session_id: parse(&target.session_id)?,
                        expected_generation: target.expected_generation.get(),
                        expected_stream_id: parse(&target.stream_id)?,
                        attachment_id: parse(&target.attachment_id)?,
                        view_id: target.view_id.clone(),
                        expected_state_revision: target.expected_state_revision.get(),
                        focus_epoch: response.focus_epoch.get(),
                    },
                )
                .await
                .map_err(|_| conflict_error(request_id.clone()))
        }
        .await;
        match grant {
            Ok(lease) => {
                response.lease = Some(TerminalInputLease::Plugin(
                    crate::plugin_service::protocol_terminal::project_lease(&lease),
                ))
            }
            Err(error) => {
                let _ = service
                    .clear_plugin_focus_exact_unserialized(
                        target.clone(),
                        response.focus_epoch.get(),
                    )
                    .await;
                return Err(error);
            }
        }
    }

    if let Some(TerminalInputFocusTarget::Telnet(target)) = response.target.as_ref() {
        let lease = telnet
            .grant_input(crate::telnet_session_service::TelnetInputGrantRequest {
                session_id: target.session_id.as_str().to_owned(),
                expected_generation: target.expected_generation.get(),
                expected_socket_id: target.socket_id.as_str().to_owned(),
                attachment_id: target.attachment_id.as_str().to_owned(),
                view_id: target.view_id.as_str().to_owned(),
                expected_state_revision: target.expected_state_revision.get(),
                focus_epoch: response.focus_epoch.get(),
            })
            .await;
        match lease {
            Ok(lease) => {
                response.lease = Some(TerminalInputLease::Telnet(
                    norishell_core_api::TelnetInputLease {
                        lease_id: norishell_core_api::TelnetInputLeaseId::parse(lease.lease_id)
                            .map_err(|_| conflict_error(request_id.clone()))?,
                        session_id: target.session_id.clone(),
                        generation: WireSequence::new(lease.generation),
                        socket_id: target.socket_id.clone(),
                        attachment_id: target.attachment_id.clone(),
                        view_id: target.view_id.clone(),
                        focus_epoch: WireSequence::new(lease.focus_epoch),
                        input_epoch: WireSequence::new(lease.input_epoch),
                        expires_at_unix_ms: lease.expires_at_unix_ms,
                    },
                ));
            }
            Err(_) => {
                let rollback = TerminalInputFocusChangeRequest {
                    meta: norishell_core_api::RequestMeta {
                        request_id: request_id.clone(),
                    },
                    operation_id: norishell_core_api::OperationId::new(),
                    idempotency_key: format!("telnet-focus-rollback-{}", uuid::Uuid::now_v7()),
                    expected_focus_epoch: response.focus_epoch,
                    target: None,
                };
                let _ = service
                    .request(
                        |reply| Message::TerminalFocusChange {
                            request: rollback,
                            reply,
                        },
                        request_id.clone(),
                    )
                    .await;
                return Err(conflict_error(request_id));
            }
        }
    }
    let canonical = service
        .request(
            |reply| Message::TerminalFocusSnapshot { reply },
            request_id.clone(),
        )
        .await?;
    if canonical.focus_epoch != response.focus_epoch || canonical.target != response.target {
        if let (Some(plugin), Some(TerminalInputLease::Plugin(lease))) =
            (plugin, response.lease.as_ref())
            && let Ok(session_id) = uuid::Uuid::parse_str(&lease.session_id)
        {
            let _ = plugin
                .revoke_input(session_id, lease.focus_epoch.get())
                .await;
        }
        if let Some(TerminalInputLease::Telnet(lease)) = response.lease.as_ref() {
            let _ = telnet
                .revoke_input(
                    lease.session_id.as_str().to_owned(),
                    lease.focus_epoch.get(),
                )
                .await;
        }
        return Err(conflict_error(request_id));
    }
    Ok(response)
}
linearized_request_command!(
    ssh_terminal_input_lease_renew,
    SshSessionInputLeaseRenewRequest,
    SshSessionInputLease,
    LeaseRenew
);
linearized_request_command!(ssh_terminal_input, SshSessionInputRequest, (), Input);
linearized_request_command!(ssh_terminal_resize, SshSessionResizeRequest, (), Resize);
#[tauri::command]
pub async fn ssh_terminal_reconnect(
    request: SshSessionReconnectRequest,
    service: State<'_, SshSessionService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<SshSessionDetails> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    service
        .linearized_request(|reply| Message::Reconnect { request, reply }, request_id)
        .await
}
linearized_request_command!(
    ssh_terminal_disconnect,
    SshSessionDisconnectRequest,
    SshSessionDetails,
    Disconnect
);
linearized_request_command!(
    local_terminal_detach,
    LocalSessionDetachRequest,
    LocalSessionDetachResult,
    LocalDetach
);
linearized_request_command!(
    local_terminal_input_lease_renew,
    LocalSessionInputLeaseRenewRequest,
    LocalSessionInputLease,
    LocalLeaseRenew
);
linearized_request_command!(
    local_terminal_input,
    LocalSessionInputRequest,
    (),
    LocalInput
);
linearized_request_command!(
    local_terminal_resize,
    LocalSessionResizeRequest,
    (),
    LocalResize
);
linearized_request_command!(
    local_terminal_terminate,
    LocalSessionTerminateRequest,
    LocalSessionSummary,
    LocalTerminate
);

struct SessionRecord {
    shared_channels: Option<norishell_ssh_transport::SharedSessionChannels>,
    channel_lease_cancel: watch::Sender<bool>,
    terminal_startup: Option<crate::plugin_service::ApprovedPluginTerminalStartup>,
    summary: SshSessionSummary,
    attachments: BTreeMap<String, AttachmentRecord>,
    active_challenge: Option<ActiveChallenge>,
    active_keyboard_interactive_challenge: Option<ActiveKeyboardInteractiveChallenge>,
    prepared_keyboard_interactive_answers: BTreeMap<String, PreparedKeyboardInteractiveAnswer>,
    active_login_automation: Option<ActiveLoginAutomation>,
    heartbeat_policy: ResolvedHeartbeatPolicy,
    heartbeat: SshSessionHeartbeatStatus,
    transport_heartbeat_tasks: Vec<HeartbeatTask>,
    shell_heartbeat_task: Option<HeartbeatTask>,
    last_user_input_at: Option<tokio::time::Instant>,
    input_activity_epoch: Arc<AtomicU64>,
    shell_heartbeat_write_in_flight: bool,
    input_lease: Option<SshSessionInputLease>,
    next_input_epoch: u64,
    last_client_seq: u64,
    last_resize_seq: u64,
    next_output_seq: u64,
    output_ring: VecDeque<SshSessionOutputItem>,
    output_ring_bytes: usize,
    shell: Option<mpsc::Sender<ShellCommand>>,
    shell_stop: Option<watch::Sender<Option<ShellStopSignal>>>,
}

struct ActiveLoginAutomation {
    policy_revision: WireSequence,
    steps: Vec<LoginAutomationStepInput>,
    current_step_index: usize,
    matcher_buffer: Vec<u8>,
    timeout_scheduled_for: Option<usize>,
    total_deadline_unix_ms: i64,
    total_deadline: tokio::time::Instant,
    step_deadline: tokio::time::Instant,
    owner_attachment_id: SshAttachmentId,
    owner_view_id: norishell_core_api::SshViewId,
    write_in_flight: Option<usize>,
    pending_takeover: Option<PendingLoginAutomationTakeover>,
    progress: SshLoginAutomationProgress,
}

struct PendingLoginAutomationTakeover {
    request: SshLoginAutomationTakeoverRequest,
    replies: Vec<(
        RequestId,
        oneshot::Sender<ActorResult<SshLoginAutomationTakeoverResponse>>,
    )>,
}

struct GlobalFocusFence<'a> {
    session_id: &'a norishell_core_api::SshSessionId,
    generation: WireSequence,
    channel_id: Option<&'a SshChannelId>,
    attachment_id: &'a SshAttachmentId,
    view_id: &'a norishell_core_api::SshViewId,
    focus_epoch: WireSequence,
}

struct AttachmentRecord {
    summary: SshSessionAttachment,
    events: Channel<SshSessionEvent>,
    last_seen_at_unix_ms: i64,
}

struct ActiveChallenge {
    challenge: SshHostKeyChallenge,
    observed: ObservedHostKey,
    decision: oneshot::Sender<Result<HostKeyDecision, TransportError>>,
}

struct ActiveKeyboardInteractiveChallenge {
    challenge: SshKeyboardInteractiveChallenge,
    answers: oneshot::Sender<Result<Vec<Zeroizing<String>>, TransportError>>,
}

struct PreparedKeyboardInteractiveAnswer {
    challenge_id: SshKeyboardInteractiveChallengeId,
    generation: WireSequence,
    state_revision: WireSequence,
    round_index: u8,
    prompt_index: u8,
    expires_at_unix_ms: i64,
    answer: Zeroizing<String>,
}

enum Message {
    PluginChannelLease {
        request_id: RequestId,
        session_id: norishell_core_api::SshSessionId,
        generation: WireSequence,
        reply: oneshot::Sender<ActorResult<SessionChannelLease>>,
    },
    ShutdownAll {
        request_id: RequestId,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    ShutdownDeadline {
        token: Uuid,
    },
    Open {
        terminal_startup: Option<Box<crate::plugin_service::ApprovedPluginTerminalStartup>>,
        request: SshSessionOpenRequest,
        events: Channel<SshSessionEvent>,
        reply: oneshot::Sender<ActorResult<SshSessionOpenResponse>>,
    },
    Snapshot {
        reply: oneshot::Sender<ActorResult<SshSessionSnapshot>>,
    },
    Get {
        session_id: String,
        request_id: RequestId,
        reply: oneshot::Sender<ActorResult<SshSessionDetails>>,
    },
    FocusSnapshot {
        reply: oneshot::Sender<ActorResult<SshTerminalInputFocusSnapshot>>,
    },
    TerminalFocusSnapshot {
        reply: oneshot::Sender<ActorResult<TerminalInputFocusSnapshot>>,
    },
    FocusChange {
        request: SshTerminalInputFocusChangeRequest,
        reply: oneshot::Sender<ActorResult<SshTerminalInputFocusChangeResponse>>,
    },
    TerminalFocusChange {
        request: TerminalInputFocusChangeRequest,
        reply: oneshot::Sender<ActorResult<TerminalInputFocusChangeResponse>>,
    },
    PluginTerminalFocusInvalidateExact {
        target: norishell_core_api::PluginTerminalInputFocusTarget,
        focus_epoch: u64,
        reply: oneshot::Sender<ActorResult<TerminalInputFocusSnapshot>>,
    },
    TerminalFocusInvalidateExact {
        invalidation: crate::telnet_session_service::TelnetFocusInvalidation,
        reply: oneshot::Sender<ActorResult<TerminalInputFocusSnapshot>>,
    },
    LocalOpen {
        request: LocalSessionOpenRequest,
        events: Channel<LocalSessionEvent>,
        reply: oneshot::Sender<ActorResult<LocalSessionOpenResponse>>,
    },
    LocalSnapshot {
        reply: oneshot::Sender<ActorResult<LocalSessionSnapshot>>,
    },
    LocalGet {
        request: LocalSessionGetRequest,
        reply: oneshot::Sender<ActorResult<LocalSessionDetails>>,
    },
    LocalAttach {
        request: LocalSessionAttachRequest,
        events: Channel<LocalSessionEvent>,
        reply: oneshot::Sender<ActorResult<LocalSessionAttachResponse>>,
    },
    LocalAttachmentHeartbeat {
        request: LocalSessionAttachmentHeartbeatRequest,
        reply: oneshot::Sender<ActorResult<LocalSessionAttachment>>,
    },
    LocalDetach {
        request: LocalSessionDetachRequest,
        reply: oneshot::Sender<ActorResult<LocalSessionDetachResult>>,
    },
    LocalLeaseRenew {
        request: LocalSessionInputLeaseRenewRequest,
        reply: oneshot::Sender<ActorResult<LocalSessionInputLease>>,
    },
    LocalInput {
        request: LocalSessionInputRequest,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    LocalResize {
        request: LocalSessionResizeRequest,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    LocalTerminate {
        request: LocalSessionTerminateRequest,
        reply: oneshot::Sender<ActorResult<LocalSessionSummary>>,
    },
    Attach {
        request: SshSessionAttachRequest,
        events: Channel<SshSessionEvent>,
        reply: oneshot::Sender<ActorResult<SshSessionAttachResponse>>,
    },
    AttachmentHeartbeat {
        request: SshSessionAttachmentHeartbeatRequest,
        reply: oneshot::Sender<ActorResult<SshSessionAttachment>>,
    },
    Detach {
        request: SshSessionDetachRequest,
        reply: oneshot::Sender<ActorResult<SshSessionDetachResult>>,
    },
    HostKeyDecide {
        request: SshHostKeyDecisionRequest,
        reply: oneshot::Sender<ActorResult<SshSessionDetails>>,
    },
    KeyboardInteractiveAnswerPrepare {
        request: SshKeyboardInteractiveAnswerPrepareRequest,
        reply: oneshot::Sender<ActorResult<SshKeyboardInteractiveAnswerPrepareResponse>>,
    },
    KeyboardInteractiveRespond {
        request: SshKeyboardInteractiveResponseRequest,
        reply: oneshot::Sender<ActorResult<SshSessionDetails>>,
    },
    LoginAutomationTakeover {
        request: SshLoginAutomationTakeoverRequest,
        reply: oneshot::Sender<ActorResult<SshLoginAutomationTakeoverResponse>>,
    },
    #[cfg(test)]
    LeaseAcquire {
        request: SshSessionInputLeaseAcquireRequest,
        reply: oneshot::Sender<ActorResult<SshSessionInputLease>>,
    },
    LeaseRenew {
        request: SshSessionInputLeaseRenewRequest,
        reply: oneshot::Sender<ActorResult<SshSessionInputLease>>,
    },
    Input {
        request: SshSessionInputRequest,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    NativeTerminalEnableSsh {
        request_id: RequestId,
        prepared: PreparedNativeTerminalEnable,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    NativeTerminalEnableLocal {
        request_id: RequestId,
        prepared: PreparedNativeTerminalEnable,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    ApprovedPluginInput {
        request: ApprovedPluginInput,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    PluginObserveAttach {
        request: ApprovedPluginObservationAttach,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    PluginObserveDetach {
        observer_id: Uuid,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    Resize {
        request: SshSessionResizeRequest,
        reply: oneshot::Sender<ActorResult<()>>,
    },
    Reconnect {
        request: SshSessionReconnectRequest,
        reply: oneshot::Sender<ActorResult<SshSessionDetails>>,
    },
    Disconnect {
        request: SshSessionDisconnectRequest,
        reply: oneshot::Sender<ActorResult<SshSessionDetails>>,
    },
    ConnectionState {
        session_id: String,
        generation: u64,
        state: SshSessionState,
    },
    HostKeyObserved {
        session_id: String,
        generation: u64,
        endpoint: Endpoint,
        observed: ObservedHostKey,
        reply: oneshot::Sender<Result<HostKeyDecision, TransportError>>,
    },
    KeyboardInteractiveChallengeObserved {
        session_id: String,
        generation: u64,
        route_stage: SshSessionRouteStage,
        credential_ref_id: CredentialRefId,
        attempt_index: u8,
        round_index: u8,
        challenge: TransportKeyboardChallenge,
        answers: oneshot::Sender<Result<Vec<Zeroizing<String>>, TransportError>>,
    },
    ShellReady {
        channels: norishell_ssh_transport::SharedSessionChannels,
        session_id: String,
        generation: u64,
        credential_ref_id: Option<CredentialRefId>,
        negotiated_algorithms: Vec<SshNegotiatedAlgorithms>,
        login_automation: Option<ResolvedLoginAutomation>,
        automation_attachment_id: SshAttachmentId,
        automation_view_id: norishell_core_api::SshViewId,
        heartbeat_policy: ResolvedHeartbeatPolicy,
        transport_heartbeats: Vec<RoutedTransportHeartbeat>,
        shell: Box<SessionShell>,
    },
    ShellOutput {
        session_id: String,
        generation: u64,
        bytes: Vec<u8>,
    },
    LoginAutomationStepTimeout {
        session_id: String,
        generation: u64,
        state_revision: u64,
        step_index: usize,
    },
    LoginAutomationWriteCompleted {
        session_id: String,
        generation: u64,
        state_revision: u64,
        step_index: usize,
        succeeded: bool,
    },
    TransportHeartbeatScheduled {
        session_id: String,
        generation: u64,
        policy_revision: Option<WireSequence>,
        route_stage: SshSessionRouteStage,
        next_due_at_unix_ms: i64,
    },
    TransportHeartbeatObserved {
        session_id: String,
        generation: u64,
        policy_revision: Option<WireSequence>,
        route_stage: SshSessionRouteStage,
        last_sent_at_unix_ms: i64,
        last_ack_at_unix_ms: Option<i64>,
        consecutive_failures: u8,
        next_due_at_unix_ms: Option<i64>,
    },
    TransportHeartbeatFailed {
        session_id: String,
        generation: u64,
        policy_revision: Option<WireSequence>,
        route_stage: SshSessionRouteStage,
    },
    ShellHeartbeatTick {
        session_id: String,
        generation: u64,
        policy_revision: Option<WireSequence>,
        next_due_at_unix_ms: i64,
    },
    ShellHeartbeatWriteCompleted {
        session_id: String,
        generation: u64,
        policy_revision: Option<WireSequence>,
        outcome: ShellHeartbeatWriteOutcome,
        sent_at_unix_ms: i64,
    },
    ShellClosed {
        session_id: String,
        generation: u64,
        reason: SshSessionCloseReason,
    },
    ShellCleanupCompleted {
        session_id: String,
        generation: u64,
    },
    ConnectionFailed {
        session_id: String,
        generation: u64,
        error: TransportError,
        route_stage: Option<SshSessionRouteStage>,
    },
    LocalProcessReady {
        session_id: String,
        generation: u64,
        metadata: LocalPtyMetadata,
        process: LocalPtyProcess,
    },
    LocalProcessOutput {
        session_id: String,
        generation: u64,
        bytes: Vec<u8>,
    },
    LocalProcessOutputDrained {
        session_id: String,
        generation: u64,
    },
    LocalProcessExited {
        session_id: String,
        generation: u64,
        exit: LocalPtyExit,
    },
    LocalProcessFailed {
        session_id: String,
        generation: u64,
        failure: LocalSessionFailureReason,
    },
    ReapStaleAttachments {
        now_unix_ms: i64,
    },
}

enum ShellCommand {
    Input {
        bytes: Vec<u8>,
        deadline: tokio::time::Instant,
        completion: oneshot::Sender<bool>,
    },
    ApprovedPluginInput {
        bytes: Vec<u8>,
        deadline: tokio::time::Instant,
        completion: oneshot::Sender<bool>,
    },
    AutomationInput {
        bytes: Zeroizing<Vec<u8>>,
        deadline: tokio::time::Instant,
        completion: oneshot::Sender<bool>,
    },
    HeartbeatInput {
        bytes: Vec<u8>,
        expected_input_activity_epoch: u64,
        deadline: tokio::time::Instant,
        completion: oneshot::Sender<ShellHeartbeatWriteOutcome>,
    },
    Resize {
        size: PtySize,
        deadline: tokio::time::Instant,
        completion: oneshot::Sender<bool>,
    },
    Disconnect(SshSessionCloseReason),
    Abort,
}

#[derive(Clone, Debug)]
enum ShellStopSignal {
    Disconnect(SshSessionCloseReason),
    Abort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellHeartbeatWriteOutcome {
    Sent,
    UserActive,
    WriterBusy,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransportHeartbeatAttempt {
    Acknowledged,
    TimedOut,
    Closed,
}

fn next_transport_heartbeat_failures(
    attempt: TransportHeartbeatAttempt,
    current: u8,
    failure_threshold: u8,
) -> u8 {
    match attempt {
        TransportHeartbeatAttempt::Acknowledged => 0,
        TransportHeartbeatAttempt::TimedOut => current.saturating_add(1),
        TransportHeartbeatAttempt::Closed => failure_threshold,
    }
}

fn shell_heartbeat_preflight(
    expected_input_activity_epoch: u64,
    input_activity_epoch: &AtomicU64,
    deadline: tokio::time::Instant,
    now: tokio::time::Instant,
) -> Result<(), ShellHeartbeatWriteOutcome> {
    if now >= deadline {
        Err(ShellHeartbeatWriteOutcome::WriterBusy)
    } else if input_activity_epoch.load(Ordering::Acquire) != expected_input_activity_epoch {
        Err(ShellHeartbeatWriteOutcome::UserActive)
    } else {
        Ok(())
    }
}

#[derive(Clone)]
struct ActorHostKeyVerifier {
    tx: mpsc::Sender<Message>,
    session_id: String,
    generation: u64,
}

impl HostKeyVerifier for ActorHostKeyVerifier {
    fn verify(&self, endpoint: Endpoint, observed: ObservedHostKey) -> VerifyFuture {
        let tx = self.tx.clone();
        let session_id = self.session_id.clone();
        let generation = self.generation;
        Box::pin(async move {
            let (reply, decision) = oneshot::channel();
            tx.send(Message::HostKeyObserved {
                session_id,
                generation,
                endpoint,
                observed,
                reply,
            })
            .await
            .map_err(|_| TransportError::HostKeyVerificationFailed)?;
            decision
                .await
                .map_err(|_| TransportError::HostKeyVerificationFailed)?
        })
    }
}

struct ActorConnectionInteraction {
    tx: mpsc::Sender<Message>,
    session_id: String,
    generation: u64,
    keyboard_interactive_deadline: Option<tokio::time::Instant>,
}

impl SshConnectionInteraction for ActorConnectionInteraction {
    async fn phase_changed(
        &mut self,
        phase: ConnectionPhase,
        _route_stage: ConnectionRouteStage,
    ) -> Result<(), TransportError> {
        let state = match phase {
            ConnectionPhase::Connecting => SshSessionState::Connecting,
            ConnectionPhase::Authenticating => SshSessionState::Authenticating,
        };
        self.tx
            .send(Message::ConnectionState {
                session_id: self.session_id.clone(),
                generation: self.generation,
                state,
            })
            .await
            .map_err(|_| TransportError::ConnectionLost)
    }

    async fn answer_keyboard_interactive(
        &mut self,
        request: KeyboardInteractiveRequest,
    ) -> Result<Vec<Zeroizing<String>>, TransportError> {
        let now = tokio::time::Instant::now();
        if request.round_index == 1 {
            self.keyboard_interactive_deadline = Some(now + KEYBOARD_INTERACTIVE_TOTAL_TIMEOUT);
        }
        let total_deadline = self
            .keyboard_interactive_deadline
            .unwrap_or(now + KEYBOARD_INTERACTIVE_TOTAL_TIMEOUT);
        let round_deadline =
            std::cmp::min(total_deadline, now + KEYBOARD_INTERACTIVE_ROUND_TIMEOUT);
        let (answers, answer_rx) = oneshot::channel();
        self.tx
            .send(Message::KeyboardInteractiveChallengeObserved {
                session_id: self.session_id.clone(),
                generation: self.generation,
                route_stage: connection_route_stage_to_wire(request.route_stage),
                credential_ref_id: request.credential_ref_id,
                attempt_index: request.attempt_index,
                round_index: request.round_index,
                challenge: request.challenge,
                answers,
            })
            .await
            .map_err(|_| TransportError::ConnectionLost)?;
        let responses = tokio::time::timeout_at(round_deadline, answer_rx)
            .await
            .map_err(|_| TransportError::AuthenticationTimeout)?
            .map_err(|_| TransportError::ConnectionLost)??;
        let total_bytes = responses
            .iter()
            .try_fold(0_usize, |total, response| total.checked_add(response.len()))
            .ok_or(TransportError::InvalidKeyboardInteractiveResponse)?;
        if total_bytes > KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES {
            return Err(TransportError::InvalidKeyboardInteractiveResponse);
        }
        Ok(responses)
    }
}

/// A writer can require the shell to consume input while that shell is also
/// emitting output. ACK waits must therefore keep draining live output, while
/// retaining the original FIFO for every control message. The extra queue is
/// capped at one mailbox: control floods keep bounded upstream backpressure
/// and the existing writer deadline, never an unbounded bypass queue.
struct ActorMailbox {
    rx: mpsc::Receiver<Message>,
    deferred: VecDeque<Message>,
}

impl ActorMailbox {
    fn new(rx: mpsc::Receiver<Message>) -> Self {
        Self {
            rx,
            deferred: VecDeque::new(),
        }
    }

    async fn next(&mut self) -> Option<Message> {
        match self.deferred.pop_front() {
            Some(message) => Some(message),
            None => self.rx.recv().await,
        }
    }
}

async fn run_actor(mut actor: Actor, rx: mpsc::Receiver<Message>) {
    let mut mailbox = ActorMailbox::new(rx);
    while let Some(message) = mailbox.next().await {
        match message {
            Message::NativeTerminalEnableSsh {
                request_id,
                prepared,
                reply,
            } => {
                actor
                    .native_terminal_enable_ssh_with_ack(request_id, prepared, reply, &mut mailbox)
                    .await;
            }
            Message::NativeTerminalEnableLocal {
                request_id,
                prepared,
                reply,
            } => {
                actor
                    .native_terminal_enable_local_with_ack(
                        request_id,
                        prepared,
                        reply,
                        &mut mailbox,
                    )
                    .await;
            }
            Message::Input { request, reply } => {
                actor.input_with_ack(request, reply, &mut mailbox).await;
            }
            Message::LocalInput { request, reply } => {
                actor
                    .local_input_with_ack(request, reply, &mut mailbox)
                    .await;
            }
            Message::LocalResize { request, reply } => {
                actor
                    .local_resize_with_ack(request, reply, &mut mailbox)
                    .await;
            }
            Message::ApprovedPluginInput { request, reply } => {
                actor
                    .approved_plugin_input(request, reply, &mut mailbox)
                    .await;
            }
            Message::Resize { request, reply } => {
                actor.resize_with_ack(request, reply, &mut mailbox).await;
            }
            message => actor.handle(message),
        }
    }
}

impl Actor {
    /// Only already-running output is independent of the control ordering
    /// barrier. In particular, SSH login-automation output may drive a send or
    /// state transition, and output preceding a deferred Ready must stay FIFO.
    fn output_can_progress_during_ack(&self, message: &Message) -> bool {
        match message {
            Message::ShellOutput {
                session_id,
                generation,
                ..
            } => self.sessions.get(session_id).is_some_and(|record| {
                record.summary.generation.get() == *generation
                    && record.summary.state == SshSessionState::Running
            }),
            Message::LocalProcessOutput {
                session_id,
                generation,
                ..
            } => self
                .local_sessions
                .output_can_progress_during_ack(session_id, *generation),
            _ => false,
        }
    }

    async fn wait_for_writer_ack(
        &mut self,
        mut completion: oneshot::Receiver<bool>,
        duration: Duration,
        mailbox: &mut ActorMailbox,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + duration;
        // Earlier control processing may have made buffered output eligible.
        // Drain it first so newer mailbox output cannot overtake the same stream.
        let mut index = 0;
        while index < mailbox.deferred.len() {
            if self.output_can_progress_during_ack(&mailbox.deferred[index]) {
                self.handle(mailbox.deferred.remove(index).expect("buffered output"));
            } else {
                index += 1;
            }
        }
        let mut receive_open = true;
        loop {
            tokio::select! {
                biased;
                result = &mut completion => return matches!(result, Ok(true)),
                _ = tokio::time::sleep_until(deadline) => return false,
                message = mailbox.rx.recv(),
                    if receive_open && mailbox.deferred.len() < ACTOR_MAILBOX_CAPACITY => {
                    match message {
                        Some(message) if self.output_can_progress_during_ack(&message) => {
                            self.handle(message);
                        }
                        Some(message) => mailbox.deferred.push_back(message),
                        None => receive_open = false,
                    }
                }
            }
        }
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::PluginChannelLease {
                request_id,
                session_id,
                generation,
                reply,
            } => {
                let result = self
                    .sessions
                    .get(session_id.as_str())
                    .filter(|record| {
                        record.terminal_startup.is_none()
                            && record.summary.state == SshSessionState::Running
                            && record.summary.generation == generation
                    })
                    .and_then(|record| {
                        Some(SessionChannelLease {
                            session_id: session_id.clone(),
                            generation,
                            parent_channel_id: record.summary.channel_id.clone()?,
                            target: record.summary.target.clone(),
                            endpoint: record.summary.endpoint.clone()?,
                            credential_ref_id: record.summary.credential_ref_id.clone(),
                            channels: record.shared_channels.clone()?,
                            cancelled: record.channel_lease_cancel.subscribe(),
                        })
                    })
                    .filter(SessionChannelLease::is_current)
                    .ok_or_else(|| unavailable_error(request_id));
                let _ = reply.send(result);
            }

            Message::ShutdownAll { request_id, reply } => {
                self.begin_shutdown(request_id, reply);
            }
            Message::ShutdownDeadline { token } => {
                self.fail_shutdown_if_timed_out(token);
            }
            Message::Open {
                terminal_startup,
                request,
                events,
                reply,
            } => {
                let _ = reply.send(self.open_with_startup(
                    request,
                    events,
                    terminal_startup.map(|startup| *startup),
                ));
            }
            Message::Snapshot { reply } => {
                let _ = reply.send(Ok(SshSessionSnapshot {
                    snapshot_revision: WireSequence::new(self.snapshot_revision),
                    sessions: self
                        .sessions
                        .values()
                        .map(|value| value.summary.clone())
                        .collect(),
                }));
            }
            Message::Get {
                session_id,
                request_id,
                reply,
            } => {
                let _ = reply.send(self.details(&session_id, request_id));
            }
            Message::FocusSnapshot { reply } => {
                let _ = reply.send(Ok(self.focus_snapshot()));
            }
            Message::TerminalFocusSnapshot { reply } => {
                let _ = reply.send(Ok(self.terminal_focus_snapshot()));
            }
            Message::FocusChange { request, reply } => {
                let _ = reply.send(self.focus_change(request));
            }
            Message::TerminalFocusChange { request, reply } => {
                let _ = reply.send(self.terminal_focus_change(request));
            }
            Message::PluginTerminalFocusInvalidateExact {
                target,
                focus_epoch,
                reply,
            } => {
                if self.focus_epoch == focus_epoch
                    && self.focused_target.as_ref()
                        == Some(&TerminalInputFocusTarget::Plugin(target))
                {
                    self.revoke_focused_lease();
                    self.focus_epoch = self.focus_epoch.saturating_add(1);
                }
                let _ = reply.send(Ok(self.terminal_focus_snapshot()));
            }
            Message::TerminalFocusInvalidateExact {
                invalidation,
                reply,
            } => {
                let _ = reply.send(Ok(self.clear_telnet_focus_exact(invalidation)));
            }
            Message::LocalOpen {
                request,
                events,
                reply,
            } => {
                let tx = self.tx.clone();
                let _ = reply.send(self.local_sessions.open(request, events, tx));
            }
            Message::LocalSnapshot { reply } => {
                let _ = reply.send(Ok(self.local_sessions.snapshot()));
            }
            Message::LocalGet { request, reply } => {
                let _ = reply.send(self.local_sessions.get(request));
            }
            Message::LocalAttach {
                request,
                events,
                reply,
            } => {
                let replaces_focus = matches!(
                    self.focused_target.as_ref(),
                    Some(TerminalInputFocusTarget::Local(target))
                        if self.local_sessions.same_view_target(&request, target)
                );
                // The local actor performs the attachment replacement first;
                // an invalid attach must not revoke a valid global target.
                let result = self.local_sessions.attach(request, events);
                if replaces_focus && result.is_ok() {
                    self.revoke_focused_lease();
                    self.focus_epoch = self.focus_epoch.saturating_add(1);
                }
                let _ = reply.send(result);
            }
            Message::LocalAttachmentHeartbeat { request, reply } => {
                let _ = reply.send(self.local_sessions.attachment_heartbeat(request));
            }
            Message::LocalDetach { request, reply } => {
                let clears_focus = matches!(
                    self.focused_target.as_ref(),
                    Some(TerminalInputFocusTarget::Local(target))
                        if self.local_sessions.detach_target(&request, target)
                );
                let result = self.local_sessions.detach(request);
                let focus_was_removed = result.as_ref().is_ok_and(|result| {
                    matches!(
                        result,
                        LocalSessionDetachResult::Detached { .. }
                            | LocalSessionDetachResult::Stopping { .. }
                            | LocalSessionDetachResult::TerminatedAndDetached { .. }
                    )
                });
                if clears_focus && focus_was_removed {
                    self.revoke_focused_lease();
                    self.focus_epoch = self.focus_epoch.saturating_add(1);
                }
                let _ = reply.send(result);
            }
            Message::LocalLeaseRenew { request, reply } => {
                let result = match self.focused_target.as_ref() {
                    Some(TerminalInputFocusTarget::Local(target)) => self
                        .local_sessions
                        .lease_renew(request, target, self.focus_epoch),
                    _ => Err(local_terminal::local_stale_focus(request.meta.request_id)),
                };
                let _ = reply.send(result);
            }
            Message::LocalInput { .. } => {
                unreachable!("local input is serialized by run_actor")
            }
            Message::LocalResize { .. } => {
                unreachable!("local resize is serialized by run_actor")
            }
            Message::LocalTerminate { request, reply } => {
                self.clear_focus_if(|target| {
                    matches!(
                        target,
                        TerminalInputFocusTarget::Local(local)
                            if local.session_id == request.session_id
                    )
                });
                let _ = reply.send(self.local_sessions.terminate(request));
            }
            Message::Attach {
                request,
                events,
                reply,
            } => {
                let _ = reply.send(self.attach(request, events));
            }
            Message::AttachmentHeartbeat { request, reply } => {
                let _ = reply.send(self.attachment_heartbeat(request));
            }
            Message::Detach { request, reply } => {
                let _ = reply.send(self.detach(request));
            }
            Message::HostKeyDecide { request, reply } => {
                let _ = reply.send(self.host_key_decide(request));
            }
            Message::KeyboardInteractiveAnswerPrepare { request, reply } => {
                let _ = reply.send(self.keyboard_interactive_answer_prepare(request));
            }
            Message::KeyboardInteractiveRespond { request, reply } => {
                let _ = reply.send(self.keyboard_interactive_respond(request));
            }
            #[cfg(test)]
            Message::LeaseAcquire { request, reply } => {
                let _ = reply.send(self.lease_acquire(request));
            }
            Message::LeaseRenew { request, reply } => {
                let _ = reply.send(self.lease_renew(request));
            }
            Message::Input { .. } => unreachable!("input is serialized by run_actor"),
            Message::NativeTerminalEnableSsh { .. } | Message::NativeTerminalEnableLocal { .. } => {
                unreachable!("native terminal enable is serialized by run_actor")
            }
            Message::ApprovedPluginInput { .. } => {
                unreachable!("approved plugin input is serialized by run_actor")
            }
            Message::PluginObserveAttach { request, reply } => {
                let _ = reply.send(self.plugin_observe_attach(request));
            }
            Message::PluginObserveDetach { observer_id, reply } => {
                self.plugin_observe_detach(observer_id);
                let _ = reply.send(Ok(()));
            }
            Message::Resize { .. } => unreachable!("resize is serialized by run_actor"),
            Message::Reconnect { request, reply } => {
                let _ = reply.send(self.reconnect(request));
            }
            Message::Disconnect { request, reply } => {
                let _ = reply.send(self.disconnect(request));
            }
            Message::ConnectionState {
                session_id,
                generation,
                state,
            } => {
                if self.connection_active(&session_id, generation) {
                    self.transition(&session_id, state, None, None);
                }
            }
            Message::HostKeyObserved {
                session_id,
                generation,
                endpoint,
                observed,
                reply,
            } => {
                self.host_key_observed(session_id, generation, endpoint, observed, reply);
            }
            Message::KeyboardInteractiveChallengeObserved {
                session_id,
                generation,
                route_stage,
                credential_ref_id,
                attempt_index,
                round_index,
                challenge,
                answers,
            } => {
                self.keyboard_interactive_challenge_observed(
                    session_id,
                    generation,
                    route_stage,
                    credential_ref_id,
                    attempt_index,
                    round_index,
                    challenge,
                    answers,
                );
            }
            Message::LoginAutomationTakeover { request, reply } => {
                self.login_automation_takeover(request, reply);
            }
            Message::ShellReady {
                channels,
                session_id,
                generation,
                credential_ref_id,
                negotiated_algorithms,
                login_automation,
                automation_attachment_id,
                automation_view_id,
                heartbeat_policy,
                transport_heartbeats,
                shell,
            } => {
                self.shell_ready(
                    session_id,
                    generation,
                    credential_ref_id,
                    negotiated_algorithms,
                    ShellReadyContext {
                        channels,
                        login_automation,
                        attachment_id: automation_attachment_id,
                        view_id: automation_view_id,
                        heartbeat_policy,
                        transport_heartbeats,
                    },
                    *shell,
                );
            }
            Message::ShellOutput {
                session_id,
                generation,
                bytes,
            } => {
                self.shell_output(&session_id, generation, bytes);
            }
            Message::LoginAutomationStepTimeout {
                session_id,
                generation,
                state_revision,
                step_index,
            } => {
                self.login_automation_step_timeout(
                    &session_id,
                    generation,
                    state_revision,
                    step_index,
                );
            }
            Message::LoginAutomationWriteCompleted {
                session_id,
                generation,
                state_revision,
                step_index,
                succeeded,
            } => {
                self.login_automation_write_completed(
                    &session_id,
                    generation,
                    state_revision,
                    step_index,
                    succeeded,
                );
            }
            Message::TransportHeartbeatScheduled {
                session_id,
                generation,
                policy_revision,
                route_stage,
                next_due_at_unix_ms,
            } => self.transport_heartbeat_scheduled(
                &session_id,
                generation,
                policy_revision,
                route_stage,
                next_due_at_unix_ms,
            ),
            Message::TransportHeartbeatObserved {
                session_id,
                generation,
                policy_revision,
                route_stage,
                last_sent_at_unix_ms,
                last_ack_at_unix_ms,
                consecutive_failures,
                next_due_at_unix_ms,
            } => self.transport_heartbeat_observed(
                &session_id,
                generation,
                policy_revision,
                route_stage,
                last_sent_at_unix_ms,
                last_ack_at_unix_ms,
                consecutive_failures,
                next_due_at_unix_ms,
            ),
            Message::TransportHeartbeatFailed {
                session_id,
                generation,
                policy_revision,
                route_stage,
            } => self.transport_heartbeat_failed(
                &session_id,
                generation,
                policy_revision,
                route_stage,
            ),
            Message::ShellHeartbeatTick {
                session_id,
                generation,
                policy_revision,
                next_due_at_unix_ms,
            } => self.shell_heartbeat_tick(
                &session_id,
                generation,
                policy_revision,
                next_due_at_unix_ms,
            ),
            Message::ShellHeartbeatWriteCompleted {
                session_id,
                generation,
                policy_revision,
                outcome,
                sent_at_unix_ms,
            } => self.shell_heartbeat_write_completed(
                &session_id,
                generation,
                policy_revision,
                outcome,
                sent_at_unix_ms,
            ),
            Message::ShellClosed {
                session_id,
                generation,
                reason,
            } => {
                self.shell_closed(&session_id, generation, reason);
            }
            Message::ShellCleanupCompleted {
                session_id,
                generation,
            } => self.shell_cleanup_completed(&session_id, generation),
            Message::ConnectionFailed {
                session_id,
                generation,
                error,
                route_stage,
            } => {
                self.connection_failed(&session_id, generation, error, route_stage);
            }
            Message::LocalProcessReady {
                session_id,
                generation,
                metadata,
                process,
            } => {
                let tx = self.tx.clone();
                self.local_sessions
                    .process_ready(session_id, generation, metadata, process, tx);
            }
            Message::LocalProcessOutput {
                session_id,
                generation,
                bytes,
            } => {
                self.local_sessions
                    .process_output(&session_id, generation, bytes);
            }
            Message::LocalProcessOutputDrained {
                session_id,
                generation,
            } => {
                self.local_sessions
                    .process_output_drained(&session_id, generation);
            }
            Message::LocalProcessExited {
                session_id,
                generation,
                exit,
            } => {
                self.clear_focus_if(|target| {
                    matches!(
                        target,
                        TerminalInputFocusTarget::Local(local)
                            if local.session_id.as_str() == session_id
                                && local.expected_generation.get() == generation
                    )
                });
                self.local_sessions
                    .process_exited(&session_id, generation, exit);
                if let Ok(session_id) = norishell_core_api::LocalSessionId::parse(&session_id) {
                    self.native_terminal
                        .clear_local_session(&session_id, WireSequence::new(generation));
                }
            }
            Message::LocalProcessFailed {
                session_id,
                generation,
                failure,
            } => {
                self.clear_focus_if(|target| {
                    matches!(
                        target,
                        TerminalInputFocusTarget::Local(local)
                            if local.session_id.as_str() == session_id
                                && local.expected_generation.get() == generation
                    )
                });
                self.local_sessions
                    .process_failed(&session_id, generation, failure);
                if let Ok(session_id) = norishell_core_api::LocalSessionId::parse(&session_id) {
                    self.native_terminal
                        .clear_local_session(&session_id, WireSequence::new(generation));
                }
            }
            Message::ReapStaleAttachments { now_unix_ms } => {
                self.reap_stale_attachments(now_unix_ms);
            }
        }
        self.complete_shutdown_if_ready();
    }

    fn begin_shutdown(&mut self, request_id: RequestId, reply: oneshot::Sender<ActorResult<()>>) {
        if self.shutdown_waiter.is_some() {
            let _ = reply.send(Err(unavailable_error(request_id)));
            return;
        }

        self.revoke_focused_lease();
        self.focused_target = None;
        self.focus_epoch = self.focus_epoch.saturating_add(1);

        let ssh_sessions = self
            .live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut initiation_failed = false;
        for session_id in ssh_sessions {
            let Some(generation) = self
                .sessions
                .get(&session_id)
                .map(|record| record.summary.generation.get())
            else {
                self.live_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&session_id);
                continue;
            };
            self.abort_connect(&session_id, generation);
            let (pending_takeover, has_shell, shell_stop_failed) = {
                let record = self
                    .sessions
                    .get_mut(&session_id)
                    .expect("shutdown target was collected from the session map");
                if let Some(active) = record.active_challenge.take() {
                    let _ = active.decision.send(Ok(HostKeyDecision::Rejected));
                }
                clear_keyboard_interactive_challenge(
                    record,
                    TransportError::AuthenticationRejected,
                );
                let pending_takeover = clear_login_automation(record);
                abort_heartbeat_tasks(record);
                let has_shell = record.shell.is_some();
                let shell_stop_failed = has_shell
                    && request_shell_stop(
                        record,
                        ShellStopSignal::Disconnect(SshSessionCloseReason::ApplicationExit),
                    )
                    .is_err();
                (pending_takeover, has_shell, shell_stop_failed)
            };
            if let Some(pending) = pending_takeover {
                self.reject_pending_login_automation_takeover(pending, true);
            }
            if shell_stop_failed {
                initiation_failed = true;
            } else if has_shell {
                self.transition(&session_id, SshSessionState::Disconnecting, None, None);
            } else {
                self.transition(
                    &session_id,
                    SshSessionState::Closed,
                    Some(SshSessionCloseReason::ApplicationExit),
                    None,
                );
            }
        }

        if self.local_sessions.shutdown_all().is_err() {
            initiation_failed = true;
        }
        if initiation_failed {
            let _ = reply.send(Err(unavailable_error(request_id)));
            return;
        }
        if self.shutdown_blockers_cleared() {
            let _ = reply.send(Ok(()));
            return;
        }

        let token = Uuid::new_v4();
        self.shutdown_waiter = Some(ShutdownWaiter {
            request_id,
            token,
            reply,
        });
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(APPLICATION_EXIT_SHUTDOWN_TIMEOUT).await;
            let _ = tx.send(Message::ShutdownDeadline { token }).await;
        });
    }

    fn shutdown_blockers_cleared(&self) -> bool {
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
            && self.local_sessions.live_sessions_empty()
    }

    fn complete_shutdown_if_ready(&mut self) {
        if self.shutdown_waiter.is_some() && self.shutdown_blockers_cleared() {
            let waiter = self
                .shutdown_waiter
                .take()
                .expect("shutdown waiter presence was checked");
            let _ = waiter.reply.send(Ok(()));
        }
    }

    fn fail_shutdown_if_timed_out(&mut self, token: Uuid) {
        if self
            .shutdown_waiter
            .as_ref()
            .is_none_or(|waiter| waiter.token != token)
        {
            return;
        }
        let waiter = self
            .shutdown_waiter
            .take()
            .expect("matching shutdown waiter exists");
        let _ = waiter.reply.send(Err(unavailable_error(waiter.request_id)));
    }

    #[cfg(test)]
    fn open(
        &mut self,
        request: SshSessionOpenRequest,
        events: Channel<SshSessionEvent>,
    ) -> ActorResult<SshSessionOpenResponse> {
        self.open_with_startup(request, events, None)
    }

    fn open_with_startup(
        &mut self,
        request: SshSessionOpenRequest,
        events: Channel<SshSessionEvent>,
        terminal_startup: Option<crate::plugin_service::ApprovedPluginTerminalStartup>,
    ) -> ActorResult<SshSessionOpenResponse> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.open_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.open_once(request, events, terminal_startup);
        record_control_operation(
            &mut self.open_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn open_once(
        &mut self,
        request: SshSessionOpenRequest,
        events: Channel<SshSessionEvent>,
        terminal_startup: Option<crate::plugin_service::ApprovedPluginTerminalStartup>,
    ) -> ActorResult<SshSessionOpenResponse> {
        if request.rows == 0 || request.cols == 0 {
            return Err(validation_error(request.meta.request_id));
        }
        if terminal_startup
            .as_ref()
            .is_some_and(|startup| !startup.parent.is_current() || !(startup.is_current)())
        {
            return Err(plugin_authorization_error(request.meta.request_id));
        }
        let profile = if terminal_startup.is_none() {
            Some(self.resolve_open(&request)?)
        } else {
            None
        };
        let endpoint = terminal_startup
            .as_ref()
            .map(|startup| startup.parent.endpoint.clone())
            .or_else(|| profile.as_ref().map(|profile| profile.endpoint.clone()))
            .expect("prepared session endpoint");
        let heartbeat_policy = profile
            .as_ref()
            .map(|profile| profile.heartbeat_policy.clone())
            .unwrap_or(ResolvedHeartbeatPolicy {
                revision: None,
                policy: HeartbeatPolicy::Disabled,
            });
        let heartbeat = heartbeat_status_from_policy(&heartbeat_policy);
        let initial_credential_ref_id = terminal_startup
            .as_ref()
            .and_then(|startup| startup.parent.credential_ref_id.clone())
            .or_else(|| {
                profile
                    .as_ref()
                    .and_then(|profile| profile.credentials.first())
                    .map(|credential| credential.credential_ref_id().clone())
            });
        let now = unix_time_ms();
        let session_id = norishell_core_api::SshSessionId::new();
        let attachment_id = SshAttachmentId::new();
        let summary = SshSessionSummary {
            session_id: session_id.clone(),
            open_attempt_id: request.open_attempt_id.clone(),
            target: request.target.clone(),
            credential_ref_id: initial_credential_ref_id,
            endpoint: Some(endpoint.clone()),
            generation: WireSequence::new(1),
            state_revision: WireSequence::new(1),
            attachment_revision: WireSequence::new(1),
            event_seq: WireSequence::new(0),
            channel_id: None,
            negotiated_algorithms: Vec::new(),
            state: SshSessionState::Resolving,
            close_reason: None,
            failure_reason: None,
            attachment_count: 1,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let attachment = SshSessionAttachment {
            attachment_id: attachment_id.clone(),
            attach_attempt_id: request.attach_attempt_id.clone(),
            session_id: session_id.clone(),
            generation: WireSequence::new(1),
            channel_id: None,
            view_id: request.view_id.clone(),
            state_revision: WireSequence::new(1),
            attachment_revision: WireSequence::new(1),
            attached_at_unix_ms: now,
        };
        let mut attachments = BTreeMap::new();
        attachments.insert(
            attachment_id.as_str().to_owned(),
            AttachmentRecord {
                summary: attachment.clone(),
                events,
                last_seen_at_unix_ms: now,
            },
        );
        self.sessions.insert(
            session_id.as_str().to_owned(),
            SessionRecord {
                shared_channels: None,
                channel_lease_cancel: watch::channel(false).0,
                terminal_startup: terminal_startup.clone(),
                summary: summary.clone(),
                attachments,
                active_challenge: None,
                active_keyboard_interactive_challenge: None,
                prepared_keyboard_interactive_answers: BTreeMap::new(),
                active_login_automation: None,
                heartbeat_policy: heartbeat_policy.clone(),
                heartbeat,
                transport_heartbeat_tasks: Vec::new(),
                shell_heartbeat_task: None,
                last_user_input_at: None,
                shell_heartbeat_write_in_flight: false,
                input_activity_epoch: Arc::new(AtomicU64::new(0)),
                input_lease: None,
                next_input_epoch: 0,
                last_client_seq: 0,
                last_resize_seq: 0,
                next_output_seq: 1,
                output_ring: VecDeque::new(),
                output_ring_bytes: 0,
                shell: None,
                shell_stop: None,
            },
        );
        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.as_str().to_owned());
        self.bump_snapshot();
        let automation_attachment_id = attachment_id.clone();
        let automation_view_id = request.view_id.clone();
        let response = SshSessionOpenResponse {
            operation_id: request.operation_id,
            idempotency_key: request.idempotency_key,
            open_attempt_id: request.open_attempt_id.clone(),
            state_revision: WireSequence::new(1),
            session: summary,
            attachment,
        };
        if let Some(startup) = terminal_startup {
            self.start_shared_terminal(
                session_id.to_string(),
                1,
                startup,
                automation_attachment_id,
                automation_view_id,
                request.rows,
                request.cols,
            );
        } else {
            let ResolvedConnectionProfile {
                endpoint,
                username,
                ingress,
                jump_hosts,
                credentials,
                algorithm_policy,
                heartbeat_policy,
                login_automation,
                revision_token,
            } = profile.expect("normal connection profile");
            self.start_connect(ConnectPlan {
                session_id: session_id.to_string(),
                generation: 1,
                endpoint,
                username,
                ingress,
                jump_hosts,
                credentials,
                algorithm_policy,
                heartbeat_policy,
                login_automation,
                automation_attachment_id,
                automation_view_id,
                profile_revision_token: revision_token,
                rows: request.rows,
                cols: request.cols,
            });
        }
        Ok(response)
    }

    fn resolve_open(
        &self,
        request: &SshSessionOpenRequest,
    ) -> ActorResult<ResolvedConnectionProfile> {
        self.resolve_target(
            &request.target,
            request.credential_ref_id.as_ref(),
            &request.meta.request_id,
        )
    }

    fn resolve_target(
        &self,
        target: &SshSessionTarget,
        credential_ref_id: Option<&CredentialRefId>,
        request_id: &RequestId,
    ) -> ActorResult<ResolvedConnectionProfile> {
        resolve_connection_profile(
            &self.hosts,
            &self.transient_credentials,
            target,
            credential_ref_id,
        )
        .map_err(|error| map_connection_profile_error(request_id.clone(), error))
    }

    #[allow(clippy::too_many_arguments)]
    fn start_shared_terminal(
        &mut self,
        session_id: String,
        generation: u64,
        startup: crate::plugin_service::ApprovedPluginTerminalStartup,
        attachment_id: SshAttachmentId,
        view_id: norishell_core_api::SshViewId,
        rows: u16,
        cols: u16,
    ) {
        if let Some(record) = self.sessions.get_mut(&session_id) {
            transition_record(record, SshSessionState::OpeningChannel, None, None);
        }
        let tx = self.tx.clone();
        let task_session_id = session_id.clone();
        let task = tauri::async_runtime::spawn(async move {
            let result = async {
                if !startup.parent.is_current() || !(startup.is_current)() { return Err(TransportError::RemoteExecRejected); }
                let size = PtySize::new(u32::from(cols), u32::from(rows), 0, 0)?;
                let channel = tokio::select! {
                    biased;
                    () = startup.parent.wait_cancelled() => return Err(TransportError::ConnectionLost),
                    result = startup.parent.channels.open_terminal_command(size, &startup.command, || startup.parent.is_current() && (startup.is_current)()) => result?,
                };
                Ok(SessionShell::Child(Box::new(channel), Box::new(startup.clone())))
            }.await;
            match result {
                Ok(shell) => {
                    let _ = tx
                        .send(Message::ShellReady {
                            channels: startup.parent.channels.clone(),
                            session_id: task_session_id,
                            generation,
                            credential_ref_id: startup.parent.credential_ref_id.clone(),
                            negotiated_algorithms: Vec::new(),
                            login_automation: None,
                            automation_attachment_id: attachment_id,
                            automation_view_id: view_id,
                            heartbeat_policy: ResolvedHeartbeatPolicy {
                                revision: None,
                                policy: HeartbeatPolicy::Disabled,
                            },
                            transport_heartbeats: Vec::new(),
                            shell: Box::new(shell),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = tx
                        .send(Message::ConnectionFailed {
                            session_id: task_session_id,
                            generation,
                            error,
                            route_stage: None,
                        })
                        .await;
                }
            }
        });
        self.connect_tasks.insert(
            session_id,
            ConnectTask {
                generation,
                abort_handle: task.inner().abort_handle(),
            },
        );
    }

    fn start_connect(&mut self, plan: ConnectPlan) {
        let session_id = plan.session_id.clone();
        let generation = plan.generation;
        if let Some(previous) = self.connect_tasks.remove(&session_id) {
            previous.abort_handle.abort();
        }
        let abort_handle = self.spawn_connect(plan);
        self.connect_tasks.insert(
            session_id,
            ConnectTask {
                generation,
                abort_handle,
            },
        );
    }

    fn abort_connect(&mut self, session_id: &str, generation: u64) {
        if self
            .connect_tasks
            .get(session_id)
            .is_some_and(|task| task.generation == generation)
            && let Some(task) = self.connect_tasks.remove(session_id)
        {
            task.abort_handle.abort();
        }
    }

    fn finish_connect(&mut self, session_id: &str, generation: u64) {
        if self
            .connect_tasks
            .get(session_id)
            .is_some_and(|task| task.generation == generation)
        {
            self.connect_tasks.remove(session_id);
        }
    }

    fn spawn_connect(&self, plan: ConnectPlan) -> AbortHandle {
        let tx = self.tx.clone();
        let vault = self.vault.clone();
        let transient_credentials = self.transient_credentials.clone();
        let ssh_agent = self.ssh_agent.clone();
        let transient_credential_ref_ids = plan
            .credentials
            .iter()
            .chain(
                plan.jump_hosts
                    .iter()
                    .flat_map(|jump| jump.credentials.iter()),
            )
            .filter_map(|credential| match credential {
                ConnectionCredential::Stored(_) => None,
                ConnectionCredential::Transient(credential_ref_id) => {
                    Some(credential_ref_id.clone())
                }
            })
            .collect::<Vec<_>>();
        let transient_cleanup = TransientCredentialCleanup {
            service: transient_credentials.clone(),
            credential_ref_ids: transient_credential_ref_ids,
        };
        let task = tauri::async_runtime::spawn(async move {
            let _transient_cleanup = transient_cleanup;
            let ConnectPlan {
                session_id,
                generation,
                endpoint,
                username,
                ingress,
                jump_hosts,
                credentials,
                algorithm_policy,
                heartbeat_policy,
                login_automation,
                automation_attachment_id,
                automation_view_id,
                profile_revision_token,
                rows,
                cols,
            } = plan;
            debug_assert!(!profile_revision_token.is_empty());
            debug_assert!(jump_hosts.len() <= 5);
            let transport_keepalive_interval = match &heartbeat_policy.policy {
                HeartbeatPolicy::TransportKeepalive {
                    interval_seconds, ..
                } => Some(Duration::from_secs(u64::from(*interval_seconds))),
                _ => None,
            };
            let verifier = Arc::new(ActorHostKeyVerifier {
                tx: tx.clone(),
                session_id: session_id.clone(),
                generation,
            });
            let mut interaction = ActorConnectionInteraction {
                tx: tx.clone(),
                session_id: session_id.clone(),
                generation,
                keyboard_interactive_deadline: None,
            };
            let result = async {
                let connection =
                    SshConnectionOrchestrator::new(&vault, &transient_credentials, &ssh_agent)
                        .connect(
                            ResolvedSshConnectionBase {
                                endpoint,
                                username,
                                ingress,
                                jump_hosts,
                                credentials,
                                algorithm_policy,
                                revision_token: profile_revision_token,
                            },
                            verifier,
                            &mut interaction,
                            transport_keepalive_interval,
                        )
                        .await?;
                tx.send(Message::ConnectionState {
                    session_id: session_id.clone(),
                    generation,
                    state: SshSessionState::OpeningChannel,
                })
                .await
                .map_err(|_| {
                    crate::ssh_connection_orchestrator::SshConnectionFailure {
                        error: TransportError::ConnectionLost,
                        route_stage: ConnectionRouteStage::Target,
                    }
                })?;
                let size =
                    PtySize::new(u32::from(cols), u32::from(rows), 0, 0).map_err(|error| {
                        SshConnectionFailure {
                            error,
                            route_stage: ConnectionRouteStage::Target,
                        }
                    })?;
                let channels = connection.transport.shared_channels();
                let shell = SessionShell::Connection(
                    connection
                        .transport
                        .open_remote_shell(size)
                        .await
                        .map_err(|error| SshConnectionFailure {
                            error,
                            route_stage: ConnectionRouteStage::Target,
                        })?,
                );
                let negotiated_algorithms = connection
                    .negotiated_algorithms
                    .into_iter()
                    .map(negotiated_algorithms_to_wire)
                    .collect();
                let transport_heartbeats = connection
                    .transport_heartbeats
                    .into_iter()
                    .map(routed_transport_heartbeat_to_terminal)
                    .collect();
                Ok::<_, SshConnectionFailure>((
                    shell,
                    channels,
                    connection.credential_ref_id,
                    negotiated_algorithms,
                    transport_heartbeats,
                ))
            }
            .await;
            match result {
                Ok((
                    shell,
                    channels,
                    credential_ref_id,
                    negotiated_algorithms,
                    transport_heartbeats,
                )) => {
                    let _ = tx
                        .send(Message::ShellReady {
                            channels,
                            session_id,
                            generation,
                            credential_ref_id: Some(credential_ref_id),
                            negotiated_algorithms,
                            login_automation,
                            automation_attachment_id,
                            automation_view_id,
                            heartbeat_policy,
                            transport_heartbeats,
                            shell: Box::new(shell),
                        })
                        .await;
                }
                Err(failure) => {
                    let _ = tx
                        .send(Message::ConnectionFailed {
                            session_id,
                            generation,
                            error: failure.error,
                            route_stage: Some(connection_route_stage_to_wire(failure.route_stage)),
                        })
                        .await;
                }
            }
        });
        task.inner().abort_handle()
    }

    fn details(&self, session_id: &str, request_id: RequestId) -> ActorResult<SshSessionDetails> {
        let record = self
            .sessions
            .get(session_id)
            .ok_or_else(|| not_found_error(request_id))?;
        Ok(details_from(record))
    }

    fn attach(
        &mut self,
        request: SshSessionAttachRequest,
        events: Channel<SshSessionEvent>,
    ) -> ActorResult<SshSessionAttachResponse> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.attach_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.attach_once(request, events);
        record_control_operation(
            &mut self.attach_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn attach_once(
        &mut self,
        request: SshSessionAttachRequest,
        events: Channel<SshSessionEvent>,
    ) -> ActorResult<SshSessionAttachResponse> {
        {
            let record = self
                .sessions
                .get(request.session_id.as_str())
                .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
            if record.summary.generation != request.expected_generation
                || record.summary.state_revision != request.expected_state_revision
            {
                return Err(conflict_error(request.meta.request_id));
            }
        }
        let replaces_focused_view = matches!(
            self.focused_target.as_ref(),
            Some(TerminalInputFocusTarget::Ssh(target))
                if target.session_id == request.session_id && target.view_id == request.view_id
        );
        if replaces_focused_view {
            self.revoke_focused_lease();
            self.focus_epoch = self.focus_epoch.saturating_add(1);
        }
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        if record.summary.generation != request.expected_generation
            || record.summary.state_revision != request.expected_state_revision
        {
            return Err(conflict_error(request.meta.request_id));
        }
        if record
            .attachments
            .values()
            .any(|value| value.summary.attach_attempt_id == request.attach_attempt_id)
        {
            // Only the operation ledger may replay an attach. Reusing an
            // attempt through another operation must not replace its Channel.
            return Err(conflict_error(request.meta.request_id));
        }
        let now = unix_time_ms();
        let replaced = record
            .attachments
            .iter()
            .find(|(_, value)| value.summary.view_id == request.view_id)
            .map(|(key, _)| key.clone())
            .and_then(|key| record.attachments.remove(&key));
        let automation_owner_replaced = replaced.as_ref().is_some_and(|old| {
            record
                .active_login_automation
                .as_ref()
                .is_some_and(|automation| {
                    automation.owner_attachment_id == old.summary.attachment_id
                        && automation.owner_view_id == old.summary.view_id
                })
        });
        if automation_owner_replaced {
            fail_login_automation_record(
                record,
                SshLoginAutomationFailureCode::AttachmentUnavailable,
            );
        }
        let replaced_lease = replaced.as_ref().is_some_and(|old| {
            record
                .input_lease
                .as_ref()
                .is_some_and(|lease| lease.attachment_id == old.summary.attachment_id)
        });
        if replaced_lease {
            record.input_lease = None;
        }
        let detached_revision = replaced
            .as_ref()
            .map(|_| WireSequence::new(record.summary.attachment_revision.get().saturating_add(1)));
        let next_revision = detached_revision
            .unwrap_or(record.summary.attachment_revision)
            .get()
            .saturating_add(1);
        let attachment = SshSessionAttachment {
            attachment_id: SshAttachmentId::new(),
            attach_attempt_id: request.attach_attempt_id,
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            channel_id: record.summary.channel_id.clone(),
            view_id: request.view_id,
            state_revision: record.summary.state_revision,
            attachment_revision: WireSequence::new(next_revision),
            attached_at_unix_ms: now,
        };
        record.attachments.insert(
            attachment.attachment_id.as_str().to_owned(),
            AttachmentRecord {
                summary: attachment.clone(),
                events,
                last_seen_at_unix_ms: now,
            },
        );
        record.summary.attachment_revision = WireSequence::new(next_revision);
        record.summary.attachment_count = record.attachments.len() as u32;
        let replay = replay_after(record, request.after_output_seq);
        if replaced_lease {
            emit_payload(
                record,
                SshSessionEventPayload::InputLeaseChanged {
                    change: SshSessionInputLeaseChange::Released,
                    lease: None,
                },
            );
        }
        if let (Some(old), Some(attachment_revision)) = (replaced, detached_revision) {
            emit_payload(
                record,
                SshSessionEventPayload::AttachmentChanged {
                    change: SshSessionAttachmentChange::Detached,
                    attachment_revision,
                    attachment: old.summary,
                },
            );
        }
        let payload = SshSessionEventPayload::AttachmentChanged {
            change: SshSessionAttachmentChange::Attached,
            attachment_revision: WireSequence::new(next_revision),
            attachment: attachment.clone(),
        };
        emit_payload(record, payload);
        let response = SshSessionAttachResponse {
            state_revision: record.summary.state_revision,
            attachment_revision: WireSequence::new(next_revision),
            attachment,
            replay,
        };
        self.bump_snapshot();
        Ok(response)
    }

    fn attachment_heartbeat(
        &mut self,
        request: SshSessionAttachmentHeartbeatRequest,
    ) -> ActorResult<SshSessionAttachment> {
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        let attachment = record
            .attachments
            .get_mut(request.attachment_id.as_str())
            .expect("validated attachment");
        if attachment.summary.attachment_revision != request.expected_attachment_revision {
            return Err(conflict_error(request.meta.request_id));
        }
        attachment.last_seen_at_unix_ms = unix_time_ms();
        Ok(attachment.summary.clone())
    }

    fn detach(&mut self, request: SshSessionDetachRequest) -> ActorResult<SshSessionDetachResult> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.detach_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.detach_once(request);
        record_control_operation(
            &mut self.detach_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn detach_once(
        &mut self,
        request: SshSessionDetachRequest,
    ) -> ActorResult<SshSessionDetachResult> {
        let session_id = request.session_id.as_str().to_owned();
        let record = self
            .sessions
            .get(&session_id)
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        if record.summary.state_revision != request.expected_state_revision {
            return Err(conflict_error(request.meta.request_id));
        }
        if let Some(confirmation) = &request.confirmation {
            if confirmation.expected_state_revision != record.summary.state_revision
                || confirmation.expected_attachment_revision != record.summary.attachment_revision
            {
                return Err(conflict_error(request.meta.request_id));
            }
            if confirmation.action == SshSessionLastDetachAction::DisconnectAndDetach {
                self.abort_connect(&session_id, request.expected_generation.get());
            }
        }
        self.clear_focus_if(|target| {
            matches!(
                target,
                TerminalInputFocusTarget::Ssh(ssh)
                    if ssh.session_id == request.session_id
                        && ssh.attachment_id == request.attachment_id
                        && ssh.view_id == request.view_id
            )
        });
        let record = self
            .sessions
            .get_mut(&session_id)
            .expect("detach target was validated");
        if request.intent == SshSessionDetachIntent::RendererUnavailable {
            if request.confirmation.is_some() {
                return Err(validation_error(request.meta.request_id));
            }
            remove_attachment(record, &request.attachment_id)
                .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
            let result = SshSessionDetachResult::Detached {
                session: record.summary.clone(),
                remaining_attachment_count: record.summary.attachment_count,
            };
            self.bump_snapshot();
            return Ok(result);
        }
        let is_last = record.attachments.len() == 1;
        let active = !matches!(
            record.summary.state,
            SshSessionState::Closed | SshSessionState::Failed
        );
        if is_last && active && request.confirmation.is_none() {
            return Ok(SshSessionDetachResult::ConfirmationRequired {
                session: record.summary.clone(),
                expected_state_revision: record.summary.state_revision,
                expected_attachment_revision: record.summary.attachment_revision,
            });
        }
        if let Some(confirmation) = &request.confirmation {
            if confirmation.action == SshSessionLastDetachAction::KeepAttached {
                return Ok(SshSessionDetachResult::KeptAttached {
                    session: record.summary.clone(),
                });
            }
            if record.shell.is_some() {
                abort_heartbeat_tasks(record);
                request_shell_stop(
                    record,
                    ShellStopSignal::Disconnect(SshSessionCloseReason::LastAttachmentConfirmed),
                )
                .map_err(|_| unavailable_error(request.meta.request_id.clone()))?;
                transition_record(record, SshSessionState::Disconnecting, None, None);
                let result = SshSessionDetachResult::Disconnecting {
                    session: record.summary.clone(),
                };
                self.bump_snapshot();
                return Ok(result);
            }
            if let Some(active_challenge) = record.active_challenge.take() {
                let _ = active_challenge
                    .decision
                    .send(Ok(HostKeyDecision::Rejected));
            }
            clear_keyboard_interactive_challenge(record, TransportError::AuthenticationRejected);
            abort_heartbeat_tasks(record);
            transition_record(
                record,
                SshSessionState::Closed,
                Some(SshSessionCloseReason::LastAttachmentConfirmed),
                None,
            );
            self.live_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&session_id);
        }
        remove_attachment(record, &request.attachment_id)
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        let result = if is_last && active {
            SshSessionDetachResult::DisconnectedAndDetached {
                session: record.summary.clone(),
            }
        } else {
            SshSessionDetachResult::Detached {
                session: record.summary.clone(),
                remaining_attachment_count: record.summary.attachment_count,
            }
        };
        self.bump_snapshot();
        Ok(result)
    }

    fn host_key_observed(
        &mut self,
        session_id: String,
        generation: u64,
        endpoint: Endpoint,
        observed: ObservedHostKey,
        reply: oneshot::Sender<Result<HostKeyDecision, TransportError>>,
    ) {
        if !self.connection_active(&session_id, generation) {
            let _ = reply.send(Err(TransportError::HostKeyVerificationFailed));
            return;
        }
        self.transition(&session_id, SshSessionState::VerifyingHostKey, None, None);
        let observation = self.hosts.observe_known_host(
            endpoint.normalized_address(),
            endpoint.port(),
            &observed.algorithm,
            &observed.public_key_blob,
        );
        match observation {
            Ok(KnownHostObservation::Trusted(trusted)) => {
                let decision = self.hosts.record_known_host_verified(
                    endpoint.normalized_address(),
                    endpoint.port(),
                    &observed.algorithm,
                    &observed.public_key_blob,
                );
                let _ = reply.send(if decision.is_ok() {
                    Ok(HostKeyDecision::Trusted)
                } else {
                    Err(TransportError::HostKeyVerificationFailed)
                });
                let _ = trusted;
            }
            Ok(KnownHostObservation::Unknown(_)) => {
                self.transition(
                    &session_id,
                    SshSessionState::AwaitingHostKeyDecision,
                    None,
                    None,
                );
                let Some(record) = self.sessions.get_mut(&session_id) else {
                    let _ = reply.send(Err(TransportError::HostKeyVerificationFailed));
                    return;
                };
                let challenge = SshHostKeyChallenge {
                    challenge_id: SshHostKeyChallengeId::new(),
                    session_id: record.summary.session_id.clone(),
                    generation: record.summary.generation,
                    state_revision: record.summary.state_revision,
                    endpoint: SshSessionEndpoint {
                        address: endpoint.normalized_address().to_owned(),
                        port: endpoint.port(),
                        username: None,
                    },
                    key_algorithm: observed.algorithm.clone(),
                    fingerprint_sha256: observed.fingerprint_sha256.clone(),
                };
                record.active_challenge = Some(ActiveChallenge {
                    challenge: challenge.clone(),
                    observed,
                    decision: reply,
                });
                emit_payload(
                    record,
                    SshSessionEventPayload::HostKeyChallenge { challenge },
                );
            }
            Ok(KnownHostObservation::Mismatch { trusted, .. })
            | Ok(KnownHostObservation::AlgorithmChanged { trusted, .. }) => {
                let _ = reply.send(Ok(HostKeyDecision::Mismatch {
                    trusted_fingerprint: trusted.fingerprint_sha256,
                }));
            }
            Err(_) => {
                let _ = reply.send(Err(TransportError::HostKeyVerificationFailed));
            }
        }
    }

    fn host_key_decide(
        &mut self,
        request: SshHostKeyDecisionRequest,
    ) -> ActorResult<SshSessionDetails> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.host_key_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.host_key_decide_once(request);
        record_control_operation(
            &mut self.host_key_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn host_key_decide_once(
        &mut self,
        request: SshHostKeyDecisionRequest,
    ) -> ActorResult<SshSessionDetails> {
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        let active = record
            .active_challenge
            .take()
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        if active.challenge.challenge_id != request.challenge_id
            || active.challenge.state_revision != request.expected_state_revision
            || active.challenge.generation != request.expected_generation
        {
            record.active_challenge = Some(active);
            return Err(conflict_error(request.meta.request_id));
        }
        let decision = match request.decision {
            SshHostKeyDecision::Reject => HostKeyDecision::Rejected,
            SshHostKeyDecision::AcceptAndStore => {
                let endpoint = &active.challenge.endpoint;
                self.hosts
                    .trust_known_host(
                        &endpoint.address,
                        endpoint.port,
                        &active.observed.algorithm,
                        &active.observed.public_key_blob,
                    )
                    .map_err(|_| conflict_error(request.meta.request_id.clone()))?;
                HostKeyDecision::Trusted
            }
        };
        let _ = active.decision.send(Ok(decision));
        Ok(details_from(record))
    }

    #[allow(clippy::too_many_arguments)]
    fn keyboard_interactive_challenge_observed(
        &mut self,
        session_id: String,
        generation: u64,
        route_stage: SshSessionRouteStage,
        credential_ref_id: CredentialRefId,
        attempt_index: u8,
        round_index: u8,
        challenge: TransportKeyboardChallenge,
        answers: oneshot::Sender<Result<Vec<Zeroizing<String>>, TransportError>>,
    ) {
        let Some(record) = self.sessions.get_mut(&session_id) else {
            let _ = answers.send(Err(TransportError::AuthenticationRejected));
            return;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != SshSessionState::Authenticating
            || record.active_keyboard_interactive_challenge.is_some()
        {
            let _ = answers.send(Err(TransportError::AuthenticationRejected));
            return;
        }
        record.prepared_keyboard_interactive_answers.clear();
        record.summary.state_revision =
            WireSequence::new(record.summary.state_revision.get().saturating_add(1));
        record.summary.updated_at_unix_ms = unix_time_ms();
        let expires_at_unix_ms = unix_time_ms().saturating_add(
            i64::try_from(KEYBOARD_INTERACTIVE_ROUND_TIMEOUT.as_millis()).unwrap_or(i64::MAX),
        );
        let challenge = SshKeyboardInteractiveChallenge {
            challenge_id: SshKeyboardInteractiveChallengeId::new(),
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            state_revision: record.summary.state_revision,
            route_stage,
            credential_ref_id,
            attempt_index,
            round_index,
            name: challenge.name,
            instructions: challenge.instructions,
            prompts: challenge
                .prompts
                .into_iter()
                .enumerate()
                .map(|(index, prompt)| SshKeyboardInteractivePrompt {
                    prompt_index: u8::try_from(index).unwrap_or(u8::MAX),
                    text: prompt.text,
                    echo: prompt.echo,
                    sensitive: !prompt.echo,
                })
                .collect(),
            expires_at_unix_ms,
        };
        record.active_keyboard_interactive_challenge = Some(ActiveKeyboardInteractiveChallenge {
            challenge: challenge.clone(),
            answers,
        });
        emit_payload(
            record,
            SshSessionEventPayload::KeyboardInteractiveChallengeChanged {
                challenge: Some(challenge),
            },
        );
        self.bump_snapshot();
    }

    fn keyboard_interactive_answer_prepare(
        &mut self,
        mut request: SshKeyboardInteractiveAnswerPrepareRequest,
    ) -> ActorResult<SshKeyboardInteractiveAnswerPrepareResponse> {
        let now = unix_time_ms();
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        let active = record
            .active_keyboard_interactive_challenge
            .as_ref()
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        let challenge = &active.challenge;
        if challenge.challenge_id != request.challenge_id
            || challenge.generation != request.expected_generation
            || challenge.state_revision != request.expected_state_revision
            || challenge.round_index != request.round_index
            || challenge.expires_at_unix_ms <= now
            || challenge
                .prompts
                .get(usize::from(request.prompt_index))
                .is_none_or(|prompt| prompt.prompt_index != request.prompt_index)
            || request.answer.len() > KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES
        {
            return Err(conflict_error(request.meta.request_id));
        }
        let answer_ref_id = SshKeyboardInteractiveAnswerRefId::new();
        record.prepared_keyboard_interactive_answers.insert(
            answer_ref_id.as_str().to_owned(),
            PreparedKeyboardInteractiveAnswer {
                challenge_id: request.challenge_id,
                generation: request.expected_generation,
                state_revision: request.expected_state_revision,
                round_index: request.round_index,
                prompt_index: request.prompt_index,
                expires_at_unix_ms: challenge.expires_at_unix_ms,
                answer: Zeroizing::new(std::mem::take(&mut request.answer)),
            },
        );
        Ok(SshKeyboardInteractiveAnswerPrepareResponse {
            answer_ref_id,
            expires_at_unix_ms: challenge.expires_at_unix_ms,
        })
    }

    fn keyboard_interactive_respond(
        &mut self,
        request: SshKeyboardInteractiveResponseRequest,
    ) -> ActorResult<SshSessionDetails> {
        let now = unix_time_ms();
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        let active = record
            .active_keyboard_interactive_challenge
            .as_ref()
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        let challenge = &active.challenge;
        if challenge.challenge_id != request.challenge_id
            || challenge.generation != request.expected_generation
            || challenge.state_revision != request.expected_state_revision
            || challenge.round_index != request.round_index
            || challenge.expires_at_unix_ms <= now
            || request.answers.len() != challenge.prompts.len()
        {
            return Err(conflict_error(request.meta.request_id));
        }

        let mut seen = BTreeSet::new();
        for answer in &request.answers {
            let (prompt_index, reference) = match answer {
                SshKeyboardInteractiveAnswerInput::EchoText {
                    prompt_index,
                    value,
                } => {
                    let Some(prompt) = challenge.prompts.get(usize::from(*prompt_index)) else {
                        return Err(conflict_error(request.meta.request_id.clone()));
                    };
                    if prompt.prompt_index != *prompt_index
                        || !prompt.echo
                        || prompt.sensitive
                        || value.len() > KEYBOARD_INTERACTIVE_ANSWER_MAX_BYTES
                    {
                        return Err(conflict_error(request.meta.request_id.clone()));
                    }
                    (*prompt_index, None)
                }
                SshKeyboardInteractiveAnswerInput::OneTimeAnswerRef {
                    prompt_index,
                    answer_ref_id,
                } => (*prompt_index, Some(answer_ref_id)),
            };
            if !seen.insert(prompt_index) {
                return Err(conflict_error(request.meta.request_id.clone()));
            }
            let Some(prompt) = challenge.prompts.get(usize::from(prompt_index)) else {
                return Err(conflict_error(request.meta.request_id.clone()));
            };
            if prompt.prompt_index != prompt_index {
                return Err(conflict_error(request.meta.request_id.clone()));
            }
            if let Some(reference) = reference {
                let prepared = record
                    .prepared_keyboard_interactive_answers
                    .get(reference.as_str())
                    .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
                if prepared.challenge_id != request.challenge_id
                    || prepared.generation != request.expected_generation
                    || prepared.state_revision != request.expected_state_revision
                    || prepared.round_index != request.round_index
                    || prepared.prompt_index != prompt_index
                    || prepared.expires_at_unix_ms <= now
                {
                    return Err(conflict_error(request.meta.request_id.clone()));
                }
            }
        }

        let mut ordered = std::iter::repeat_with(|| None)
            .take(challenge.prompts.len())
            .collect::<Vec<Option<Zeroizing<String>>>>();
        for answer in request.answers {
            let (prompt_index, answer) = match answer {
                SshKeyboardInteractiveAnswerInput::EchoText {
                    prompt_index,
                    value,
                } => (prompt_index, Zeroizing::new(value)),
                SshKeyboardInteractiveAnswerInput::OneTimeAnswerRef {
                    prompt_index,
                    answer_ref_id,
                } => {
                    let prepared = record
                        .prepared_keyboard_interactive_answers
                        .remove(answer_ref_id.as_str())
                        .expect("one-time answer ref was validated");
                    (prompt_index, prepared.answer)
                }
            };
            ordered[usize::from(prompt_index)] = Some(answer);
        }
        let answers = ordered
            .into_iter()
            .map(|answer| answer.expect("all keyboard-interactive prompts were answered"))
            .collect();
        let active = record
            .active_keyboard_interactive_challenge
            .take()
            .expect("active keyboard-interactive challenge was validated");
        record.prepared_keyboard_interactive_answers.clear();
        record.summary.state_revision =
            WireSequence::new(record.summary.state_revision.get().saturating_add(1));
        record.summary.updated_at_unix_ms = now;
        emit_payload(
            record,
            SshSessionEventPayload::KeyboardInteractiveChallengeChanged { challenge: None },
        );
        let _ = active.answers.send(Ok(answers));
        let details = details_from(record);
        self.bump_snapshot();
        Ok(details)
    }

    fn focus_snapshot(&self) -> SshTerminalInputFocusSnapshot {
        let (target, lease) = match &self.focused_target {
            Some(TerminalInputFocusTarget::Ssh(target)) => (
                Some(target.clone()),
                self.sessions
                    .get(target.session_id.as_str())
                    .and_then(|record| record.input_lease.clone())
                    .filter(|lease| lease.focus_epoch.get() == self.focus_epoch),
            ),
            _ => (None, None),
        };
        SshTerminalInputFocusSnapshot {
            focus_epoch: WireSequence::new(self.focus_epoch),
            target,
            lease,
        }
    }

    fn terminal_focus_snapshot(&self) -> TerminalInputFocusSnapshot {
        let lease = match &self.focused_target {
            Some(TerminalInputFocusTarget::Ssh(target)) => self
                .sessions
                .get(target.session_id.as_str())
                .and_then(|record| record.input_lease.clone())
                .filter(|lease| lease.focus_epoch.get() == self.focus_epoch)
                .map(TerminalInputLease::Ssh),
            Some(TerminalInputFocusTarget::Local(target)) => self
                .local_sessions
                .focused_lease(target, self.focus_epoch)
                .map(TerminalInputLease::Local),
            Some(TerminalInputFocusTarget::Telnet(_) | TerminalInputFocusTarget::Plugin(_)) => None,
            None => None,
        };
        TerminalInputFocusSnapshot {
            focus_epoch: WireSequence::new(self.focus_epoch),
            target: self.focused_target.clone(),
            lease,
        }
    }

    fn clear_telnet_focus_exact(
        &mut self,
        invalidation: crate::telnet_session_service::TelnetFocusInvalidation,
    ) -> TerminalInputFocusSnapshot {
        let matches = self.focus_epoch == invalidation.focus_epoch
            && matches!(
                self.focused_target.as_ref(),
                Some(TerminalInputFocusTarget::Telnet(target))
                    if target.session_id.as_str() == invalidation.session_id
                        && target.expected_generation.get() == invalidation.generation
                        && target.expected_state_revision.get() == invalidation.state_revision
                        && target.socket_id.as_str() == invalidation.socket_id
                        && target.attachment_id.as_str() == invalidation.attachment_id
                        && target.view_id.as_str() == invalidation.view_id
            );
        if matches {
            self.revoke_focused_lease();
            self.focus_epoch = self.focus_epoch.saturating_add(1);
        }
        self.terminal_focus_snapshot()
    }

    fn focus_change(
        &mut self,
        request: SshTerminalInputFocusChangeRequest,
    ) -> ActorResult<SshTerminalInputFocusChangeResponse> {
        let response = self.terminal_focus_change(TerminalInputFocusChangeRequest {
            meta: request.meta,
            operation_id: request.operation_id,
            idempotency_key: request.idempotency_key,
            expected_focus_epoch: request.expected_focus_epoch,
            target: request.target.map(TerminalInputFocusTarget::Ssh),
        })?;
        let target = response.target.and_then(|target| match target {
            TerminalInputFocusTarget::Ssh(target) => Some(target),
            TerminalInputFocusTarget::Local(_)
            | TerminalInputFocusTarget::Telnet(_)
            | TerminalInputFocusTarget::Plugin(_) => None,
        });
        let lease = response.lease.and_then(|lease| match lease {
            TerminalInputLease::Ssh(lease) => Some(lease),
            TerminalInputLease::Local(_)
            | TerminalInputLease::Telnet(_)
            | TerminalInputLease::Plugin(_) => None,
        });
        Ok(SshTerminalInputFocusChangeResponse {
            focus_epoch: response.focus_epoch,
            target,
            lease,
        })
    }

    fn terminal_focus_change(
        &mut self,
        request: TerminalInputFocusChangeRequest,
    ) -> ActorResult<TerminalInputFocusChangeResponse> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.focus_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.terminal_focus_change_once(request);
        record_control_operation(
            &mut self.focus_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn terminal_focus_change_once(
        &mut self,
        request: TerminalInputFocusChangeRequest,
    ) -> ActorResult<TerminalInputFocusChangeResponse> {
        if request.expected_focus_epoch.get() != self.focus_epoch {
            return Err(stale_focus_error(request.meta.request_id));
        }
        if let Some(target) = &request.target {
            match target {
                TerminalInputFocusTarget::Ssh(target) => {
                    let record = self
                        .sessions
                        .get(target.session_id.as_str())
                        .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
                    validate_attachment(
                        record,
                        target.expected_generation,
                        &target.attachment_id,
                        &target.view_id,
                        &request.meta.request_id,
                    )?;
                    if record.summary.state != SshSessionState::Running
                        || record.summary.state_revision != target.expected_state_revision
                        || record.summary.channel_id.as_ref() != Some(&target.channel_id)
                    {
                        return Err(conflict_error(request.meta.request_id));
                    }
                }
                TerminalInputFocusTarget::Local(target) => self
                    .local_sessions
                    .validate_focus_target(target, &request.meta.request_id)?,
                // The public async command validates and grants the Telnet
                // actor lease after this broker commits the new focus epoch.
                TerminalInputFocusTarget::Telnet(_) | TerminalInputFocusTarget::Plugin(_) => {}
            }
        }

        self.revoke_focused_lease();
        self.focus_epoch = self.focus_epoch.saturating_add(1);
        let focus_epoch = WireSequence::new(self.focus_epoch);
        let lease = match &request.target {
            Some(TerminalInputFocusTarget::Ssh(target)) => {
                let record = self
                    .sessions
                    .get_mut(target.session_id.as_str())
                    .expect("focus target was validated");
                record.next_input_epoch = record.next_input_epoch.saturating_add(1);
                let lease = SshSessionInputLease {
                    lease_id: SshInputLeaseId::new(),
                    session_id: record.summary.session_id.clone(),
                    generation: record.summary.generation,
                    attachment_id: target.attachment_id.clone(),
                    view_id: target.view_id.clone(),
                    focus_epoch,
                    input_epoch: WireSequence::new(record.next_input_epoch),
                    expires_at_unix_ms: unix_time_ms().saturating_add(INPUT_LEASE_MILLIS),
                };
                record.input_lease = Some(lease.clone());
                record.last_client_seq = 0;
                record.last_resize_seq = 0;
                emit_payload(
                    record,
                    SshSessionEventPayload::InputLeaseChanged {
                        change: SshSessionInputLeaseChange::Acquired,
                        lease: Some(lease.clone()),
                    },
                );
                Some(TerminalInputLease::Ssh(lease))
            }
            Some(TerminalInputFocusTarget::Local(target)) => Some(TerminalInputLease::Local(
                self.local_sessions.acquire_focus(target, focus_epoch),
            )),
            Some(TerminalInputFocusTarget::Telnet(_) | TerminalInputFocusTarget::Plugin(_)) => None,
            None => None,
        };
        self.focused_target = request.target.clone();
        Ok(TerminalInputFocusChangeResponse {
            focus_epoch,
            target: request.target,
            lease,
        })
    }

    fn revoke_focused_lease(&mut self) {
        let Some(target) = self.focused_target.take() else {
            return;
        };
        match target {
            TerminalInputFocusTarget::Ssh(target) => {
                let Some(record) = self.sessions.get_mut(target.session_id.as_str()) else {
                    return;
                };
                if record.input_lease.take().is_some() {
                    emit_payload(
                        record,
                        SshSessionEventPayload::InputLeaseChanged {
                            change: SshSessionInputLeaseChange::Released,
                            lease: None,
                        },
                    );
                }
            }
            TerminalInputFocusTarget::Local(target) => {
                self.local_sessions.revoke_focus(&target);
            }
            TerminalInputFocusTarget::Telnet(_) | TerminalInputFocusTarget::Plugin(_) => {}
        }
    }

    fn clear_focus_if(&mut self, predicate: impl FnOnce(&TerminalInputFocusTarget) -> bool) {
        if self.focused_target.as_ref().is_some_and(predicate) {
            self.revoke_focused_lease();
            self.focus_epoch = self.focus_epoch.saturating_add(1);
        }
    }

    fn validate_global_focus(
        &self,
        fence: GlobalFocusFence<'_>,
        request_id: &RequestId,
    ) -> ActorResult<()> {
        let target = self
            .focused_target
            .as_ref()
            .and_then(|target| match target {
                TerminalInputFocusTarget::Ssh(target) => Some(target),
                TerminalInputFocusTarget::Local(_)
                | TerminalInputFocusTarget::Telnet(_)
                | TerminalInputFocusTarget::Plugin(_) => None,
            })
            .ok_or_else(|| stale_focus_error(request_id.clone()))?;
        if fence.focus_epoch.get() != self.focus_epoch
            || target.session_id != *fence.session_id
            || target.expected_generation != fence.generation
            || target.attachment_id != *fence.attachment_id
            || target.view_id != *fence.view_id
            || fence
                .channel_id
                .is_some_and(|value| target.channel_id != *value)
        {
            return Err(stale_focus_error(request_id.clone()));
        }
        let record = self
            .sessions
            .get(fence.session_id.as_str())
            .ok_or_else(|| stale_focus_error(request_id.clone()))?;
        if record.summary.state != SshSessionState::Running
            || record.summary.generation != target.expected_generation
            || record.summary.state_revision != target.expected_state_revision
            || record.summary.channel_id.as_ref() != Some(&target.channel_id)
            || record
                .attachments
                .get(fence.attachment_id.as_str())
                .is_none_or(|attachment| {
                    attachment.summary.view_id != target.view_id
                        || attachment.summary.generation != target.expected_generation
                        || attachment.summary.channel_id.as_ref() != Some(&target.channel_id)
                })
        {
            return Err(stale_focus_error(request_id.clone()));
        }
        Ok(())
    }

    #[cfg(test)]
    fn lease_acquire(
        &mut self,
        request: SshSessionInputLeaseAcquireRequest,
    ) -> ActorResult<SshSessionInputLease> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.lease_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.lease_acquire_once(request);
        record_control_operation(
            &mut self.lease_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    #[cfg(test)]
    fn lease_acquire_once(
        &mut self,
        request: SshSessionInputLeaseAcquireRequest,
    ) -> ActorResult<SshSessionInputLease> {
        let target = {
            let record = self
                .sessions
                .get(request.session_id.as_str())
                .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
            validate_attachment(
                record,
                request.expected_generation,
                &request.attachment_id,
                &request.view_id,
                &request.meta.request_id,
            )?;
            if record.summary.state != SshSessionState::Running
                || record.summary.state_revision != request.expected_state_revision
            {
                return Err(conflict_error(request.meta.request_id));
            }
            SshTerminalInputFocusTarget {
                session_id: request.session_id.clone(),
                expected_generation: request.expected_generation,
                expected_state_revision: request.expected_state_revision,
                channel_id: record
                    .summary
                    .channel_id
                    .clone()
                    .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?,
                attachment_id: request.attachment_id.clone(),
                view_id: request.view_id.clone(),
            }
        };
        self.revoke_focused_lease();
        self.focus_epoch = self.focus_epoch.saturating_add(1);
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .expect("lease target was validated");
        record.next_input_epoch = record.next_input_epoch.saturating_add(1);
        let lease = SshSessionInputLease {
            lease_id: SshInputLeaseId::new(),
            session_id: record.summary.session_id.clone(),
            generation: record.summary.generation,
            attachment_id: request.attachment_id,
            view_id: request.view_id,
            focus_epoch: WireSequence::new(self.focus_epoch),
            input_epoch: WireSequence::new(record.next_input_epoch),
            expires_at_unix_ms: unix_time_ms().saturating_add(INPUT_LEASE_MILLIS),
        };
        record.input_lease = Some(lease.clone());
        record.last_client_seq = 0;
        record.last_resize_seq = 0;
        self.focused_target = Some(TerminalInputFocusTarget::Ssh(target));
        emit_payload(
            record,
            SshSessionEventPayload::InputLeaseChanged {
                change: SshSessionInputLeaseChange::Acquired,
                lease: Some(lease.clone()),
            },
        );
        Ok(lease)
    }

    fn lease_renew(
        &mut self,
        request: SshSessionInputLeaseRenewRequest,
    ) -> ActorResult<SshSessionInputLease> {
        self.validate_global_focus(
            GlobalFocusFence {
                session_id: &request.session_id,
                generation: request.expected_generation,
                channel_id: None,
                attachment_id: &request.attachment_id,
                view_id: &request.view_id,
                focus_epoch: request.focus_epoch,
            },
            &request.meta.request_id,
        )?;
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_lease(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.lease_id,
            request.input_epoch,
            &request.meta.request_id,
            false,
        )?;
        let lease = record.input_lease.as_mut().expect("validated lease");
        lease.expires_at_unix_ms = unix_time_ms().saturating_add(INPUT_LEASE_MILLIS);
        let lease = lease.clone();
        emit_payload(
            record,
            SshSessionEventPayload::InputLeaseChanged {
                change: SshSessionInputLeaseChange::Renewed,
                lease: Some(lease.clone()),
            },
        );
        Ok(lease)
    }

    async fn input_with_ack(
        &mut self,
        request: SshSessionInputRequest,
        reply: oneshot::Sender<ActorResult<()>>,
        mailbox: &mut ActorMailbox,
    ) {
        let session_id = request.session_id.as_str().to_owned();
        let generation = request.expected_generation.get();
        let request_id = request.meta.request_id.clone();
        let completion = match self.queue_input(request) {
            Ok(completion) => completion,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };

        // Manual terminal input is non-idempotent. Keep the actor at this
        // linearization point until the only Shell writer confirms the actual
        // write; a timeout is delivery-uncertain and must poison this generation.
        let succeeded = self
            .wait_for_writer_ack(
                completion,
                MANUAL_INPUT_WRITE_TIMEOUT.saturating_add(Duration::from_secs(1)),
                mailbox,
            )
            .await;
        if !succeeded {
            self.connection_failed(
                &session_id,
                generation,
                TransportError::ConnectionLost,
                None,
            );
        }
        let _ = reply.send(if succeeded {
            Ok(())
        } else {
            Err(unavailable_error(request_id))
        });
    }

    async fn local_input_with_ack(
        &mut self,
        request: LocalSessionInputRequest,
        reply: oneshot::Sender<ActorResult<()>>,
        mailbox: &mut ActorMailbox,
    ) {
        let session_id = request.session_id.as_str().to_owned();
        let generation = request.expected_generation.get();
        let request_id = request.meta.request_id.clone();
        let completion = match self.focused_target.as_ref() {
            Some(TerminalInputFocusTarget::Local(target)) => {
                self.local_sessions.input(request, target, self.focus_epoch)
            }
            _ => Err(local_terminal::local_stale_focus(request_id.clone())),
        };
        let completion = match completion {
            Ok(completion) => completion,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };

        // Local PTY writes have the same non-idempotent boundary as SSH input.
        // Do not return success until the single PTY owner acknowledges its
        // write, and never retry bytes after an uncertain result.
        let succeeded = self
            .wait_for_writer_ack(completion, MANUAL_INPUT_WRITE_TIMEOUT, mailbox)
            .await;
        if !succeeded {
            self.clear_focus_if(|target| {
                matches!(
                    target,
                    TerminalInputFocusTarget::Local(local)
                        if local.session_id.as_str() == session_id
                            && local.expected_generation.get() == generation
                )
            });
            self.local_sessions.poison_input(&session_id, generation);
        }
        let _ = reply.send(if succeeded {
            Ok(())
        } else {
            Err(unavailable_error(request_id))
        });
    }

    async fn native_terminal_enable_ssh_with_ack(
        &mut self,
        request_id: RequestId,
        mut prepared: PreparedNativeTerminalEnable,
        reply: oneshot::Sender<ActorResult<()>>,
        mailbox: &mut ActorMailbox,
    ) {
        let history_scope = self
            .sessions
            .get(match &prepared.scope {
                norishell_core_api::NativeTerminalSessionScope::Ssh { session_id, .. } => {
                    session_id.as_str()
                }
                norishell_core_api::NativeTerminalSessionScope::Local { .. } => {
                    let _ = reply.send(Err(validation_error(request_id)));
                    return;
                }
            })
            .and_then(|record| match &record.summary.target {
                SshSessionTarget::Host { host_id, .. } => {
                    Some(norishell_core_api::NativeTerminalHistoryScope::Host {
                        host_id: host_id.clone(),
                    })
                }
                SshSessionTarget::QuickConnect { .. } => None,
            });
        if history_scope.is_none() {
            prepared.disable_command_capture();
        }
        let scope = prepared.scope.clone();
        let nonce = prepared.nonce.clone();
        let Some(request) = prepared.ssh_input(request_id.clone()) else {
            let _ = reply.send(Err(validation_error(request_id)));
            return;
        };
        let session_id = request.session_id.as_str().to_owned();
        let generation = request.expected_generation.get();
        let completion = match self.queue_input(request) {
            Ok(completion) => completion,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        // This state transition shares the writer/focus ordering domain with
        // the script bytes themselves. Ready output is only a hint, never an
        // authorization or connection fact.
        self.native_terminal.mark_pending(&prepared, history_scope);
        let succeeded = self
            .wait_for_writer_ack(
                completion,
                MANUAL_INPUT_WRITE_TIMEOUT.saturating_add(Duration::from_secs(1)),
                mailbox,
            )
            .await;
        if !succeeded {
            self.native_terminal.mark_enable_failed(&scope, &nonce);
            self.connection_failed(
                &session_id,
                generation,
                TransportError::ConnectionLost,
                None,
            );
            let _ = reply.send(Err(unavailable_error(request_id)));
            return;
        }
        let _ = reply.send(Ok(()));
    }

    async fn native_terminal_enable_local_with_ack(
        &mut self,
        request_id: RequestId,
        prepared: PreparedNativeTerminalEnable,
        reply: oneshot::Sender<ActorResult<()>>,
        mailbox: &mut ActorMailbox,
    ) {
        let scope = prepared.scope.clone();
        let nonce = prepared.nonce.clone();
        let Some(request) = prepared.local_input(request_id.clone()) else {
            let _ = reply.send(Err(validation_error(request_id)));
            return;
        };
        let session_id = request.session_id.as_str().to_owned();
        let generation = request.expected_generation.get();
        let completion = match self.focused_target.as_ref() {
            Some(TerminalInputFocusTarget::Local(target)) => {
                self.local_sessions.input(request, target, self.focus_epoch)
            }
            _ => Err(local_terminal::local_stale_focus(request_id.clone())),
        };
        let completion = match completion {
            Ok(completion) => completion,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        self.native_terminal.mark_pending(
            &prepared,
            Some(norishell_core_api::NativeTerminalHistoryScope::Local),
        );
        let succeeded = self
            .wait_for_writer_ack(completion, MANUAL_INPUT_WRITE_TIMEOUT, mailbox)
            .await;
        if !succeeded {
            self.native_terminal.mark_enable_failed(&scope, &nonce);
            self.clear_focus_if(|target| {
                matches!(
                    target,
                    TerminalInputFocusTarget::Local(local)
                        if local.session_id.as_str() == session_id
                            && local.expected_generation.get() == generation
                )
            });
            self.local_sessions.poison_input(&session_id, generation);
            let _ = reply.send(Err(unavailable_error(request_id)));
            return;
        }
        let _ = reply.send(Ok(()));
    }

    async fn local_resize_with_ack(
        &mut self,
        request: LocalSessionResizeRequest,
        reply: oneshot::Sender<ActorResult<()>>,
        mailbox: &mut ActorMailbox,
    ) {
        let session_id = request.session_id.as_str().to_owned();
        let generation = request.expected_generation.get();
        let resize_seq = request.resize_seq.get();
        let request_id = request.meta.request_id.clone();
        let completion = match self.focused_target.as_ref() {
            Some(TerminalInputFocusTarget::Local(target)) => {
                self.local_sessions
                    .resize(request, target, self.focus_epoch)
            }
            _ => Err(local_terminal::local_stale_focus(request_id.clone())),
        };
        let completion = match completion {
            Ok(completion) => completion,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };

        // Keep the actor and TerminalFocusBroker at this linearization point
        // until the PTY owner confirms the real resize. A closed owner or an
        // uncertain timeout poisons the generation and never advances sequence.
        let succeeded = self
            .wait_for_writer_ack(completion, MANUAL_INPUT_WRITE_TIMEOUT, mailbox)
            .await;
        let committed = succeeded
            && self
                .local_sessions
                .commit_resize(&session_id, generation, resize_seq);
        if !committed {
            self.clear_focus_if(|target| {
                matches!(
                    target,
                    TerminalInputFocusTarget::Local(local)
                        if local.session_id.as_str() == session_id
                            && local.expected_generation.get() == generation
                )
            });
            self.local_sessions.poison_resize(&session_id, generation);
        }
        let _ = reply.send(if committed {
            Ok(())
        } else {
            Err(unavailable_error(request_id))
        });
    }

    fn queue_input(
        &mut self,
        request: SshSessionInputRequest,
    ) -> ActorResult<oneshot::Receiver<bool>> {
        if request.bytes.is_empty() || request.bytes.len() > SSH_TERMINAL_OUTPUT_FRAME_MAX_BYTES {
            return Err(validation_error(request.meta.request_id));
        }
        self.validate_global_focus(
            GlobalFocusFence {
                session_id: &request.session_id,
                generation: request.expected_generation,
                channel_id: Some(&request.channel_id),
                attachment_id: &request.attachment_id,
                view_id: &request.view_id,
                focus_epoch: request.focus_epoch,
            },
            &request.meta.request_id,
        )?;
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_lease(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.lease_id,
            request.input_epoch,
            &request.meta.request_id,
            true,
        )?;
        if record.summary.channel_id.as_ref() != Some(&request.channel_id)
            || request.client_seq.get() <= record.last_client_seq
        {
            return Err(conflict_error(request.meta.request_id));
        }
        let shell = record
            .shell
            .as_ref()
            .ok_or_else(|| unavailable_error(request.meta.request_id.clone()))?;
        let (completion, completed) = oneshot::channel();
        record.input_activity_epoch.fetch_add(1, Ordering::AcqRel);
        shell
            .try_send(ShellCommand::Input {
                bytes: request.bytes,
                deadline: tokio::time::Instant::now() + MANUAL_INPUT_WRITE_TIMEOUT,
                completion,
            })
            .map_err(|_| unavailable_error(request.meta.request_id))?;
        record.last_client_seq = request.client_seq.get();
        record.last_user_input_at = Some(tokio::time::Instant::now());
        Ok(completed)
    }

    async fn approved_plugin_input(
        &mut self,
        request: ApprovedPluginInput,
        reply: oneshot::Sender<ActorResult<()>>,
        mailbox: &mut ActorMailbox,
    ) {
        let result = self.queue_approved_plugin_input(&request);
        let completion = match result {
            Ok(completion) => completion,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let session_id = request.session_id.as_str().to_owned();
        let generation = request.expected_generation.get();
        let request_id = request.request_id;
        // Keep the actor at this linearization point until the dedicated Shell
        // writer acknowledges the write. Focus, attachment, lease, generation,
        // and input-epoch mutations are therefore serialized after this write
        // instead of invalidating an already queued approval behind its back.
        let succeeded = self
            .wait_for_writer_ack(
                completion,
                PLUGIN_INPUT_WRITE_TIMEOUT.saturating_add(Duration::from_secs(1)),
                mailbox,
            )
            .await;
        if !succeeded {
            // Failure must be authoritative before deferred controls resume;
            // a full mailbox must not drop this non-idempotent write outcome.
            self.connection_failed(
                &session_id,
                generation,
                TransportError::ConnectionLost,
                None,
            );
        }
        let _ = reply.send(if succeeded {
            Ok(())
        } else {
            Err(unavailable_error(request_id))
        });
    }

    fn queue_approved_plugin_input(
        &mut self,
        request: &ApprovedPluginInput,
    ) -> ActorResult<oneshot::Receiver<bool>> {
        if request.bytes.is_empty() || request.bytes.len() > SSH_TERMINAL_OUTPUT_FRAME_MAX_BYTES {
            return Err(validation_error(request.request_id.clone()));
        }
        if !request.approval_fence.as_ref().is_none_or(|fence| fence()) {
            return Err(conflict_error(request.request_id.clone()));
        }
        self.validate_global_focus(
            GlobalFocusFence {
                session_id: &request.session_id,
                generation: request.expected_generation,
                channel_id: Some(&request.channel_id),
                attachment_id: &request.attachment_id,
                view_id: &request.view_id,
                focus_epoch: request.focus_epoch,
            },
            &request.request_id,
        )?;
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.request_id.clone()))?;
        validate_lease(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.lease_id,
            request.input_epoch,
            &request.request_id,
            true,
        )?;
        if record.summary.channel_id.as_ref() != Some(&request.channel_id) {
            return Err(conflict_error(request.request_id.clone()));
        }
        let shell = record
            .shell
            .as_ref()
            .ok_or_else(|| unavailable_error(request.request_id.clone()))?;
        let (completion, completed) = oneshot::channel();
        record.input_activity_epoch.fetch_add(1, Ordering::AcqRel);
        shell
            .try_send(ShellCommand::ApprovedPluginInput {
                bytes: request.bytes.clone(),
                deadline: tokio::time::Instant::now() + PLUGIN_INPUT_WRITE_TIMEOUT,
                completion,
            })
            .map_err(|_| unavailable_error(request.request_id.clone()))?;
        record.last_user_input_at = Some(tokio::time::Instant::now());
        Ok(completed)
    }

    fn plugin_observe_attach(
        &mut self,
        request: ApprovedPluginObservationAttach,
    ) -> ActorResult<()> {
        if request.sink.capacity() == 0 || request.focus_epoch.get() != self.focus_epoch {
            return Err(stale_focus_error(request.request_id));
        }
        self.validate_global_focus(
            GlobalFocusFence {
                session_id: &request.target.session_id,
                generation: request.target.expected_generation,
                channel_id: Some(&request.target.channel_id),
                attachment_id: &request.target.attachment_id,
                view_id: &request.target.view_id,
                focus_epoch: request.focus_epoch,
            },
            &request.request_id,
        )?;
        if self.plugin_observers.contains_key(&request.observer_id) {
            return Err(conflict_error(request.request_id));
        }
        self.plugin_observers.insert(
            request.observer_id,
            PluginObserverRecord {
                target: request.target,
                sink: request.sink,
                projection_state: TerminalTextProjectionState::Ground,
                utf8_pending: Vec::new(),
            },
        );
        Ok(())
    }

    fn plugin_observe_detach(&mut self, observer_id: Uuid) {
        if let Some(observer) = self.plugin_observers.remove(&observer_id) {
            let _ = observer.sink.try_send(PluginTerminalObservation::Detached);
        }
    }

    async fn resize_with_ack(
        &mut self,
        request: SshSessionResizeRequest,
        reply: oneshot::Sender<ActorResult<()>>,
        mailbox: &mut ActorMailbox,
    ) {
        let session_id = request.session_id.as_str().to_owned();
        let generation = request.expected_generation.get();
        let resize_seq = request.resize_seq.get();
        let request_id = request.meta.request_id.clone();
        let completion = match self.queue_resize(request) {
            Ok(completion) => completion,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };

        // The broker must not release focus ordering while window-change is
        // merely queued. Wait for the sole Shell owner, then commit sequence.
        let succeeded = self
            .wait_for_writer_ack(
                completion,
                MANUAL_INPUT_WRITE_TIMEOUT.saturating_add(Duration::from_secs(1)),
                mailbox,
            )
            .await;
        let committed = succeeded
            && self.sessions.get_mut(&session_id).is_some_and(|record| {
                if record.summary.generation.get() != generation
                    || resize_seq <= record.last_resize_seq
                {
                    return false;
                }
                record.last_resize_seq = resize_seq;
                true
            });
        if !committed {
            self.connection_failed(
                &session_id,
                generation,
                TransportError::ConnectionLost,
                None,
            );
        }
        let _ = reply.send(if committed {
            Ok(())
        } else {
            Err(unavailable_error(request_id))
        });
    }

    fn queue_resize(
        &mut self,
        request: SshSessionResizeRequest,
    ) -> ActorResult<oneshot::Receiver<bool>> {
        let size = PtySize::new(u32::from(request.cols), u32::from(request.rows), 0, 0)
            .map_err(|_| validation_error(request.meta.request_id.clone()))?;
        self.validate_global_focus(
            GlobalFocusFence {
                session_id: &request.session_id,
                generation: request.expected_generation,
                channel_id: Some(&request.channel_id),
                attachment_id: &request.attachment_id,
                view_id: &request.view_id,
                focus_epoch: request.focus_epoch,
            },
            &request.meta.request_id,
        )?;
        let record = self
            .sessions
            .get_mut(request.session_id.as_str())
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_lease(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.lease_id,
            request.input_epoch,
            &request.meta.request_id,
            true,
        )?;
        if record.summary.channel_id.as_ref() != Some(&request.channel_id)
            || request.resize_seq.get() <= record.last_resize_seq
        {
            return Err(conflict_error(request.meta.request_id));
        }
        let shell = record
            .shell
            .as_ref()
            .ok_or_else(|| unavailable_error(request.meta.request_id.clone()))?;
        let (completion, completed) = oneshot::channel();
        shell
            .try_send(ShellCommand::Resize {
                size,
                deadline: tokio::time::Instant::now() + MANUAL_INPUT_WRITE_TIMEOUT,
                completion,
            })
            .map_err(|_| unavailable_error(request.meta.request_id))?;
        Ok(completed)
    }

    fn reconnect(&mut self, request: SshSessionReconnectRequest) -> ActorResult<SshSessionDetails> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.reconnect_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.reconnect_once(request);
        record_control_operation(
            &mut self.reconnect_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn reconnect_once(
        &mut self,
        request: SshSessionReconnectRequest,
    ) -> ActorResult<SshSessionDetails> {
        if request.rows == 0 || request.cols == 0 {
            return Err(validation_error(request.meta.request_id));
        }
        let session_id = request.session_id.as_str().to_owned();
        let (target, previous_credential_ref_id) = {
            let record = self
                .sessions
                .get(&session_id)
                .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
            if record.terminal_startup.is_some() {
                return Err(plugin_authorization_error(request.meta.request_id));
            }
            validate_attachment(
                record,
                request.expected_generation,
                &request.attachment_id,
                &request.view_id,
                &request.meta.request_id,
            )?;
            if record.summary.generation != request.expected_generation
                || record.summary.state_revision != request.expected_state_revision
                || !matches!(
                    record.summary.state,
                    SshSessionState::Closed | SshSessionState::Failed
                )
            {
                return Err(conflict_error(request.meta.request_id));
            }
            (
                record.summary.target.clone(),
                record.summary.credential_ref_id.clone(),
            )
        };
        let effective_credential_ref_id = match &target {
            SshSessionTarget::QuickConnect { .. } => request
                .credential_ref_id
                .as_ref()
                .or(previous_credential_ref_id.as_ref()),
            SshSessionTarget::Host { .. } => request.credential_ref_id.as_ref(),
        };
        let (profile, target) = resolve_reconnect_connection_profile(
            &self.hosts,
            &self.transient_credentials,
            &target,
            effective_credential_ref_id,
        )
        .map_err(|error| map_connection_profile_error(request.meta.request_id.clone(), error))?;
        let ResolvedConnectionProfile {
            endpoint,
            username,
            ingress,
            jump_hosts,
            credentials,
            algorithm_policy,
            heartbeat_policy,
            login_automation,
            revision_token,
        } = profile;
        let heartbeat = heartbeat_status_from_policy(&heartbeat_policy);
        let credential_ref_id = credentials
            .first()
            .map(|credential| credential.credential_ref_id().clone())
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        let next_generation = request
            .expected_generation
            .get()
            .checked_add(1)
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;

        let pending_takeover;
        let details = {
            self.clear_focus_if(|target| {
                matches!(
                    target,
                    TerminalInputFocusTarget::Ssh(ssh) if ssh.session_id == request.session_id
                )
            });
            self.native_terminal
                .clear_ssh_session(&request.session_id, request.expected_generation);
            let record = self
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
            if let Some(active) = record.active_challenge.take() {
                let _ = active.decision.send(Ok(HostKeyDecision::Rejected));
            }
            clear_keyboard_interactive_challenge(record, TransportError::AuthenticationRejected);
            pending_takeover = clear_login_automation(record);
            abort_heartbeat_tasks(record);
            let _ = request_shell_stop(record, ShellStopSignal::Abort);
            record.shell = None;
            if record.input_lease.take().is_some() {
                emit_payload(
                    record,
                    SshSessionEventPayload::InputLeaseChanged {
                        change: SshSessionInputLeaseChange::Released,
                        lease: None,
                    },
                );
            }

            record.summary.generation = WireSequence::new(next_generation);
            record.summary.target = target;
            record.summary.credential_ref_id = Some(credential_ref_id.clone());
            record.summary.endpoint = Some(endpoint.clone());
            record.summary.channel_id = None;
            record.summary.negotiated_algorithms.clear();
            record.heartbeat_policy = heartbeat_policy.clone();
            record.heartbeat = heartbeat;
            record.last_user_input_at = None;
            record.input_activity_epoch = Arc::new(AtomicU64::new(0));
            record.next_input_epoch = 0;
            record.last_client_seq = 0;
            record.last_resize_seq = 0;
            record.next_output_seq = 1;
            record.output_ring.clear();
            record.output_ring_bytes = 0;

            let next_attachment_revision =
                record.summary.attachment_revision.get().saturating_add(1);
            record.summary.attachment_revision = WireSequence::new(next_attachment_revision);
            transition_record(record, SshSessionState::Resolving, None, None);
            let previous_attachments = std::mem::take(&mut record.attachments);
            for (_, attachment) in previous_attachments {
                let attachment_id = SshAttachmentId::new();
                let summary = SshSessionAttachment {
                    attachment_id: attachment_id.clone(),
                    attach_attempt_id: SshAttachAttemptId::new(),
                    session_id: record.summary.session_id.clone(),
                    generation: WireSequence::new(next_generation),
                    channel_id: None,
                    view_id: attachment.summary.view_id,
                    state_revision: record.summary.state_revision,
                    attachment_revision: WireSequence::new(next_attachment_revision),
                    attached_at_unix_ms: unix_time_ms(),
                };
                record.attachments.insert(
                    attachment_id.as_str().to_owned(),
                    AttachmentRecord {
                        summary,
                        events: attachment.events,
                        last_seen_at_unix_ms: unix_time_ms(),
                    },
                );
            }
            let updated_attachments = record
                .attachments
                .values()
                .map(|value| value.summary.clone())
                .collect::<Vec<_>>();
            for attachment in updated_attachments {
                emit_payload(
                    record,
                    SshSessionEventPayload::AttachmentChanged {
                        change: SshSessionAttachmentChange::Attached,
                        attachment_revision: WireSequence::new(next_attachment_revision),
                        attachment,
                    },
                );
            }
            details_from(record)
        };
        if let Some(pending) = pending_takeover {
            self.reject_pending_login_automation_takeover(pending, true);
        }

        self.live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.clone());
        self.bump_snapshot();
        let automation_view_id = request.view_id.clone();
        let automation_attachment_id = details
            .attachments
            .iter()
            .find(|attachment| attachment.view_id == automation_view_id)
            .map(|attachment| attachment.attachment_id.clone())
            .ok_or_else(|| conflict_error(request.meta.request_id.clone()))?;
        self.start_connect(ConnectPlan {
            session_id,
            generation: next_generation,
            endpoint,
            username,
            ingress,
            jump_hosts,
            credentials,
            algorithm_policy,
            heartbeat_policy,
            login_automation,
            automation_attachment_id,
            automation_view_id,
            profile_revision_token: revision_token,
            rows: request.rows,
            cols: request.cols,
        });
        Ok(details)
    }

    fn disconnect(
        &mut self,
        request: SshSessionDisconnectRequest,
    ) -> ActorResult<SshSessionDetails> {
        if request.idempotency_key.trim().is_empty() {
            return Err(validation_error(request.meta.request_id));
        }
        let request_id = request.meta.request_id.clone();
        let operation_id = request.operation_id.as_str().to_owned();
        let idempotency_key = request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.disconnect_operations,
            &operation_id,
            &idempotency_key,
            &fingerprint,
            request_id.clone(),
        ) {
            return result;
        }
        let result = self.disconnect_once(request);
        record_control_operation(
            &mut self.disconnect_operations,
            operation_id,
            idempotency_key,
            fingerprint,
            request_id,
            result,
        )
    }

    fn disconnect_once(
        &mut self,
        request: SshSessionDisconnectRequest,
    ) -> ActorResult<SshSessionDetails> {
        let session_id = request.session_id.as_str().to_owned();
        let record = self
            .sessions
            .get(&session_id)
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        if record.summary.generation != request.expected_generation
            || record.summary.state_revision != request.expected_state_revision
        {
            return Err(conflict_error(request.meta.request_id));
        }
        if matches!(
            record.summary.state,
            SshSessionState::Closed | SshSessionState::Failed
        ) {
            return Ok(details_from(record));
        }
        self.clear_focus_if(|target| {
            matches!(
                target,
                TerminalInputFocusTarget::Ssh(ssh) if ssh.session_id == request.session_id
            )
        });
        self.abort_connect(&session_id, request.expected_generation.get());
        let (pending_takeover, has_shell, shell_send_failed, details) = {
            let record = self
                .sessions
                .get_mut(&session_id)
                .expect("disconnect target was validated");
            if let Some(active) = record.active_challenge.take() {
                let _ = active.decision.send(Ok(HostKeyDecision::Rejected));
            }
            clear_keyboard_interactive_challenge(record, TransportError::AuthenticationRejected);
            let pending_takeover = clear_login_automation(record);
            let has_shell = record.shell.is_some();
            abort_heartbeat_tasks(record);
            let shell_send_failed = has_shell
                && request_shell_stop(
                    record,
                    ShellStopSignal::Disconnect(SshSessionCloseReason::UserRequested),
                )
                .is_err();
            if has_shell && !shell_send_failed {
                transition_record(record, SshSessionState::Disconnecting, None, None);
            }
            (
                pending_takeover,
                has_shell,
                shell_send_failed,
                details_from(record),
            )
        };
        if let Some(pending) = pending_takeover {
            self.reject_pending_login_automation_takeover(pending, true);
        }
        if shell_send_failed {
            return Err(unavailable_error(request.meta.request_id));
        }
        if !has_shell {
            self.transition(
                &session_id,
                SshSessionState::Closed,
                Some(SshSessionCloseReason::UserRequested),
                None,
            );
            return self.details(&session_id, request.meta.request_id);
        }
        Ok(details)
    }

    fn shell_ready(
        &mut self,
        session_id: String,
        generation: u64,
        credential_ref_id: Option<CredentialRefId>,
        negotiated_algorithms: Vec<SshNegotiatedAlgorithms>,
        context: ShellReadyContext,
        shell: SessionShell,
    ) {
        let ShellReadyContext {
            channels,
            login_automation,
            attachment_id,
            view_id,
            heartbeat_policy,
            transport_heartbeats,
        } = context;
        self.finish_connect(&session_id, generation);
        if !self.sessions.get(&session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && record.summary.state == SshSessionState::OpeningChannel
        }) {
            tauri::async_runtime::spawn(async move {
                let _ = shell.disconnect().await;
            });
            return;
        }
        let automation_enabled = login_automation.is_some();
        let channel_id = SshChannelId::new();
        let (commands, command_rx) = mpsc::channel(SHELL_MAILBOX_CAPACITY);
        let (shell_stop, shell_stop_rx) = watch::channel(None);
        let input_activity_epoch = self
            .sessions
            .get(&session_id)
            .map(|record| Arc::clone(&record.input_activity_epoch))
            .expect("validated shell-ready session exists");
        let tx = self.tx.clone();
        tauri::async_runtime::spawn(run_shell(
            tx,
            session_id.clone(),
            generation,
            shell,
            command_rx,
            input_activity_epoch,
            shell_stop_rx,
        ));
        let Some(record) = self.sessions.get_mut(&session_id) else {
            return;
        };
        record.shared_channels = Some(channels);
        record.channel_lease_cancel = watch::channel(false).0;
        record.shell = Some(commands);
        record.shell_stop = Some(shell_stop);
        record.summary.credential_ref_id = credential_ref_id;
        record.summary.channel_id = Some(channel_id.clone());
        record.summary.negotiated_algorithms = negotiated_algorithms.clone();
        abort_heartbeat_tasks(record);
        record.heartbeat_policy = heartbeat_policy.clone();
        record.heartbeat = heartbeat_status_from_policy(&heartbeat_policy);
        record.last_user_input_at = Some(tokio::time::Instant::now());
        emit_payload(
            record,
            SshSessionEventPayload::NegotiatedAlgorithmsChanged {
                algorithms: negotiated_algorithms,
            },
        );
        let next_attachment_revision = record.summary.attachment_revision.get().saturating_add(1);
        record.summary.attachment_revision = WireSequence::new(next_attachment_revision);
        let updated_attachments = record
            .attachments
            .values_mut()
            .map(|attachment| {
                attachment.summary.channel_id = Some(channel_id.clone());
                attachment.summary.state_revision =
                    WireSequence::new(record.summary.state_revision.get().saturating_add(1));
                attachment.summary.attachment_revision =
                    WireSequence::new(next_attachment_revision);
                attachment.summary.clone()
            })
            .collect::<Vec<_>>();
        for attachment in updated_attachments {
            // Reconnect rotates attachments before a replacement Channel exists.
            // Publish the same attachment identity again once its Channel fence is
            // ready so renderers cannot show Running with a stale null channel.
            emit_payload(
                record,
                SshSessionEventPayload::AttachmentChanged {
                    change: SshSessionAttachmentChange::Attached,
                    attachment_revision: WireSequence::new(next_attachment_revision),
                    attachment,
                },
            );
        }
        transition_record(
            record,
            if automation_enabled {
                SshSessionState::AutomatingLogin
            } else {
                SshSessionState::Running
            },
            None,
            None,
        );
        if let Some(plan) = login_automation {
            let started_at_unix_ms = unix_time_ms();
            let total_deadline_unix_ms = started_at_unix_ms.saturating_add(300_000);
            let started_at = tokio::time::Instant::now();
            let total_deadline = started_at + Duration::from_secs(300);
            let step_deadline = (started_at
                + login_automation_step_timeout_duration(&plan.steps[0]))
            .min(total_deadline);
            let progress = login_automation_progress(
                plan.revision,
                &plan.steps,
                0,
                started_at_unix_ms,
                total_deadline_unix_ms,
                SshLoginAutomationStatus::Running,
                None,
            );
            record.active_login_automation = Some(ActiveLoginAutomation {
                policy_revision: plan.revision,
                steps: plan.steps,
                current_step_index: 0,
                matcher_buffer: Vec::new(),
                timeout_scheduled_for: None,
                total_deadline_unix_ms,
                total_deadline,
                step_deadline,
                owner_attachment_id: attachment_id,
                owner_view_id: view_id,
                write_in_flight: None,
                pending_takeover: None,
                progress: progress.clone(),
            });
            emit_payload(
                record,
                SshSessionEventPayload::LoginAutomationProgressChanged {
                    progress: Some(progress),
                },
            );
        }
        let updated_attachments = record
            .attachments
            .values()
            .map(|attachment| attachment.summary.clone())
            .collect::<Vec<_>>();
        for attachment in updated_attachments {
            emit_payload(
                record,
                SshSessionEventPayload::AttachmentChanged {
                    change: SshSessionAttachmentChange::Attached,
                    attachment_revision: WireSequence::new(next_attachment_revision),
                    attachment,
                },
            );
        }
        self.start_transport_heartbeats(&session_id, generation, transport_heartbeats);
        self.bump_snapshot();
        if automation_enabled {
            self.drive_login_automation(&session_id, generation);
        } else {
            self.start_shell_heartbeat(&session_id, generation);
            self.record_successful_host_connection(&session_id);
        }
    }

    fn drive_login_automation(&mut self, session_id: &str, generation: u64) {
        loop {
            let now = tokio::time::Instant::now();
            let (blocked, deadline_failure) = self
                .sessions
                .get(session_id)
                .and_then(|record| {
                    record
                        .active_login_automation
                        .as_ref()
                        .map(|automation| (record, automation))
                })
                .map(|(record, automation)| {
                    let owner_is_current = record
                        .attachments
                        .get(automation.owner_attachment_id.as_str())
                        .is_some_and(|attachment| {
                            attachment.summary.view_id == automation.owner_view_id
                                && attachment.summary.generation.get() == generation
                                && attachment.summary.channel_id == record.summary.channel_id
                        });
                    let failure = if !owner_is_current {
                        Some(SshLoginAutomationFailureCode::AttachmentUnavailable)
                    } else if now >= automation.total_deadline {
                        Some(SshLoginAutomationFailureCode::TotalTimeout)
                    } else if now >= automation.step_deadline {
                        Some(SshLoginAutomationFailureCode::StepTimeout)
                    } else {
                        None
                    };
                    (
                        automation.write_in_flight.is_some()
                            || automation.pending_takeover.is_some(),
                        failure,
                    )
                })
                .unwrap_or((true, None));
            if let Some(failure) = deadline_failure {
                self.fail_login_automation(session_id, generation, failure);
                return;
            }
            if blocked {
                return;
            }
            let Some(step) = self.sessions.get(session_id).and_then(|record| {
                if record.summary.generation.get() != generation
                    || record.summary.state != SshSessionState::AutomatingLogin
                {
                    return None;
                }
                let automation = record.active_login_automation.as_ref()?;
                (automation.progress.status == SshLoginAutomationStatus::Running)
                    .then(|| automation.steps.get(automation.current_step_index).cloned())
                    .flatten()
            }) else {
                return;
            };

            match step {
                LoginAutomationStepInput::Expect { literal_text, .. } => {
                    let matched = self
                        .sessions
                        .get_mut(session_id)
                        .and_then(|record| record.active_login_automation.as_mut())
                        .and_then(|automation| {
                            let literal = literal_text.as_bytes();
                            let offset = find_literal_bytes(&automation.matcher_buffer, literal)?;
                            let consumed = offset.saturating_add(literal.len());
                            automation.matcher_buffer.drain(..consumed);
                            Some(())
                        })
                        .is_some();
                    if !matched {
                        self.schedule_login_automation_timeout(session_id, generation);
                        return;
                    }
                }
                LoginAutomationStepInput::SendText {
                    text, append_enter, ..
                } => {
                    let payload = login_automation_payload(text.as_bytes(), append_enter);
                    if let Err(failure) =
                        self.queue_login_automation_write(session_id, generation, payload)
                    {
                        self.fail_login_automation(session_id, generation, failure);
                        return;
                    }
                    return;
                }
                LoginAutomationStepInput::SendSecret {
                    secret_ref_id,
                    append_enter,
                    ..
                } => {
                    let secret = match self
                        .vault
                        .read_secret(&secret_ref_id, SecretKind::LoginAutomation)
                    {
                        Ok(secret) => secret,
                        Err(VaultServiceError::NotUnlocked) => {
                            self.fail_login_automation(
                                session_id,
                                generation,
                                SshLoginAutomationFailureCode::VaultLocked,
                            );
                            return;
                        }
                        Err(_) => {
                            self.fail_login_automation(
                                session_id,
                                generation,
                                SshLoginAutomationFailureCode::SecretUnavailable,
                            );
                            return;
                        }
                    };
                    let payload = login_automation_payload(secret.expose(), append_enter);
                    if let Err(failure) =
                        self.queue_login_automation_write(session_id, generation, payload)
                    {
                        self.fail_login_automation(session_id, generation, failure);
                        return;
                    }
                    return;
                }
                LoginAutomationStepInput::PreserveExistingSecret { .. } => {
                    self.fail_login_automation(
                        session_id,
                        generation,
                        SshLoginAutomationFailureCode::SecretUnavailable,
                    );
                    return;
                }
            }

            if self.advance_login_automation(session_id, generation) {
                self.finish_login_automation(session_id, generation);
                return;
            }
        }
    }

    fn queue_login_automation_write(
        &mut self,
        session_id: &str,
        generation: u64,
        payload: Zeroizing<Vec<u8>>,
    ) -> Result<(), SshLoginAutomationFailureCode> {
        let Some(shell) = self
            .sessions
            .get(session_id)
            .and_then(|record| record.shell.clone())
        else {
            return Err(SshLoginAutomationFailureCode::ChannelUnavailable);
        };
        let Some(record) = self.sessions.get_mut(session_id) else {
            return Err(SshLoginAutomationFailureCode::ChannelUnavailable);
        };
        if record.summary.generation.get() != generation
            || record.summary.state != SshSessionState::AutomatingLogin
        {
            return Err(SshLoginAutomationFailureCode::ChannelUnavailable);
        }
        let Some(automation) = record.active_login_automation.as_mut() else {
            return Err(SshLoginAutomationFailureCode::ChannelUnavailable);
        };
        let now = tokio::time::Instant::now();
        if now >= automation.total_deadline {
            return Err(SshLoginAutomationFailureCode::TotalTimeout);
        }
        if now >= automation.step_deadline {
            return Err(SshLoginAutomationFailureCode::StepTimeout);
        }
        if automation.write_in_flight.is_some()
            || automation.progress.status != SshLoginAutomationStatus::Running
        {
            return Err(SshLoginAutomationFailureCode::ChannelUnavailable);
        }
        let step_index = automation.current_step_index;
        let state_revision = record.summary.state_revision.get();
        let deadline = automation.step_deadline;
        let (completion, completed) = oneshot::channel();
        if shell
            .try_send(ShellCommand::AutomationInput {
                bytes: payload,
                deadline,
                completion,
            })
            .is_err()
        {
            return Err(SshLoginAutomationFailureCode::ChannelUnavailable);
        }
        automation.write_in_flight = Some(step_index);
        let tx = self.tx.clone();
        let session_id = session_id.to_owned();
        tauri::async_runtime::spawn(async move {
            let succeeded = completed.await.unwrap_or(false);
            let _ = tx
                .send(Message::LoginAutomationWriteCompleted {
                    session_id,
                    generation,
                    state_revision,
                    step_index,
                    succeeded,
                })
                .await;
        });
        Ok(())
    }

    fn login_automation_write_completed(
        &mut self,
        session_id: &str,
        generation: u64,
        state_revision: u64,
        step_index: usize,
        succeeded: bool,
    ) {
        let valid = self.sessions.get(session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && record.summary.state == SshSessionState::AutomatingLogin
                && record.summary.state_revision.get() == state_revision
                && record
                    .active_login_automation
                    .as_ref()
                    .is_some_and(|automation| {
                        automation.current_step_index == step_index
                            && automation.write_in_flight == Some(step_index)
                    })
        });
        if !valid {
            return;
        }
        let pending_takeover = self
            .sessions
            .get_mut(session_id)
            .and_then(|record| record.active_login_automation.as_mut())
            .and_then(|automation| {
                automation.write_in_flight = None;
                automation.pending_takeover.take()
            });
        if !succeeded {
            self.fail_login_automation(
                session_id,
                generation,
                SshLoginAutomationFailureCode::ChannelUnavailable,
            );
            if let Some(pending) = pending_takeover {
                let result = Err(unavailable_error(pending.request.meta.request_id.clone()));
                self.complete_pending_login_automation_takeover(pending, result);
            }
            return;
        }
        if self.sessions.get(session_id).is_some_and(|record| {
            record
                .active_login_automation
                .as_ref()
                .is_some_and(|automation| {
                    automation.progress.status == SshLoginAutomationStatus::Failed
                })
        }) {
            if let Some(pending) = pending_takeover {
                self.reject_pending_login_automation_takeover(pending, false);
            }
            return;
        }
        if let Some(pending) = pending_takeover {
            let result = self.finish_login_automation_takeover(pending.request.clone());
            let takeover_failed = result.is_err();
            self.complete_pending_login_automation_takeover(pending, result);
            if takeover_failed {
                if self.advance_login_automation(session_id, generation) {
                    self.finish_login_automation(session_id, generation);
                } else {
                    self.drive_login_automation(session_id, generation);
                }
            }
            return;
        }
        if self.advance_login_automation(session_id, generation) {
            self.finish_login_automation(session_id, generation);
        } else {
            self.drive_login_automation(session_id, generation);
        }
    }

    fn advance_login_automation(&mut self, session_id: &str, generation: u64) -> bool {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return false;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != SshSessionState::AutomatingLogin
        {
            return false;
        }
        let Some(automation) = record.active_login_automation.as_mut() else {
            return false;
        };
        automation.current_step_index = automation.current_step_index.saturating_add(1);
        automation.timeout_scheduled_for = None;
        if automation.current_step_index >= automation.steps.len() {
            return true;
        }
        automation.step_deadline = (tokio::time::Instant::now()
            + login_automation_step_timeout_duration(
                &automation.steps[automation.current_step_index],
            ))
        .min(automation.total_deadline);
        automation.progress = login_automation_progress(
            automation.policy_revision,
            &automation.steps,
            automation.current_step_index,
            automation.progress.started_at_unix_ms,
            automation.total_deadline_unix_ms,
            SshLoginAutomationStatus::Running,
            None,
        );
        let progress = automation.progress.clone();
        emit_payload(
            record,
            SshSessionEventPayload::LoginAutomationProgressChanged {
                progress: Some(progress),
            },
        );
        false
    }

    fn schedule_login_automation_timeout(&mut self, session_id: &str, generation: u64) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != SshSessionState::AutomatingLogin
        {
            return;
        }
        let Some(automation) = record.active_login_automation.as_mut() else {
            return;
        };
        if automation.timeout_scheduled_for == Some(automation.current_step_index)
            || automation.progress.status != SshLoginAutomationStatus::Running
        {
            return;
        }
        automation.timeout_scheduled_for = Some(automation.current_step_index);
        let step_index = automation.current_step_index;
        let state_revision = record.summary.state_revision.get();
        let delay = automation
            .step_deadline
            .saturating_duration_since(tokio::time::Instant::now());
        let tx = self.tx.clone();
        let session_id = session_id.to_owned();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx
                .send(Message::LoginAutomationStepTimeout {
                    session_id,
                    generation,
                    state_revision,
                    step_index,
                })
                .await;
        });
    }

    fn login_automation_step_timeout(
        &mut self,
        session_id: &str,
        generation: u64,
        state_revision: u64,
        step_index: usize,
    ) {
        let failure = self.sessions.get(session_id).and_then(|record| {
            if record.summary.generation.get() != generation {
                return None;
            }
            if record.summary.state != SshSessionState::AutomatingLogin
                || record.summary.state_revision.get() != state_revision
            {
                return None;
            }
            let automation = record.active_login_automation.as_ref()?;
            if automation.current_step_index != step_index
                || automation.progress.status != SshLoginAutomationStatus::Running
            {
                return None;
            }
            let owner_is_current = record
                .attachments
                .get(automation.owner_attachment_id.as_str())
                .is_some_and(|attachment| {
                    attachment.summary.view_id == automation.owner_view_id
                        && attachment.summary.generation.get() == generation
                        && attachment.summary.channel_id == record.summary.channel_id
                });
            if !owner_is_current {
                Some(SshLoginAutomationFailureCode::AttachmentUnavailable)
            } else if tokio::time::Instant::now() >= automation.total_deadline {
                Some(SshLoginAutomationFailureCode::TotalTimeout)
            } else if tokio::time::Instant::now() >= automation.step_deadline {
                Some(SshLoginAutomationFailureCode::StepTimeout)
            } else {
                None
            }
        });
        if let Some(failure) = failure {
            self.fail_login_automation(session_id, generation, failure);
        }
    }

    fn fail_login_automation(
        &mut self,
        session_id: &str,
        generation: u64,
        failure_code: SshLoginAutomationFailureCode,
    ) {
        let pending = {
            let Some(record) = self.sessions.get_mut(session_id) else {
                return;
            };
            if record.summary.generation.get() != generation
                || record.summary.state != SshSessionState::AutomatingLogin
            {
                return;
            }
            fail_login_automation_record(record, failure_code);
            record
                .active_login_automation
                .as_mut()
                .and_then(|automation| automation.pending_takeover.take())
        };
        if let Some(pending) = pending {
            self.reject_pending_login_automation_takeover(pending, false);
        }
    }

    fn finish_login_automation(&mut self, session_id: &str, generation: u64) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != SshSessionState::AutomatingLogin
        {
            return;
        }
        record.active_login_automation.take();
        emit_payload(
            record,
            SshSessionEventPayload::LoginAutomationProgressChanged { progress: None },
        );
        let next_state_revision = record.summary.state_revision.get().saturating_add(1);
        for attachment in record.attachments.values_mut() {
            attachment.summary.state_revision = WireSequence::new(next_state_revision);
        }
        record.last_user_input_at = Some(tokio::time::Instant::now());
        transition_record(record, SshSessionState::Running, None, None);
        self.bump_snapshot();
        self.start_shell_heartbeat(session_id, generation);
        self.record_successful_host_connection(session_id);
    }

    fn start_transport_heartbeats(
        &mut self,
        session_id: &str,
        generation: u64,
        transports: Vec<RoutedTransportHeartbeat>,
    ) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        let HeartbeatPolicy::TransportKeepalive {
            interval_seconds,
            reply_timeout_seconds,
            failure_threshold,
        } = record.heartbeat_policy.policy
        else {
            return;
        };
        for task in record.transport_heartbeat_tasks.drain(..) {
            task.abort();
        }
        let policy_revision = record.heartbeat_policy.revision;
        record.heartbeat.transports = transports
            .iter()
            .map(|transport| SshTransportHeartbeatStatus {
                route_stage: transport.route_stage.clone(),
                next_due_at_unix_ms: Some(transport.first_due_at_unix_ms),
                last_sent_at_unix_ms: None,
                last_ack_at_unix_ms: None,
                consecutive_failures: 0,
            })
            .collect();
        let heartbeat = record.heartbeat.clone();
        emit_payload(
            record,
            SshSessionEventPayload::HeartbeatChanged { heartbeat },
        );
        let tasks = transports
            .into_iter()
            .map(|transport| {
                let tx = self.tx.clone();
                let session_id = session_id.to_owned();
                let route_stage = transport.route_stage;
                let handle = transport.handle;
                let mut next_due = transport.first_due;
                let mut next_due_at_unix_ms = transport.first_due_at_unix_ms;
                let task = tauri::async_runtime::spawn(async move {
                    let interval = Duration::from_secs(u64::from(interval_seconds));
                    let reply_timeout = Duration::from_secs(u64::from(reply_timeout_seconds));
                    let mut consecutive_failures = 0_u8;
                    loop {
                        if tx
                            .send(Message::TransportHeartbeatScheduled {
                                session_id: session_id.clone(),
                                generation,
                                policy_revision,
                                route_stage: route_stage.clone(),
                                next_due_at_unix_ms,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        tokio::time::sleep_until(next_due).await;
                        let last_sent_at_unix_ms = unix_time_ms();
                        let request_result =
                            tokio::time::timeout(reply_timeout, handle.request_reply()).await;
                        let attempt = match request_result {
                            Ok(Ok(())) => TransportHeartbeatAttempt::Acknowledged,
                            Ok(Err(_)) => TransportHeartbeatAttempt::Closed,
                            Err(_) => TransportHeartbeatAttempt::TimedOut,
                        };
                        let last_ack_at_unix_ms =
                            (attempt == TransportHeartbeatAttempt::Acknowledged).then(unix_time_ms);
                        consecutive_failures = next_transport_heartbeat_failures(
                            attempt,
                            consecutive_failures,
                            failure_threshold,
                        );
                        let failed = consecutive_failures >= failure_threshold;
                        let observed_next_due_at_unix_ms = (!failed).then(|| {
                            next_due = tokio::time::Instant::now() + interval;
                            next_due_at_unix_ms = unix_time_ms()
                                .saturating_add(i64::from(interval_seconds).saturating_mul(1_000));
                            next_due_at_unix_ms
                        });
                        if tx
                            .send(Message::TransportHeartbeatObserved {
                                session_id: session_id.clone(),
                                generation,
                                policy_revision,
                                route_stage: route_stage.clone(),
                                last_sent_at_unix_ms,
                                last_ack_at_unix_ms,
                                consecutive_failures,
                                next_due_at_unix_ms: observed_next_due_at_unix_ms,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if failed {
                            let _ = tx
                                .send(Message::TransportHeartbeatFailed {
                                    session_id,
                                    generation,
                                    policy_revision,
                                    route_stage,
                                })
                                .await;
                            break;
                        }
                    }
                });
                HeartbeatTask::new(task)
            })
            .collect();
        if let Some(record) = self.sessions.get_mut(session_id) {
            record.transport_heartbeat_tasks = tasks;
        }
    }

    fn transport_heartbeat_scheduled(
        &mut self,
        session_id: &str,
        generation: u64,
        policy_revision: Option<WireSequence>,
        route_stage: SshSessionRouteStage,
        next_due_at_unix_ms: i64,
    ) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation.get() != generation
            || record.heartbeat.policy_revision != policy_revision
            || record.heartbeat.mode != SshHeartbeatMode::TransportKeepalive
            || !matches!(
                record.summary.state,
                SshSessionState::AutomatingLogin | SshSessionState::Running
            )
        {
            return;
        }
        let Some(status) = record
            .heartbeat
            .transports
            .iter_mut()
            .find(|status| status.route_stage == route_stage)
        else {
            return;
        };
        status.next_due_at_unix_ms = Some(next_due_at_unix_ms);
        let heartbeat = record.heartbeat.clone();
        emit_payload(
            record,
            SshSessionEventPayload::HeartbeatChanged { heartbeat },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn transport_heartbeat_observed(
        &mut self,
        session_id: &str,
        generation: u64,
        policy_revision: Option<WireSequence>,
        route_stage: SshSessionRouteStage,
        last_sent_at_unix_ms: i64,
        last_ack_at_unix_ms: Option<i64>,
        consecutive_failures: u8,
        next_due_at_unix_ms: Option<i64>,
    ) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation.get() != generation
            || record.heartbeat.policy_revision != policy_revision
            || record.heartbeat.mode != SshHeartbeatMode::TransportKeepalive
            || !matches!(
                record.summary.state,
                SshSessionState::AutomatingLogin | SshSessionState::Running
            )
        {
            return;
        }
        let Some(status) = record
            .heartbeat
            .transports
            .iter_mut()
            .find(|status| status.route_stage == route_stage)
        else {
            return;
        };
        status.last_sent_at_unix_ms = Some(last_sent_at_unix_ms);
        if let Some(last_ack_at_unix_ms) = last_ack_at_unix_ms {
            status.last_ack_at_unix_ms = Some(last_ack_at_unix_ms);
        }
        status.consecutive_failures = consecutive_failures;
        status.next_due_at_unix_ms = next_due_at_unix_ms;
        let heartbeat = record.heartbeat.clone();
        emit_payload(
            record,
            SshSessionEventPayload::HeartbeatChanged { heartbeat },
        );
    }

    fn transport_heartbeat_failed(
        &mut self,
        session_id: &str,
        generation: u64,
        policy_revision: Option<WireSequence>,
        route_stage: SshSessionRouteStage,
    ) {
        let valid = self.sessions.get(session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && record.heartbeat.policy_revision == policy_revision
                && record.heartbeat.mode == SshHeartbeatMode::TransportKeepalive
                && matches!(
                    record.summary.state,
                    SshSessionState::AutomatingLogin | SshSessionState::Running
                )
        });
        if !valid {
            return;
        }
        if let Some(record) = self.sessions.get_mut(session_id) {
            let _ = request_shell_stop(record, ShellStopSignal::Abort);
        }
        let failure = failure_from_route(TransportError::ConnectionLost, Some(route_stage));
        self.transition(session_id, SshSessionState::Failed, None, Some(failure));
    }

    fn start_shell_heartbeat(&mut self, session_id: &str, generation: u64) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        let HeartbeatPolicy::ShellHeartbeat {
            interval_seconds, ..
        } = record.heartbeat_policy.policy
        else {
            return;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != SshSessionState::Running
        {
            return;
        }
        if let Some(task) = record.shell_heartbeat_task.take() {
            task.abort();
        }
        let policy_revision = record.heartbeat_policy.revision;
        let next_due_at_unix_ms =
            unix_time_ms().saturating_add(i64::from(interval_seconds).saturating_mul(1_000));
        record.heartbeat.shell = Some(SshShellHeartbeatStatus {
            next_due_at_unix_ms: Some(next_due_at_unix_ms),
            last_sent_at_unix_ms: None,
            skip_reason: None,
        });
        let heartbeat = record.heartbeat.clone();
        emit_payload(
            record,
            SshSessionEventPayload::HeartbeatChanged { heartbeat },
        );
        let tx = self.tx.clone();
        let owned_session_id = session_id.to_owned();
        let task = tauri::async_runtime::spawn(async move {
            let interval = Duration::from_secs(u64::from(interval_seconds));
            loop {
                tokio::time::sleep(interval).await;
                let next_due_at_unix_ms = unix_time_ms()
                    .saturating_add(i64::from(interval_seconds).saturating_mul(1_000));
                if tx
                    .send(Message::ShellHeartbeatTick {
                        session_id: owned_session_id.clone(),
                        generation,
                        policy_revision,
                        next_due_at_unix_ms,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        if let Some(record) = self.sessions.get_mut(session_id) {
            record.shell_heartbeat_task = Some(HeartbeatTask::new(task));
        }
    }

    fn shell_heartbeat_tick(
        &mut self,
        session_id: &str,
        generation: u64,
        policy_revision: Option<WireSequence>,
        next_due_at_unix_ms: i64,
    ) {
        let (completion, completed) = oneshot::channel();
        let queued = {
            let Some(record) = self.sessions.get_mut(session_id) else {
                return;
            };
            if record.summary.generation.get() != generation
                || record.summary.state != SshSessionState::Running
                || record.heartbeat.policy_revision != policy_revision
                || record.heartbeat.mode != SshHeartbeatMode::ShellHeartbeat
            {
                return;
            }
            let (payload, user_idle_seconds) = match &record.heartbeat_policy.policy {
                HeartbeatPolicy::ShellHeartbeat {
                    payload_text,
                    line_ending,
                    user_idle_seconds,
                    ..
                } => (
                    shell_heartbeat_payload(payload_text, *line_ending),
                    *user_idle_seconds,
                ),
                _ => return,
            };
            let skip_reason = if record.shell_heartbeat_write_in_flight {
                Some(SshShellHeartbeatSkipReason::WriterBusy)
            } else if record.last_user_input_at.is_some_and(|last_input| {
                tokio::time::Instant::now().saturating_duration_since(last_input)
                    < Duration::from_secs(u64::from(user_idle_seconds))
            }) {
                Some(SshShellHeartbeatSkipReason::UserActive)
            } else {
                None
            };
            let status = record
                .heartbeat
                .shell
                .get_or_insert(SshShellHeartbeatStatus {
                    next_due_at_unix_ms: None,
                    last_sent_at_unix_ms: None,
                    skip_reason: None,
                });
            status.next_due_at_unix_ms = Some(next_due_at_unix_ms);
            if let Some(skip_reason) = skip_reason {
                status.skip_reason = Some(skip_reason);
                let heartbeat = record.heartbeat.clone();
                emit_payload(
                    record,
                    SshSessionEventPayload::HeartbeatChanged { heartbeat },
                );
                false
            } else {
                let Some(shell) = record.shell.clone() else {
                    return;
                };
                let expected_input_activity_epoch =
                    record.input_activity_epoch.load(Ordering::Acquire);
                if shell
                    .try_send(ShellCommand::HeartbeatInput {
                        bytes: payload,
                        expected_input_activity_epoch,
                        deadline: tokio::time::Instant::now() + SHELL_HEARTBEAT_WRITE_TIMEOUT,
                        completion,
                    })
                    .is_err()
                {
                    status.skip_reason = Some(SshShellHeartbeatSkipReason::WriterBusy);
                    let heartbeat = record.heartbeat.clone();
                    emit_payload(
                        record,
                        SshSessionEventPayload::HeartbeatChanged { heartbeat },
                    );
                    false
                } else {
                    record.shell_heartbeat_write_in_flight = true;
                    status.skip_reason = None;
                    true
                }
            }
        };
        if !queued {
            return;
        }
        let tx = self.tx.clone();
        let session_id = session_id.to_owned();
        tauri::async_runtime::spawn(async move {
            let outcome = completed
                .await
                .unwrap_or(ShellHeartbeatWriteOutcome::Failed);
            let _ = tx
                .send(Message::ShellHeartbeatWriteCompleted {
                    session_id,
                    generation,
                    policy_revision,
                    outcome,
                    sent_at_unix_ms: unix_time_ms(),
                })
                .await;
        });
    }

    fn shell_heartbeat_write_completed(
        &mut self,
        session_id: &str,
        generation: u64,
        policy_revision: Option<WireSequence>,
        outcome: ShellHeartbeatWriteOutcome,
        sent_at_unix_ms: i64,
    ) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation.get() != generation
            || record.summary.state != SshSessionState::Running
            || record.heartbeat.policy_revision != policy_revision
            || record.heartbeat.mode != SshHeartbeatMode::ShellHeartbeat
            || !record.shell_heartbeat_write_in_flight
        {
            return;
        }
        record.shell_heartbeat_write_in_flight = false;
        if let Some(status) = record.heartbeat.shell.as_mut() {
            match outcome {
                ShellHeartbeatWriteOutcome::Sent => {
                    status.last_sent_at_unix_ms = Some(sent_at_unix_ms);
                    status.skip_reason = None;
                }
                ShellHeartbeatWriteOutcome::UserActive => {
                    status.skip_reason = Some(SshShellHeartbeatSkipReason::UserActive);
                }
                ShellHeartbeatWriteOutcome::WriterBusy => {
                    status.skip_reason = Some(SshShellHeartbeatSkipReason::WriterBusy);
                }
                ShellHeartbeatWriteOutcome::Failed => return,
            }
            let heartbeat = record.heartbeat.clone();
            emit_payload(
                record,
                SshSessionEventPayload::HeartbeatChanged { heartbeat },
            );
        }
    }

    fn login_automation_takeover(
        &mut self,
        request: SshLoginAutomationTakeoverRequest,
        reply: oneshot::Sender<ActorResult<SshLoginAutomationTakeoverResponse>>,
    ) {
        if request.idempotency_key.trim().is_empty() {
            let _ = reply.send(Err(validation_error(request.meta.request_id)));
            return;
        }
        let fingerprint = SshOperationFingerprint::from(&request);
        if let Some(result) = replay_control_operation(
            &self.login_automation_takeover_operations,
            request.operation_id.as_str(),
            &request.idempotency_key,
            &fingerprint,
            request.meta.request_id.clone(),
        ) {
            let _ = reply.send(result);
            return;
        }
        if let Some(pending) = self
            .sessions
            .get_mut(request.session_id.as_str())
            .and_then(|record| record.active_login_automation.as_mut())
            .and_then(|automation| automation.pending_takeover.as_mut())
        {
            let exact_duplicate = pending.request.operation_id == request.operation_id
                && pending.request.idempotency_key == request.idempotency_key
                && SshOperationFingerprint::from(&pending.request)
                    == SshOperationFingerprint::from(&request);
            if exact_duplicate {
                pending
                    .replies
                    .push((request.meta.request_id.clone(), reply));
            } else {
                let _ = reply.send(Err(conflict_error(request.meta.request_id)));
            }
            return;
        }
        let validation = self.validate_login_automation_takeover(&request);
        if let Err(error) = validation {
            let _ = reply.send(Err(error));
            return;
        }
        let session_id = request.session_id.as_str().to_owned();
        let write_in_flight = self
            .sessions
            .get(&session_id)
            .and_then(|record| record.active_login_automation.as_ref())
            .is_some_and(|automation| automation.write_in_flight.is_some());
        if write_in_flight {
            let automation = self
                .sessions
                .get_mut(&session_id)
                .and_then(|record| record.active_login_automation.as_mut())
                .expect("validated automation remains active");
            automation.pending_takeover = Some(PendingLoginAutomationTakeover {
                replies: vec![(request.meta.request_id.clone(), reply)],
                request,
            });
            return;
        }
        let result = self.finish_login_automation_takeover(request.clone());
        let result = self.record_login_automation_takeover(request, result);
        let _ = reply.send(result);
    }

    fn record_login_automation_takeover(
        &mut self,
        request: SshLoginAutomationTakeoverRequest,
        result: ActorResult<SshLoginAutomationTakeoverResponse>,
    ) -> ActorResult<SshLoginAutomationTakeoverResponse> {
        record_control_operation(
            &mut self.login_automation_takeover_operations,
            request.operation_id.as_str().to_owned(),
            request.idempotency_key.clone(),
            SshOperationFingerprint::from(&request),
            request.meta.request_id.clone(),
            result,
        )
    }

    fn complete_pending_login_automation_takeover(
        &mut self,
        pending: PendingLoginAutomationTakeover,
        result: ActorResult<SshLoginAutomationTakeoverResponse>,
    ) {
        let operation_id = pending.request.operation_id.as_str().to_owned();
        let idempotency_key = pending.request.idempotency_key.clone();
        let fingerprint = SshOperationFingerprint::from(&pending.request);
        let _ = self.record_login_automation_takeover(pending.request, result);
        for (request_id, reply) in pending.replies {
            let replay = replay_control_operation(
                &self.login_automation_takeover_operations,
                &operation_id,
                &idempotency_key,
                &fingerprint,
                request_id.clone(),
            )
            .unwrap_or_else(|| Err(unavailable_error(request_id)));
            let _ = reply.send(replay);
        }
    }

    fn reject_pending_login_automation_takeover(
        &mut self,
        pending: PendingLoginAutomationTakeover,
        unavailable: bool,
    ) {
        let request_id = pending.request.meta.request_id.clone();
        let result = if unavailable {
            Err(unavailable_error(request_id))
        } else {
            Err(conflict_error(request_id))
        };
        self.complete_pending_login_automation_takeover(pending, result);
    }

    fn validate_login_automation_takeover(
        &self,
        request: &SshLoginAutomationTakeoverRequest,
    ) -> ActorResult<()> {
        if request.idempotency_key.trim().is_empty()
            || request.expected_focus_epoch.get() != self.focus_epoch
        {
            return Err(conflict_error(request.meta.request_id.clone()));
        }
        let session_id = request.session_id.as_str().to_owned();
        let record = self
            .sessions
            .get(&session_id)
            .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
        validate_attachment(
            record,
            request.expected_generation,
            &request.attachment_id,
            &request.view_id,
            &request.meta.request_id,
        )?;
        if record.summary.generation != request.expected_generation
            || record.summary.state_revision != request.expected_state_revision
            || record.summary.state != SshSessionState::AutomatingLogin
            || record.summary.channel_id.as_ref() != Some(&request.channel_id)
            || record.active_login_automation.is_none()
        {
            return Err(conflict_error(request.meta.request_id.clone()));
        }
        Ok(())
    }

    fn finish_login_automation_takeover(
        &mut self,
        request: SshLoginAutomationTakeoverRequest,
    ) -> ActorResult<SshLoginAutomationTakeoverResponse> {
        self.validate_login_automation_takeover(&request)?;
        let session_id = request.session_id.as_str().to_owned();
        let next_state_revision = {
            let record = self
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| not_found_error(request.meta.request_id.clone()))?;
            record.active_login_automation.take();
            emit_payload(
                record,
                SshSessionEventPayload::LoginAutomationProgressChanged { progress: None },
            );
            let next_state_revision = record.summary.state_revision.get().saturating_add(1);
            for attachment in record.attachments.values_mut() {
                attachment.summary.state_revision = WireSequence::new(next_state_revision);
            }
            record.last_user_input_at = Some(tokio::time::Instant::now());
            transition_record(record, SshSessionState::Running, None, None);
            next_state_revision
        };
        let response = self.terminal_focus_change_once(TerminalInputFocusChangeRequest {
            meta: request.meta.clone(),
            operation_id: request.operation_id,
            idempotency_key: request.idempotency_key,
            expected_focus_epoch: request.expected_focus_epoch,
            target: Some(TerminalInputFocusTarget::Ssh(
                norishell_core_api::SshTerminalInputFocusTarget {
                    session_id: request.session_id.clone(),
                    expected_generation: request.expected_generation,
                    expected_state_revision: WireSequence::new(next_state_revision),
                    channel_id: request.channel_id,
                    attachment_id: request.attachment_id,
                    view_id: request.view_id,
                },
            )),
        })?;
        let focus = SshTerminalInputFocusChangeResponse {
            focus_epoch: response.focus_epoch,
            target: response.target.and_then(|target| match target {
                TerminalInputFocusTarget::Ssh(target) => Some(target),
                TerminalInputFocusTarget::Local(_)
                | TerminalInputFocusTarget::Telnet(_)
                | TerminalInputFocusTarget::Plugin(_) => None,
            }),
            lease: response.lease.and_then(|lease| match lease {
                TerminalInputLease::Ssh(lease) => Some(lease),
                TerminalInputLease::Local(_)
                | TerminalInputLease::Telnet(_)
                | TerminalInputLease::Plugin(_) => None,
            }),
        };
        self.start_shell_heartbeat(&session_id, request.expected_generation.get());
        let details = self.details(&session_id, request.meta.request_id)?;
        self.bump_snapshot();
        self.record_successful_host_connection(&session_id);
        Ok(SshLoginAutomationTakeoverResponse { details, focus })
    }

    fn record_successful_host_connection(&self, session_id: &str) {
        let host_id = self.sessions.get(session_id).and_then(|record| {
            if record.summary.state != SshSessionState::Running {
                return None;
            }
            match &record.summary.target {
                SshSessionTarget::Host { host_id, .. } => Some(host_id.clone()),
                SshSessionTarget::QuickConnect { .. } => None,
            }
        });
        if let Some(host_id) = host_id {
            // Recent history is a secondary non-secret projection. A local database failure must
            // not tear down a successfully authenticated remote shell.
            let _ = self.hosts.record_successful_connection(&host_id);
        }
    }

    fn shell_output(&mut self, session_id: &str, generation: u64, bytes: Vec<u8>) {
        if !self.sessions.get(session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && matches!(
                    record.summary.state,
                    SshSessionState::AutomatingLogin | SshSessionState::Running
                )
        }) {
            return;
        }
        let Some((wire_session_id, channel_id, prompt_input_sequence, prompt_input_epoch)) =
            self.sessions.get(session_id).and_then(|record| {
                Some((
                    record.summary.session_id.clone(),
                    record.summary.channel_id.clone()?,
                    WireSequence::new(record.last_client_seq),
                    record.input_lease.as_ref().map(|lease| lease.input_epoch),
                ))
            })
        else {
            return;
        };
        // Strip only our bounded private OSC frames before they enter the
        // ring, renderer, plugin observer projection, or automation matcher.
        // All unrelated VT bytes remain verbatim.
        let bytes = self.native_terminal.filter_ssh_output(
            &wire_session_id,
            WireSequence::new(generation),
            &channel_id,
            prompt_input_sequence,
            prompt_input_epoch,
            bytes,
        );
        if bytes.is_empty() {
            return;
        }
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        let mut plugin_frames = Vec::new();
        for chunk in bytes.chunks(SSH_TERMINAL_OUTPUT_FRAME_MAX_BYTES) {
            let frame = SshSessionOutputFrame {
                session_id: record.summary.session_id.clone(),
                generation: record.summary.generation,
                channel_id: record.summary.channel_id.clone().expect("running channel"),
                output_seq: WireSequence::new(record.next_output_seq),
                bytes: chunk.to_vec(),
            };
            record.next_output_seq = record.next_output_seq.saturating_add(1);
            record.output_ring_bytes = record.output_ring_bytes.saturating_add(frame.bytes.len());
            record
                .output_ring
                .push_back(SshSessionOutputItem::Frame(frame.clone()));
            if record.summary.state == SshSessionState::Running {
                plugin_frames.push((frame.output_seq, frame.bytes.clone()));
            }
            emit_payload(record, SshSessionEventPayload::OutputFrame { frame });
        }
        trim_output_ring(record);
        let matcher_within_limit = if record.summary.state == SshSessionState::AutomatingLogin
            && let Some(automation) = record.active_login_automation.as_mut()
            && automation.progress.status == SshLoginAutomationStatus::Running
        {
            append_login_automation_matcher(&mut automation.matcher_buffer, &bytes)
        } else {
            true
        };
        self.emit_plugin_observations(session_id, generation, plugin_frames);
        if !matcher_within_limit {
            self.fail_login_automation(
                session_id,
                generation,
                SshLoginAutomationFailureCode::MatchBufferExceeded,
            );
            return;
        }
        if self.sessions.get(session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && record.summary.state == SshSessionState::AutomatingLogin
        }) {
            self.drive_login_automation(session_id, generation);
        }
    }

    fn emit_plugin_observations(
        &mut self,
        session_id: &str,
        generation: u64,
        frames: Vec<(WireSequence, Vec<u8>)>,
    ) {
        if frames.is_empty() {
            return;
        }
        let sessions = &self.sessions;
        self.plugin_observers.retain(|_, observer| {
            let target = &observer.target;
            let valid = target.session_id.as_str() == session_id
                && target.expected_generation.get() == generation
                && sessions.get(session_id).is_some_and(|record| {
                    record.summary.state == SshSessionState::Running
                        && record.summary.generation == target.expected_generation
                        && record.summary.channel_id.as_ref() == Some(&target.channel_id)
                        && record
                            .attachments
                            .get(target.attachment_id.as_str())
                            .is_some_and(|attachment| {
                                attachment.summary.view_id == target.view_id
                                    && attachment.summary.generation == target.expected_generation
                                    && attachment.summary.channel_id.as_ref()
                                        == Some(&target.channel_id)
                            })
                });
            if !valid {
                let _ = observer.sink.try_send(PluginTerminalObservation::Detached);
                return false;
            }
            for (output_seq, bytes) in &frames {
                let text = project_terminal_text(
                    &mut observer.projection_state,
                    &mut observer.utf8_pending,
                    bytes,
                );
                if text.is_empty() {
                    continue;
                }
                if observer
                    .sink
                    .try_send(PluginTerminalObservation::Text {
                        output_seq: *output_seq,
                        text,
                    })
                    .is_err()
                {
                    return false;
                }
            }
            true
        });
    }

    fn shell_closed(&mut self, session_id: &str, generation: u64, reason: SshSessionCloseReason) {
        if !self.generation_matches(session_id, generation) {
            return;
        }
        self.clear_focus_if(|target| {
            matches!(
                target,
                TerminalInputFocusTarget::Ssh(ssh)
                    if ssh.session_id.as_str() == session_id
                        && ssh.expected_generation.get() == generation
            )
        });
        if let Some(record) = self.sessions.get_mut(session_id) {
            record.shell = None;
            record.shell_stop = None;
            record.input_lease = None;
        }
        if let Ok(session_id) = norishell_core_api::SshSessionId::parse(session_id) {
            self.native_terminal
                .clear_ssh_session(&session_id, WireSequence::new(generation));
        }
        let observer_ids = self
            .plugin_observers
            .iter()
            .filter_map(|(observer_id, observer)| {
                (observer.target.session_id.as_str() == session_id
                    && observer.target.expected_generation.get() == generation)
                    .then_some(*observer_id)
            })
            .collect::<Vec<_>>();
        for observer_id in observer_ids {
            self.plugin_observe_detach(observer_id);
        }
        self.transition(session_id, SshSessionState::Closed, Some(reason), None);
    }

    fn shell_cleanup_completed(&mut self, session_id: &str, generation: u64) {
        let Some(record) = self.sessions.get_mut(session_id) else {
            return;
        };
        if record.summary.generation.get() != generation {
            return;
        }
        record.shell = None;
        record.shell_stop = None;
        if matches!(
            record.summary.state,
            SshSessionState::Closed | SshSessionState::Failed
        ) {
            self.live_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(session_id);
        }
    }

    fn connection_failed(
        &mut self,
        session_id: &str,
        generation: u64,
        error: TransportError,
        route_stage: Option<SshSessionRouteStage>,
    ) {
        self.finish_connect(session_id, generation);
        if self.sessions.get(session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && record.summary.state == SshSessionState::Disconnecting
        }) {
            self.shell_closed(session_id, generation, SshSessionCloseReason::UserRequested);
            return;
        }
        if !self.connection_active(session_id, generation) {
            return;
        }
        let failure = failure_from_route(error, route_stage);
        if let Ok(wire_session_id) = norishell_core_api::SshSessionId::parse(session_id) {
            self.native_terminal
                .clear_ssh_session(&wire_session_id, WireSequence::new(generation));
        }
        if let Some(record) = self.sessions.get_mut(session_id) {
            clear_keyboard_interactive_challenge(record, TransportError::AuthenticationRejected);
        }
        self.transition(session_id, SshSessionState::Failed, None, Some(failure));
    }

    fn reap_stale_attachments(&mut self, now_unix_ms: i64) {
        let mut changed = false;
        let stale_focus = match self.focused_target.as_ref() {
            Some(TerminalInputFocusTarget::Ssh(target)) => self
                .sessions
                .get(target.session_id.as_str())
                .is_none_or(|record| {
                    record.input_lease.as_ref().is_none_or(|lease| {
                        lease.expires_at_unix_ms <= now_unix_ms
                            || record
                                .attachments
                                .get(target.attachment_id.as_str())
                                .is_none_or(|attachment| {
                                    now_unix_ms.saturating_sub(attachment.last_seen_at_unix_ms)
                                        >= ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS
                                })
                    })
                }),
            Some(TerminalInputFocusTarget::Local(target)) => {
                self.local_sessions.focus_is_stale(target, now_unix_ms)
            }
            Some(TerminalInputFocusTarget::Telnet(_) | TerminalInputFocusTarget::Plugin(_)) => {
                false
            }
            None => false,
        };
        if stale_focus {
            self.revoke_focused_lease();
            self.focus_epoch = self.focus_epoch.saturating_add(1);
        }
        for record in self.sessions.values_mut() {
            if record
                .input_lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_unix_ms <= now_unix_ms)
            {
                record.input_lease = None;
                emit_payload(
                    record,
                    SshSessionEventPayload::InputLeaseChanged {
                        change: SshSessionInputLeaseChange::Expired,
                        lease: None,
                    },
                );
            }
            let stale = record
                .attachments
                .values()
                .filter(|attachment| {
                    now_unix_ms.saturating_sub(attachment.last_seen_at_unix_ms)
                        >= ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS
                })
                .map(|attachment| attachment.summary.attachment_id.clone())
                .collect::<Vec<_>>();
            for attachment_id in stale {
                changed |= remove_attachment(record, &attachment_id).is_some();
            }
        }
        self.local_sessions.reap_stale_attachments(now_unix_ms);
        if changed {
            self.bump_snapshot();
        }
    }

    fn generation_matches(&self, session_id: &str, generation: u64) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|record| record.summary.generation.get() == generation)
    }

    fn connection_active(&self, session_id: &str, generation: u64) -> bool {
        self.sessions.get(session_id).is_some_and(|record| {
            record.summary.generation.get() == generation
                && !matches!(
                    record.summary.state,
                    SshSessionState::Closed
                        | SshSessionState::Failed
                        | SshSessionState::Disconnecting
                )
        })
    }

    fn transition(
        &mut self,
        session_id: &str,
        state: SshSessionState,
        close_reason: Option<SshSessionCloseReason>,
        failure_reason: Option<SshSessionFailureReason>,
    ) {
        if state != SshSessionState::Running {
            self.clear_focus_if(|target| {
                matches!(
                    target,
                    TerminalInputFocusTarget::Ssh(ssh) if ssh.session_id.as_str() == session_id
                )
            });
            let observer_ids = self
                .plugin_observers
                .iter()
                .filter_map(|(observer_id, observer)| {
                    (observer.target.session_id.as_str() == session_id).then_some(*observer_id)
                })
                .collect::<Vec<_>>();
            for observer_id in observer_ids {
                self.plugin_observe_detach(observer_id);
            }
        }
        let pending_takeover = {
            let Some(record) = self.sessions.get_mut(session_id) else {
                return;
            };
            let pending = if state != SshSessionState::AutomatingLogin {
                clear_login_automation(record)
            } else {
                None
            };
            if matches!(
                state,
                SshSessionState::Disconnecting | SshSessionState::Closed | SshSessionState::Failed
            ) {
                abort_heartbeat_tasks(record);
            }
            if state == SshSessionState::Failed {
                let _ = request_shell_stop(record, ShellStopSignal::Abort);
            }
            transition_record(record, state, close_reason, failure_reason);
            let _ = self
                .plugin_metadata_events
                .send(PluginSessionMetadataEvent::Ssh {
                    session_id: record.summary.session_id.clone(),
                    generation: record.summary.generation,
                    state_revision: record.summary.state_revision,
                    state: record.summary.state,
                });
            pending
        };
        if let Some(pending) = pending_takeover {
            self.reject_pending_login_automation_takeover(pending, true);
        }
        if matches!(state, SshSessionState::Closed | SshSessionState::Failed)
            && self
                .sessions
                .get(session_id)
                .is_some_and(|record| record.shell.is_none())
        {
            self.live_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(session_id);
        }
        self.bump_snapshot();
    }

    fn bump_snapshot(&mut self) {
        self.snapshot_revision = self.snapshot_revision.saturating_add(1);
    }
}

fn project_terminal_text(
    state: &mut TerminalTextProjectionState,
    utf8_pending: &mut Vec<u8>,
    bytes: &[u8],
) -> String {
    const MAX_PROJECTED_FRAME_BYTES: usize = 8 * 1024;
    let mut projected = Vec::with_capacity(bytes.len().min(MAX_PROJECTED_FRAME_BYTES));
    for byte in bytes {
        match *state {
            TerminalTextProjectionState::Ground => match *byte {
                0x1b => *state = TerminalTextProjectionState::Escape,
                b'\n' | b'\r' | b'\t' => projected.push(*byte),
                0x20..=0x7e | 0x80..=0xff => projected.push(*byte),
                _ => {}
            },
            TerminalTextProjectionState::Escape => match *byte {
                b'[' => *state = TerminalTextProjectionState::ControlSequence,
                b']' | b'P' | b'^' | b'_' => {
                    *state = TerminalTextProjectionState::OperatingSystemCommand;
                }
                _ => *state = TerminalTextProjectionState::Ground,
            },
            TerminalTextProjectionState::ControlSequence => {
                if (0x40..=0x7e).contains(byte) {
                    *state = TerminalTextProjectionState::Ground;
                }
            }
            TerminalTextProjectionState::OperatingSystemCommand => match *byte {
                0x07 => *state = TerminalTextProjectionState::Ground,
                0x1b => *state = TerminalTextProjectionState::OperatingSystemCommandEscape,
                _ => {}
            },
            TerminalTextProjectionState::OperatingSystemCommandEscape => {
                *state = if *byte == b'\\' {
                    TerminalTextProjectionState::Ground
                } else {
                    TerminalTextProjectionState::OperatingSystemCommand
                };
            }
        }
        if projected.len() >= MAX_PROJECTED_FRAME_BYTES {
            break;
        }
    }

    if projected.is_empty() {
        return String::new();
    }
    let mut pending = std::mem::take(utf8_pending);
    pending.extend_from_slice(&projected);
    let mut result = String::new();
    let mut remaining = pending.as_slice();
    loop {
        match std::str::from_utf8(remaining) {
            Ok(text) => {
                result.push_str(text);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    result.push_str(
                        std::str::from_utf8(&remaining[..valid])
                            .expect("valid UTF-8 prefix was reported"),
                    );
                }
                match error.error_len() {
                    Some(length) => {
                        result.push(char::REPLACEMENT_CHARACTER);
                        remaining = &remaining[valid.saturating_add(length)..];
                        if remaining.is_empty() {
                            break;
                        }
                    }
                    None => {
                        utf8_pending.extend_from_slice(&remaining[valid..]);
                        break;
                    }
                }
            }
        }
    }
    result
}

impl Drop for Actor {
    fn drop(&mut self) {
        for (_, task) in std::mem::take(&mut self.connect_tasks) {
            task.abort_handle.abort();
        }
        for record in self.sessions.values_mut() {
            abort_heartbeat_tasks(record);
            let _ = request_shell_stop(record, ShellStopSignal::Abort);
            record.shell = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc as std_mpsc,
        },
        time::Duration,
    };

    use norishell_app_persistence::CredentialImportState;
    use norishell_core_api::{
        AuthenticationMethodKind, CredentialKind, IdentityId,
        LOCAL_TERMINAL_OUTPUT_FRAME_MAX_BYTES, LocalAttachAttemptId, LocalOpenAttemptId,
        LocalSessionId, LocalSessionOutputItem, LocalViewId, LoginAutomationSecretStageId,
        OperationId, RequestMeta, SecretRefId, SshAttachAttemptId, SshOpenAttemptId, SshSessionId,
        SshViewId, TransientCredentialPrepareRequest, TransientCredentialRef,
    };
    use norishell_ssh_domain::ssh_sha256_fingerprint;
    use russh::{
        Channel as RusshChannel, ChannelId, Pty,
        keys::{Algorithm, PrivateKey, PublicKeyBase64},
        server::{self, Auth, Msg, Session},
    };
    use serde::de::DeserializeOwned;
    use tauri::{
        WebviewWindow,
        ipc::{CallbackFn, Channel, InvokeBody},
        test::{
            INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
        },
        webview::InvokeRequest,
    };
    use tokio::{
        net::TcpListener,
        sync::mpsc,
        task::{AbortHandle, JoinHandle},
        time::{sleep, timeout},
    };

    use super::*;

    struct TestSession {
        session_id: SshSessionId,
        attachment_id: SshAttachmentId,
        view_id: SshViewId,
    }

    const NETWORK_TEST_USER: &str = "network-acceptance";
    const NETWORK_TEST_PASSWORD: &str = "loopback-only-password";
    const NETWORK_EXIT_INPUT: &[u8] = b"exit-with-status-23\r";
    const NETWORK_EOF_INPUT: &[u8] = b"remote-eof-now\r";
    const NETWORK_DROP_INPUT: &[u8] = b"drop-transport-now\r";

    fn test_algorithm_policy() -> ResolvedAlgorithmPolicy {
        ResolvedAlgorithmPolicy {
            transport: norishell_ssh_transport::AlgorithmPolicy::secure_default(),
            policy_id: norishell_ssh_transport::SECURE_DEFAULT_ALGORITHM_POLICY_ID.to_owned(),
            policy_revision: None,
            catalog_version: norishell_ssh_transport::ALGORITHM_POLICY_CATALOG_VERSION,
        }
    }

    fn test_heartbeat_policy() -> ResolvedHeartbeatPolicy {
        ResolvedHeartbeatPolicy {
            revision: None,
            policy: HeartbeatPolicy::Disabled,
        }
    }

    #[test]
    fn heartbeat_helpers_preserve_exact_shell_bytes_and_bounded_failure_semantics() {
        assert_eq!(
            shell_heartbeat_payload("echo ok", ShellHeartbeatLineEnding::None),
            b"echo ok"
        );
        assert_eq!(
            shell_heartbeat_payload("echo ok", ShellHeartbeatLineEnding::Cr),
            b"echo ok\r"
        );
        assert_eq!(
            shell_heartbeat_payload("echo ok", ShellHeartbeatLineEnding::Lf),
            b"echo ok\n"
        );
        assert_eq!(
            shell_heartbeat_payload("echo ok", ShellHeartbeatLineEnding::Crlf),
            b"echo ok\r\n"
        );
        assert_eq!(
            next_transport_heartbeat_failures(TransportHeartbeatAttempt::Acknowledged, 2, 3),
            0
        );
        assert_eq!(
            next_transport_heartbeat_failures(TransportHeartbeatAttempt::TimedOut, 2, 4),
            3
        );
        assert_eq!(
            next_transport_heartbeat_failures(TransportHeartbeatAttempt::Closed, 0, 4),
            4
        );
    }

    #[test]
    fn shell_heartbeat_writer_preflight_rejects_late_or_user_raced_writes() {
        let activity = AtomicU64::new(7);
        let now = tokio::time::Instant::now();
        assert_eq!(
            shell_heartbeat_preflight(7, &activity, now + Duration::from_secs(1), now),
            Ok(())
        );
        activity.store(8, Ordering::Release);
        assert_eq!(
            shell_heartbeat_preflight(7, &activity, now + Duration::from_secs(1), now),
            Err(ShellHeartbeatWriteOutcome::UserActive)
        );
        assert_eq!(
            shell_heartbeat_preflight(8, &activity, now, now),
            Err(ShellHeartbeatWriteOutcome::WriterBusy)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn heartbeat_task_drop_aborts_the_owned_timer() {
        struct NotifyOnDrop(Option<oneshot::Sender<()>>);
        impl Drop for NotifyOnDrop {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }

        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let handle = tauri::async_runtime::spawn(async move {
            let _guard = NotifyOnDrop(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx.await.expect("timer task starts");
        drop(HeartbeatTask::new(handle));
        timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("timer task is aborted")
            .expect("drop notification");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ssh_transitions_publish_secret_free_plugin_metadata() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Running, 3, 7, None);
        let mut metadata = actor.plugin_metadata_events.subscribe();

        actor.transition(
            session.session_id.as_str(),
            SshSessionState::Disconnecting,
            None,
            None,
        );

        assert_eq!(
            metadata.recv().await.expect("metadata event"),
            PluginSessionMetadataEvent::Ssh {
                session_id: session.session_id,
                generation: WireSequence::new(3),
                state_revision: WireSequence::new(8),
                state: SshSessionState::Disconnecting,
            }
        );
    }

    #[test]
    fn stopping_heartbeat_invalidates_queued_shell_writes_and_uses_out_of_band_cancel() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let (shell_tx, mut shell_rx) = mpsc::channel(1);
        let (completion, _completed) = oneshot::channel();
        shell_tx
            .try_send(ShellCommand::Resize {
                size: PtySize::new(80, 24, 0, 0).expect("valid size"),
                deadline: tokio::time::Instant::now() + Duration::from_secs(1),
                completion,
            })
            .expect("fill shell mailbox");
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 2, 7, Some(shell_tx));
        let (stop_tx, mut stop_rx) = watch::channel(None);
        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session");
        record.shell_stop = Some(stop_tx);
        let old_activity_epoch = record.input_activity_epoch.load(Ordering::Acquire);

        abort_heartbeat_tasks(record);
        request_shell_stop(record, ShellStopSignal::Abort)
            .expect("out-of-band stop is accepted even while mailbox is full");

        assert_eq!(
            record.input_activity_epoch.load(Ordering::Acquire),
            old_activity_epoch + 1
        );
        assert!(matches!(
            stop_rx.borrow_and_update().as_ref(),
            Some(ShellStopSignal::Abort)
        ));
        assert!(matches!(
            shell_rx.try_recv(),
            Ok(ShellCommand::Resize { .. })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shell_heartbeat_tick_is_idle_gated_and_acknowledged_by_the_single_writer() {
        let (_directory, mut actor, _live_sessions, mut rx) = actor_fixture();
        let (shell_tx, mut shell_rx) = mpsc::channel(SHELL_MAILBOX_CAPACITY);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 2, 7, Some(shell_tx));
        let policy = ResolvedHeartbeatPolicy {
            revision: Some(WireSequence::new(9)),
            policy: HeartbeatPolicy::ShellHeartbeat {
                payload_text: "echo alive".to_owned(),
                line_ending: ShellHeartbeatLineEnding::Crlf,
                interval_seconds: 30,
                user_idle_seconds: 5,
            },
        };
        {
            let record = actor
                .sessions
                .get_mut(session.session_id.as_str())
                .expect("test session");
            record.heartbeat_policy = policy.clone();
            record.heartbeat = heartbeat_status_from_policy(&policy);
            record.last_user_input_at = Some(tokio::time::Instant::now() - Duration::from_secs(6));
        }

        actor.shell_heartbeat_tick(
            session.session_id.as_str(),
            2,
            policy.revision,
            unix_time_ms() + 30_000,
        );
        let completion = match shell_rx.recv().await.expect("heartbeat writer command") {
            ShellCommand::HeartbeatInput {
                bytes,
                expected_input_activity_epoch,
                completion,
                ..
            } => {
                assert_eq!(bytes, b"echo alive\r\n");
                assert_eq!(expected_input_activity_epoch, 0);
                completion
            }
            _ => panic!("expected heartbeat input"),
        };
        completion
            .send(ShellHeartbeatWriteOutcome::Sent)
            .expect("writer completion accepted");
        let message = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("completion message")
            .expect("actor mailbox remains open");
        actor.handle(message);
        assert!(
            actor.sessions[session.session_id.as_str()]
                .heartbeat
                .shell
                .as_ref()
                .and_then(|status| status.last_sent_at_unix_ms)
                .is_some()
        );

        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session");
        record.last_user_input_at = Some(tokio::time::Instant::now());
        actor.shell_heartbeat_tick(
            session.session_id.as_str(),
            2,
            policy.revision,
            unix_time_ms() + 30_000,
        );
        assert!(shell_rx.try_recv().is_err());
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .heartbeat
                .shell
                .as_ref()
                .and_then(|status| status.skip_reason),
            Some(SshShellHeartbeatSkipReason::UserActive)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn login_automation_finish_starts_shell_heartbeat_before_details_are_read() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::AutomatingLogin, 2, 7, None);
        let policy = ResolvedHeartbeatPolicy {
            revision: Some(WireSequence::new(9)),
            policy: HeartbeatPolicy::ShellHeartbeat {
                payload_text: "echo alive".to_owned(),
                line_ending: ShellHeartbeatLineEnding::Cr,
                interval_seconds: 30,
                user_idle_seconds: 5,
            },
        };
        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session");
        record.heartbeat_policy = policy.clone();
        record.heartbeat = heartbeat_status_from_policy(&policy);

        actor.finish_login_automation(session.session_id.as_str(), 2);

        let details = actor
            .details(session.session_id.as_str(), RequestId::new())
            .expect("post-automation details");
        assert_eq!(details.session.state, SshSessionState::Running);
        assert!(
            details
                .heartbeat
                .shell
                .as_ref()
                .and_then(|status| status.next_due_at_unix_ms)
                .is_some(),
            "the projection includes the newly scheduled full interval"
        );
        assert!(
            actor.sessions[session.session_id.as_str()]
                .shell_heartbeat_task
                .is_some()
        );
    }

    #[test]
    fn transport_heartbeat_updates_are_fenced_and_exact_route_failure_is_isolated() {
        let (_directory, mut actor, live_sessions, _rx) = actor_fixture();
        let (shell_tx, mut shell_rx) = mpsc::channel(SHELL_MAILBOX_CAPACITY);
        let failing =
            insert_test_session(&mut actor, SshSessionState::Running, 3, 4, Some(shell_tx));
        let healthy = insert_test_session(&mut actor, SshSessionState::Running, 1, 1, None);
        live_sessions
            .lock()
            .expect("live session lock")
            .insert(failing.session_id.as_str().to_owned());
        let route_stage = SshSessionRouteStage::JumpHost {
            hop_index: 1,
            host_id: norishell_core_api::HostId::new(),
            endpoint: SshSessionEndpoint {
                address: "jump.example.test".to_owned(),
                port: 22,
                username: Some("ops".to_owned()),
            },
        };
        let policy = ResolvedHeartbeatPolicy {
            revision: Some(WireSequence::new(6)),
            policy: HeartbeatPolicy::TransportKeepalive {
                interval_seconds: 30,
                reply_timeout_seconds: 10,
                failure_threshold: 3,
            },
        };
        {
            let record = actor
                .sessions
                .get_mut(failing.session_id.as_str())
                .expect("test session");
            record.heartbeat_policy = policy.clone();
            record.heartbeat = heartbeat_status_from_policy(&policy);
            record
                .heartbeat
                .transports
                .push(SshTransportHeartbeatStatus {
                    route_stage: route_stage.clone(),
                    next_due_at_unix_ms: Some(100),
                    last_sent_at_unix_ms: None,
                    last_ack_at_unix_ms: None,
                    consecutive_failures: 0,
                });
        }
        actor.transport_heartbeat_observed(
            failing.session_id.as_str(),
            2,
            policy.revision,
            route_stage.clone(),
            200,
            None,
            1,
            Some(300),
        );
        assert_eq!(
            actor.sessions[failing.session_id.as_str()]
                .heartbeat
                .transports[0]
                .consecutive_failures,
            0
        );

        actor.transport_heartbeat_failed(
            failing.session_id.as_str(),
            3,
            policy.revision,
            route_stage.clone(),
        );
        let failed = &actor.sessions[failing.session_id.as_str()].summary;
        assert_eq!(failed.state, SshSessionState::Failed);
        assert_eq!(
            failed
                .failure_reason
                .as_ref()
                .and_then(|failure| failure.route_stage.as_ref()),
            Some(&route_stage)
        );
        assert!(matches!(shell_rx.try_recv(), Ok(ShellCommand::Abort)));
        assert!(
            live_sessions
                .lock()
                .expect("live session lock")
                .contains(failing.session_id.as_str()),
            "exit blocker stays live until remote shell cleanup completes"
        );
        actor.shell_cleanup_completed(failing.session_id.as_str(), 3);
        assert!(
            !live_sessions
                .lock()
                .expect("live session lock")
                .contains(failing.session_id.as_str())
        );
        assert_eq!(
            actor.sessions[healthy.session_id.as_str()].summary.state,
            SshSessionState::Running
        );
    }

    #[test]
    fn disconnecting_session_rejects_queued_transport_heartbeat_messages() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Disconnecting, 3, 4, None);
        let route_stage = SshSessionRouteStage::Target;
        let policy = ResolvedHeartbeatPolicy {
            revision: Some(WireSequence::new(6)),
            policy: HeartbeatPolicy::TransportKeepalive {
                interval_seconds: 30,
                reply_timeout_seconds: 10,
                failure_threshold: 3,
            },
        };
        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session");
        record.heartbeat_policy = policy.clone();
        record.heartbeat = heartbeat_status_from_policy(&policy);
        record
            .heartbeat
            .transports
            .push(SshTransportHeartbeatStatus {
                route_stage: route_stage.clone(),
                next_due_at_unix_ms: None,
                last_sent_at_unix_ms: None,
                last_ack_at_unix_ms: None,
                consecutive_failures: 0,
            });

        actor.transport_heartbeat_scheduled(
            session.session_id.as_str(),
            3,
            policy.revision,
            route_stage.clone(),
            100,
        );
        actor.transport_heartbeat_observed(
            session.session_id.as_str(),
            3,
            policy.revision,
            route_stage.clone(),
            100,
            None,
            2,
            Some(200),
        );
        actor.transport_heartbeat_failed(
            session.session_id.as_str(),
            3,
            policy.revision,
            route_stage,
        );

        let record = &actor.sessions[session.session_id.as_str()];
        assert_eq!(record.summary.state, SshSessionState::Disconnecting);
        assert_eq!(record.heartbeat.transports[0].next_due_at_unix_ms, None);
        assert_eq!(record.heartbeat.transports[0].last_sent_at_unix_ms, None);
        assert_eq!(record.heartbeat.transports[0].consecutive_failures, 0);
    }

    #[test]
    fn login_automation_matcher_preserves_cross_frame_and_trailing_bytes_and_fails_on_overflow() {
        let mut matcher = Vec::new();
        assert!(append_login_automation_matcher(&mut matcher, b"Pass"));
        assert_eq!(find_literal_bytes(&matcher, b"Password:"), None);
        assert!(append_login_automation_matcher(
            &mut matcher,
            b"word:trailing-output"
        ));
        let offset = find_literal_bytes(&matcher, b"Password:").expect("cross-frame match");
        matcher.drain(..offset + b"Password:".len());
        assert_eq!(matcher, b"trailing-output");

        let mut full = vec![b'x'; LOGIN_AUTOMATION_MATCH_BUFFER_MAX_BYTES];
        assert!(!append_login_automation_matcher(&mut full, b"y"));
        assert_eq!(full.len(), LOGIN_AUTOMATION_MATCH_BUFFER_MAX_BYTES);
        assert_eq!(&*login_automation_payload(b"secret", true), b"secret\r");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn login_automation_takeover_replays_exactly_without_a_second_focus_transition() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Running, 2, 7, None);
        let steps = vec![LoginAutomationStepInput::Expect {
            literal_text: "Password:".to_owned(),
            timeout_seconds: 10,
        }];
        let now = unix_time_ms();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session");
        record.summary.state = SshSessionState::AutomatingLogin;
        let policy = ResolvedHeartbeatPolicy {
            revision: Some(WireSequence::new(9)),
            policy: HeartbeatPolicy::ShellHeartbeat {
                payload_text: "echo alive".to_owned(),
                line_ending: ShellHeartbeatLineEnding::Cr,
                interval_seconds: 30,
                user_idle_seconds: 5,
            },
        };
        record.heartbeat_policy = policy.clone();
        record.heartbeat = heartbeat_status_from_policy(&policy);
        record.active_login_automation = Some(ActiveLoginAutomation {
            policy_revision: WireSequence::new(4),
            steps: steps.clone(),
            current_step_index: 0,
            matcher_buffer: Vec::new(),
            timeout_scheduled_for: None,
            total_deadline_unix_ms: now + 10_000,
            total_deadline: deadline,
            step_deadline: deadline,
            owner_attachment_id: session.attachment_id.clone(),
            owner_view_id: session.view_id.clone(),
            write_in_flight: None,
            pending_takeover: None,
            progress: login_automation_progress(
                WireSequence::new(4),
                &steps,
                0,
                now,
                now + 10_000,
                SshLoginAutomationStatus::Running,
                None,
            ),
        });
        let channel_id = record.summary.channel_id.clone().expect("test channel");
        let operation_id = OperationId::new();
        let request = SshLoginAutomationTakeoverRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: operation_id.clone(),
            idempotency_key: "takeover-exact-replay".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            expected_state_revision: WireSequence::new(7),
            expected_focus_epoch: WireSequence::new(0),
            channel_id,
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
        };
        let (first_reply, first_response) = oneshot::channel();
        actor.login_automation_takeover(request.clone(), first_reply);
        let first = first_response
            .await
            .expect("first reply")
            .expect("first takeover");
        assert_eq!(first.details.session.state, SshSessionState::Running);
        assert!(
            first
                .details
                .heartbeat
                .shell
                .as_ref()
                .and_then(|status| status.next_due_at_unix_ms)
                .is_some(),
            "takeover response cannot overwrite the scheduled heartbeat with stale details"
        );
        assert_eq!(first.focus.focus_epoch, WireSequence::new(1));

        let (replay_reply, replay_response) = oneshot::channel();
        actor.login_automation_takeover(request, replay_reply);
        let replay = replay_response
            .await
            .expect("replay reply")
            .expect("exact takeover replay");
        assert_eq!(replay.focus.focus_epoch, first.focus.focus_epoch);
        assert_eq!(actor.focus_epoch, 1);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .summary
                .state_revision,
            WireSequence::new(8)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn login_automation_advances_only_after_the_unique_shell_writer_acknowledges() {
        let (_directory, mut actor, _live_sessions, mut actor_rx) = actor_fixture();
        let (shell_tx, mut shell_rx) = mpsc::channel(2);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 1, 3, Some(shell_tx));
        let steps = vec![
            LoginAutomationStepInput::SendText {
                text: "enable".to_owned(),
                append_enter: true,
                timeout_seconds: 10,
            },
            LoginAutomationStepInput::Expect {
                literal_text: "Password:".to_owned(),
                timeout_seconds: 10,
            },
        ];
        let now = unix_time_ms();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session");
        record.summary.state = SshSessionState::AutomatingLogin;
        record.active_login_automation = Some(ActiveLoginAutomation {
            policy_revision: WireSequence::new(5),
            steps: steps.clone(),
            current_step_index: 0,
            matcher_buffer: Vec::new(),
            timeout_scheduled_for: None,
            total_deadline_unix_ms: now + 10_000,
            total_deadline: deadline,
            step_deadline: deadline,
            owner_attachment_id: session.attachment_id.clone(),
            owner_view_id: session.view_id.clone(),
            write_in_flight: None,
            pending_takeover: None,
            progress: login_automation_progress(
                WireSequence::new(5),
                &steps,
                0,
                now,
                now + 10_000,
                SshLoginAutomationStatus::Running,
                None,
            ),
        });
        let channel_id = record.summary.channel_id.clone().expect("test channel");

        actor.drive_login_automation(session.session_id.as_str(), 1);
        let command = shell_rx.recv().await.expect("automation writer command");
        let completion = match command {
            ShellCommand::AutomationInput {
                bytes, completion, ..
            } => {
                assert_eq!(&*bytes, b"enable\r");
                completion
            }
            _ => panic!("expected automation input"),
        };
        let automation = actor.sessions[session.session_id.as_str()]
            .active_login_automation
            .as_ref()
            .expect("active automation");
        assert_eq!(automation.current_step_index, 0);
        assert_eq!(automation.write_in_flight, Some(0));

        let takeover_request = SshLoginAutomationTakeoverRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "takeover-while-write-in-flight".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(1),
            expected_state_revision: WireSequence::new(3),
            expected_focus_epoch: WireSequence::new(0),
            channel_id,
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
        };
        let (takeover_reply, takeover_response) = oneshot::channel();
        actor.login_automation_takeover(takeover_request.clone(), takeover_reply);
        actor.focus_epoch = 1;

        completion.send(true).expect("writer ack");
        let completion_message = timeout(Duration::from_secs(1), actor_rx.recv())
            .await
            .expect("completion message timeout")
            .expect("completion message");
        actor.handle(completion_message);
        assert!(
            takeover_response
                .await
                .expect("takeover terminal response")
                .is_err()
        );
        let automation = actor.sessions[session.session_id.as_str()]
            .active_login_automation
            .as_ref()
            .expect("automation waits for next Expect");
        assert_eq!(automation.current_step_index, 1);
        assert_eq!(automation.write_in_flight, None);
        assert!(matches!(
            shell_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let (replay_reply, replay_response) = oneshot::channel();
        actor.login_automation_takeover(takeover_request, replay_reply);
        assert!(
            replay_response
                .await
                .expect("takeover error replay")
                .is_err()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn login_automation_timeouts_and_stale_owner_fences_fail_closed() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Running, 4, 6, None);
        let steps = vec![LoginAutomationStepInput::Expect {
            literal_text: "ready>".to_owned(),
            timeout_seconds: 10,
        }];
        let now = tokio::time::Instant::now();
        let past = now
            .checked_sub(Duration::from_secs(1))
            .expect("past instant");

        activate_test_login_automation(
            &mut actor,
            &session,
            steps.clone(),
            now + Duration::from_secs(10),
            past,
        );
        actor.login_automation_step_timeout(session.session_id.as_str(), 3, 6, 0);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .active_login_automation
                .as_ref()
                .expect("stale generation is ignored")
                .progress
                .status,
            SshLoginAutomationStatus::Running
        );
        actor.login_automation_step_timeout(session.session_id.as_str(), 4, 6, 0);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .active_login_automation
                .as_ref()
                .expect("failed automation remains visible")
                .progress
                .failure_code,
            Some(SshLoginAutomationFailureCode::StepTimeout)
        );

        activate_test_login_automation(&mut actor, &session, steps.clone(), past, past);
        actor.login_automation_step_timeout(session.session_id.as_str(), 4, 6, 0);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .active_login_automation
                .as_ref()
                .expect("total timeout remains visible")
                .progress
                .failure_code,
            Some(SshLoginAutomationFailureCode::TotalTimeout)
        );

        activate_test_login_automation(
            &mut actor,
            &session,
            steps,
            now + Duration::from_secs(10),
            now + Duration::from_secs(10),
        );
        actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session")
            .attachments
            .remove(session.attachment_id.as_str());
        actor.login_automation_step_timeout(session.session_id.as_str(), 4, 6, 0);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .active_login_automation
                .as_ref()
                .expect("lost owner remains visible")
                .progress
                .failure_code,
            Some(SshLoginAutomationFailureCode::AttachmentUnavailable)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn login_automation_writer_failure_poison_completes_and_replays_pending_takeover() {
        let (_directory, mut actor, _live_sessions, mut actor_rx) = actor_fixture();
        let (shell_tx, mut shell_rx) = mpsc::channel(2);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 2, 8, Some(shell_tx));
        activate_test_login_automation(
            &mut actor,
            &session,
            vec![LoginAutomationStepInput::SendText {
                text: "configure".to_owned(),
                append_enter: true,
                timeout_seconds: 10,
            }],
            tokio::time::Instant::now() + Duration::from_secs(10),
            tokio::time::Instant::now() + Duration::from_secs(10),
        );
        let channel_id = actor.sessions[session.session_id.as_str()]
            .summary
            .channel_id
            .clone()
            .expect("test channel");
        actor.drive_login_automation(session.session_id.as_str(), 2);
        let completion = match shell_rx.recv().await.expect("automation writer command") {
            ShellCommand::AutomationInput { completion, .. } => completion,
            _ => panic!("expected automation input"),
        };
        let takeover_request = SshLoginAutomationTakeoverRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "takeover-before-writer-failure".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            expected_state_revision: WireSequence::new(8),
            expected_focus_epoch: WireSequence::new(0),
            channel_id,
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
        };
        let (takeover_reply, takeover_response) = oneshot::channel();
        actor.login_automation_takeover(takeover_request.clone(), takeover_reply);
        completion.send(false).expect("writer failure ack");
        actor.handle(
            timeout(Duration::from_secs(1), actor_rx.recv())
                .await
                .expect("writer completion timeout")
                .expect("writer completion message"),
        );
        assert!(
            takeover_response
                .await
                .expect("pending takeover terminal response")
                .is_err()
        );
        actor.handle(Message::ConnectionFailed {
            session_id: session.session_id.as_str().to_owned(),
            generation: 2,
            error: TransportError::ConnectionLost,
            route_stage: None,
        });
        assert_eq!(
            actor.sessions[session.session_id.as_str()].summary.state,
            SshSessionState::Failed
        );
        assert!(
            actor.sessions[session.session_id.as_str()]
                .active_login_automation
                .is_none()
        );

        let (replay_reply, replay_response) = oneshot::channel();
        actor.login_automation_takeover(takeover_request, replay_reply);
        assert!(
            replay_response
                .await
                .expect("failed takeover replay")
                .is_err()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn login_automation_send_secret_is_borrowed_only_by_the_writer_and_redacted_publicly() {
        let (_directory, mut actor, _live_sessions, mut actor_rx) = actor_fixture();
        actor
            .vault
            .create_for_tests(b"test-vault-password")
            .expect("test vault");
        let secret_ref_id = SecretRefId::new();
        let secret_value = b"automation-secret-value";
        actor
            .vault
            .insert_secrets(&[crate::vault_service::VaultSecretInsert {
                secret_ref_id: secret_ref_id.clone(),
                kind: SecretKind::LoginAutomation,
                value: Zeroizing::new(secret_value.to_vec()),
            }])
            .expect("login automation secret");
        let (shell_tx, mut shell_rx) = mpsc::channel(2);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 1, 2, Some(shell_tx));
        activate_test_login_automation(
            &mut actor,
            &session,
            vec![LoginAutomationStepInput::SendSecret {
                secret_ref_id: LoginAutomationSecretStageId::parse(secret_ref_id.as_str())
                    .expect("stage id from secret ref"),
                secret_label: "Privilege password".to_owned(),
                append_enter: true,
                timeout_seconds: 10,
            }],
            tokio::time::Instant::now() + Duration::from_secs(10),
            tokio::time::Instant::now() + Duration::from_secs(10),
        );
        let public_details =
            serde_json::to_string(&details_from(&actor.sessions[session.session_id.as_str()]))
                .expect("serialize public details");
        assert!(!public_details.contains("automation-secret-value"));
        assert!(!public_details.contains(secret_ref_id.as_str()));

        actor.drive_login_automation(session.session_id.as_str(), 1);
        let completion = match shell_rx.recv().await.expect("secret writer command") {
            ShellCommand::AutomationInput {
                bytes, completion, ..
            } => {
                assert_eq!(&*bytes, b"automation-secret-value\r");
                completion
            }
            _ => panic!("expected secret automation input"),
        };
        completion.send(true).expect("secret writer ack");
        actor.handle(
            timeout(Duration::from_secs(1), actor_rx.recv())
                .await
                .expect("secret completion timeout")
                .expect("secret completion message"),
        );
        assert_eq!(
            actor.sessions[session.session_id.as_str()].summary.state,
            SshSessionState::Running
        );
        assert!(
            actor.sessions[session.session_id.as_str()]
                .active_login_automation
                .is_none()
        );
        assert_eq!(
            actor
                .vault
                .read_secret(&secret_ref_id, SecretKind::LoginAutomation)
                .expect("vault retains configured secret")
                .expose(),
            secret_value
        );
    }

    #[test]
    fn jump_transport_failure_keeps_the_exact_safe_route_stage() {
        let route_stage = SshSessionRouteStage::JumpHost {
            hop_index: 1,
            host_id: norishell_core_api::HostId::new(),
            endpoint: SshSessionEndpoint {
                address: "jump.example".to_owned(),
                port: 2222,
                username: Some("deploy".to_owned()),
            },
        };
        let reason = failure_from_route(
            TransportError::Jump(norishell_ssh_transport::JumpTransportError {
                stage: norishell_ssh_transport::JumpStage::DirectTcpipOpen,
                kind: norishell_ssh_transport::JumpFailureKind::Rejected,
            }),
            Some(route_stage.clone()),
        );
        assert_eq!(reason.code, SshSessionFailureCode::JumpChannelFailed);
        assert_eq!(reason.stage, SshSessionFailureStage::RouteIngress);
        assert_eq!(reason.route_stage, Some(route_stage));
        assert_eq!(reason.retry_strategy, SshSessionRetryStrategy::RetryOpen);
        assert_eq!(reason.message_key, "errors.sshSession.jumpChannelFailed");
        assert!(!format!("{reason:?}").contains("password"));
    }

    #[test]
    fn algorithm_failure_keeps_category_and_bounded_candidate_summary() {
        let reason = failure_from_route(
            TransportError::AlgorithmNegotiationFailed {
                category: norishell_ssh_transport::AlgorithmCategory::HostKey,
                client_candidates: vec!["ssh-ed25519".to_owned()],
                server_candidates: vec!["ssh-rsa".to_owned()],
            },
            Some(SshSessionRouteStage::Target),
        );
        assert_eq!(
            reason.algorithm_negotiation,
            Some(SshAlgorithmNegotiationFailure {
                category: WireAlgorithmCategory::HostKey,
                client_candidates: vec!["ssh-ed25519".to_owned()],
                server_candidates: vec!["ssh-rsa".to_owned()],
            })
        );
        assert_eq!(reason.route_stage, Some(SshSessionRouteStage::Target));
        assert_eq!(reason.retry_strategy, SshSessionRetryStrategy::Never);
    }

    #[derive(Default)]
    struct NetworkServerEvidence {
        auth_attempts: AtomicUsize,
        pty_requests: Mutex<Vec<(String, u32, u32)>>,
        resizes: Mutex<Vec<(u32, u32)>>,
        inputs: Mutex<Vec<Vec<u8>>>,
    }

    #[derive(Clone)]
    struct NetworkServerHandler {
        evidence: Arc<NetworkServerEvidence>,
    }

    impl server::Handler for NetworkServerHandler {
        type Error = russh::Error;

        async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
            self.evidence.auth_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(
                if user == NETWORK_TEST_USER && password == NETWORK_TEST_PASSWORD {
                    Auth::Accept
                } else {
                    Auth::reject()
                },
            )
        }

        async fn channel_open_session(
            &mut self,
            _channel: RusshChannel<Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            term: &str,
            columns: u32,
            rows: u32,
            _pixel_width: u32,
            _pixel_height: u32,
            _modes: &[(Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((term.to_owned(), columns, rows));
            session.channel_success(channel)?;
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            session.data(
                channel,
                b"\x1b[36mactor-ready:\xe5\xae\x89\xe5\x85\xa8\x1b[0m\r\n".as_slice(),
            )?;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            _command: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            session.data(channel, b"child-command-ready\r\n".as_slice())?;
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .inputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(data.to_vec());
            if data == NETWORK_EXIT_INPUT {
                session.exit_status_request(channel, 23)?;
                session.eof(channel)?;
                session.close(channel)?;
            } else if data == NETWORK_EOF_INPUT {
                session.eof(channel)?;
                session.close(channel)?;
            } else if data == NETWORK_DROP_INPUT {
                session.disconnect(
                    russh::Disconnect::ByApplication,
                    "isolated network loss",
                    "",
                )?;
            } else {
                session.data(channel, data.to_vec())?;
            }
            Ok(())
        }

        async fn window_change_request(
            &mut self,
            channel: ChannelId,
            columns: u32,
            rows: u32,
            _pixel_width: u32,
            _pixel_height: u32,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            self.evidence
                .resizes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((columns, rows));
            session.channel_success(channel)?;
            Ok(())
        }
    }

    struct NetworkTestServer {
        port: u16,
        host_key_blob: Vec<u8>,
        evidence: Arc<NetworkServerEvidence>,
        accept_task: JoinHandle<()>,
        connection_tasks: Arc<Mutex<Vec<AbortHandle>>>,
    }

    impl NetworkTestServer {
        async fn start() -> Self {
            let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
                .expect("generate isolated actor host key");
            let host_key_blob = host_key.public_key().public_key_bytes();
            let config = Arc::new(server::Config {
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                keys: vec![host_key],
                ..server::Config::default()
            });
            let evidence = Arc::new(NetworkServerEvidence::default());
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind isolated actor SSH server");
            let port = listener
                .local_addr()
                .expect("isolated server address")
                .port();
            let server_evidence = Arc::clone(&evidence);
            let connection_tasks = Arc::new(Mutex::new(Vec::new()));
            let tracked_connections = Arc::clone(&connection_tasks);
            let accept_task = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let config = Arc::clone(&config);
                    let handler = NetworkServerHandler {
                        evidence: Arc::clone(&server_evidence),
                    };
                    let task = tokio::spawn(async move {
                        if let Ok(session) = server::run_stream(config, stream, handler).await {
                            let _ = session.await;
                        }
                    });
                    tracked_connections
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(task.abort_handle());
                }
            });
            Self {
                port,
                host_key_blob,
                evidence,
                accept_task,
                connection_tasks,
            }
        }

        fn abort_connections(&self) {
            for task in self
                .connection_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain(..)
            {
                task.abort();
            }
        }
    }

    impl Drop for NetworkTestServer {
        fn drop(&mut self) {
            self.accept_task.abort();
            self.abort_connections();
        }
    }

    fn actor_fixture() -> (
        tempfile::TempDir,
        Actor,
        Arc<Mutex<BTreeSet<String>>>,
        mpsc::Receiver<Message>,
    ) {
        let directory = tempfile::tempdir().expect("temporary application data");
        let hosts = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        let native_terminal = NativeTerminalService::start(directory.path(), vault.clone());
        let transient_credentials = TransientCredentialService::default();
        let (tx, rx) = mpsc::channel(ACTOR_MAILBOX_CAPACITY);
        let live_sessions = Arc::new(Mutex::new(BTreeSet::new()));
        let live_local_sessions = Arc::new(Mutex::new(BTreeSet::new()));
        let actor = Actor::new(
            tx,
            hosts,
            vault,
            transient_credentials,
            crate::ssh_agent_service::SshAgentService::default(),
            native_terminal,
            live_sessions.clone(),
            live_local_sessions,
            broadcast::channel(64).0,
        );
        (directory, actor, live_sessions, rx)
    }

    fn network_service_fixture() -> (
        tempfile::TempDir,
        HostService,
        TransientCredentialService,
        SshSessionService,
    ) {
        let directory = tempfile::tempdir().expect("temporary network acceptance data");
        let hosts = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        let transient_credentials = TransientCredentialService::default();
        let service = SshSessionService::start(hosts.clone(), vault, transient_credentials.clone());
        (directory, hosts, transient_credentials, service)
    }

    fn prepare_network_password(
        transient_credentials: &TransientCredentialService,
        password: &str,
        key: &str,
    ) -> CredentialRefId {
        transient_credentials
            .prepare(network_password_prepare_request(password, key))
            .expect("prepare isolated network password")
            .credential_ref_id
    }

    fn network_password_prepare_request(
        password: &str,
        key: &str,
    ) -> TransientCredentialPrepareRequest {
        TransientCredentialPrepareRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: key.to_owned(),
            kind: CredentialKind::Password,
            secret: password.to_owned(),
            passphrase: None,
        }
    }

    fn network_open_request(
        port: u16,
        credential_ref_id: CredentialRefId,
        key: &str,
    ) -> SshSessionOpenRequest {
        SshSessionOpenRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: key.to_owned(),
            open_attempt_id: SshOpenAttemptId::new(),
            attach_attempt_id: SshAttachAttemptId::new(),
            target: SshSessionTarget::QuickConnect {
                endpoint: SshSessionEndpoint {
                    address: "127.0.0.1".to_owned(),
                    port,
                    username: Some(NETWORK_TEST_USER.to_owned()),
                },
            },
            credential_ref_id: Some(credential_ref_id),
            plugin_authorization_token: None,
            view_id: SshViewId::new(),
            rows: 24,
            cols: 80,
        }
    }

    async fn open_network_session(
        service: &SshSessionService,
        port: u16,
        credential_ref_id: CredentialRefId,
        key: &str,
    ) -> SshSessionOpenResponse {
        let request = network_open_request(port, credential_ref_id, key);
        let request_id = request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Open {
                    terminal_startup: None,
                    request,
                    events: Channel::<SshSessionEvent>::new(|_| Ok(())),
                    reply,
                },
                request_id,
            )
            .await
            .expect("open isolated network session")
    }

    async fn network_session_details(
        service: &SshSessionService,
        session_id: &SshSessionId,
    ) -> SshSessionDetails {
        let request_id = RequestId::new();
        let session_id_text = session_id.as_str().to_owned();
        service
            .request(
                |reply| Message::Get {
                    session_id: session_id_text,
                    request_id: request_id.clone(),
                    reply,
                },
                request_id.clone(),
            )
            .await
            .expect("get isolated network session")
    }

    async fn wait_for_network_state(
        service: &SshSessionService,
        session_id: &SshSessionId,
        expected: SshSessionState,
    ) -> SshSessionDetails {
        timeout(Duration::from_secs(10), async {
            loop {
                let details = network_session_details(service, session_id).await;
                if details.session.state == expected {
                    return details;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("session did not reach {expected:?}"))
    }

    async fn local_session_details(
        service: &SshSessionService,
        session_id: &LocalSessionId,
    ) -> LocalSessionDetails {
        let request = LocalSessionGetRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: session_id.clone(),
        };
        let request_id = request.meta.request_id.clone();
        service
            .request(|reply| Message::LocalGet { request, reply }, request_id)
            .await
            .expect("get local session")
    }

    async fn wait_for_local_state(
        service: &SshSessionService,
        session_id: &LocalSessionId,
        expected: norishell_core_api::LocalSessionState,
    ) -> LocalSessionDetails {
        timeout(Duration::from_secs(10), async {
            loop {
                let details = local_session_details(service, session_id).await;
                if details.session.state == expected {
                    return details;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("local session did not reach {expected:?}"))
    }

    async fn open_running_local_session(
        service: &SshSessionService,
        key: &str,
    ) -> LocalSessionDetails {
        let request = LocalSessionOpenRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: key.to_owned(),
            open_attempt_id: LocalOpenAttemptId::new(),
            attach_attempt_id: LocalAttachAttemptId::new(),
            view_id: LocalViewId::new(),
            rows: 24,
            cols: 80,
        };
        let request_id = request.meta.request_id.clone();
        let opened = service
            .request(
                |reply| Message::LocalOpen {
                    request,
                    events: Channel::new(|_| Ok(())),
                    reply,
                },
                request_id,
            )
            .await
            .expect("open local terminal");
        wait_for_local_state(
            service,
            &opened.session.session_id,
            norishell_core_api::LocalSessionState::Running,
        )
        .await
    }

    async fn focus_local_session(
        service: &SshSessionService,
        details: &LocalSessionDetails,
        expected_focus_epoch: u64,
        key: &str,
    ) -> norishell_core_api::LocalSessionInputLease {
        let attachment = details.attachments.first().expect("local attachment");
        let request = TerminalInputFocusChangeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: key.to_owned(),
            expected_focus_epoch: WireSequence::new(expected_focus_epoch),
            target: Some(TerminalInputFocusTarget::Local(
                norishell_core_api::LocalTerminalInputFocusTarget {
                    session_id: details.session.session_id.clone(),
                    expected_generation: details.session.generation,
                    expected_state_revision: details.session.state_revision,
                    pty_id: details.session.pty_id.clone().expect("local PTY id"),
                    attachment_id: attachment.attachment_id.clone(),
                    view_id: attachment.view_id.clone(),
                },
            )),
        };
        let request_id = request.meta.request_id.clone();
        let response = service
            .request(
                |reply| Message::TerminalFocusChange { request, reply },
                request_id,
            )
            .await
            .expect("focus local terminal");
        match response.lease.expect("local input lease") {
            TerminalInputLease::Local(lease) => lease,
            TerminalInputLease::Ssh(_)
            | TerminalInputLease::Telnet(_)
            | TerminalInputLease::Plugin(_) => {
                panic!("local focus returned a non-local lease")
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_zsh_enable_through_actor_keeps_pty_running_and_records_completion() {
        use norishell_core_api::{
            LocalSessionEventPayload, LocalSessionState, NativeTerminalCaptureState,
            NativeTerminalEnableRequest, NativeTerminalHistoryListRequest,
            NativeTerminalHistoryScope, NativeTerminalInputFence, NativeTerminalLocalInputFence,
            NativeTerminalSettings, NativeTerminalSettingsReplaceRequest, NativeTerminalShellKind,
            NativeTerminalSnapshotRequest,
        };

        let directory = tempfile::tempdir().expect("isolated native terminal settings");
        let hosts = HostService::start(directory.path()).unwrap();
        let vault = VaultService::start(directory.path());
        let native = NativeTerminalService::start(directory.path(), vault.clone());
        native
            .replace_settings(NativeTerminalSettingsReplaceRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                expected_settings_revision: WireSequence::new(1),
                settings: NativeTerminalSettings {
                    history_enabled: true,
                    ..NativeTerminalSettings::default()
                },
            })
            .unwrap();
        let service = SshSessionService::start_with_agent_and_native(
            hosts,
            vault,
            TransientCredentialService::default(),
            crate::ssh_agent_service::SshAgentService::default(),
            native.clone(),
        );
        let test_service = service.clone();
        let mut check = tokio::spawn(async move {
            let output = Arc::new(Mutex::new(Vec::<u8>::new()));
            let output_sink = output.clone();
            let request = LocalSessionOpenRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "native-zsh-actor-open".to_owned(),
                open_attempt_id: LocalOpenAttemptId::new(),
                attach_attempt_id: LocalAttachAttemptId::new(),
                view_id: LocalViewId::new(),
                rows: 24,
                cols: 80,
            };
            let request_id = request.meta.request_id.clone();
            let opened = test_service
                .request(
                    |reply| Message::LocalOpen {
                        request,
                        events: Channel::<LocalSessionEvent>::new(move |event| {
                            if let tauri::ipc::InvokeResponseBody::Json(json) = event {
                                let event: LocalSessionEvent = serde_json::from_str(&json).unwrap();
                                if let LocalSessionEventPayload::OutputFrame { frame } =
                                    event.payload
                                {
                                    let mut bytes = output_sink.lock().unwrap();
                                    let available =
                                        OUTPUT_RING_MAX_BYTES.saturating_sub(bytes.len());
                                    bytes.extend(frame.bytes.into_iter().take(available));
                                }
                            }
                            Ok(())
                        }),
                        reply,
                    },
                    request_id,
                )
                .await
                .unwrap();
            let details = wait_for_local_state(
                &test_service,
                &opened.session.session_id,
                LocalSessionState::Running,
            )
            .await;
            let lease = focus_local_session(&test_service, &details, 0, "native-zsh-focus").await;
            let attachment = details.attachments[0].clone();
            let input = |sequence, bytes: &[u8]| LocalSessionInputRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: details.session.session_id.clone(),
                expected_generation: details.session.generation,
                expected_state_revision: details.session.state_revision,
                pty_id: details.session.pty_id.clone().unwrap(),
                attachment_id: attachment.attachment_id.clone(),
                view_id: attachment.view_id.clone(),
                lease_id: lease.lease_id.clone(),
                focus_epoch: lease.focus_epoch,
                input_epoch: lease.input_epoch,
                client_seq: WireSequence::new(sequence),
                bytes: bytes.to_vec(),
            };
            // Keep the real Core-owned PTY, but make this gate independent of
            // the account's default shell and personal zsh startup hooks.
            let isolated_shell = input(1, b"exec /bin/zsh -f\r");
            let request_id = isolated_shell.meta.request_id.clone();
            test_service
                .linearized_request(
                    |reply| Message::LocalInput {
                        request: isolated_shell,
                        reply,
                    },
                    request_id,
                )
                .await
                .unwrap();
            // Only this isolated test shell changes: prevent its bootstrap or
            // test commands from being saved into the user's shell history.
            let initial = input(2, b"unset HISTFILE; HISTSIZE=0; SAVEHIST=0; printf '%s%s\\n' '__NVX_' 'BOOT_READY__'\r");
            let request_id = initial.meta.request_id.clone();
            test_service
                .linearized_request(
                    |reply| Message::LocalInput {
                        request: initial,
                        reply,
                    },
                    request_id,
                )
                .await
                .unwrap();
            timeout(Duration::from_secs(5), async {
                loop {
                    if output
                        .lock()
                        .unwrap()
                        .windows(b"__NVX_BOOT_READY__".len())
                        .any(|bytes| bytes == b"__NVX_BOOT_READY__")
                    {
                        break;
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("real shell reached an empty prompt");
            let prepared = native
                .prepare_enable(&NativeTerminalEnableRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    input_fence: NativeTerminalInputFence::Local(NativeTerminalLocalInputFence {
                        session_id: details.session.session_id.clone(),
                        expected_generation: details.session.generation,
                        expected_state_revision: details.session.state_revision,
                        pty_id: details.session.pty_id.clone().unwrap(),
                        attachment_id: attachment.attachment_id.clone(),
                        view_id: attachment.view_id.clone(),
                        lease_id: lease.lease_id.clone(),
                        focus_epoch: lease.focus_epoch,
                        input_epoch: lease.input_epoch,
                        client_seq: WireSequence::new(3),
                    }),
                    shell_kind: NativeTerminalShellKind::Zsh,
                    confirmed_empty_prompt: true,
                })
                .unwrap();
            let scope = prepared.scope.clone();
            test_service
                .native_terminal_enable_local(RequestId::new(), prepared)
                .await
                .expect("real PTY ACKs the full bootstrap");
            timeout(Duration::from_secs(5), async {
                loop {
                    if native.status_for(&scope).is_some_and(|status| {
                        status.capture_state == NativeTerminalCaptureState::Ready
                            && status.prompt_observed
                    }) {
                        break;
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("real zsh emits ready and prompt through the actor");
            let command = "printf '__NVX_NATIVE_COMMAND__\\n'; false";
            let request = input(4, format!("{command}\r").as_bytes());
            let request_id = request.meta.request_id.clone();
            test_service
                .linearized_request(|reply| Message::LocalInput { request, reply }, request_id)
                .await
                .unwrap();
            timeout(Duration::from_secs(5), async {
                loop {
                    let snapshot = native.snapshot(NativeTerminalSnapshotRequest {
                        meta: RequestMeta {
                            request_id: RequestId::new(),
                        },
                        after_completion_cursor: None,
                    });
                    if snapshot.completions.iter().any(|completion| {
                        completion.session == scope && completion.exit_code == Some(1)
                    }) {
                        break;
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("real zsh completion reaches native terminal service");
            let history = native
                .history_list(NativeTerminalHistoryListRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    scope: Some(NativeTerminalHistoryScope::Local),
                    query: "__NVX_NATIVE_COMMAND__".to_owned(),
                    limit: 10,
                })
                .unwrap();
            assert!(history.iter().any(|entry| entry.command == command));
            assert_eq!(
                local_session_details(&test_service, &details.session.session_id)
                    .await
                    .session
                    .state,
                LocalSessionState::Running
            );
        });
        let outcome = timeout(Duration::from_secs(25), &mut check).await;
        if outcome.is_err() {
            check.abort();
            let _ = check.await;
        }
        // Cleanup runs even when the assertion task fails or times out.
        service
            .shutdown_all(RequestId::new())
            .await
            .expect("cleanup isolated native PTY");
        assert!(service.local_exit_blockers().is_empty());
        outcome
            .expect("native zsh actor gate completes")
            .expect("native zsh actor assertions");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_terminal_actor_owns_pty_focus_replay_exit_and_blocker_cleanup() {
        let (_directory, _hosts, _credentials, service) = network_service_fixture();
        let open_request = LocalSessionOpenRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "local-open".to_owned(),
            open_attempt_id: LocalOpenAttemptId::new(),
            attach_attempt_id: LocalAttachAttemptId::new(),
            view_id: LocalViewId::new(),
            rows: 24,
            cols: 80,
        };
        let request_id = open_request.meta.request_id.clone();
        let opened = service
            .request(
                |reply| Message::LocalOpen {
                    request: open_request,
                    events: Channel::new(|_| Ok(())),
                    reply,
                },
                request_id,
            )
            .await
            .expect("open local terminal");
        assert_eq!(
            service.local_exit_blockers(),
            vec![opened.session.session_id.clone()]
        );

        let running = wait_for_local_state(
            &service,
            &opened.session.session_id,
            norishell_core_api::LocalSessionState::Running,
        )
        .await;
        assert!(!running.session.shell_name.is_empty());
        let pty_id = running
            .session
            .pty_id
            .clone()
            .expect("running local PTY id");
        let attachment = running.attachments.first().expect("local attachment");
        let target = norishell_core_api::LocalTerminalInputFocusTarget {
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            expected_state_revision: running.session.state_revision,
            pty_id: pty_id.clone(),
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
        };
        let focus_request = TerminalInputFocusChangeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "focus-local".to_owned(),
            expected_focus_epoch: WireSequence::new(0),
            target: Some(TerminalInputFocusTarget::Local(target.clone())),
        };
        let request_id = focus_request.meta.request_id.clone();
        let focused = service
            .request(
                |reply| Message::TerminalFocusChange {
                    request: focus_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("focus local terminal");
        let lease = match focused.lease.expect("local input lease") {
            TerminalInputLease::Local(lease) => lease,
            TerminalInputLease::Ssh(_)
            | TerminalInputLease::Telnet(_)
            | TerminalInputLease::Plugin(_) => {
                panic!("local focus returned a non-local lease")
            }
        };

        let resize_request = LocalSessionResizeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            expected_state_revision: running.session.state_revision,
            pty_id: pty_id.clone(),
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            lease_id: lease.lease_id.clone(),
            focus_epoch: lease.focus_epoch,
            input_epoch: lease.input_epoch,
            resize_seq: WireSequence::new(1),
            rows: 39,
            cols: 117,
        };
        let request_id = resize_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::LocalResize {
                    request: resize_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("resize local PTY");

        service
            .tx
            .send(Message::LocalProcessOutput {
                session_id: running.session.session_id.as_str().to_owned(),
                generation: running.session.generation.get(),
                bytes: vec![b'g'; OUTPUT_RING_MAX_BYTES + LOCAL_TERMINAL_OUTPUT_FRAME_MAX_BYTES],
            })
            .await
            .expect("queue bounded local output overflow");

        let input_request = LocalSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            expected_state_revision: running.session.state_revision,
            pty_id,
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            lease_id: lease.lease_id,
            focus_epoch: lease.focus_epoch,
            input_epoch: lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: b"printf '__NVX_LOCAL_ACTOR__:%s\\n' \"$(stty size)\"; exit 7\r".to_vec(),
        };
        let request_id = input_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::LocalInput {
                    request: input_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("write local PTY input");

        let exited = wait_for_local_state(
            &service,
            &running.session.session_id,
            norishell_core_api::LocalSessionState::Exited,
        )
        .await;
        assert_eq!(
            exited.session.exit,
            Some(norishell_core_api::LocalSessionExit::ExitStatus { exit_status: 7 })
        );
        assert!(service.local_exit_blockers().is_empty());
        let focus_snapshot = service
            .request(
                |reply| Message::TerminalFocusSnapshot { reply },
                RequestId::new(),
            )
            .await
            .expect("read focus after local exit");
        assert_eq!(focus_snapshot.focus_epoch, WireSequence::new(2));
        assert!(focus_snapshot.target.is_none());
        assert!(focus_snapshot.lease.is_none());

        let attach_request = LocalSessionAttachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "local-replay".to_owned(),
            attach_attempt_id: LocalAttachAttemptId::new(),
            session_id: exited.session.session_id,
            expected_generation: exited.session.generation,
            expected_state_revision: exited.session.state_revision,
            view_id: LocalViewId::new(),
            after_output_seq: None,
        };
        let request_id = attach_request.meta.request_id.clone();
        let replay = service
            .request(
                |reply| Message::LocalAttach {
                    request: attach_request,
                    events: Channel::new(|_| Ok(())),
                    reply,
                },
                request_id,
            )
            .await
            .expect("reattach exited local terminal");
        assert!(
            replay
                .replay
                .iter()
                .any(|item| matches!(item, LocalSessionOutputItem::Gap(_)))
        );
        let replay_bytes = replay
            .replay
            .into_iter()
            .filter_map(|item| match item {
                LocalSessionOutputItem::Frame(frame) => Some(frame.bytes),
                LocalSessionOutputItem::Gap(_) => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert!(String::from_utf8_lossy(&replay_bytes).contains("__NVX_LOCAL_ACTOR__:39 117"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_last_attachment_requires_confirmation_then_terminates_and_clears_focus() {
        let (_directory, _hosts, _credentials, service) = network_service_fixture();
        let running = open_running_local_session(&service, "local-close-open").await;
        let _lease = focus_local_session(&service, &running, 0, "local-close-focus").await;
        let attachment = running.attachments.first().expect("local attachment");
        let detach = LocalSessionDetachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "local-close-challenge".to_owned(),
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            expected_state_revision: running.session.state_revision,
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            intent: norishell_core_api::LocalSessionDetachIntent::UserClose,
            confirmation: None,
        };
        let request_id = detach.meta.request_id.clone();
        let challenged = service
            .request(
                |reply| Message::LocalDetach {
                    request: detach,
                    reply,
                },
                request_id,
            )
            .await
            .expect("request last local attachment close");
        let (expected_state_revision, expected_attachment_revision) = match challenged {
            LocalSessionDetachResult::ConfirmationRequired {
                expected_state_revision,
                expected_attachment_revision,
                ..
            } => (expected_state_revision, expected_attachment_revision),
            other => panic!("unexpected local detach result: {other:?}"),
        };
        let focused = service
            .request(
                |reply| Message::TerminalFocusSnapshot { reply },
                RequestId::new(),
            )
            .await
            .expect("focus survives unconfirmed detach");
        assert!(matches!(
            focused.target,
            Some(TerminalInputFocusTarget::Local(_))
        ));

        let confirmed = LocalSessionDetachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "local-close-confirm".to_owned(),
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            expected_state_revision,
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            intent: norishell_core_api::LocalSessionDetachIntent::UserClose,
            confirmation: Some(norishell_core_api::LocalSessionLastDetachConfirmation {
                action: norishell_core_api::LocalSessionLastDetachAction::TerminateAndDetach,
                expected_state_revision,
                expected_attachment_revision,
            }),
        };
        let request_id = confirmed.meta.request_id.clone();
        let stopping = service
            .request(
                |reply| Message::LocalDetach {
                    request: confirmed,
                    reply,
                },
                request_id,
            )
            .await
            .expect("confirm local termination");
        assert!(matches!(
            stopping,
            LocalSessionDetachResult::Stopping { .. }
        ));
        let closed = wait_for_local_state(
            &service,
            &running.session.session_id,
            norishell_core_api::LocalSessionState::Closed,
        )
        .await;
        assert_eq!(closed.session.attachment_count, 0);
        assert!(service.local_exit_blockers().is_empty());
        let cleared = service
            .request(
                |reply| Message::TerminalFocusSnapshot { reply },
                RequestId::new(),
            )
            .await
            .expect("focus cleared after confirmed local close");
        assert!(cleared.target.is_none());
        assert!(cleared.lease.is_none());
    }

    fn invoke_command<T: DeserializeOwned>(
        webview: &WebviewWindow<MockRuntime>,
        command: &str,
        body: serde_json::Value,
    ) -> T {
        let response = get_ipc_response(
            webview,
            InvokeRequest {
                cmd: command.to_owned(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .expect("local mock Tauri URL"),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_owned(),
            },
        )
        .unwrap_or_else(|error| panic!("{command} invoke failed: {error}"));
        response
            .deserialize()
            .unwrap_or_else(|error| panic!("deserialize {command} response: {error}"))
    }

    fn invoke_session_details(
        webview: &WebviewWindow<MockRuntime>,
        session_id: &SshSessionId,
    ) -> SshSessionDetails {
        invoke_command(
            webview,
            "ssh_terminal_get",
            serde_json::json!({
                "request": SshSessionGetRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    session_id: session_id.clone(),
                },
            }),
        )
    }

    async fn wait_for_invoke_state(
        webview: &WebviewWindow<MockRuntime>,
        session_id: &SshSessionId,
        expected: SshSessionState,
    ) -> SshSessionDetails {
        timeout(Duration::from_secs(10), async {
            loop {
                let details = invoke_session_details(webview, session_id);
                if details.session.state == expected {
                    return details;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("invoke session did not reach {expected:?}"))
    }

    fn insert_test_session(
        actor: &mut Actor,
        state: SshSessionState,
        generation: u64,
        state_revision: u64,
        shell: Option<mpsc::Sender<ShellCommand>>,
    ) -> TestSession {
        let session_id = SshSessionId::new();
        let attachment_id = SshAttachmentId::new();
        let view_id = SshViewId::new();
        let channel_id = (state == SshSessionState::Running).then(SshChannelId::new);
        let now = unix_time_ms();
        let target = SshSessionTarget::QuickConnect {
            endpoint: SshSessionEndpoint {
                address: "127.0.0.1".to_owned(),
                port: 9,
                username: Some("acceptance".to_owned()),
            },
        };
        let summary = SshSessionSummary {
            session_id: session_id.clone(),
            open_attempt_id: SshOpenAttemptId::new(),
            target: target.clone(),
            credential_ref_id: None,
            endpoint: match target {
                SshSessionTarget::QuickConnect { endpoint } => Some(endpoint),
                SshSessionTarget::Host { .. } => None,
            },
            generation: WireSequence::new(generation),
            state_revision: WireSequence::new(state_revision),
            attachment_revision: WireSequence::new(1),
            event_seq: WireSequence::new(0),
            channel_id: channel_id.clone(),
            negotiated_algorithms: Vec::new(),
            state,
            close_reason: None,
            failure_reason: None,
            attachment_count: 1,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let attachment = SshSessionAttachment {
            attachment_id: attachment_id.clone(),
            attach_attempt_id: SshAttachAttemptId::new(),
            session_id: session_id.clone(),
            generation: WireSequence::new(generation),
            channel_id,
            view_id: view_id.clone(),
            state_revision: WireSequence::new(state_revision),
            attachment_revision: WireSequence::new(1),
            attached_at_unix_ms: now,
        };
        actor.sessions.insert(
            session_id.as_str().to_owned(),
            SessionRecord {
                shared_channels: None,
                channel_lease_cancel: watch::channel(false).0,
                terminal_startup: None,
                summary,
                attachments: BTreeMap::from([(
                    attachment_id.as_str().to_owned(),
                    AttachmentRecord {
                        summary: attachment,
                        events: Channel::<SshSessionEvent>::new(|_| Ok(())),
                        last_seen_at_unix_ms: now,
                    },
                )]),
                active_challenge: None,
                active_keyboard_interactive_challenge: None,
                prepared_keyboard_interactive_answers: BTreeMap::new(),
                active_login_automation: None,
                heartbeat_policy: test_heartbeat_policy(),
                heartbeat: heartbeat_status_from_policy(&test_heartbeat_policy()),
                transport_heartbeat_tasks: Vec::new(),
                shell_heartbeat_task: None,
                last_user_input_at: None,
                shell_heartbeat_write_in_flight: false,
                input_activity_epoch: Arc::new(AtomicU64::new(0)),
                input_lease: None,
                next_input_epoch: 0,
                last_client_seq: 0,
                last_resize_seq: 0,
                next_output_seq: 1,
                output_ring: VecDeque::new(),
                output_ring_bytes: 0,
                shell,
                shell_stop: None,
            },
        );
        TestSession {
            session_id,
            attachment_id,
            view_id,
        }
    }

    fn activate_test_login_automation(
        actor: &mut Actor,
        session: &TestSession,
        steps: Vec<LoginAutomationStepInput>,
        total_deadline: tokio::time::Instant,
        step_deadline: tokio::time::Instant,
    ) {
        let now = unix_time_ms();
        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session");
        record.summary.state = SshSessionState::AutomatingLogin;
        record.active_login_automation = Some(ActiveLoginAutomation {
            policy_revision: WireSequence::new(9),
            progress: login_automation_progress(
                WireSequence::new(9),
                &steps,
                0,
                now,
                now + 300_000,
                SshLoginAutomationStatus::Running,
                None,
            ),
            steps,
            current_step_index: 0,
            matcher_buffer: Vec::new(),
            timeout_scheduled_for: None,
            total_deadline_unix_ms: now + 300_000,
            total_deadline,
            step_deadline,
            owner_attachment_id: session.attachment_id.clone(),
            owner_view_id: session.view_id.clone(),
            write_in_flight: None,
            pending_takeover: None,
        });
    }

    #[tokio::test]
    async fn keyboard_interactive_actor_fences_one_time_answers_to_exact_round_and_view() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Authenticating, 3, 5, None);
        let credential_ref_id = CredentialRefId::new();
        let (answers_tx, answers_rx) = oneshot::channel();
        actor.keyboard_interactive_challenge_observed(
            session.session_id.as_str().to_owned(),
            3,
            SshSessionRouteStage::Target,
            credential_ref_id,
            0,
            1,
            TransportKeyboardChallenge {
                name: "Verification".to_owned(),
                instructions: "Enter account and code".to_owned(),
                prompts: vec![
                    norishell_ssh_transport::KeyboardInteractivePrompt {
                        text: "Account".to_owned(),
                        echo: true,
                    },
                    norishell_ssh_transport::KeyboardInteractivePrompt {
                        text: "Code".to_owned(),
                        echo: false,
                    },
                ],
            },
            answers_tx,
        );
        let challenge = actor
            .sessions
            .get(session.session_id.as_str())
            .and_then(|record| record.active_keyboard_interactive_challenge.as_ref())
            .map(|active| active.challenge.clone())
            .expect("active challenge");
        assert_eq!(challenge.state_revision, WireSequence::new(6));
        assert!(!challenge.prompts[0].sensitive);
        assert!(challenge.prompts[1].sensitive);

        let stale_view =
            actor.keyboard_interactive_answer_prepare(SshKeyboardInteractiveAnswerPrepareRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: session.session_id.clone(),
                expected_generation: WireSequence::new(3),
                challenge_id: challenge.challenge_id.clone(),
                expected_state_revision: challenge.state_revision,
                round_index: 1,
                prompt_index: 1,
                attachment_id: session.attachment_id.clone(),
                view_id: SshViewId::new(),
                answer: "must-not-be-stored".to_owned(),
            });
        assert!(stale_view.is_err());
        let prepared = actor
            .keyboard_interactive_answer_prepare(SshKeyboardInteractiveAnswerPrepareRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: session.session_id.clone(),
                expected_generation: WireSequence::new(3),
                challenge_id: challenge.challenge_id.clone(),
                expected_state_revision: challenge.state_revision,
                round_index: 1,
                prompt_index: 1,
                attachment_id: session.attachment_id.clone(),
                view_id: session.view_id.clone(),
                answer: "827391".to_owned(),
            })
            .expect("prepare hidden answer");
        let request = SshKeyboardInteractiveResponseRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(3),
            challenge_id: challenge.challenge_id.clone(),
            expected_state_revision: challenge.state_revision,
            round_index: 1,
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
            answers: vec![
                SshKeyboardInteractiveAnswerInput::EchoText {
                    prompt_index: 0,
                    value: "deploy".to_owned(),
                },
                SshKeyboardInteractiveAnswerInput::OneTimeAnswerRef {
                    prompt_index: 1,
                    answer_ref_id: prepared.answer_ref_id,
                },
            ],
        };
        let details = actor
            .keyboard_interactive_respond(request.clone())
            .expect("respond exact round");
        assert_eq!(details.session.state_revision, WireSequence::new(7));
        assert!(details.active_keyboard_interactive_challenge.is_none());
        let answers = answers_rx
            .await
            .expect("connect task received answers")
            .expect("answers");
        assert_eq!(answers[0].as_str(), "deploy");
        assert_eq!(answers[1].as_str(), "827391");
        assert!(actor.keyboard_interactive_respond(request).is_err());
    }

    fn focus_target(actor: &Actor, session: &TestSession) -> SshTerminalInputFocusTarget {
        let record = actor
            .sessions
            .get(session.session_id.as_str())
            .expect("test session");
        SshTerminalInputFocusTarget {
            session_id: session.session_id.clone(),
            expected_generation: record.summary.generation,
            expected_state_revision: record.summary.state_revision,
            channel_id: record.summary.channel_id.clone().expect("running channel"),
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
        }
    }

    fn focus_change_request(
        expected_focus_epoch: u64,
        target: Option<SshTerminalInputFocusTarget>,
        operation_id: OperationId,
        idempotency_key: &str,
    ) -> SshTerminalInputFocusChangeRequest {
        SshTerminalInputFocusChangeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id,
            idempotency_key: idempotency_key.to_owned(),
            expected_focus_epoch: WireSequence::new(expected_focus_epoch),
            target,
        }
    }

    #[tokio::test]
    async fn failed_telnet_focus_grant_never_exposes_a_writable_target() {
        let (_directory, _hosts, _credentials, service) = network_service_fixture();
        let telnet = crate::telnet_session_service::TelnetSessionService::start();
        let request = TerminalInputFocusChangeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "missing-telnet-target".to_owned(),
            expected_focus_epoch: WireSequence::new(0),
            target: Some(TerminalInputFocusTarget::Telnet(
                norishell_core_api::TelnetTerminalInputFocusTarget {
                    session_id: norishell_core_api::TelnetSessionId::new(),
                    expected_generation: WireSequence::new(1),
                    expected_state_revision: WireSequence::new(1),
                    socket_id: norishell_core_api::TelnetSocketId::new(),
                    attachment_id: norishell_core_api::TelnetAttachmentId::new(),
                    view_id: norishell_core_api::TelnetViewId::new(),
                },
            )),
        };
        let broker = service.focus_broker();
        let operation_request = request.clone();
        let focus_service = service.clone();
        let focus_telnet = telnet.clone();
        let focus = tokio::spawn(async move {
            broker
                .linearize_focus_change(
                    &request,
                    terminal_input_focus_change_once(
                        operation_request,
                        &focus_service,
                        &focus_telnet,
                        None,
                    ),
                )
                .await
        });
        tokio::task::yield_now().await;
        let snapshot_service = service.clone();
        let snapshot = tokio::spawn(async move {
            snapshot_service
                .terminal_focus_snapshot_internal(RequestId::new())
                .await
        });

        assert_eq!(
            focus
                .await
                .expect("focus task")
                .expect_err("grant failure")
                .code,
            "ssh_terminal.stale_fence"
        );
        let snapshot = snapshot
            .await
            .expect("snapshot task")
            .expect("focus snapshot");
        assert!(snapshot.target.is_none());
        assert!(snapshot.lease.is_none());
        assert_eq!(snapshot.focus_epoch, WireSequence::new(2));

        service
            .shutdown_all(RequestId::new())
            .await
            .expect("ssh shutdown");
        telnet.shutdown_all().await.expect("telnet shutdown");
    }

    #[tokio::test]
    async fn telnet_focus_grant_rechecks_canonical_target_before_returning_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut byte = [0_u8; 1];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut byte).await;
        });
        let (_directory, _hosts, _credentials, service) = network_service_fixture();
        let telnet = crate::telnet_session_service::TelnetSessionService::start();
        let opened = telnet
            .open(crate::telnet_session_service::TelnetOpenRequest {
                operation_id: Uuid::now_v7().to_string(),
                idempotency_key: "focus-race-open".to_owned(),
                open_attempt_id: Uuid::now_v7().to_string(),
                address: "127.0.0.1".to_owned(),
                port: Some(port),
                rows: 24,
                cols: 80,
                cleartext_risk_accepted: true,
                attach_attempt_id: Uuid::now_v7().to_string(),
                view_id: Uuid::now_v7().to_string(),
            })
            .await
            .expect("telnet open");
        let running = loop {
            if let Some(summary) = telnet
                .snapshot()
                .await
                .expect("telnet snapshot")
                .into_iter()
                .find(|summary| {
                    summary.session_id == opened.session.session_id
                        && summary.state
                            == crate::telnet_session_service::TelnetSessionState::Running
                })
            {
                break summary;
            }
            sleep(Duration::from_millis(10)).await;
        };
        let attachment = telnet
            .attachment_heartbeat(
                running.session_id.clone(),
                running.generation,
                running.attachment_revision,
                opened.attachment.attachment_id.clone(),
                opened.attachment.view_id.clone(),
            )
            .await
            .expect("current attachment");
        let target = norishell_core_api::TelnetTerminalInputFocusTarget {
            session_id: norishell_core_api::TelnetSessionId::parse(running.session_id.clone())
                .expect("session id"),
            expected_generation: WireSequence::new(running.generation),
            expected_state_revision: WireSequence::new(running.state_revision),
            socket_id: norishell_core_api::TelnetSocketId::parse(
                running.socket_id.clone().expect("socket"),
            )
            .expect("socket id"),
            attachment_id: norishell_core_api::TelnetAttachmentId::parse(
                attachment.attachment_id.clone(),
            )
            .expect("attachment id"),
            view_id: norishell_core_api::TelnetViewId::parse(attachment.view_id.clone())
                .expect("view id"),
        };
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        telnet
            .barrier(entered_tx, release_rx)
            .await
            .expect("enqueue barrier");
        entered_rx.await.expect("barrier entered");

        let request = TerminalInputFocusChangeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "focus-race".to_owned(),
            expected_focus_epoch: WireSequence::new(0),
            target: Some(TerminalInputFocusTarget::Telnet(target.clone())),
        };
        let focus_service = service.clone();
        let focus_telnet = telnet.clone();
        let focus = tokio::spawn(async move {
            terminal_input_focus_change_once(request, &focus_service, &focus_telnet, None).await
        });
        loop {
            let snapshot = service
                .terminal_focus_snapshot_unserialized(RequestId::new())
                .await
                .expect("canonical snapshot");
            if snapshot.target == Some(TerminalInputFocusTarget::Telnet(target.clone())) {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        service
            .clear_telnet_focus_exact_unserialized(
                crate::telnet_session_service::TelnetFocusInvalidation {
                    session_id: running.session_id.clone(),
                    generation: running.generation,
                    state_revision: running.state_revision,
                    socket_id: running.socket_id.expect("socket"),
                    attachment_id: attachment.attachment_id,
                    view_id: attachment.view_id,
                    focus_epoch: 1,
                },
            )
            .await
            .expect("background invalidation");
        let _ = release_tx.send(());
        assert_eq!(
            focus
                .await
                .expect("focus task")
                .expect_err("stale success must fail")
                .code,
            "ssh_terminal.stale_fence"
        );
        let snapshot = service
            .terminal_focus_snapshot_internal(RequestId::new())
            .await
            .expect("final snapshot");
        assert!(snapshot.target.is_none());
        assert_eq!(snapshot.focus_epoch, WireSequence::new(2));

        telnet.shutdown_all().await.expect("telnet shutdown");
        server.await.expect("server");
        service
            .shutdown_all(RequestId::new())
            .await
            .expect("ssh shutdown");
    }

    #[tokio::test]
    async fn telnet_remote_close_pump_clears_exact_canonical_focus_and_advances_epoch() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let port = listener.local_addr().expect("address").port();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let _ = release_rx.await;
            let _ = tokio::io::AsyncWriteExt::shutdown(&mut socket).await;
        });
        let (_directory, _hosts, _credentials, service) = network_service_fixture();
        let telnet = crate::telnet_session_service::TelnetSessionService::start();
        telnet.ensure_focus_coordinator(service.clone());
        let opened = telnet
            .open(crate::telnet_session_service::TelnetOpenRequest {
                operation_id: Uuid::now_v7().to_string(),
                idempotency_key: "remote-close-open".to_owned(),
                open_attempt_id: Uuid::now_v7().to_string(),
                address: "127.0.0.1".to_owned(),
                port: Some(port),
                rows: 24,
                cols: 80,
                cleartext_risk_accepted: true,
                attach_attempt_id: Uuid::now_v7().to_string(),
                view_id: Uuid::now_v7().to_string(),
            })
            .await
            .expect("telnet open");
        let running = loop {
            if let Some(summary) = telnet
                .snapshot()
                .await
                .expect("telnet snapshot")
                .into_iter()
                .find(|summary| {
                    summary.session_id == opened.session.session_id
                        && summary.state
                            == crate::telnet_session_service::TelnetSessionState::Running
                })
            {
                break summary;
            }
            sleep(Duration::from_millis(10)).await;
        };
        let attachment = telnet
            .attachment_heartbeat(
                running.session_id.clone(),
                running.generation,
                running.attachment_revision,
                opened.attachment.attachment_id,
                opened.attachment.view_id,
            )
            .await
            .expect("current attachment");
        let target = norishell_core_api::TelnetTerminalInputFocusTarget {
            session_id: norishell_core_api::TelnetSessionId::parse(running.session_id.clone())
                .expect("session id"),
            expected_generation: WireSequence::new(running.generation),
            expected_state_revision: WireSequence::new(running.state_revision),
            socket_id: norishell_core_api::TelnetSocketId::parse(
                running.socket_id.clone().expect("socket"),
            )
            .expect("socket id"),
            attachment_id: norishell_core_api::TelnetAttachmentId::parse(attachment.attachment_id)
                .expect("attachment id"),
            view_id: norishell_core_api::TelnetViewId::parse(attachment.view_id).expect("view id"),
        };
        let request = TerminalInputFocusChangeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "focus-before-remote-close".to_owned(),
            expected_focus_epoch: WireSequence::new(0),
            target: Some(TerminalInputFocusTarget::Telnet(target)),
        };
        let broker = service.focus_broker();
        let response = broker
            .linearize_focus_change(
                &request,
                terminal_input_focus_change_once(request.clone(), &service, &telnet, None),
            )
            .await
            .expect("focus telnet");
        assert_eq!(response.focus_epoch, WireSequence::new(1));
        assert!(matches!(
            response.lease,
            Some(TerminalInputLease::Telnet(_))
        ));

        let _ = release_tx.send(());
        server.await.expect("server");
        for _ in 0..100 {
            let snapshot = service
                .terminal_focus_snapshot_internal(RequestId::new())
                .await
                .expect("focus snapshot");
            if snapshot.target.is_none() && snapshot.focus_epoch == WireSequence::new(2) {
                telnet.shutdown_all().await.expect("telnet shutdown");
                service
                    .shutdown_all(RequestId::new())
                    .await
                    .expect("ssh shutdown");
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("remote close did not clear canonical focus");
    }

    #[test]
    fn global_focus_linearizes_input_before_and_after_cross_session_transfer() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let (first_shell, mut first_commands) = mpsc::channel(4);
        let (second_shell, _second_commands) = mpsc::channel(4);
        let first = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            1,
            4,
            Some(first_shell),
        );
        let second = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            1,
            7,
            Some(second_shell),
        );

        let first_focus = actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &first)),
                OperationId::new(),
                "focus-first",
            ))
            .expect("focus first session");
        let first_lease = first_focus.lease.expect("first focus lease");
        let first_channel = actor
            .sessions
            .get(first.session_id.as_str())
            .and_then(|record| record.summary.channel_id.clone())
            .expect("first channel");
        actor
            .queue_input(SshSessionInputRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: first.session_id.clone(),
                expected_generation: WireSequence::new(1),
                channel_id: first_channel.clone(),
                attachment_id: first.attachment_id.clone(),
                view_id: first.view_id.clone(),
                focus_epoch: first_lease.focus_epoch,
                lease_id: first_lease.lease_id.clone(),
                input_epoch: first_lease.input_epoch,
                client_seq: WireSequence::new(1),
                bytes: b"before-focus-change".to_vec(),
            })
            .expect("input ordered before focus change remains valid");
        assert!(matches!(
            first_commands.try_recv(),
            Ok(ShellCommand::Input { bytes, .. }) if bytes == b"before-focus-change"
        ));

        let second_focus = actor
            .focus_change(focus_change_request(
                1,
                Some(focus_target(&actor, &second)),
                OperationId::new(),
                "focus-second",
            ))
            .expect("focus second session");
        assert_eq!(second_focus.focus_epoch, WireSequence::new(2));
        assert!(
            actor
                .sessions
                .get(first.session_id.as_str())
                .expect("first session")
                .input_lease
                .is_none()
        );
        let stale = actor
            .queue_input(SshSessionInputRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: first.session_id,
                expected_generation: WireSequence::new(1),
                channel_id: first_channel,
                attachment_id: first.attachment_id,
                view_id: first.view_id,
                focus_epoch: first_lease.focus_epoch,
                lease_id: first_lease.lease_id,
                input_epoch: first_lease.input_epoch,
                client_seq: WireSequence::new(2),
                bytes: b"after-focus-change".to_vec(),
            })
            .expect_err("old focused input must fail closed");
        assert_eq!(stale.code, "ssh_terminal.stale_focus");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_ack_drains_output_beyond_mailbox_capacity_without_reordering_focus() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let (shell, mut commands) = mpsc::channel(4);
        let session = insert_test_session(&mut actor, SshSessionState::Running, 1, 4, Some(shell));
        let focus = actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &session)),
                OperationId::new(),
                "output-progress-focus",
            ))
            .expect("focus SSH session");
        let lease = focus.lease.expect("focused SSH lease");
        let tx = actor.tx.clone();
        let request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(1),
            channel_id: actor.sessions[session.session_id.as_str()]
                .summary
                .channel_id
                .clone()
                .unwrap(),
            attachment_id: session.attachment_id,
            view_id: session.view_id,
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id,
            input_epoch: lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: vec![b'x'; 8 * 1024],
        };
        let actor_task = tokio::spawn(run_actor(actor, rx));
        let (reply, response) = oneshot::channel();
        tx.send(Message::Input { request, reply }).await.unwrap();
        let completion = match commands.recv().await.expect("writer receives input") {
            ShellCommand::Input { completion, .. } => completion,
            _ => panic!("expected input"),
        };
        let (focus_reply, mut focus_response) = oneshot::channel();
        tx.send(Message::FocusChange {
            request: focus_change_request(1, None, OperationId::new(), "focus-after-output-ack"),
            reply: focus_reply,
        })
        .await
        .unwrap();
        // A real PTY writer may need the shell to drain its input while the
        // shell echoes that input. Model its ACK as depending on output progress.
        let output_progress = timeout(Duration::from_secs(1), async {
            for _ in 0..ACTOR_MAILBOX_CAPACITY * 3 {
                tx.send(Message::ShellOutput {
                    session_id: session.session_id.as_str().to_owned(),
                    generation: 1,
                    bytes: b"echo\r\n".to_vec(),
                })
                .await
                .expect("queue echo before writer ACK");
            }
        })
        .await;
        assert!(matches!(
            focus_response.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        completion.send(true).expect("writer ACK after echo drain");
        response
            .await
            .unwrap()
            .expect("acknowledged input succeeds");
        let changed = focus_response
            .await
            .unwrap()
            .expect("deferred focus succeeds");
        assert!(changed.target.is_none());
        actor_task.abort();
        assert!(
            output_progress.is_ok(),
            "output backpressure must not prevent writer ACK"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_ack_control_flood_has_a_hard_bound_and_preserves_fifo() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let tx = actor.tx.clone();
        let mut mailbox = ActorMailbox::new(rx);
        let accepted = Arc::new(AtomicUsize::new(0));
        let producer_progress = accepted.clone();
        let producer = tokio::spawn(async move {
            for index in 0..ACTOR_MAILBOX_CAPACITY * 2 + 1 {
                tx.send(Message::ReapStaleAttachments {
                    now_unix_ms: index as i64,
                })
                .await
                .expect("queue bounded control flood");
                producer_progress.store(index + 1, Ordering::Release);
            }
        });
        let (ack, completion) = oneshot::channel();
        let wait = actor.wait_for_writer_ack(completion, Duration::from_secs(2), &mut mailbox);
        let observe_bound = async {
            timeout(Duration::from_secs(1), async {
                while accepted.load(Ordering::Acquire) < ACTOR_MAILBOX_CAPACITY * 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("one mailbox plus one deferred queue can fill");
            tokio::task::yield_now().await;
            assert_eq!(accepted.load(Ordering::Acquire), ACTOR_MAILBOX_CAPACITY * 2);
            assert!(
                !producer.is_finished(),
                "overflow must retain upstream backpressure"
            );
            ack.send(true)
                .expect("ACK is independent of control backpressure");
        };
        let (succeeded, ()) = tokio::join!(wait, observe_bound);
        assert!(succeeded);
        assert_eq!(mailbox.deferred.len(), ACTOR_MAILBOX_CAPACITY);
        assert_eq!(mailbox.rx.len(), ACTOR_MAILBOX_CAPACITY);
        producer.abort();
        let _ = producer.await;
        for index in 0..ACTOR_MAILBOX_CAPACITY * 2 {
            assert!(matches!(mailbox.next().await,
                Some(Message::ReapStaleAttachments { now_unix_ms }) if now_unix_ms == index as i64));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_ack_only_bypasses_running_output_and_preserves_stream_order() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let running = insert_test_session(&mut actor, SshSessionState::Running, 1, 4, None);
        let automating =
            insert_test_session(&mut actor, SshSessionState::AutomatingLogin, 1, 4, None);
        let tx = actor.tx.clone();
        let mut mailbox = ActorMailbox::new(rx);
        mailbox.deferred.push_back(Message::ShellClosed {
            session_id: running.session_id.as_str().to_owned(),
            generation: 1,
            reason: SshSessionCloseReason::UserRequested,
        });
        mailbox.deferred.push_back(Message::ShellOutput {
            session_id: automating.session_id.as_str().to_owned(),
            generation: 1,
            bytes: b"automation".to_vec(),
        });
        mailbox.deferred.push_back(Message::ShellOutput {
            session_id: running.session_id.as_str().to_owned(),
            generation: 1,
            bytes: b"older".to_vec(),
        });
        tx.send(Message::ShellOutput {
            session_id: running.session_id.as_str().to_owned(),
            generation: 1,
            bytes: b"newer".to_vec(),
        })
        .await
        .unwrap();
        let (ack, completion) = oneshot::channel();
        let wait = actor.wait_for_writer_ack(completion, Duration::from_secs(1), &mut mailbox);
        let acknowledge = async {
            while tx.capacity() < ACTOR_MAILBOX_CAPACITY {
                tokio::task::yield_now().await;
            }
            ack.send(true).unwrap();
        };
        let (succeeded, ()) = tokio::join!(wait, acknowledge);
        assert!(succeeded);
        let record = &actor.sessions[running.session_id.as_str()];
        assert_eq!(
            record.summary.state,
            SshSessionState::Running,
            "lifecycle cannot cross ACK"
        );
        let output: Vec<u8> = record
            .output_ring
            .iter()
            .filter_map(|item| match item {
                SshSessionOutputItem::Frame(frame) => Some(frame.bytes.iter().copied()),
                SshSessionOutputItem::Gap(_) => None,
            })
            .flatten()
            .collect();
        assert_eq!(output, b"oldernewer");
        assert!(
            actor.sessions[automating.session_id.as_str()]
                .output_ring
                .is_empty(),
            "automation output may cause control effects and stays deferred"
        );
        assert!(matches!(
            mailbox.deferred.pop_front(),
            Some(Message::ShellClosed { .. })
        ));
        assert!(
            matches!(mailbox.deferred.pop_front(), Some(Message::ShellOutput { session_id, .. }) if session_id == automating.session_id.as_str())
        );
        assert!(mailbox.deferred.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn manual_input_waits_for_writer_ack_and_poison_failure_is_authoritative() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let mut mailbox = ActorMailbox::new(rx);
        let (shell, mut commands) = mpsc::channel(4);
        let session = insert_test_session(&mut actor, SshSessionState::Running, 1, 4, Some(shell));
        let focus = actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &session)),
                OperationId::new(),
                "manual-input-focus",
            ))
            .expect("focus SSH session");
        let lease = focus.lease.expect("focused SSH lease");
        let channel_id = actor.sessions[session.session_id.as_str()]
            .summary
            .channel_id
            .clone()
            .expect("running channel");
        let request = |client_seq, bytes: &[u8]| SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(1),
            channel_id: channel_id.clone(),
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id.clone(),
            input_epoch: lease.input_epoch,
            client_seq: WireSequence::new(client_seq),
            bytes: bytes.to_vec(),
        };

        {
            let (reply, mut response) = oneshot::channel();
            let pending = actor.input_with_ack(request(1, b"acknowledged"), reply, &mut mailbox);
            let acknowledge = async {
                let completion = match commands.recv().await.expect("manual input command") {
                    ShellCommand::Input {
                        bytes, completion, ..
                    } => {
                        assert_eq!(bytes, b"acknowledged");
                        completion
                    }
                    _ => panic!("expected manual input command"),
                };
                assert!(matches!(
                    response.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                completion.send(true).expect("ack manual input");
            };
            tokio::join!(pending, acknowledge);
            response
                .await
                .expect("manual input reply")
                .expect("acknowledged input succeeds");
        }

        {
            let (reply, mut response) = oneshot::channel();
            let pending = actor.input_with_ack(request(2, b"uncertain"), reply, &mut mailbox);
            let reject = async {
                let completion = match commands.recv().await.expect("second manual input command") {
                    ShellCommand::Input {
                        bytes, completion, ..
                    } => {
                        assert_eq!(bytes, b"uncertain");
                        completion
                    }
                    _ => panic!("expected manual input command"),
                };
                assert!(matches!(
                    response.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                completion.send(false).expect("reject manual input");
            };
            tokio::join!(pending, reject);
            response
                .await
                .expect("failed manual input reply")
                .expect_err("writer failure must reject input");
        }
        assert_eq!(
            actor.sessions[session.session_id.as_str()].summary.state,
            SshSessionState::Failed
        );
        assert!(actor.focused_target.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ssh_resize_waits_for_owner_ack_and_failure_does_not_advance_sequence() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let mut mailbox = ActorMailbox::new(rx);
        let (shell, mut commands) = mpsc::channel(4);
        let session = insert_test_session(&mut actor, SshSessionState::Running, 1, 4, Some(shell));
        let focus = actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &session)),
                OperationId::new(),
                "resize-owner-ack-focus",
            ))
            .expect("focus SSH session");
        let lease = focus.lease.expect("focused SSH lease");
        let channel_id = actor.sessions[session.session_id.as_str()]
            .summary
            .channel_id
            .clone()
            .expect("running channel");
        let request = SshSessionResizeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(1),
            channel_id,
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id,
            input_epoch: lease.input_epoch,
            resize_seq: WireSequence::new(1),
            rows: 39,
            cols: 117,
        };

        let (reply, mut response) = oneshot::channel();
        let pending = actor.resize_with_ack(request, reply, &mut mailbox);
        let reject = async {
            let completion = match commands.recv().await.expect("resize owner command") {
                ShellCommand::Resize {
                    size, completion, ..
                } => {
                    assert_eq!(
                        size,
                        PtySize::new(117, 39, 0, 0).expect("expected resize size")
                    );
                    completion
                }
                _ => panic!("expected SSH resize command"),
            };
            assert!(matches!(
                response.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            drop(completion);
        };
        tokio::join!(pending, reject);
        response
            .await
            .expect("resize response")
            .expect_err("owner failure must reject resize");

        let record = &actor.sessions[session.session_id.as_str()];
        assert_eq!(record.last_resize_seq, 0);
        assert_eq!(record.summary.state, SshSessionState::Failed);
        assert!(actor.focused_target.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ssh_resize_holds_actor_order_until_owner_ack_before_focus_change() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let (first_shell, mut first_commands) = mpsc::channel(4);
        let (second_shell, _second_commands) = mpsc::channel(4);
        let first = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            1,
            4,
            Some(first_shell),
        );
        let second = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            1,
            7,
            Some(second_shell),
        );
        let focus = actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &first)),
                OperationId::new(),
                "resize-focus-first",
            ))
            .expect("focus first session");
        let lease = focus.lease.expect("focused SSH lease");
        let channel_id = actor.sessions[first.session_id.as_str()]
            .summary
            .channel_id
            .clone()
            .expect("running channel");
        let second_target = focus_target(&actor, &second);
        let tx = actor.tx.clone();
        let actor_task = tokio::spawn(run_actor(actor, rx));

        let (resize_reply, resize_response) = oneshot::channel();
        tx.send(Message::Resize {
            request: SshSessionResizeRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: first.session_id,
                expected_generation: WireSequence::new(1),
                channel_id,
                attachment_id: first.attachment_id,
                view_id: first.view_id,
                focus_epoch: lease.focus_epoch,
                lease_id: lease.lease_id,
                input_epoch: lease.input_epoch,
                resize_seq: WireSequence::new(1),
                rows: 42,
                cols: 132,
            },
            reply: resize_reply,
        })
        .await
        .expect("queue resize");
        let completion = match first_commands.recv().await.expect("resize owner command") {
            ShellCommand::Resize { completion, .. } => completion,
            _ => panic!("expected SSH resize command"),
        };

        let (focus_reply, mut focus_response) = oneshot::channel();
        tx.send(Message::FocusChange {
            request: focus_change_request(
                1,
                Some(second_target),
                OperationId::new(),
                "focus-after-resize-ack",
            ),
            reply: focus_reply,
        })
        .await
        .expect("queue focus change");
        tokio::task::yield_now().await;
        assert!(matches!(
            focus_response.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        completion.send(true).expect("ack resize");
        resize_response
            .await
            .expect("resize response")
            .expect("acknowledged resize succeeds");
        focus_response
            .await
            .expect("focus response")
            .expect("focus changes only after resize acknowledgement");
        actor_task.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_resize_holds_actor_order_until_owner_ack_before_terminate() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let (process, owner_commands) = std_mpsc::sync_channel(4);
        let (resize_request, target) = actor.local_sessions.insert_resize_test_session(process, 1);
        let session_id = resize_request.session_id.clone();
        let generation = resize_request.expected_generation;
        let state_revision = resize_request.expected_state_revision;
        actor.focus_epoch = 1;
        actor.focused_target = Some(TerminalInputFocusTarget::Local(target));
        let tx = actor.tx.clone();
        let actor_task = tokio::spawn(run_actor(actor, rx));

        let (resize_reply, resize_response) = oneshot::channel();
        tx.send(Message::LocalResize {
            request: resize_request,
            reply: resize_reply,
        })
        .await
        .expect("queue local resize");
        let completion = timeout(Duration::from_secs(1), async {
            loop {
                match owner_commands.try_recv() {
                    Ok(local_terminal::LocalProcessCommand::Resize {
                        rows,
                        cols,
                        completion,
                    }) => {
                        assert_eq!((rows, cols), (43, 133));
                        break completion;
                    }
                    Ok(_) => panic!("expected local resize command"),
                    Err(std_mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(std_mpsc::TryRecvError::Disconnected) => {
                        panic!("local owner closed before resize")
                    }
                }
            }
        })
        .await
        .expect("local resize reaches owner");

        let (terminate_reply, mut terminate_response) = oneshot::channel();
        tx.send(Message::LocalTerminate {
            request: LocalSessionTerminateRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "terminate-after-resize-ack".to_owned(),
                session_id,
                expected_generation: generation,
                expected_state_revision: state_revision,
            },
            reply: terminate_reply,
        })
        .await
        .expect("queue terminate");
        tokio::task::yield_now().await;
        assert!(matches!(
            terminate_response.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            owner_commands.try_recv(),
            Err(std_mpsc::TryRecvError::Empty)
        ));

        completion.send(true).expect("ack local resize");
        resize_response
            .await
            .expect("local resize response")
            .expect("acknowledged local resize succeeds");
        terminate_response
            .await
            .expect("terminate response")
            .expect("terminate follows resize acknowledgement");
        assert!(matches!(
            owner_commands.recv_timeout(Duration::from_secs(1)),
            Ok(local_terminal::LocalProcessCommand::Terminate)
        ));
        actor_task.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_resize_owner_failure_does_not_advance_sequence() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let mut mailbox = ActorMailbox::new(rx);
        let (process, owner_commands) = std_mpsc::sync_channel(4);
        let (request, target) = actor.local_sessions.insert_resize_test_session(process, 1);
        let session_id = request.session_id.clone();
        actor.focus_epoch = 1;
        actor.focused_target = Some(TerminalInputFocusTarget::Local(target));

        let (reply, mut response) = oneshot::channel();
        let pending = actor.local_resize_with_ack(request, reply, &mut mailbox);
        let reject = async {
            let completion = loop {
                match owner_commands.try_recv() {
                    Ok(local_terminal::LocalProcessCommand::Resize { completion, .. }) => {
                        break completion;
                    }
                    Ok(_) => panic!("expected local resize command"),
                    Err(std_mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(std_mpsc::TryRecvError::Disconnected) => {
                        panic!("local owner closed before resize")
                    }
                }
            };
            assert!(matches!(
                response.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            completion.send(false).expect("reject local resize");
        };
        tokio::join!(pending, reject);
        response
            .await
            .expect("local resize response")
            .expect_err("owner failure must reject local resize");

        assert_eq!(
            actor
                .local_sessions
                .resize_seq_for_test(session_id.as_str()),
            Some(0)
        );
        let summary = actor
            .local_sessions
            .snapshot()
            .sessions
            .into_iter()
            .find(|summary| summary.session_id == session_id)
            .expect("local test session");
        assert_eq!(summary.state, norishell_core_api::LocalSessionState::Failed);
        assert!(actor.focused_target.is_none());
        assert!(matches!(
            owner_commands.try_recv(),
            Ok(local_terminal::LocalProcessCommand::FailAndTerminate(_))
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn approved_plugin_input_freezes_focus_until_the_dedicated_writer_ack() {
        let (_directory, mut actor, _live_sessions, rx) = actor_fixture();
        let (first_shell, mut first_commands) = mpsc::channel(4);
        let (second_shell, _second_commands) = mpsc::channel(4);
        let first = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            3,
            8,
            Some(first_shell),
        );
        let second = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            4,
            9,
            Some(second_shell),
        );
        let focus = actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &first)),
                OperationId::new(),
                "focus-plugin-target",
            ))
            .expect("focus plugin target");
        let lease = focus.lease.expect("focused lease");
        let channel_id = actor.sessions[first.session_id.as_str()]
            .summary
            .channel_id
            .clone()
            .expect("running channel");
        let approved = ApprovedPluginInput {
            request_id: RequestId::new(),
            session_id: first.session_id.clone(),
            expected_generation: WireSequence::new(3),
            channel_id,
            attachment_id: first.attachment_id.clone(),
            view_id: first.view_id.clone(),
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id.clone(),
            input_epoch: lease.input_epoch,
            bytes: b"printf approved\r".to_vec(),
            approval_fence: None,
        };
        let mut revoked = approved.clone();
        revoked.approval_fence = Some(Arc::new(|| false));
        assert!(actor.queue_approved_plugin_input(&revoked).is_err());
        assert!(
            first_commands.try_recv().is_err(),
            "revoked permission must never reach the writer"
        );
        let second_target = focus_target(&actor, &second);
        let tx = actor.tx.clone();
        let actor_task = tokio::spawn(run_actor(actor, rx));
        let (approved_reply, approved_response) = oneshot::channel();
        tx.send(Message::ApprovedPluginInput {
            request: approved.clone(),
            reply: approved_reply,
        })
        .await
        .expect("queue approved input");
        let completion = match first_commands.recv().await {
            Some(ShellCommand::ApprovedPluginInput {
                bytes, completion, ..
            }) if bytes == b"printf approved\r" => completion,
            _ => panic!("dedicated approved input command"),
        };

        let (focus_reply, mut focus_response) = oneshot::channel();
        tx.send(Message::FocusChange {
            request: focus_change_request(
                1,
                Some(second_target),
                OperationId::new(),
                "focus-away-from-plugin-target",
            ),
            reply: focus_reply,
        })
        .await
        .expect("queue focus change");
        tokio::task::yield_now().await;
        assert!(
            matches!(
                focus_response.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "focus mutation must remain behind the in-flight approved write"
        );

        completion.send(true).expect("ack approved write");
        approved_response
            .await
            .expect("approved response")
            .expect("approved write succeeds");
        focus_response
            .await
            .expect("focus response")
            .expect("focus moves after the write acknowledgement");
        actor_task.abort();
    }

    #[test]
    fn plugin_observe_starts_without_ring_replay_strips_terminal_controls_and_hides_automation() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let (shell, _commands) = mpsc::channel(4);
        let session = insert_test_session(&mut actor, SshSessionState::Running, 5, 11, Some(shell));
        actor.shell_output(
            session.session_id.as_str(),
            5,
            b"before attach must stay private\n".to_vec(),
        );
        let focus = actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &session)),
                OperationId::new(),
                "focus-plugin-observe",
            ))
            .expect("focus observed terminal");
        let (sink, mut observations) = mpsc::channel(4);
        actor
            .plugin_observe_attach(ApprovedPluginObservationAttach {
                request_id: RequestId::new(),
                observer_id: Uuid::new_v4(),
                focus_epoch: focus.focus_epoch,
                target: focus.target.expect("focused target"),
                sink,
            })
            .expect("attach plugin observer");
        assert!(observations.try_recv().is_err(), "no history is replayed");

        actor.shell_output(
            session.session_id.as_str(),
            5,
            b"\x1b[31mhello \xe4\xb8".to_vec(),
        );
        actor.shell_output(session.session_id.as_str(), 5, b"\x96\x1b[0m\n".to_vec());
        assert!(matches!(
            observations.try_recv(),
            Ok(PluginTerminalObservation::Text { text, .. }) if text == "hello "
        ));
        assert!(matches!(
            observations.try_recv(),
            Ok(PluginTerminalObservation::Text { text, .. }) if text == "世\n"
        ));

        actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("session")
            .summary
            .state = SshSessionState::AutomatingLogin;
        actor.shell_output(
            session.session_id.as_str(),
            5,
            b"authentication and automation output\n".to_vec(),
        );
        assert!(
            observations.try_recv().is_err(),
            "automation output never enters plugin projection"
        );
    }

    #[test]
    fn focus_change_replays_exactly_clears_blank_and_handles_fast_a_b_a() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let (first_shell, _first_commands) = mpsc::channel(4);
        let (second_shell, _second_commands) = mpsc::channel(4);
        let first = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            1,
            3,
            Some(first_shell),
        );
        let second = insert_test_session(
            &mut actor,
            SshSessionState::Running,
            1,
            5,
            Some(second_shell),
        );
        let operation_id = OperationId::new();
        let request = focus_change_request(
            0,
            Some(focus_target(&actor, &first)),
            operation_id.clone(),
            "focus-replay",
        );
        let first_response = actor.focus_change(request.clone()).expect("focus A");
        let replay = actor
            .focus_change(SshTerminalInputFocusChangeRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                ..request.clone()
            })
            .expect("exact replay");
        assert_eq!(replay, first_response);
        assert_eq!(actor.focus_epoch, 1);
        let changed = actor
            .focus_change(SshTerminalInputFocusChangeRequest {
                target: Some(focus_target(&actor, &second)),
                ..request
            })
            .expect_err("same operation with another target must conflict");
        assert_eq!(changed.code, "ssh_terminal.stale_fence");

        actor
            .focus_change(focus_change_request(
                1,
                Some(focus_target(&actor, &second)),
                OperationId::new(),
                "focus-b",
            ))
            .expect("focus B");
        let final_a = actor
            .focus_change(focus_change_request(
                2,
                Some(focus_target(&actor, &first)),
                OperationId::new(),
                "focus-a-again",
            ))
            .expect("focus A again");
        assert_eq!(final_a.focus_epoch, WireSequence::new(3));
        assert_eq!(
            final_a.target.as_ref().map(|target| &target.session_id),
            Some(&first.session_id)
        );
        let old_lease = final_a.lease.expect("final A lease");

        let cleared = actor
            .focus_change(focus_change_request(
                3,
                None,
                OperationId::new(),
                "focus-clear-blank",
            ))
            .expect("clear focus for blank tab");
        assert_eq!(cleared.focus_epoch, WireSequence::new(4));
        assert!(cleared.target.is_none());
        assert!(cleared.lease.is_none());
        let stale_renew = actor
            .lease_renew(SshSessionInputLeaseRenewRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: first.session_id,
                expected_generation: WireSequence::new(1),
                attachment_id: first.attachment_id,
                view_id: first.view_id,
                focus_epoch: old_lease.focus_epoch,
                lease_id: old_lease.lease_id,
                input_epoch: old_lease.input_epoch,
            })
            .expect_err("cleared focus rejects old renewal");
        assert_eq!(stale_renew.code, "ssh_terminal.stale_focus");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_replay_is_exact_and_changed_parameters_conflict() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let credential = actor
            .transient_credentials
            .prepare(TransientCredentialPrepareRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "prepare-open-password".to_owned(),
                kind: CredentialKind::Password,
                secret: "one-time-password".to_owned(),
                passphrase: None,
            })
            .expect("transient credential");
        let operation_id = OperationId::new();
        let request = SshSessionOpenRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: operation_id.clone(),
            idempotency_key: "open-once".to_owned(),
            open_attempt_id: SshOpenAttemptId::new(),
            attach_attempt_id: SshAttachAttemptId::new(),
            target: SshSessionTarget::QuickConnect {
                endpoint: SshSessionEndpoint {
                    address: "127.0.0.1".to_owned(),
                    port: 9,
                    username: Some("acceptance".to_owned()),
                },
            },
            credential_ref_id: Some(credential.credential_ref_id),
            plugin_authorization_token: None,
            view_id: SshViewId::new(),
            rows: 30,
            cols: 100,
        };

        let first = actor
            .open(request.clone(), Channel::<SshSessionEvent>::new(|_| Ok(())))
            .expect("open accepted");
        let replay = actor
            .open(
                SshSessionOpenRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    ..request.clone()
                },
                Channel::<SshSessionEvent>::new(|_| Ok(())),
            )
            .expect("exact open replay");

        assert_eq!(replay, first);
        assert_eq!(actor.sessions.len(), 1);

        let conflict_request_id = RequestId::new();
        let conflict = actor
            .open(
                SshSessionOpenRequest {
                    meta: RequestMeta {
                        request_id: conflict_request_id.clone(),
                    },
                    rows: 31,
                    ..request
                },
                Channel::<SshSessionEvent>::new(|_| Ok(())),
            )
            .expect_err("changed open fingerprint must conflict");
        assert_eq!(conflict.code, "ssh_terminal.stale_fence");
        assert_eq!(conflict.request_id, Some(conflict_request_id));
        assert_eq!(actor.sessions.len(), 1);
    }

    #[test]
    fn attach_and_detach_replays_do_not_repeat_attachment_side_effects() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Running, 2, 4, None);
        let attach_operation_id = OperationId::new();
        let attach_request = SshSessionAttachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: attach_operation_id,
            idempotency_key: "attach-once".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            expected_state_revision: WireSequence::new(4),
            attach_attempt_id: SshAttachAttemptId::new(),
            view_id: SshViewId::new(),
            after_output_seq: None,
        };
        let attached = actor
            .attach(
                attach_request.clone(),
                Channel::<SshSessionEvent>::new(|_| Ok(())),
            )
            .expect("attach accepted");
        let attach_replay = actor
            .attach(
                SshSessionAttachRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    ..attach_request
                },
                Channel::<SshSessionEvent>::new(|_| Ok(())),
            )
            .expect("attach replay");
        assert_eq!(attach_replay, attached);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .attachments
                .len(),
            2
        );

        let detach_operation_id = OperationId::new();
        let detach_request = SshSessionDetachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: detach_operation_id,
            idempotency_key: "detach-once".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            expected_state_revision: WireSequence::new(4),
            attachment_id: attached.attachment.attachment_id,
            view_id: attached.attachment.view_id,
            intent: SshSessionDetachIntent::UserClose,
            confirmation: None,
        };
        let detached = actor
            .detach(detach_request.clone())
            .expect("detach accepted");
        let detach_replay = actor
            .detach(SshSessionDetachRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                ..detach_request
            })
            .expect("detach replay");
        assert_eq!(detach_replay, detached);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .attachments
                .len(),
            1
        );
    }

    #[test]
    fn renderer_rebind_replaces_channel_releases_lease_and_replays_without_swapping_again() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Running, 2, 4, None);
        let old_events = Arc::new(AtomicUsize::new(0));
        let old_events_sink = old_events.clone();
        actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("session")
            .attachments
            .get_mut(session.attachment_id.as_str())
            .expect("attachment")
            .events = Channel::new(move |event| {
            let _ = event;
            old_events_sink.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let lease = actor
            .lease_acquire(SshSessionInputLeaseAcquireRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "lease-before-rebind".to_owned(),
                session_id: session.session_id.clone(),
                expected_generation: WireSequence::new(2),
                expected_state_revision: WireSequence::new(4),
                attachment_id: session.attachment_id.clone(),
                view_id: session.view_id.clone(),
            })
            .expect("lease");

        let new_events = Arc::new(AtomicUsize::new(0));
        let new_events_sink = new_events.clone();
        let request = SshSessionAttachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "renderer-rebind".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            expected_state_revision: WireSequence::new(4),
            attach_attempt_id: SshAttachAttemptId::new(),
            view_id: session.view_id.clone(),
            after_output_seq: None,
        };
        let rebound = actor
            .attach(
                request.clone(),
                Channel::new(move |event| {
                    let _ = event;
                    new_events_sink.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            )
            .expect("renderer rebind");

        assert_ne!(rebound.attachment.attachment_id, session.attachment_id);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .attachments
                .len(),
            1
        );
        assert!(
            actor.sessions[session.session_id.as_str()]
                .input_lease
                .is_none()
        );
        assert_ne!(lease.attachment_id, rebound.attachment.attachment_id);

        old_events.store(0, Ordering::SeqCst);
        new_events.store(0, Ordering::SeqCst);
        actor.shell_output(session.session_id.as_str(), 2, b"fresh-channel".to_vec());
        assert_eq!(old_events.load(Ordering::SeqCst), 0);
        assert_eq!(new_events.load(Ordering::SeqCst), 1);

        let replay_events = Arc::new(AtomicUsize::new(0));
        let replay_events_sink = replay_events.clone();
        let replay = actor
            .attach(
                SshSessionAttachRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    ..request.clone()
                },
                Channel::new(move |event| {
                    let _ = event;
                    replay_events_sink.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            )
            .expect("exact rebind replay");
        assert_eq!(replay, rebound);
        new_events.store(0, Ordering::SeqCst);
        actor.shell_output(
            session.session_id.as_str(),
            2,
            b"still-first-channel".to_vec(),
        );
        assert_eq!(new_events.load(Ordering::SeqCst), 1);
        assert_eq!(replay_events.load(Ordering::SeqCst), 0);

        let conflict = actor
            .attach(
                SshSessionAttachRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    after_output_seq: Some(WireSequence::new(1)),
                    ..request
                },
                Channel::<SshSessionEvent>::new(|_| Ok(())),
            )
            .expect_err("changed replay fingerprint must fail closed");
        assert_eq!(conflict.code, "ssh_terminal.stale_fence");
    }

    #[test]
    fn renderer_release_and_timeout_reclaim_attachments_without_disconnecting_shell() {
        let (_directory, mut actor, live_sessions, _rx) = actor_fixture();
        let (shell_tx, _shell_rx) = mpsc::channel(SHELL_MAILBOX_CAPACITY);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 3, 7, Some(shell_tx));
        live_sessions
            .lock()
            .expect("live sessions")
            .insert(session.session_id.as_str().to_owned());
        let released = actor
            .detach(SshSessionDetachRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "renderer-unload".to_owned(),
                session_id: session.session_id.clone(),
                expected_generation: WireSequence::new(3),
                expected_state_revision: WireSequence::new(7),
                attachment_id: session.attachment_id,
                view_id: session.view_id,
                intent: SshSessionDetachIntent::RendererUnavailable,
                confirmation: None,
            })
            .expect("renderer release");
        assert!(matches!(released, SshSessionDetachResult::Detached { .. }));
        let record = &actor.sessions[session.session_id.as_str()];
        assert_eq!(record.summary.attachment_count, 0);
        assert_eq!(record.summary.state, SshSessionState::Running);
        assert!(record.shell.is_some());
        assert!(
            live_sessions
                .lock()
                .expect("live sessions")
                .contains(session.session_id.as_str())
        );

        let timeout_session = insert_test_session(&mut actor, SshSessionState::Running, 4, 8, None);
        let record = actor
            .sessions
            .get_mut(timeout_session.session_id.as_str())
            .expect("timeout session");
        record
            .attachments
            .get_mut(timeout_session.attachment_id.as_str())
            .expect("timeout attachment")
            .last_seen_at_unix_ms = 1;
        actor.reap_stale_attachments(ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS + 1);
        let record = &actor.sessions[timeout_session.session_id.as_str()];
        assert_eq!(record.summary.attachment_count, 0);
        assert_eq!(record.summary.state, SshSessionState::Running);
    }

    #[test]
    fn heartbeat_reclaim_preserves_shell_blocker_and_fresh_attach_replays_real_gap() {
        let (_directory, mut actor, live_sessions, _rx) = actor_fixture();
        let (shell_tx, mut shell_rx) = mpsc::channel(SHELL_MAILBOX_CAPACITY);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 6, 11, Some(shell_tx));
        live_sessions
            .lock()
            .expect("live sessions")
            .insert(session.session_id.as_str().to_owned());
        let service_projection = SshSessionService {
            plugin_metadata_events: actor.plugin_metadata_events.clone(),
            tx: actor.tx.clone(),
            focus_broker: crate::terminal_focus_broker::TerminalFocusBroker::default(),
            live_sessions: live_sessions.clone(),
            live_local_sessions: Arc::new(Mutex::new(BTreeSet::new())),
        };

        actor.shell_output(
            session.session_id.as_str(),
            6,
            vec![b'g'; OUTPUT_RING_MAX_BYTES + SSH_TERMINAL_OUTPUT_FRAME_MAX_BYTES],
        );
        let record = actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("running session");
        record
            .attachments
            .get_mut(session.attachment_id.as_str())
            .expect("initial attachment")
            .last_seen_at_unix_ms = 1;
        actor.reap_stale_attachments(ATTACHMENT_HEARTBEAT_TIMEOUT_MILLIS + 1);

        let record = &actor.sessions[session.session_id.as_str()];
        assert_eq!(record.summary.state, SshSessionState::Running);
        assert_eq!(record.summary.attachment_count, 0);
        assert!(record.shell.is_some());
        assert!(matches!(
            shell_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(
            service_projection.exit_blockers(),
            vec![session.session_id.clone()]
        );

        let events = Arc::new(AtomicUsize::new(0));
        let events_sink = events.clone();
        let attached = actor
            .attach(
                SshSessionAttachRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: "attach-after-heartbeat-reclaim".to_owned(),
                    session_id: session.session_id.clone(),
                    expected_generation: WireSequence::new(6),
                    expected_state_revision: WireSequence::new(11),
                    attach_attempt_id: SshAttachAttemptId::new(),
                    view_id: SshViewId::new(),
                    after_output_seq: None,
                },
                Channel::new(move |event| {
                    let _ = event;
                    events_sink.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            )
            .expect("attach fresh renderer after heartbeat reclaim");
        assert!(matches!(
            attached.replay.first(),
            Some(SshSessionOutputItem::Gap(_))
        ));
        assert!(
            attached
                .replay
                .iter()
                .any(|item| matches!(item, SshSessionOutputItem::Frame(_)))
        );
        let gap = attached
            .replay
            .iter()
            .find_map(|item| match item {
                SshSessionOutputItem::Gap(gap) => Some(gap),
                SshSessionOutputItem::Frame(_) => None,
            })
            .expect("ring overflow gap");
        assert_eq!(gap.reason, SshSessionOutputGapReason::RingBufferOverflow);

        events.store(0, Ordering::SeqCst);
        actor.shell_output(
            session.session_id.as_str(),
            6,
            b"fresh-live-channel".to_vec(),
        );
        assert!(events.load(Ordering::SeqCst) >= 1);
        assert!(matches!(
            shell_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn attachment_heartbeat_rejects_old_generation_and_revision() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Running, 5, 9, None);
        let attachment_revision = actor.sessions[session.session_id.as_str()].attachments
            [session.attachment_id.as_str()]
        .summary
        .attachment_revision;
        let request = SshSessionAttachmentHeartbeatRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(5),
            expected_attachment_revision: attachment_revision,
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
        };
        actor
            .attachment_heartbeat(request.clone())
            .expect("current heartbeat");
        let stale_generation = actor
            .attachment_heartbeat(SshSessionAttachmentHeartbeatRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                expected_generation: WireSequence::new(4),
                ..request.clone()
            })
            .expect_err("old generation heartbeat");
        assert_eq!(stale_generation.code, "ssh_terminal.stale_fence");
        let stale_revision = actor
            .attachment_heartbeat(SshSessionAttachmentHeartbeatRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                expected_attachment_revision: WireSequence::new(
                    attachment_revision.get().saturating_add(1),
                ),
                ..request
            })
            .expect_err("old attachment revision heartbeat");
        assert_eq!(stale_revision.code, "ssh_terminal.stale_fence");
    }

    #[test]
    fn host_key_decision_replay_resolves_the_challenge_only_once() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(
            &mut actor,
            SshSessionState::AwaitingHostKeyDecision,
            2,
            4,
            None,
        );
        let challenge_id = SshHostKeyChallengeId::new();
        let challenge = SshHostKeyChallenge {
            challenge_id: challenge_id.clone(),
            session_id: session.session_id.clone(),
            generation: WireSequence::new(2),
            state_revision: WireSequence::new(4),
            endpoint: SshSessionEndpoint {
                address: "127.0.0.1".to_owned(),
                port: 9,
                username: Some("acceptance".to_owned()),
            },
            key_algorithm: "ssh-ed25519".to_owned(),
            fingerprint_sha256: "SHA256:test".to_owned(),
        };
        let (decision_tx, decision_rx) = oneshot::channel();
        actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("session")
            .active_challenge = Some(ActiveChallenge {
            challenge,
            observed: ObservedHostKey {
                algorithm: "ssh-ed25519".to_owned(),
                public_key_blob: vec![1, 2, 3],
                fingerprint_sha256: "SHA256:test".to_owned(),
            },
            decision: decision_tx,
        });
        let request = SshHostKeyDecisionRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "reject-host-key-once".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            challenge_id,
            expected_state_revision: WireSequence::new(4),
            attachment_id: session.attachment_id,
            view_id: session.view_id,
            decision: SshHostKeyDecision::Reject,
        };
        let first = actor
            .host_key_decide(request.clone())
            .expect("decision accepted");
        let replay = actor
            .host_key_decide(SshHostKeyDecisionRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                ..request
            })
            .expect("decision replay");

        assert_eq!(replay, first);
        assert!(
            actor.sessions[session.session_id.as_str()]
                .active_challenge
                .is_none()
        );
        assert!(matches!(
            decision_rx.blocking_recv().expect("decision delivered"),
            Ok(HostKeyDecision::Rejected)
        ));
    }

    #[test]
    fn lease_acquire_replay_does_not_issue_a_second_lease() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Running, 2, 4, None);
        let operation_id = OperationId::new();
        let request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id,
            idempotency_key: "lease-once".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            expected_state_revision: WireSequence::new(4),
            attachment_id: session.attachment_id.clone(),
            view_id: session.view_id.clone(),
        };
        let first = actor
            .lease_acquire(request.clone())
            .expect("lease acquired");
        let event_seq_after_first = actor.sessions[session.session_id.as_str()]
            .summary
            .event_seq;
        let replay = actor
            .lease_acquire(SshSessionInputLeaseAcquireRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                ..request.clone()
            })
            .expect("lease replay");

        assert_eq!(replay, first);
        let record = &actor.sessions[session.session_id.as_str()];
        assert_eq!(record.next_input_epoch, 1);
        assert_eq!(record.summary.event_seq, event_seq_after_first);

        let conflict_request_id = RequestId::new();
        let conflict = actor
            .lease_acquire(SshSessionInputLeaseAcquireRequest {
                meta: RequestMeta {
                    request_id: conflict_request_id.clone(),
                },
                view_id: SshViewId::new(),
                ..request
            })
            .expect_err("changed lease fingerprint must conflict");
        assert_eq!(conflict.code, "ssh_terminal.stale_fence");
        assert_eq!(conflict.request_id, Some(conflict_request_id));
        assert_eq!(
            actor.sessions[session.session_id.as_str()].next_input_epoch,
            1
        );
    }

    #[test]
    fn disconnect_replay_queues_only_one_shell_disconnect() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let (shell_tx, mut shell_rx) = mpsc::channel(SHELL_MAILBOX_CAPACITY);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 2, 4, Some(shell_tx));
        let request = SshSessionDisconnectRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "disconnect-once".to_owned(),
            session_id: session.session_id.clone(),
            expected_generation: WireSequence::new(2),
            expected_state_revision: WireSequence::new(4),
        };
        let first = actor
            .disconnect(request.clone())
            .expect("disconnect accepted");
        let replay = actor
            .disconnect(SshSessionDisconnectRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                ..request
            })
            .expect("disconnect replay");

        assert_eq!(replay, first);
        assert_eq!(
            actor.sessions[session.session_id.as_str()]
                .summary
                .state_revision,
            WireSequence::new(5)
        );
        assert!(matches!(
            shell_rx.try_recv(),
            Ok(ShellCommand::Disconnect(
                SshSessionCloseReason::UserRequested
            ))
        ));
        assert!(matches!(
            shell_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_aborts_the_live_connect_task_and_discards_transient_credentials() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Resolving, 1, 1, None);
        let transient_credentials = actor.transient_credentials.clone();
        let credential_ref_id = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "abort-connect-transient",
        );
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind stalled handshake listener");
        let port = listener.local_addr().expect("listener address").port();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        let stalled_server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept connect task");
            let _ = accepted_tx.send(());
            std::future::pending::<()>().await;
        });

        actor.start_connect(ConnectPlan {
            session_id: session.session_id.as_str().to_owned(),
            generation: 1,
            endpoint: SshSessionEndpoint {
                address: "127.0.0.1".to_owned(),
                port,
                username: Some(NETWORK_TEST_USER.to_owned()),
            },
            username: NETWORK_TEST_USER.to_owned(),
            ingress: ResolvedRouteIngress::DirectTcp,
            jump_hosts: Vec::new(),
            credentials: vec![ConnectionCredential::Transient(credential_ref_id.clone())],
            algorithm_policy: test_algorithm_policy(),
            heartbeat_policy: test_heartbeat_policy(),
            login_automation: None,
            automation_attachment_id: SshAttachmentId::new(),
            automation_view_id: norishell_core_api::SshViewId::new(),
            profile_revision_token: "test-abort-connect".to_owned(),
            rows: 24,
            cols: 80,
        });
        timeout(Duration::from_secs(2), accepted_rx)
            .await
            .expect("connect task reaches listener")
            .expect("accept notification");
        assert!(transient_credentials.contains(&credential_ref_id));

        let details = actor
            .disconnect(SshSessionDisconnectRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "abort-live-connect".to_owned(),
                session_id: session.session_id.clone(),
                expected_generation: WireSequence::new(1),
                expected_state_revision: WireSequence::new(1),
            })
            .expect("disconnect live connect task");
        assert_eq!(details.session.state, SshSessionState::Closed);
        assert!(
            !actor
                .connect_tasks
                .contains_key(session.session_id.as_str())
        );
        timeout(Duration::from_secs(2), async {
            while transient_credentials.contains(&credential_ref_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted task drops transient cleanup guard");
        stalled_server.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_before_first_connect_poll_discards_transient_credentials() {
        let (_directory, mut actor, _live_sessions, _rx) = actor_fixture();
        let session = insert_test_session(&mut actor, SshSessionState::Resolving, 1, 1, None);
        let transient_credentials = actor.transient_credentials.clone();
        let credential_ref_id = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "abort-before-first-poll",
        );

        actor.start_connect(ConnectPlan {
            session_id: session.session_id.as_str().to_owned(),
            generation: 1,
            endpoint: SshSessionEndpoint {
                address: "127.0.0.1".to_owned(),
                port: 9,
                username: Some(NETWORK_TEST_USER.to_owned()),
            },
            username: NETWORK_TEST_USER.to_owned(),
            ingress: ResolvedRouteIngress::DirectTcp,
            jump_hosts: Vec::new(),
            credentials: vec![ConnectionCredential::Transient(credential_ref_id.clone())],
            algorithm_policy: test_algorithm_policy(),
            heartbeat_policy: test_heartbeat_policy(),
            login_automation: None,
            automation_attachment_id: SshAttachmentId::new(),
            automation_view_id: norishell_core_api::SshViewId::new(),
            profile_revision_token: "test-abort-before-first-poll".to_owned(),
            rows: 24,
            cols: 80,
        });
        assert!(transient_credentials.contains(&credential_ref_id));

        let details = actor
            .disconnect(SshSessionDisconnectRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "abort-before-first-poll".to_owned(),
                session_id: session.session_id.clone(),
                expected_generation: WireSequence::new(1),
                expected_state_revision: WireSequence::new(1),
            })
            .expect("disconnect before the connect future is polled");
        assert_eq!(details.session.state, SshSessionState::Closed);
        assert!(
            !actor
                .connect_tasks
                .contains_key(session.session_id.as_str())
        );
        timeout(Duration::from_secs(2), async {
            while transient_credentials.contains(&credential_ref_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborting an unpolled task drops the captured cleanup guard");
    }

    #[test]
    fn locked_proxy_secret_maps_to_ingress_unlock_instead_of_target_authentication() {
        let directory = tempfile::tempdir().expect("temporary locked vault");
        let vault = VaultService::start(directory.path());
        let authentication = crate::connection_profile::ResolvedProxyAuthentication {
            username: "proxy-user".to_owned(),
            credential: Box::new(CredentialRecord {
                credential_ref_id: CredentialRefId::new(),
                identity_id: IdentityId::new(),
                method: AuthenticationMethodKind::Password,
                details: CredentialRecordDetails::Password {
                    secret_ref_id: SecretRefId::new(),
                },
                priority: 0,
                label: "Proxy password".to_owned(),
                state_version: WireSequence::new(1),
                import_operation_id: None,
                import_idempotency_key: None,
                import_state: CredentialImportState::Ready,
            }),
        };
        let error =
            crate::ssh_connection_orchestrator::proxy_credentials_from(&vault, authentication)
                .expect_err("locked Vault must not expose proxy credentials");
        let reason = failure_from_route(error, Some(SshSessionRouteStage::Ingress));
        assert_eq!(reason.code, SshSessionFailureCode::ProxyCredentialLocked);
        assert_eq!(reason.stage, SshSessionFailureStage::RouteIngress);
        assert_eq!(reason.route_stage, Some(SshSessionRouteStage::Ingress));
        assert_eq!(reason.retry_strategy, SshSessionRetryStrategy::UnlockVault);
        assert_eq!(
            reason.message_key,
            "errors.sshSession.proxyCredentialLocked"
        );
    }

    #[test]
    fn unavailable_proxy_secret_stays_an_ingress_configuration_failure() {
        let reason = failure_from_route(
            TransportError::RouteIngress(RouteIngressError {
                stage: IngressStage::Configuration,
                kind: IngressFailureKind::CredentialUnavailable,
            }),
            Some(SshSessionRouteStage::Ingress),
        );

        assert_eq!(
            reason.code,
            SshSessionFailureCode::ProxyCredentialUnavailable
        );
        assert_eq!(reason.stage, SshSessionFailureStage::RouteIngress);
        assert_eq!(reason.route_stage, Some(SshSessionRouteStage::Ingress));
        assert_eq!(reason.retry_strategy, SshSessionRetryStrategy::RetryOpen);
        assert_eq!(
            reason.message_key,
            "errors.sshSession.proxyCredentialUnavailable"
        );
    }

    #[test]
    fn connection_failure_after_disconnect_request_converges_to_user_closed() {
        let (_directory, mut actor, live_sessions, _rx) = actor_fixture();
        let (shell_tx, shell_rx) = mpsc::channel(SHELL_MAILBOX_CAPACITY);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 2, 4, Some(shell_tx));
        live_sessions
            .lock()
            .expect("live sessions")
            .insert(session.session_id.as_str().to_owned());
        let service_projection = SshSessionService {
            plugin_metadata_events: actor.plugin_metadata_events.clone(),
            tx: actor.tx.clone(),
            focus_broker: crate::terminal_focus_broker::TerminalFocusBroker::default(),
            live_sessions: live_sessions.clone(),
            live_local_sessions: Arc::new(Mutex::new(BTreeSet::new())),
        };

        let events = Arc::new(AtomicUsize::new(0));
        let events_sink = events.clone();
        actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("session")
            .attachments
            .get_mut(session.attachment_id.as_str())
            .expect("attachment")
            .events = Channel::new(move |event| {
            let _ = event;
            events_sink.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        actor
            .focus_change(focus_change_request(
                0,
                Some(focus_target(&actor, &session)),
                OperationId::new(),
                "focus-before-disconnect-race",
            ))
            .expect("focus running session");

        actor
            .disconnect(SshSessionDisconnectRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "disconnect-before-connection-failure".to_owned(),
                session_id: session.session_id.clone(),
                expected_generation: WireSequence::new(2),
                expected_state_revision: WireSequence::new(4),
            })
            .expect("disconnect accepted");
        let disconnecting = &actor.sessions[session.session_id.as_str()];
        assert_eq!(disconnecting.summary.state, SshSessionState::Disconnecting);
        assert_eq!(shell_rx.len(), 1, "disconnect command remains queued");
        let disconnecting_state_revision = disconnecting.summary.state_revision;
        let disconnecting_event_seq = disconnecting.summary.event_seq;
        let delivered_events = events.load(Ordering::SeqCst);

        actor.handle(Message::ConnectionFailed {
            session_id: session.session_id.as_str().to_owned(),
            generation: 2,
            error: TransportError::ConnectionLost,
            route_stage: None,
        });

        let closed = &actor.sessions[session.session_id.as_str()];
        assert_eq!(closed.summary.state, SshSessionState::Closed);
        assert_eq!(
            closed.summary.close_reason,
            Some(SshSessionCloseReason::UserRequested)
        );
        assert!(closed.summary.failure_reason.is_none());
        assert_eq!(
            closed.summary.state_revision,
            WireSequence::new(disconnecting_state_revision.get() + 1)
        );
        assert_eq!(
            closed.summary.event_seq,
            WireSequence::new(disconnecting_event_seq.get() + 1)
        );
        assert_eq!(events.load(Ordering::SeqCst), delivered_events + 1);
        assert!(closed.shell.is_none());
        assert!(closed.input_lease.is_none());
        assert!(actor.focused_target.is_none());
        assert!(service_projection.exit_blockers().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_creates_a_fresh_generation_and_rejects_old_input_fences() {
        let directory = tempfile::tempdir().expect("temporary application data");
        let hosts = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        let native_terminal = NativeTerminalService::start(directory.path(), vault.clone());
        let transient_credentials = TransientCredentialService::default();
        let replacement = transient_credentials
            .prepare(TransientCredentialPrepareRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "prepare-reconnect-password".to_owned(),
                kind: CredentialKind::Password,
                secret: "replacement-password".to_owned(),
                passphrase: None,
            })
            .expect("replacement credential");

        let (tx, mut rx) = mpsc::channel(ACTOR_MAILBOX_CAPACITY);
        let live_sessions = Arc::new(Mutex::new(BTreeSet::new()));
        let live_local_sessions = Arc::new(Mutex::new(BTreeSet::new()));
        let mut actor = Actor::new(
            tx,
            hosts,
            vault,
            transient_credentials,
            crate::ssh_agent_service::SshAgentService::default(),
            native_terminal,
            live_sessions.clone(),
            live_local_sessions,
            broadcast::channel(64).0,
        );

        let session_id = SshSessionId::new();
        let attachment_id = SshAttachmentId::new();
        let attach_attempt_id = SshAttachAttemptId::new();
        let view_id = SshViewId::new();
        let old_lease_id = SshInputLeaseId::new();
        let target = SshSessionTarget::QuickConnect {
            endpoint: SshSessionEndpoint {
                address: "127.0.0.1".to_owned(),
                port: 9,
                username: Some("acceptance".to_owned()),
            },
        };
        let summary = SshSessionSummary {
            session_id: session_id.clone(),
            open_attempt_id: SshOpenAttemptId::new(),
            target: target.clone(),
            credential_ref_id: None,
            endpoint: match &target {
                SshSessionTarget::QuickConnect { endpoint } => Some(endpoint.clone()),
                SshSessionTarget::Host { .. } => None,
            },
            generation: WireSequence::new(3),
            state_revision: WireSequence::new(7),
            attachment_revision: WireSequence::new(4),
            event_seq: WireSequence::new(9),
            channel_id: None,
            negotiated_algorithms: Vec::new(),
            state: SshSessionState::Failed,
            close_reason: None,
            failure_reason: Some(failure_from_route(TransportError::ConnectionLost, None)),
            attachment_count: 1,
            created_at_unix_ms: unix_time_ms(),
            updated_at_unix_ms: unix_time_ms(),
        };
        let attachment = SshSessionAttachment {
            attachment_id: attachment_id.clone(),
            attach_attempt_id: attach_attempt_id.clone(),
            session_id: session_id.clone(),
            generation: WireSequence::new(3),
            channel_id: None,
            view_id: view_id.clone(),
            state_revision: WireSequence::new(7),
            attachment_revision: WireSequence::new(4),
            attached_at_unix_ms: unix_time_ms(),
        };
        let old_lease = SshSessionInputLease {
            lease_id: old_lease_id.clone(),
            session_id: session_id.clone(),
            generation: WireSequence::new(3),
            attachment_id: attachment_id.clone(),
            view_id: view_id.clone(),
            focus_epoch: WireSequence::new(1),
            input_epoch: WireSequence::new(2),
            expires_at_unix_ms: unix_time_ms().saturating_add(INPUT_LEASE_MILLIS),
        };
        actor.sessions.insert(
            session_id.as_str().to_owned(),
            SessionRecord {
                shared_channels: None,
                channel_lease_cancel: watch::channel(false).0,
                terminal_startup: None,
                summary,
                attachments: BTreeMap::from([(
                    attachment_id.as_str().to_owned(),
                    AttachmentRecord {
                        summary: attachment,
                        events: Channel::<SshSessionEvent>::new(|_| Ok(())),
                        last_seen_at_unix_ms: unix_time_ms(),
                    },
                )]),
                active_challenge: None,
                active_keyboard_interactive_challenge: None,
                prepared_keyboard_interactive_answers: BTreeMap::new(),
                active_login_automation: None,
                heartbeat_policy: test_heartbeat_policy(),
                heartbeat: heartbeat_status_from_policy(&test_heartbeat_policy()),
                transport_heartbeat_tasks: Vec::new(),
                shell_heartbeat_task: None,
                last_user_input_at: None,
                shell_heartbeat_write_in_flight: false,
                input_activity_epoch: Arc::new(AtomicU64::new(0)),
                input_lease: Some(old_lease),
                next_input_epoch: 2,
                last_client_seq: 8,
                last_resize_seq: 5,
                next_output_seq: 42,
                output_ring: VecDeque::new(),
                output_ring_bytes: 128,
                shell: None,
                shell_stop: None,
            },
        );

        let operation_id = OperationId::new();
        let request = SshSessionReconnectRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: operation_id.clone(),
            idempotency_key: "reconnect-generation-four".to_owned(),
            session_id: session_id.clone(),
            expected_generation: WireSequence::new(3),
            expected_state_revision: WireSequence::new(7),
            attachment_id: attachment_id.clone(),
            view_id: view_id.clone(),
            credential_ref_id: Some(replacement.credential_ref_id),
            rows: 30,
            cols: 100,
        };
        let details = actor
            .reconnect(request.clone())
            .expect("reconnect accepted");

        assert_eq!(details.session.generation, WireSequence::new(4));
        assert_eq!(details.session.state, SshSessionState::Resolving);
        assert!(details.input_lease.is_none());
        assert_eq!(details.attachments.len(), 1);
        assert_eq!(details.attachments[0].generation, WireSequence::new(4));
        assert!(details.attachments[0].channel_id.is_none());
        assert_ne!(details.attachments[0].attachment_id, attachment_id);
        assert_ne!(details.attachments[0].attach_attempt_id, attach_attempt_id);
        assert!(
            live_sessions
                .lock()
                .expect("live sessions")
                .contains(session_id.as_str())
        );
        let record = actor
            .sessions
            .get(session_id.as_str())
            .expect("session record");
        assert_eq!(record.next_input_epoch, 0);
        assert_eq!(record.last_client_seq, 0);
        assert_eq!(record.last_resize_seq, 0);
        assert_eq!(record.next_output_seq, 1);
        assert_eq!(record.output_ring_bytes, 0);

        let replay = actor
            .reconnect(SshSessionReconnectRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                ..request.clone()
            })
            .expect("exact reconnect replay");
        assert_eq!(replay.session.generation, WireSequence::new(4));

        let conflict_request_id = RequestId::new();
        let conflict = actor
            .reconnect(SshSessionReconnectRequest {
                meta: RequestMeta {
                    request_id: conflict_request_id.clone(),
                },
                rows: 31,
                ..request
            })
            .expect_err("changed reconnect fingerprint must conflict");
        assert_eq!(conflict.code, "ssh_terminal.stale_fence");
        assert_eq!(conflict.request_id, Some(conflict_request_id));
        assert_eq!(
            actor.sessions[session_id.as_str()].summary.generation,
            WireSequence::new(4)
        );

        let first_connect_message =
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("connect task should start")
                .expect("actor mailbox should remain open");
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        let connect_starts = std::iter::once(first_connect_message)
            .chain(std::iter::from_fn(|| rx.try_recv().ok()))
            .filter(|message| {
                matches!(
                    message,
                    Message::ConnectionState {
                        generation: 4,
                        state: SshSessionState::Connecting,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(connect_starts, 1, "replay must not spawn a second connect");

        let stale_input = actor
            .queue_input(SshSessionInputRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: session_id.clone(),
                expected_generation: WireSequence::new(3),
                channel_id: SshChannelId::new(),
                attachment_id: attachment_id.clone(),
                view_id: view_id.clone(),
                focus_epoch: WireSequence::new(1),
                lease_id: old_lease_id.clone(),
                input_epoch: WireSequence::new(2),
                client_seq: WireSequence::new(9),
                bytes: vec![b'x'],
            })
            .expect_err("old generation input must fail closed");
        assert_eq!(stale_input.code, "ssh_terminal.stale_focus");

        let stale_resize = actor
            .queue_resize(SshSessionResizeRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: session_id.clone(),
                expected_generation: WireSequence::new(3),
                channel_id: SshChannelId::new(),
                attachment_id: attachment_id.clone(),
                view_id: view_id.clone(),
                focus_epoch: WireSequence::new(1),
                lease_id: old_lease_id.clone(),
                input_epoch: WireSequence::new(2),
                resize_seq: WireSequence::new(6),
                rows: 31,
                cols: 101,
            })
            .expect_err("old generation resize must fail closed");
        assert_eq!(stale_resize.code, "ssh_terminal.stale_focus");

        let stale_renew = actor
            .lease_renew(SshSessionInputLeaseRenewRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                session_id: session_id.clone(),
                expected_generation: WireSequence::new(3),
                attachment_id: attachment_id.clone(),
                view_id: view_id.clone(),
                focus_epoch: WireSequence::new(1),
                lease_id: old_lease_id,
                input_epoch: WireSequence::new(2),
            })
            .expect_err("old generation lease renewal must fail closed");
        assert_eq!(stale_renew.code, "ssh_terminal.stale_focus");

        let refreshed_generation_with_old_attachment = actor
            .lease_acquire(SshSessionInputLeaseAcquireRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "old-attachment-after-reconnect".to_owned(),
                session_id,
                expected_generation: WireSequence::new(4),
                expected_state_revision: details.session.state_revision,
                attachment_id,
                view_id,
            })
            .expect_err("old attachment must not cross the generation boundary");
        assert_eq!(
            refreshed_generation_with_old_attachment.code,
            "ssh_terminal.not_found"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn production_tauri_invoke_reaches_actor_and_real_loopback_transport() {
        let server = NetworkTestServer::start().await;
        let (directory, hosts, transient_credentials, service) = network_service_fixture();
        let plugin_service = crate::plugin_service::PluginService::start(
            directory.path(),
            hosts.clone(),
            service.clone(),
        )
        .expect("plugin service");
        let app = crate::with_production_invoke_handler(
            mock_builder()
                .manage(crate::lifecycle::LifecycleState::default())
                .manage(transient_credentials.clone())
                .manage(plugin_service)
                .manage(service.clone()),
        )
        .build(mock_context(noop_assets()))
        .expect("build mock Tauri app with the production invoke registry");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("build mock Tauri webview");

        let prepared: TransientCredentialRef = invoke_command(
            &webview,
            "credential_transient_prepare",
            serde_json::json!({
                "request": network_password_prepare_request(
                    NETWORK_TEST_PASSWORD,
                    "invoke-network-password",
                ),
            }),
        );
        assert!(
            transient_credentials.contains(&prepared.credential_ref_id),
            "the production credential command stores only an opaque one-time reference"
        );

        let opened: SshSessionOpenResponse = invoke_command(
            &webview,
            "ssh_terminal_open",
            serde_json::json!({
                "request": network_open_request(
                    server.port,
                    prepared.credential_ref_id,
                    "invoke-network-open",
                ),
                "onEvent": "__CHANNEL__:4100",
            }),
        );
        let awaiting = wait_for_invoke_state(
            &webview,
            &opened.session.session_id,
            SshSessionState::AwaitingHostKeyDecision,
        )
        .await;
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 0);
        let challenge = awaiting
            .active_host_key_challenge
            .expect("invoke get exposes the observed host-key challenge");
        assert_eq!(
            challenge.fingerprint_sha256,
            ssh_sha256_fingerprint(&server.host_key_blob)
        );

        let _: SshSessionDetails = invoke_command(
            &webview,
            "ssh_terminal_host_key_decide",
            serde_json::json!({
                "request": SshHostKeyDecisionRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: "invoke-network-accept-host-key".to_owned(),
                    session_id: opened.session.session_id.clone(),
                    expected_generation: awaiting.session.generation,
                    challenge_id: challenge.challenge_id,
                    expected_state_revision: challenge.state_revision,
                    attachment_id: opened.attachment.attachment_id,
                    view_id: opened.attachment.view_id,
                    decision: SshHostKeyDecision::AcceptAndStore,
                },
            }),
        );
        let running = wait_for_invoke_state(
            &webview,
            &opened.session.session_id,
            SshSessionState::Running,
        )
        .await;
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 1);

        let attached: SshSessionAttachResponse = invoke_command(
            &webview,
            "ssh_terminal_attach",
            serde_json::json!({
                "request": SshSessionAttachRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: "invoke-network-attach".to_owned(),
                    session_id: running.session.session_id.clone(),
                    expected_generation: running.session.generation,
                    expected_state_revision: running.session.state_revision,
                    attach_attempt_id: SshAttachAttemptId::new(),
                    view_id: SshViewId::new(),
                    after_output_seq: None,
                },
                "onEvent": "__CHANNEL__:4101",
            }),
        );
        let attached_details = invoke_session_details(&webview, &running.session.session_id);
        assert_eq!(attached_details.attachments.len(), 2);

        let focus_snapshot: SshTerminalInputFocusSnapshot = invoke_command(
            &webview,
            "ssh_terminal_input_focus_snapshot",
            serde_json::json!({
                "request": SshTerminalInputFocusSnapshotRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                },
            }),
        );
        assert!(focus_snapshot.target.is_none());
        assert!(focus_snapshot.lease.is_none());
        let channel_id = attached_details
            .session
            .channel_id
            .clone()
            .expect("running invoke session channel");
        let focus_target = SshTerminalInputFocusTarget {
            session_id: attached_details.session.session_id.clone(),
            expected_generation: attached_details.session.generation,
            expected_state_revision: attached_details.session.state_revision,
            channel_id: channel_id.clone(),
            attachment_id: attached.attachment.attachment_id.clone(),
            view_id: attached.attachment.view_id.clone(),
        };
        let focused: SshTerminalInputFocusChangeResponse = invoke_command(
            &webview,
            "ssh_terminal_input_focus_change",
            serde_json::json!({
                "request": SshTerminalInputFocusChangeRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: "invoke-network-focus".to_owned(),
                    expected_focus_epoch: focus_snapshot.focus_epoch,
                    target: Some(focus_target.clone()),
                },
            }),
        );
        assert_eq!(focused.target, Some(focus_target));
        let lease = focused.lease.expect("focus change returns an input lease");

        let raw_input = b"\x1b[32minvoke:\xe4\xbd\xa0\xe5\xa5\xbd\xff\x1b[0m\r".to_vec();
        let _: () = invoke_command(
            &webview,
            "ssh_terminal_input",
            serde_json::json!({
                "request": SshSessionInputRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    session_id: attached_details.session.session_id.clone(),
                    expected_generation: attached_details.session.generation,
                    channel_id: channel_id.clone(),
                    attachment_id: attached.attachment.attachment_id.clone(),
                    view_id: attached.attachment.view_id.clone(),
                    focus_epoch: lease.focus_epoch,
                    lease_id: lease.lease_id.clone(),
                    input_epoch: lease.input_epoch,
                    client_seq: WireSequence::new(1),
                    bytes: raw_input.clone(),
                },
            }),
        );
        let _: () = invoke_command(
            &webview,
            "ssh_terminal_resize",
            serde_json::json!({
                "request": SshSessionResizeRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    session_id: attached_details.session.session_id.clone(),
                    expected_generation: attached_details.session.generation,
                    channel_id,
                    attachment_id: attached.attachment.attachment_id,
                    view_id: attached.attachment.view_id,
                    focus_epoch: lease.focus_epoch,
                    lease_id: lease.lease_id,
                    input_epoch: lease.input_epoch,
                    resize_seq: WireSequence::new(1),
                    rows: 47,
                    cols: 139,
                },
            }),
        );
        timeout(Duration::from_secs(5), async {
            loop {
                let inputs = server
                    .evidence
                    .inputs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let resizes = server
                    .evidence
                    .resizes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if inputs.contains(&raw_input) && resizes.contains(&(139, 47)) {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("loopback server observes exact invoke input and resize");

        let before_disconnect =
            invoke_session_details(&webview, &attached_details.session.session_id);
        let _: SshSessionDetails = invoke_command(
            &webview,
            "ssh_terminal_disconnect",
            serde_json::json!({
                "request": SshSessionDisconnectRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: "invoke-network-disconnect".to_owned(),
                    session_id: before_disconnect.session.session_id.clone(),
                    expected_generation: before_disconnect.session.generation,
                    expected_state_revision: before_disconnect.session.state_revision,
                },
            }),
        );
        let closed = wait_for_invoke_state(
            &webview,
            &before_disconnect.session.session_id,
            SshSessionState::Closed,
        )
        .await;
        assert_eq!(
            closed.session.close_reason,
            Some(SshSessionCloseReason::UserRequested)
        );
        assert!(service.exit_blockers().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_loopback_actor_enforces_host_key_and_password_matrix() {
        let server = NetworkTestServer::start().await;
        let (_directory, _hosts, transient_credentials, service) = network_service_fixture();

        let rejected_credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-host-key-reject",
        );
        let rejected = open_network_session(
            &service,
            server.port,
            rejected_credential,
            "network-open-reject",
        )
        .await;
        let awaiting = wait_for_network_state(
            &service,
            &rejected.session.session_id,
            SshSessionState::AwaitingHostKeyDecision,
        )
        .await;
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 0);
        let challenge = awaiting
            .active_host_key_challenge
            .expect("first connection exposes the observed host key");
        let reject_request = SshHostKeyDecisionRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-reject-observed-host-key".to_owned(),
            session_id: rejected.session.session_id.clone(),
            expected_generation: awaiting.session.generation,
            challenge_id: challenge.challenge_id,
            expected_state_revision: challenge.state_revision,
            attachment_id: rejected.attachment.attachment_id,
            view_id: rejected.attachment.view_id,
            decision: SshHostKeyDecision::Reject,
        };
        let request_id = reject_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::HostKeyDecide {
                    request: reject_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("reject observed host key");
        let rejected = wait_for_network_state(
            &service,
            &rejected.session.session_id,
            SshSessionState::Failed,
        )
        .await;
        assert_eq!(
            rejected
                .session
                .failure_reason
                .expect("host-key failure")
                .code,
            SshSessionFailureCode::HostKeyRejected
        );
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 0);

        let accepted_credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-host-key-accept",
        );
        let accepted = open_network_session(
            &service,
            server.port,
            accepted_credential,
            "network-open-accept",
        )
        .await;
        let awaiting = wait_for_network_state(
            &service,
            &accepted.session.session_id,
            SshSessionState::AwaitingHostKeyDecision,
        )
        .await;
        let challenge = awaiting
            .active_host_key_challenge
            .expect("second connection still requires explicit acceptance");
        assert_eq!(
            challenge.fingerprint_sha256,
            ssh_sha256_fingerprint(&server.host_key_blob)
        );
        let accept_request = SshHostKeyDecisionRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-accept-and-store-host-key".to_owned(),
            session_id: accepted.session.session_id.clone(),
            expected_generation: awaiting.session.generation,
            challenge_id: challenge.challenge_id,
            expected_state_revision: challenge.state_revision,
            attachment_id: accepted.attachment.attachment_id,
            view_id: accepted.attachment.view_id,
            decision: SshHostKeyDecision::AcceptAndStore,
        };
        let request_id = accept_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::HostKeyDecide {
                    request: accept_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("accept and persist observed host key");
        let accepted = wait_for_network_state(
            &service,
            &accepted.session.session_id,
            SshSessionState::Running,
        )
        .await;
        assert!(accepted.active_host_key_challenge.is_none());
        assert_eq!(accepted.session.negotiated_algorithms.len(), 1);
        let negotiated = &accepted.session.negotiated_algorithms[0];
        assert_eq!(negotiated.route_stage, SshSessionRouteStage::Target);
        assert_eq!(
            negotiated.policy_id,
            norishell_ssh_transport::SECURE_DEFAULT_ALGORITHM_POLICY_ID
        );
        assert_eq!(
            negotiated.policy_catalog_version,
            norishell_ssh_transport::ALGORITHM_POLICY_CATALOG_VERSION
        );
        assert!(!negotiated.key_exchange.is_empty());
        assert!(!negotiated.host_key.is_empty());
        assert!(!negotiated.cipher_client_to_server.is_empty());
        assert!(!negotiated.mac_client_to_server.is_empty());
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 1);

        let trusted_credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-known-host-password",
        );
        let trusted = open_network_session(
            &service,
            server.port,
            trusted_credential,
            "network-open-known-host",
        )
        .await;
        let trusted = wait_for_network_state(
            &service,
            &trusted.session.session_id,
            SshSessionState::Running,
        )
        .await;
        assert!(trusted.active_host_key_challenge.is_none());
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 2);

        let wrong_credential = prepare_network_password(
            &transient_credentials,
            "wrong-loopback-password",
            "network-wrong-password",
        );
        let wrong = open_network_session(
            &service,
            server.port,
            wrong_credential,
            "network-open-wrong-password",
        )
        .await;
        let wrong =
            wait_for_network_state(&service, &wrong.session.session_id, SshSessionState::Failed)
                .await;
        assert!(wrong.active_host_key_challenge.is_none());
        assert_eq!(
            wrong
                .session
                .failure_reason
                .expect("authentication failure")
                .code,
            SshSessionFailureCode::AuthenticationRejected
        );
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 3);
        assert_eq!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            2,
            "rejected authentication must not open a PTY"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_loopback_actor_blocks_changed_host_key_before_authentication() {
        let server = NetworkTestServer::start().await;
        let (_directory, hosts, transient_credentials, service) = network_service_fixture();
        hosts
            .trust_known_host(
                "127.0.0.1",
                server.port,
                "ssh-ed25519",
                &[
                    0, 0, 0, 11, b's', b's', b'h', b'-', b'e', b'd', b'2', b'5', b'5', b'1', b'9',
                ],
            )
            .expect("pre-trust a different isolated host key");
        let credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-mismatch-password",
        );
        let mismatched =
            open_network_session(&service, server.port, credential, "network-open-mismatch").await;
        let mismatched = wait_for_network_state(
            &service,
            &mismatched.session.session_id,
            SshSessionState::Failed,
        )
        .await;
        assert_eq!(
            mismatched
                .session
                .failure_reason
                .expect("mismatch failure")
                .code,
            SshSessionFailureCode::HostKeyMismatch
        );
        assert_eq!(server.evidence.auth_attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_child_without_credential_reference_runs_and_resizes() {
        let server = NetworkTestServer::start().await;
        let (_directory, hosts, transient_credentials, service) = network_service_fixture();
        hosts
            .trust_known_host(
                "127.0.0.1",
                server.port,
                "ssh-ed25519",
                &server.host_key_blob,
            )
            .expect("trust isolated server key for shared child test");

        let credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "shared-child-parent-password",
        );
        let parent = open_network_session(
            &service,
            server.port,
            credential,
            "shared-child-parent-open",
        )
        .await;
        let parent = wait_for_network_state(
            &service,
            &parent.session.session_id,
            SshSessionState::Running,
        )
        .await;
        let mut parent_lease = service
            .plugin_channel_lease(
                RequestId::new(),
                parent.session.session_id.clone(),
                parent.session.generation,
            )
            .await
            .expect("running parent has a shareable channel lease");
        parent_lease.credential_ref_id = None;

        let request = SshSessionOpenRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "shared-child-without-credential-ref".to_owned(),
            open_attempt_id: SshOpenAttemptId::new(),
            attach_attempt_id: SshAttachAttemptId::new(),
            target: parent_lease.target.clone(),
            credential_ref_id: None,
            plugin_authorization_token: None,
            view_id: SshViewId::new(),
            rows: 31,
            cols: 101,
        };
        let request_id = request.meta.request_id.clone();
        let child_output = Arc::new(Mutex::new(Vec::new()));
        let child_output_sink = Arc::clone(&child_output);
        let child = service
            .request(
                |reply| Message::Open {
                    terminal_startup: Some(Box::new(
                        crate::plugin_service::ApprovedPluginTerminalStartup {
                            command: "docker exec -it -- fixture /bin/sh".to_owned(),
                            parent: parent_lease,
                            is_current: Arc::new(|| true),
                        },
                    )),
                    request,
                    events: Channel::<SshSessionEvent>::new(move |event| {
                        if let tauri::ipc::InvokeResponseBody::Json(json) = event {
                            let event: SshSessionEvent = serde_json::from_str(&json)
                                .expect("serialized shared child terminal event");
                            if let SshSessionEventPayload::OutputFrame { frame } = event.payload {
                                child_output_sink.lock().unwrap().extend(frame.bytes);
                            }
                        }
                        Ok(())
                    }),
                    reply,
                },
                request_id,
            )
            .await
            .expect("open shared child without credential reference");
        let child = wait_for_network_state(
            &service,
            &child.session.session_id,
            SshSessionState::Running,
        )
        .await;
        assert_eq!(child.session.credential_ref_id, None);

        let attachment = child.attachments[0].clone();
        let lease_request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "shared-child-resize-lease".to_owned(),
            session_id: child.session.session_id.clone(),
            expected_generation: child.session.generation,
            expected_state_revision: child.session.state_revision,
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
        };
        let request_id = lease_request.meta.request_id.clone();
        let lease = service
            .request(
                |reply| Message::LeaseAcquire {
                    request: lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("acquire shared child input lease");
        let channel_id = child
            .session
            .channel_id
            .clone()
            .expect("running shared child channel");
        let child_input = b"shared-child-input\r".to_vec();
        let input_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: child.session.session_id.clone(),
            expected_generation: child.session.generation,
            channel_id: channel_id.clone(),
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id.clone(),
            input_epoch: lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: child_input.clone(),
        };
        let request_id = input_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Input {
                    request: input_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("write shared child input");
        timeout(Duration::from_secs(5), async {
            loop {
                if child_output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .windows(child_input.len())
                    .any(|bytes| bytes == child_input)
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("shared child emits an output frame for its first input");

        let resize_request = SshSessionResizeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: child.session.session_id.clone(),
            expected_generation: child.session.generation,
            channel_id,
            attachment_id: attachment.attachment_id,
            view_id: attachment.view_id,
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id,
            input_epoch: lease.input_epoch,
            resize_seq: WireSequence::new(1),
            rows: 47,
            cols: 143,
        };
        let request_id = resize_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Resize {
                    request: resize_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("resize shared child PTY");
        timeout(Duration::from_secs(5), async {
            loop {
                if server
                    .evidence
                    .resizes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&(143, 47))
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("loopback server observes shared child resize");

        let child_current = network_session_details(&service, &child.session.session_id).await;
        let disconnect_request = SshSessionDisconnectRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "shared-child-disconnect".to_owned(),
            session_id: child_current.session.session_id.clone(),
            expected_generation: child_current.session.generation,
            expected_state_revision: child_current.session.state_revision,
        };
        let request_id = disconnect_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Disconnect {
                    request: disconnect_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("disconnect shared child only");
        wait_for_network_state(
            &service,
            &child_current.session.session_id,
            SshSessionState::Closed,
        )
        .await;

        let parent_current = network_session_details(&service, &parent.session.session_id).await;
        assert_eq!(parent_current.session.state, SshSessionState::Running);
        let parent_attachment = parent_current.attachments[0].clone();
        let parent_lease_request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "shared-child-parent-lease".to_owned(),
            session_id: parent_current.session.session_id.clone(),
            expected_generation: parent_current.session.generation,
            expected_state_revision: parent_current.session.state_revision,
            attachment_id: parent_attachment.attachment_id.clone(),
            view_id: parent_attachment.view_id.clone(),
        };
        let request_id = parent_lease_request.meta.request_id.clone();
        let parent_input_lease = service
            .request(
                |reply| Message::LeaseAcquire {
                    request: parent_lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("acquire parent input lease after child closes");
        let parent_input = b"parent-still-writable\r".to_vec();
        let parent_input_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: parent_current.session.session_id.clone(),
            expected_generation: parent_current.session.generation,
            channel_id: parent_current
                .session
                .channel_id
                .clone()
                .expect("running parent channel"),
            attachment_id: parent_attachment.attachment_id,
            view_id: parent_attachment.view_id,
            focus_epoch: parent_input_lease.focus_epoch,
            lease_id: parent_input_lease.lease_id,
            input_epoch: parent_input_lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: parent_input.clone(),
        };
        let request_id = parent_input_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Input {
                    request: parent_input_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("parent remains writable after child closes");
        timeout(Duration::from_secs(5), async {
            loop {
                if server
                    .evidence
                    .inputs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&parent_input)
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("loopback server observes input on the parent after child close");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_loopback_actor_maps_remote_eof_without_exit_status() {
        let server = NetworkTestServer::start().await;
        let (_directory, hosts, transient_credentials, service) = network_service_fixture();
        hosts
            .trust_known_host(
                "127.0.0.1",
                server.port,
                "ssh-ed25519",
                &server.host_key_blob,
            )
            .expect("trust isolated EOF server key");
        let credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-eof-password",
        );
        let opened =
            open_network_session(&service, server.port, credential, "network-open-eof").await;
        let running = wait_for_network_state(
            &service,
            &opened.session.session_id,
            SshSessionState::Running,
        )
        .await;
        let attachment = running.attachments[0].clone();
        let lease_request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-eof-lease".to_owned(),
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            expected_state_revision: running.session.state_revision,
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
        };
        let request_id = lease_request.meta.request_id.clone();
        let lease = service
            .request(
                |reply| Message::LeaseAcquire {
                    request: lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("acquire EOF session lease");
        let input_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            channel_id: running.session.channel_id.expect("EOF channel"),
            attachment_id: attachment.attachment_id,
            view_id: attachment.view_id,
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id,
            input_epoch: lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: NETWORK_EOF_INPUT.to_vec(),
        };
        let request_id = input_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Input {
                    request: input_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("request remote EOF without exit status");
        let closed = wait_for_network_state(
            &service,
            &running.session.session_id,
            SshSessionState::Closed,
        )
        .await;
        assert_eq!(
            closed.session.close_reason,
            Some(SshSessionCloseReason::RemoteEof)
        );
        assert!(
            !service
                .exit_blockers()
                .contains(&running.session.session_id)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_loopback_actor_covers_pty_replay_exit_loss_and_reconnect_fences() {
        let server = NetworkTestServer::start().await;
        let (_directory, hosts, transient_credentials, service) = network_service_fixture();
        hosts
            .trust_known_host(
                "127.0.0.1",
                server.port,
                "ssh-ed25519",
                &server.host_key_blob,
            )
            .expect("trust isolated server key for lifecycle test");

        let credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-lifecycle-password",
        );
        let output = Arc::new(Mutex::new(Vec::new()));
        let output_sink = output.clone();
        let request = network_open_request(server.port, credential, "network-open-lifecycle");
        let request_id = request.meta.request_id.clone();
        let opened = service
            .request(
                |reply| Message::Open {
                    terminal_startup: None,
                    request,
                    events: Channel::<SshSessionEvent>::new(move |event| {
                        if let tauri::ipc::InvokeResponseBody::Json(json) = event {
                            let event: SshSessionEvent =
                                serde_json::from_str(&json).expect("serialized terminal event");
                            if let SshSessionEventPayload::OutputFrame { frame } = event.payload {
                                output_sink.lock().unwrap().extend(frame.bytes);
                            }
                        }
                        Ok(())
                    }),
                    reply,
                },
                request_id,
            )
            .await
            .expect("open isolated lifecycle session");
        let running = wait_for_network_state(
            &service,
            &opened.session.session_id,
            SshSessionState::Running,
        )
        .await;
        assert_eq!(running.session.generation, WireSequence::new(1));
        assert_eq!(
            server
                .evidence
                .pty_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[("xterm-256color".to_owned(), 80, 24)]
        );
        let attachment = running.attachments[0].clone();
        let channel_id = running
            .session
            .channel_id
            .clone()
            .expect("running channel id");
        let lease_request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-lifecycle-lease".to_owned(),
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            expected_state_revision: running.session.state_revision,
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
        };
        let request_id = lease_request.meta.request_id.clone();
        let lease = service
            .request(
                |reply| Message::LeaseAcquire {
                    request: lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("acquire network input lease");
        let raw_input = b"\x1b[32mclient:\xe4\xbd\xa0\xe5\xa5\xbd\xff\x1b[0m\r".to_vec();
        let input_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            channel_id: channel_id.clone(),
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id.clone(),
            input_epoch: lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: raw_input.clone(),
        };
        let request_id = input_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Input {
                    request: input_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("send raw network input");
        let resize_request = SshSessionResizeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: running.session.session_id.clone(),
            expected_generation: running.session.generation,
            channel_id: channel_id.clone(),
            attachment_id: attachment.attachment_id.clone(),
            view_id: attachment.view_id.clone(),
            focus_epoch: lease.focus_epoch,
            lease_id: lease.lease_id.clone(),
            input_epoch: lease.input_epoch,
            resize_seq: WireSequence::new(1),
            rows: 43,
            cols: 132,
        };
        let request_id = resize_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Resize {
                    request: resize_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("resize real remote PTY");
        timeout(Duration::from_secs(5), async {
            loop {
                let saw_input = server
                    .evidence
                    .inputs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&raw_input);
                let saw_resize = server
                    .evidence
                    .resizes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&(132, 43));
                // Server admission precedes client output delivery; replay is only
                // authoritative after the actor has received the echoed bytes.
                let received_echo = output
                    .lock()
                    .unwrap()
                    .windows(raw_input.len())
                    .any(|bytes| bytes == raw_input);
                if saw_input && saw_resize && received_echo {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("isolated server observes raw input and resize");

        let current = network_session_details(&service, &running.session.session_id).await;
        let detach_request = SshSessionDetachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-renderer-unavailable".to_owned(),
            session_id: current.session.session_id.clone(),
            expected_generation: current.session.generation,
            expected_state_revision: current.session.state_revision,
            attachment_id: current.attachments[0].attachment_id.clone(),
            view_id: current.attachments[0].view_id.clone(),
            intent: SshSessionDetachIntent::RendererUnavailable,
            confirmation: None,
        };
        let request_id = detach_request.meta.request_id.clone();
        let detached = service
            .request(
                |reply| Message::Detach {
                    request: detach_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("release unavailable renderer");
        assert!(matches!(detached, SshSessionDetachResult::Detached { .. }));
        assert!(
            service
                .exit_blockers()
                .contains(&running.session.session_id)
        );
        let detached_details = network_session_details(&service, &running.session.session_id).await;
        assert_eq!(detached_details.session.state, SshSessionState::Running);
        assert!(detached_details.attachments.is_empty());

        let rebound_view_id = SshViewId::new();
        let attach_request = SshSessionAttachRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-renderer-rebind".to_owned(),
            session_id: detached_details.session.session_id.clone(),
            expected_generation: detached_details.session.generation,
            expected_state_revision: detached_details.session.state_revision,
            attach_attempt_id: SshAttachAttemptId::new(),
            view_id: rebound_view_id,
            after_output_seq: None,
        };
        let request_id = attach_request.meta.request_id.clone();
        let rebound = service
            .request(
                |reply| Message::Attach {
                    request: attach_request,
                    events: Channel::<SshSessionEvent>::new(|_| Ok(())),
                    reply,
                },
                request_id,
            )
            .await
            .expect("attach a fresh renderer channel");
        let replay_bytes = rebound
            .replay
            .iter()
            .filter_map(|item| match item {
                SshSessionOutputItem::Frame(frame) => Some(frame.bytes.as_slice()),
                SshSessionOutputItem::Gap(_) => None,
            })
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert!(
            replay_bytes
                .windows(raw_input.len())
                .any(|window| window == raw_input)
        );
        assert!(
            rebound
                .replay
                .iter()
                .all(|item| matches!(item, SshSessionOutputItem::Frame(_)))
        );

        let rebound_details = network_session_details(&service, &running.session.session_id).await;
        let rebound_lease_request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-rebound-lease".to_owned(),
            session_id: rebound_details.session.session_id.clone(),
            expected_generation: rebound_details.session.generation,
            expected_state_revision: rebound_details.session.state_revision,
            attachment_id: rebound.attachment.attachment_id.clone(),
            view_id: rebound.attachment.view_id.clone(),
        };
        let request_id = rebound_lease_request.meta.request_id.clone();
        let rebound_lease = service
            .request(
                |reply| Message::LeaseAcquire {
                    request: rebound_lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("acquire rebound renderer lease");
        let exit_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: rebound_details.session.session_id.clone(),
            expected_generation: rebound_details.session.generation,
            channel_id: rebound_details
                .session
                .channel_id
                .clone()
                .expect("rebound channel"),
            attachment_id: rebound.attachment.attachment_id,
            view_id: rebound.attachment.view_id,
            focus_epoch: rebound_lease.focus_epoch,
            lease_id: rebound_lease.lease_id,
            input_epoch: rebound_lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: NETWORK_EXIT_INPUT.to_vec(),
        };
        let request_id = exit_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Input {
                    request: exit_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("request structured remote exit status");
        let closed = wait_for_network_state(
            &service,
            &running.session.session_id,
            SshSessionState::Closed,
        )
        .await;
        assert_eq!(
            closed.session.close_reason,
            Some(SshSessionCloseReason::RemoteExitStatus { exit_status: 23 })
        );
        assert!(
            !service
                .exit_blockers()
                .contains(&running.session.session_id)
        );

        let loss_credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-loss-password",
        );
        let loss_opened =
            open_network_session(&service, server.port, loss_credential, "network-open-loss").await;
        let loss_running = wait_for_network_state(
            &service,
            &loss_opened.session.session_id,
            SshSessionState::Running,
        )
        .await;
        let old_attachment = loss_running.attachments[0].clone();
        let old_channel_id = loss_running
            .session
            .channel_id
            .clone()
            .expect("old channel");
        let old_lease_request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-old-generation-lease".to_owned(),
            session_id: loss_running.session.session_id.clone(),
            expected_generation: loss_running.session.generation,
            expected_state_revision: loss_running.session.state_revision,
            attachment_id: old_attachment.attachment_id.clone(),
            view_id: old_attachment.view_id.clone(),
        };
        let request_id = old_lease_request.meta.request_id.clone();
        let old_lease = service
            .request(
                |reply| Message::LeaseAcquire {
                    request: old_lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("acquire old generation lease");
        let drop_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: loss_running.session.session_id.clone(),
            expected_generation: loss_running.session.generation,
            channel_id: old_channel_id.clone(),
            attachment_id: old_attachment.attachment_id.clone(),
            view_id: old_attachment.view_id.clone(),
            focus_epoch: old_lease.focus_epoch,
            lease_id: old_lease.lease_id.clone(),
            input_epoch: old_lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: NETWORK_DROP_INPUT.to_vec(),
        };
        let request_id = drop_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Input {
                    request: drop_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("trigger isolated protocol disconnect");
        let lost = wait_for_network_state(
            &service,
            &loss_running.session.session_id,
            SshSessionState::Failed,
        )
        .await;
        assert_eq!(
            lost.session
                .failure_reason
                .as_ref()
                .map(|reason| reason.code),
            Some(SshSessionFailureCode::ConnectionLost)
        );

        let reconnect_credential = prepare_network_password(
            &transient_credentials,
            NETWORK_TEST_PASSWORD,
            "network-reconnect-password",
        );
        let reconnect_request = SshSessionReconnectRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-manual-reconnect".to_owned(),
            session_id: lost.session.session_id.clone(),
            expected_generation: lost.session.generation,
            expected_state_revision: lost.session.state_revision,
            attachment_id: lost.attachments[0].attachment_id.clone(),
            view_id: lost.attachments[0].view_id.clone(),
            credential_ref_id: Some(reconnect_credential),
            rows: 30,
            cols: 100,
        };
        let request_id = reconnect_request.meta.request_id.clone();
        let reconnecting = service
            .request(
                |reply| Message::Reconnect {
                    request: reconnect_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("start explicit reconnect");
        assert_eq!(reconnecting.session.generation, WireSequence::new(2));
        assert!(service.exit_blockers().contains(&lost.session.session_id));
        let reconnected =
            wait_for_network_state(&service, &lost.session.session_id, SshSessionState::Running)
                .await;
        let new_attachment = reconnected.attachments[0].clone();
        let new_channel_id = reconnected.session.channel_id.clone().expect("new channel");
        assert_ne!(new_attachment.attachment_id, old_attachment.attachment_id);
        assert_ne!(new_channel_id, old_channel_id);

        let stale_input_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: reconnected.session.session_id.clone(),
            expected_generation: WireSequence::new(1),
            channel_id: old_channel_id.clone(),
            attachment_id: old_attachment.attachment_id.clone(),
            view_id: old_attachment.view_id.clone(),
            focus_epoch: old_lease.focus_epoch,
            lease_id: old_lease.lease_id.clone(),
            input_epoch: old_lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: b"stale-input".to_vec(),
        };
        let request_id = stale_input_request.meta.request_id.clone();
        let stale_input = service
            .request(
                |reply| Message::Input {
                    request: stale_input_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect_err("old generation input must fail closed");
        assert_eq!(stale_input.code, "ssh_terminal.stale_focus");
        let stale_resize_request = SshSessionResizeRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: reconnected.session.session_id.clone(),
            expected_generation: WireSequence::new(1),
            channel_id: old_channel_id.clone(),
            attachment_id: old_attachment.attachment_id.clone(),
            view_id: old_attachment.view_id.clone(),
            focus_epoch: old_lease.focus_epoch,
            lease_id: old_lease.lease_id.clone(),
            input_epoch: old_lease.input_epoch,
            resize_seq: WireSequence::new(2),
            rows: 31,
            cols: 101,
        };
        let request_id = stale_resize_request.meta.request_id.clone();
        let stale_resize = service
            .request(
                |reply| Message::Resize {
                    request: stale_resize_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect_err("old generation resize must fail closed");
        assert_eq!(stale_resize.code, "ssh_terminal.stale_focus");
        let stale_lease_request = SshSessionInputLeaseRenewRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: reconnected.session.session_id.clone(),
            expected_generation: WireSequence::new(1),
            attachment_id: old_attachment.attachment_id,
            view_id: old_attachment.view_id,
            focus_epoch: old_lease.focus_epoch,
            lease_id: old_lease.lease_id,
            input_epoch: old_lease.input_epoch,
        };
        let request_id = stale_lease_request.meta.request_id.clone();
        let stale_lease = service
            .request(
                |reply| Message::LeaseRenew {
                    request: stale_lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect_err("old generation lease must fail closed");
        assert_eq!(stale_lease.code, "ssh_terminal.stale_focus");

        let new_lease_request = SshSessionInputLeaseAcquireRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-new-generation-lease".to_owned(),
            session_id: reconnected.session.session_id.clone(),
            expected_generation: reconnected.session.generation,
            expected_state_revision: reconnected.session.state_revision,
            attachment_id: new_attachment.attachment_id.clone(),
            view_id: new_attachment.view_id.clone(),
        };
        let request_id = new_lease_request.meta.request_id.clone();
        let new_lease = service
            .request(
                |reply| Message::LeaseAcquire {
                    request: new_lease_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("acquire new generation lease");
        let stale_channel_request = SshSessionInputRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            session_id: reconnected.session.session_id.clone(),
            expected_generation: reconnected.session.generation,
            channel_id: old_channel_id,
            attachment_id: new_attachment.attachment_id,
            view_id: new_attachment.view_id,
            focus_epoch: new_lease.focus_epoch,
            lease_id: new_lease.lease_id,
            input_epoch: new_lease.input_epoch,
            client_seq: WireSequence::new(1),
            bytes: b"stale-channel".to_vec(),
        };
        let request_id = stale_channel_request.meta.request_id.clone();
        let stale_channel = service
            .request(
                |reply| Message::Input {
                    request: stale_channel_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect_err("old channel id must fail closed within current generation");
        assert_eq!(stale_channel.code, "ssh_terminal.stale_focus");

        let disconnect_request = SshSessionDisconnectRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "network-final-disconnect".to_owned(),
            session_id: reconnected.session.session_id.clone(),
            expected_generation: reconnected.session.generation,
            expected_state_revision: reconnected.session.state_revision,
        };
        let request_id = disconnect_request.meta.request_id.clone();
        service
            .request(
                |reply| Message::Disconnect {
                    request: disconnect_request,
                    reply,
                },
                request_id,
            )
            .await
            .expect("disconnect reconnected shell");
        let closed = wait_for_network_state(
            &service,
            &reconnected.session.session_id,
            SshSessionState::Closed,
        )
        .await;
        assert_eq!(
            closed.session.close_reason,
            Some(SshSessionCloseReason::UserRequested)
        );
        assert!(
            !service
                .exit_blockers()
                .contains(&reconnected.session.session_id)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_all_waits_for_ssh_shell_cleanup_and_uses_application_exit_reason() {
        let (_directory, mut actor, live_sessions, _rx) = actor_fixture();
        let (shell_tx, _shell_rx) = mpsc::channel(1);
        let session =
            insert_test_session(&mut actor, SshSessionState::Running, 2, 7, Some(shell_tx));
        let (stop_tx, mut stop_rx) = watch::channel(None);
        actor
            .sessions
            .get_mut(session.session_id.as_str())
            .expect("test session")
            .shell_stop = Some(stop_tx);
        live_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session.session_id.as_str().to_owned());
        let (reply, response) = oneshot::channel();

        actor.begin_shutdown(RequestId::new(), reply);

        assert_eq!(
            actor.sessions[session.session_id.as_str()].summary.state,
            SshSessionState::Disconnecting
        );
        stop_rx.changed().await.expect("shell stop signal");
        assert!(matches!(
            stop_rx.borrow().as_ref(),
            Some(ShellStopSignal::Disconnect(
                SshSessionCloseReason::ApplicationExit
            ))
        ));
        assert!(actor.shutdown_waiter.is_some());
        assert!(
            live_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(session.session_id.as_str())
        );

        actor.shell_closed(
            session.session_id.as_str(),
            2,
            SshSessionCloseReason::ApplicationExit,
        );
        actor.complete_shutdown_if_ready();

        response
            .await
            .expect("shutdown reply")
            .expect("shutdown succeeds after cleanup fact");
        assert!(
            live_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_all_terminates_real_local_pty_before_clearing_blocker() {
        let (_directory, _hosts, _credentials, service) = network_service_fixture();
        let request = LocalSessionOpenRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: OperationId::new(),
            idempotency_key: "local-shutdown-open".to_owned(),
            open_attempt_id: LocalOpenAttemptId::new(),
            attach_attempt_id: LocalAttachAttemptId::new(),
            view_id: LocalViewId::new(),
            rows: 24,
            cols: 80,
        };
        let request_id = request.meta.request_id.clone();
        let opened = service
            .request(
                |reply| Message::LocalOpen {
                    request,
                    events: Channel::new(|_| Ok(())),
                    reply,
                },
                request_id,
            )
            .await
            .expect("open local terminal");
        wait_for_local_state(
            &service,
            &opened.session.session_id,
            norishell_core_api::LocalSessionState::Running,
        )
        .await;

        service
            .shutdown_all(RequestId::new())
            .await
            .expect("shutdown local PTY tree");

        assert!(service.local_exit_blockers().is_empty());
        let get_request_id = RequestId::new();
        let command_request_id = get_request_id.clone();
        let details = service
            .request(
                |reply| Message::LocalGet {
                    request: LocalSessionGetRequest {
                        meta: RequestMeta {
                            request_id: command_request_id,
                        },
                        session_id: opened.session.session_id,
                    },
                    reply,
                },
                get_request_id,
            )
            .await
            .expect("read terminal after shutdown");
        assert!(matches!(
            details.session.state,
            norishell_core_api::LocalSessionState::Exited
                | norishell_core_api::LocalSessionState::Closed
        ));
    }
}
