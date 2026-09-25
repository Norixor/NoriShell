//! Protocol-level SSH port-forward session runtime.
//!
//! A session owns exactly one forward transport capability and one generation. Saved-Host
//! sessions own a dedicated SSH connection; plugin sessions own only child-channel access on an
//! existing user SSH connection.
//! Local and dynamic listeners are loopback-only. Remote listeners are
//! registered and cancelled through [`SshForwardTransport`] rather than shell
//! commands. The command/API layer is intentionally outside this module so it
//! can resolve the correct fenced transport capability for every explicit start.

use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use norishell_app_persistence::KnownHostObservation;
use norishell_core_api::{self as wire, HostId};
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    ForwardedTcpipChannel, HostKeyDecision, HostKeyVerifier, IngressFailureKind, ObservedHostKey,
    SharedRemoteForward, SharedSessionChannels, SshForwardTransport, TransportError, VerifyFuture,
};
use serde::{Deserialize, Serialize};
use tauri::{State, ipc::Channel};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
    time::{Instant, timeout, timeout_at},
};
use zeroize::Zeroizing;

use crate::{
    connection_profile::{
        ConnectionProfileError, ResolvedTransportKeepalivePolicy, connection_has_vault_credentials,
        connection_requires_vault, resolve_long_lived_connection_profile,
    },
    host_service::HostService,
    ssh_agent_service::SshAgentService,
    ssh_connection_orchestrator::{
        ConnectionPhase, ConnectionRouteStage, KeyboardInteractiveRequest,
        RoutedTransportHeartbeat, SshConnectionInteraction, SshConnectionOrchestrator,
    },
    time::unix_time_ms,
    transient_credential_service::TransientCredentialService,
    vault_service::VaultService,
};

const MAX_ID_BYTES: usize = 128;
const MAX_HOST_BYTES: usize = 255;
const MAX_REMOTE_BIND_BYTES: usize = 255;
const SOCKS_VERSION: u8 = 5;
const SOCKS_NO_AUTH: u8 = 0;
const SOCKS_NO_ACCEPTABLE_AUTH: u8 = 0xff;
const SOCKS_COMMAND_CONNECT: u8 = 1;
const SOCKS_COMMAND_BIND: u8 = 2;
const SOCKS_COMMAND_UDP_ASSOCIATE: u8 = 3;
const SOCKS_ADDRESS_IPV4: u8 = 1;
const SOCKS_ADDRESS_DOMAIN: u8 = 3;
const SOCKS_ADDRESS_IPV6: u8 = 4;
const RELAY_BUFFER_BYTES: usize = 16 * 1024;
const TRAFFIC_PUBLISH_INTERVAL: Duration = Duration::from_millis(500);

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) trait ForwardIo: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> ForwardIo for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

type BoxedForwardIo = Box<dyn ForwardIo>;

#[derive(Default)]
struct ForwardTrafficCounters {
    listener_to_target: AtomicU64,
    target_to_listener: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum PortForwardRule {
    Local {
        host_ref: String,
        local_bind_address: IpAddr,
        local_listen_port: u16,
        remote_target_host: String,
        remote_target_port: u16,
    },
    Remote {
        host_ref: String,
        remote_bind_address: String,
        remote_listen_port: u16,
        local_target_host: String,
        local_target_port: u16,
    },
    Dynamic {
        host_ref: String,
        local_bind_address: IpAddr,
        local_listen_port: u16,
    },
}

impl PortForwardRule {
    pub(crate) fn validate(&self) -> ForwardResult<()> {
        validate_host_ref(self.host_ref())?;
        match self {
            Self::Local {
                local_bind_address,
                remote_target_host,
                remote_target_port,
                ..
            } => {
                require_loopback(*local_bind_address)?;
                validate_target(remote_target_host, *remote_target_port)
            }
            Self::Remote {
                remote_bind_address,
                local_target_host,
                local_target_port,
                ..
            } => {
                validate_remote_bind(remote_bind_address)?;
                validate_target(local_target_host, *local_target_port)
            }
            Self::Dynamic {
                local_bind_address, ..
            } => require_loopback(*local_bind_address),
        }
    }

    pub(crate) fn host_ref(&self) -> &str {
        match self {
            Self::Local { host_ref, .. }
            | Self::Remote { host_ref, .. }
            | Self::Dynamic { host_ref, .. } => host_ref,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ForwardGeneration(u64);

impl ForwardGeneration {
    pub(crate) fn new(value: u64) -> ForwardResult<Self> {
        (value > 0)
            .then_some(Self(value))
            .ok_or_else(|| ForwardRuntimeError::invalid("generation must be non-zero"))
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ForwardSessionState {
    Starting,
    Running,
    Failed,
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ForwardFailureCode {
    InvalidRule,
    HostUnavailable,
    HostKeyReviewRequired,
    HostKeyMismatch,
    VaultLocked,
    CredentialUnavailable,
    AuthenticationRejected,
    TransportConnect,
    Bind,
    RemoteRegistrationRejected,
    RemoteRegistrationUncertain,
    TransportLost,
    Protocol,
    ResourceLimit,
    SocksTruncated,
    SocksAuthenticationRejected,
    SocksCommandRejected,
    SocksAddressRejected,
    ConnectTimeout,
    TargetConnect,
    Relay,
    CleanupUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForwardFailure {
    pub(crate) code: ForwardFailureCode,
    pub(crate) stage: String,
    pub(crate) detail: String,
}

impl ForwardFailure {
    fn new(code: ForwardFailureCode, stage: &'static str, detail: &'static str) -> Self {
        Self {
            code,
            stage: stage.to_owned(),
            detail: detail.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardRuntimeError {
    pub(crate) failure: ForwardFailure,
}

impl ForwardRuntimeError {
    fn invalid(detail: &'static str) -> Self {
        Self {
            failure: ForwardFailure::new(ForwardFailureCode::InvalidRule, "validation", detail),
        }
    }

    const fn from_failure(failure: ForwardFailure) -> Self {
        Self { failure }
    }
}

impl fmt::Display for ForwardRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.failure.detail)
    }
}

impl std::error::Error for ForwardRuntimeError {}

type ForwardResult<T> = Result<T, ForwardRuntimeError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ActualForwardBind {
    Local { address: SocketAddr },
    Remote { address: String, port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CleanupStepState {
    NotRequired,
    Complete,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForwardCleanupFacts {
    pub(crate) listener_closed_or_remote_cancelled: CleanupStepState,
    pub(crate) children_cleared: CleanupStepState,
    pub(crate) transport_disconnected: CleanupStepState,
    pub(crate) abandoned_child_count: usize,
    pub(crate) uncertain: bool,
}

impl Default for ForwardCleanupFacts {
    fn default() -> Self {
        Self {
            listener_closed_or_remote_cancelled: CleanupStepState::NotRequired,
            children_cleared: CleanupStepState::NotRequired,
            transport_disconnected: CleanupStepState::NotRequired,
            abandoned_child_count: 0,
            uncertain: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForwardSessionSummary {
    pub(crate) session_id: String,
    pub(crate) host_ref: String,
    pub(crate) rule_id: Option<String>,
    pub(crate) rule_revision: Option<u64>,
    pub(crate) rule_snapshot: PortForwardRule,
    pub(crate) started_at_unix_ms: i64,
    pub(crate) generation: ForwardGeneration,
    pub(crate) state_revision: u64,
    pub(crate) state: ForwardSessionState,
    pub(crate) actual_bind: Option<ActualForwardBind>,
    pub(crate) child_count: usize,
    pub(crate) listener_to_target_bytes: u64,
    pub(crate) target_to_listener_bytes: u64,
    pub(crate) failure: Option<ForwardFailure>,
    pub(crate) last_child_failure: Option<ForwardFailure>,
    pub(crate) cleanup: ForwardCleanupFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ForwardRuntimeLimits {
    pub(crate) max_concurrent_children: usize,
    pub(crate) socks_handshake_timeout: Duration,
    pub(crate) connect_timeout: Duration,
    pub(crate) idle_timeout: Duration,
    pub(crate) child_shutdown_timeout: Duration,
}

impl Default for ForwardRuntimeLimits {
    fn default() -> Self {
        Self {
            max_concurrent_children: 64,
            socks_handshake_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(300),
            child_shutdown_timeout: Duration::from_secs(5),
        }
    }
}

impl ForwardRuntimeLimits {
    fn validate(self) -> ForwardResult<Self> {
        if !(1..=1024).contains(&self.max_concurrent_children)
            || self.socks_handshake_timeout.is_zero()
            || self.socks_handshake_timeout > Duration::from_secs(60)
            || self.connect_timeout.is_zero()
            || self.connect_timeout > Duration::from_secs(120)
            || self.idle_timeout.is_zero()
            || self.idle_timeout > Duration::from_secs(24 * 60 * 60)
            || self.child_shutdown_timeout.is_zero()
            || self.child_shutdown_timeout > Duration::from_secs(60)
        {
            return Err(ForwardRuntimeError::invalid(
                "forward runtime limits are outside their bounded range",
            ));
        }
        Ok(self)
    }
}

pub(crate) struct IncomingRemoteForward {
    // Keep the transport-provided endpoint facts available to a future audit surface without
    // making the relay reinterpret or expose client-controlled address text today.
    #[allow(
        dead_code,
        reason = "remote-forward audit metadata is retained at the transport boundary"
    )]
    pub(crate) connected_address: String,
    #[allow(
        dead_code,
        reason = "remote-forward audit metadata is retained at the transport boundary"
    )]
    pub(crate) connected_port: u16,
    #[allow(
        dead_code,
        reason = "remote-forward audit metadata is retained at the transport boundary"
    )]
    pub(crate) originator_address: String,
    #[allow(
        dead_code,
        reason = "remote-forward audit metadata is retained at the transport boundary"
    )]
    pub(crate) originator_port: u16,
    pub(crate) stream: BoxedForwardIo,
}

pub(crate) trait ForwardTransport: Send {
    fn open_direct_tcpip<'a>(
        &'a mut self,
        target_host: &'a str,
        target_port: u16,
        originator_address: &'a str,
        originator_port: u16,
    ) -> BoxFuture<'a, ForwardResult<BoxedForwardIo>>;

    fn request_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<u16>>;

    fn cancel_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<()>>;

    fn next_forwarded_tcpip(
        &mut self,
    ) -> BoxFuture<'_, ForwardResult<Option<IncomingRemoteForward>>>;

    /// Resolves only when an out-of-band transport fact makes the current
    /// generation unusable. Local and dynamic listeners select this alongside
    /// accepts so keepalive failure cannot leave an idle listener looking live.
    fn wait_failure(&mut self) -> BoxFuture<'_, ForwardResult<()>>;

    fn disconnect(self: Box<Self>) -> BoxFuture<'static, ForwardResult<()>>;
}

impl<V> ForwardTransport for SshForwardTransport<V>
where
    V: HostKeyVerifier,
{
    fn open_direct_tcpip<'a>(
        &'a mut self,
        target_host: &'a str,
        target_port: u16,
        originator_address: &'a str,
        originator_port: u16,
    ) -> BoxFuture<'a, ForwardResult<BoxedForwardIo>> {
        Box::pin(async move {
            SshForwardTransport::open_direct_tcpip(
                self,
                target_host,
                target_port,
                originator_address,
                originator_port,
            )
            .await
            .map(|channel| Box::new(channel) as BoxedForwardIo)
            .map_err(map_transport_error)
        })
    }

    fn request_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<u16>> {
        Box::pin(async move {
            SshForwardTransport::request_remote_forward(self, bind_address, bind_port)
                .await
                .map_err(map_transport_error)
        })
    }

    fn cancel_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<()>> {
        Box::pin(async move {
            SshForwardTransport::cancel_remote_forward(self, bind_address, bind_port)
                .await
                .map_err(map_transport_error)
        })
    }

    fn next_forwarded_tcpip(
        &mut self,
    ) -> BoxFuture<'_, ForwardResult<Option<IncomingRemoteForward>>> {
        Box::pin(async move {
            Ok(SshForwardTransport::next_forwarded_tcpip(self)
                .await
                .map(incoming_remote_forward))
        })
    }

    fn wait_failure(&mut self) -> BoxFuture<'_, ForwardResult<()>> {
        Box::pin(async move {
            // The SSH client handler owns the matching sender for the lifetime
            // of this transport. Local/dynamic forwards never register a
            // remote listener, so receiver closure is a transport-close fact
            // and an item is an impossible cross-mode protocol event.
            match SshForwardTransport::next_forwarded_tcpip(self).await {
                Some(_) => Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::Protocol,
                    "transport-liveness",
                    "the SSH server delivered an unexpected remote-forward channel",
                ))),
                None => Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::TransportLost,
                    "transport-liveness",
                    "the forward SSH transport closed unexpectedly",
                ))),
            }
        })
    }

    fn disconnect(self: Box<Self>) -> BoxFuture<'static, ForwardResult<()>> {
        Box::pin(async move {
            SshForwardTransport::disconnect(*self)
                .await
                .map_err(map_transport_error)
        })
    }
}

struct SharedForwardTransport {
    channels: SharedSessionChannels,
    cancelled: watch::Receiver<bool>,
    remote: Option<SharedRemoteForward>,
}

impl SharedForwardTransport {
    async fn wait_cancelled(cancelled: &mut watch::Receiver<bool>) {
        while !*cancelled.borrow() {
            if cancelled.changed().await.is_err() {
                return;
            }
        }
    }

    fn unavailable() -> ForwardRuntimeError {
        ForwardRuntimeError::from_failure(ForwardFailure::new(
            ForwardFailureCode::TransportLost,
            "parent-session",
            "the parent SSH session is no longer current",
        ))
    }
}

impl ForwardTransport for SharedForwardTransport {
    fn open_direct_tcpip<'a>(
        &'a mut self,
        target_host: &'a str,
        target_port: u16,
        originator_address: &'a str,
        originator_port: u16,
    ) -> BoxFuture<'a, ForwardResult<BoxedForwardIo>> {
        Box::pin(async move {
            let mut cancelled = self.cancelled.clone();
            tokio::select! {
                biased;
                () = Self::wait_cancelled(&mut cancelled) => Err(Self::unavailable()),
                () = self.channels.wait_closed() => Err(Self::unavailable()),
                result = self.channels.open_direct_tcpip(
                    target_host,
                    target_port,
                    originator_address,
                    originator_port,
                ) => result
                    .map(|channel| Box::new(channel) as BoxedForwardIo)
                    .map_err(map_transport_error),
            }
        })
    }

    fn request_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<u16>> {
        Box::pin(async move {
            if self.remote.is_some() {
                return Err(ForwardRuntimeError::invalid(
                    "the shared forward already owns a remote listener",
                ));
            }
            let mut cancelled = self.cancelled.clone();
            let remote = self
                .channels
                .begin_remote_forward(bind_address, bind_port)
                .map_err(map_transport_error)?;
            self.remote = Some(remote);
            let remote = self.remote.as_mut().expect("inserted shared remote ticket");
            tokio::select! {
                biased;
                () = Self::wait_cancelled(&mut cancelled) => Err(Self::unavailable()),
                () = self.channels.wait_closed() => Err(Self::unavailable()),
                result = remote.wait_ready() => result.map_err(map_transport_error),
            }
        })
    }

    fn cancel_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<()>> {
        Box::pin(async move {
            let remote = self.remote.as_mut().ok_or_else(|| {
                ForwardRuntimeError::invalid("the shared remote listener is not active")
            })?;
            if remote.actual_bind() != Some((bind_address, bind_port)) {
                return Err(ForwardRuntimeError::invalid(
                    "the shared remote listener binding changed",
                ));
            }
            remote.cancel().await.map_err(map_transport_error)?;
            self.remote = None;
            Ok(())
        })
    }

    fn next_forwarded_tcpip(
        &mut self,
    ) -> BoxFuture<'_, ForwardResult<Option<IncomingRemoteForward>>> {
        Box::pin(async move {
            let remote = self.remote.as_mut().ok_or_else(|| {
                ForwardRuntimeError::invalid("the shared remote listener is not active")
            })?;
            Ok(remote.next_channel().await.map(incoming_remote_forward))
        })
    }

    fn wait_failure(&mut self) -> BoxFuture<'_, ForwardResult<()>> {
        Box::pin(async move {
            let mut cancelled = self.cancelled.clone();
            tokio::select! {
                () = Self::wait_cancelled(&mut cancelled) => Err(Self::unavailable()),
                () = self.channels.wait_closed() => Err(Self::unavailable()),
            }
        })
    }

    fn disconnect(mut self: Box<Self>) -> BoxFuture<'static, ForwardResult<()>> {
        Box::pin(async move {
            if let Some(remote) = self.remote.as_mut() {
                remote.cancel().await.map_err(map_transport_error)?;
            }
            self.remote = None;
            Ok(())
        })
    }
}

struct SharedForwardTransportFactory {
    channels: SharedSessionChannels,
    cancelled: watch::Receiver<bool>,
}

impl ForwardTransportFactory for SharedForwardTransportFactory {
    fn connect<'a>(
        &'a self,
        _host_ref: &'a str,
        _generation: ForwardGeneration,
        _expected_host_state_version: Option<u64>,
    ) -> BoxFuture<'a, ForwardResult<Box<dyn ForwardTransport>>> {
        Box::pin(async move {
            if *self.cancelled.borrow() || self.channels.is_closed() {
                return Err(SharedForwardTransport::unavailable());
            }
            Ok(Box::new(SharedForwardTransport {
                channels: self.channels.clone(),
                cancelled: self.cancelled.clone(),
                remote: None,
            }) as Box<dyn ForwardTransport>)
        })
    }
}

pub(crate) trait ForwardTransportFactory: Send + Sync + 'static {
    /// This method is invoked exactly once by each explicit session start. Production saved-Host
    /// factories return a newly authenticated connection. A plugin factory instead returns an
    /// isolated child-channel capability on the already-authenticated parent selected by Core.
    fn connect<'a>(
        &'a self,
        host_ref: &'a str,
        generation: ForwardGeneration,
        expected_host_state_version: Option<u64>,
    ) -> BoxFuture<'a, ForwardResult<Box<dyn ForwardTransport>>>;
}

/// Production factory for saved Host-backed forwards.
///
/// It deliberately resolves the resource-neutral saved-Host connection base:
/// login automation and ShellHeartbeat never enter a ForwardSession. Unknown
/// host keys and keyboard-interactive prompts fail closed until the dedicated
/// Forward API exposes an explicit challenge/response surface.
#[derive(Clone)]
pub(crate) struct ProductionForwardTransportFactory {
    hosts: HostService,
    vault: VaultService,
    transient_credentials: TransientCredentialService,
    ssh_agent: SshAgentService,
}

impl ProductionForwardTransportFactory {
    pub(crate) fn new(
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: SshAgentService,
    ) -> Self {
        Self {
            hosts,
            vault,
            transient_credentials,
            ssh_agent,
        }
    }
}

impl ForwardTransportFactory for ProductionForwardTransportFactory {
    fn connect<'a>(
        &'a self,
        host_ref: &'a str,
        _generation: ForwardGeneration,
        expected_host_state_version: Option<u64>,
    ) -> BoxFuture<'a, ForwardResult<Box<dyn ForwardTransport>>> {
        Box::pin(async move {
            let host_id = HostId::parse(host_ref)
                .map_err(|_| profile_failure(ConnectionProfileError::InvalidTarget))?;
            let snapshot = self
                .hosts
                .get_connection_snapshot(&host_id)
                .map_err(|_| profile_failure(ConnectionProfileError::PersistenceUnavailable))?;
            if expected_host_state_version
                .is_some_and(|expected| snapshot.host.state_version.get() != expected)
            {
                return Err(profile_failure(ConnectionProfileError::StaleHost));
            }
            let profile = resolve_long_lived_connection_profile(
                &self.hosts,
                &host_id,
                snapshot.host.state_version,
            )
            .map_err(profile_failure)?;
            if connection_requires_vault(&profile.connection) && !self.vault.is_unlocked() {
                return Err(vault_locked_failure());
            }
            let has_vault_credentials = connection_has_vault_credentials(&profile.connection);
            let keepalive_interval = profile
                .transport_keepalive
                .as_ref()
                .map(|policy| Duration::from_secs(u64::from(policy.interval_seconds)));
            let verifier = Arc::new(TrustedForwardHostKeyVerifier {
                hosts: self.hosts.clone(),
            });
            let mut interaction = NonInteractiveForwardConnectionInteraction;
            let connection = SshConnectionOrchestrator::new(
                &self.vault,
                &self.transient_credentials,
                &self.ssh_agent,
            )
            .connect(
                profile.connection,
                verifier,
                &mut interaction,
                keepalive_interval,
            )
            .await
            .map_err(|failure| {
                if has_vault_credentials
                    && !self.vault.is_unlocked()
                    && vault_locked_after_credential_failure(&failure.error)
                {
                    vault_locked_failure()
                } else {
                    map_transport_error(failure.error)
                }
            })?;
            let heartbeat = ForwardHeartbeatTasks::start(
                profile.transport_keepalive.as_ref(),
                connection.transport_heartbeats,
            );
            Ok(Box::new(ProductionForwardTransport {
                transport: Some(connection.transport.into_forward_transport()),
                heartbeat_tasks: heartbeat,
            }) as Box<dyn ForwardTransport>)
        })
    }
}

#[derive(Clone)]
struct TrustedForwardHostKeyVerifier {
    hosts: HostService,
}

impl HostKeyVerifier for TrustedForwardHostKeyVerifier {
    fn verify(&self, endpoint: Endpoint, observed: ObservedHostKey) -> VerifyFuture {
        let hosts = self.hosts.clone();
        Box::pin(async move {
            match hosts.observe_known_host(
                endpoint.normalized_address(),
                endpoint.port(),
                &observed.algorithm,
                &observed.public_key_blob,
            ) {
                Ok(KnownHostObservation::Trusted(_)) => hosts
                    .record_known_host_verified(
                        endpoint.normalized_address(),
                        endpoint.port(),
                        &observed.algorithm,
                        &observed.public_key_blob,
                    )
                    .map(|_| HostKeyDecision::Trusted)
                    .map_err(|_| TransportError::HostKeyVerificationFailed),
                Ok(KnownHostObservation::Unknown(_)) => Ok(HostKeyDecision::Rejected),
                Ok(KnownHostObservation::Mismatch { trusted, .. })
                | Ok(KnownHostObservation::AlgorithmChanged { trusted, .. }) => {
                    Ok(HostKeyDecision::Mismatch {
                        trusted_fingerprint: trusted.fingerprint_sha256,
                    })
                }
                Err(_) => Err(TransportError::HostKeyVerificationFailed),
            }
        })
    }
}

struct NonInteractiveForwardConnectionInteraction;

impl SshConnectionInteraction for NonInteractiveForwardConnectionInteraction {
    async fn phase_changed(
        &mut self,
        _phase: ConnectionPhase,
        _route_stage: ConnectionRouteStage,
    ) -> Result<(), TransportError> {
        Ok(())
    }

    async fn answer_keyboard_interactive(
        &mut self,
        _request: KeyboardInteractiveRequest,
    ) -> Result<Vec<Zeroizing<String>>, TransportError> {
        Err(TransportError::AuthenticationRejected)
    }
}

#[derive(Clone)]
enum ForwardHeartbeatLiveness {
    Disabled,
    Watching(watch::Receiver<bool>),
}

struct ForwardHeartbeatTasks {
    liveness: ForwardHeartbeatLiveness,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ForwardHeartbeatTasks {
    fn start(
        policy: Option<&ResolvedTransportKeepalivePolicy>,
        heartbeats: Vec<RoutedTransportHeartbeat>,
    ) -> Self {
        let Some(policy) = policy else {
            return Self {
                liveness: ForwardHeartbeatLiveness::Disabled,
                tasks: Vec::new(),
            };
        };
        let (failure_tx, failure) = watch::channel(false);
        let interval = Duration::from_secs(u64::from(policy.interval_seconds));
        let reply_timeout = Duration::from_secs(u64::from(policy.reply_timeout_seconds));
        let failure_threshold = policy.failure_threshold;
        let tasks = heartbeats
            .into_iter()
            .map(|heartbeat| {
                let failure_tx = failure_tx.clone();
                tokio::spawn(async move {
                    let mut next_due = heartbeat.first_due;
                    let mut failures = 0_u8;
                    loop {
                        tokio::time::sleep_until(next_due).await;
                        let result = timeout(reply_timeout, heartbeat.handle.request_reply()).await;
                        failures = if matches!(result, Ok(Ok(()))) {
                            0
                        } else {
                            failures.saturating_add(1)
                        };
                        if failures >= failure_threshold {
                            failure_tx.send_replace(true);
                            break;
                        }
                        next_due = Instant::now() + interval;
                    }
                })
            })
            .collect();
        Self {
            liveness: ForwardHeartbeatLiveness::Watching(failure),
            tasks,
        }
    }
}

impl Drop for ForwardHeartbeatTasks {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

struct ProductionForwardTransport<T>
where
    T: ForwardTransport,
{
    transport: Option<T>,
    heartbeat_tasks: ForwardHeartbeatTasks,
}

impl<T> ProductionForwardTransport<T>
where
    T: ForwardTransport,
{
    fn transport_mut(&mut self) -> ForwardResult<&mut T> {
        self.transport.as_mut().ok_or_else(|| {
            ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::Protocol,
                "transport",
                "the forward transport is no longer available",
            ))
        })
    }

    async fn heartbeat_failed(liveness: &mut ForwardHeartbeatLiveness) -> ForwardResult<()> {
        let ForwardHeartbeatLiveness::Watching(receiver) = liveness else {
            return std::future::pending().await;
        };
        loop {
            if *receiver.borrow() {
                return Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::TransportLost,
                    "transport-keepalive",
                    "the forward transport keepalive failure threshold was reached",
                )));
            }
            receiver.changed().await.map_err(|_| {
                ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::TransportLost,
                    "transport-keepalive",
                    "the forward transport keepalive scheduler stopped unexpectedly",
                ))
            })?;
        }
    }
}

impl<T> ForwardTransport for ProductionForwardTransport<T>
where
    T: ForwardTransport + 'static,
{
    fn open_direct_tcpip<'a>(
        &'a mut self,
        target_host: &'a str,
        target_port: u16,
        originator_address: &'a str,
        originator_port: u16,
    ) -> BoxFuture<'a, ForwardResult<BoxedForwardIo>> {
        Box::pin(async move {
            let mut heartbeat = self.heartbeat_tasks.liveness.clone();
            let transport = self.transport_mut()?;
            tokio::select! {
                result = transport.open_direct_tcpip(
                    target_host,
                    target_port,
                    originator_address,
                    originator_port,
                ) => result,
                failure = Self::heartbeat_failed(&mut heartbeat) => failure.map(|()| unreachable!()),
            }
        })
    }

    fn request_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<u16>> {
        Box::pin(async move {
            let mut heartbeat = self.heartbeat_tasks.liveness.clone();
            let transport = self.transport_mut()?;
            tokio::select! {
                result = transport.request_remote_forward(bind_address, bind_port) => result,
                failure = Self::heartbeat_failed(&mut heartbeat) => failure.map(|()| unreachable!()),
            }
        })
    }

    fn cancel_remote_forward<'a>(
        &'a mut self,
        bind_address: &'a str,
        bind_port: u16,
    ) -> BoxFuture<'a, ForwardResult<()>> {
        Box::pin(async move {
            self.transport_mut()?
                .cancel_remote_forward(bind_address, bind_port)
                .await
        })
    }

    fn next_forwarded_tcpip(
        &mut self,
    ) -> BoxFuture<'_, ForwardResult<Option<IncomingRemoteForward>>> {
        Box::pin(async move {
            let mut heartbeat = self.heartbeat_tasks.liveness.clone();
            let transport = self.transport_mut()?;
            tokio::select! {
                channel = transport.next_forwarded_tcpip() => channel,
                failure = Self::heartbeat_failed(&mut heartbeat) => failure.map(|()| unreachable!()),
            }
        })
    }

    fn wait_failure(&mut self) -> BoxFuture<'_, ForwardResult<()>> {
        Box::pin(async move {
            let transport = self.transport.as_mut().ok_or_else(|| {
                ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::Protocol,
                    "transport",
                    "the forward transport is no longer available",
                ))
            })?;
            let liveness = &mut self.heartbeat_tasks.liveness;
            tokio::select! {
                failure = transport.wait_failure() => failure,
                failure = Self::heartbeat_failed(liveness) => failure.map(|()| unreachable!()),
            }
        })
    }

    fn disconnect(mut self: Box<Self>) -> BoxFuture<'static, ForwardResult<()>> {
        Box::pin(async move {
            let transport = self.transport.take().ok_or_else(|| {
                ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::Protocol,
                    "transport-disconnect",
                    "the forward transport was already disconnected",
                ))
            })?;
            drop(self.heartbeat_tasks);
            Box::new(transport).disconnect().await
        })
    }
}

#[derive(Clone)]
pub(crate) struct ForwardSessionHandle {
    stop_tx: mpsc::Sender<StopRequest>,
    summary_rx: watch::Receiver<ForwardSessionSummary>,
}

impl ForwardSessionHandle {
    #[cfg(test)]
    pub(crate) fn start<F>(
        session_id: impl Into<String>,
        rule_id: Option<String>,
        rule_revision: Option<u64>,
        rule: PortForwardRule,
        limits: ForwardRuntimeLimits,
        factory: Arc<F>,
    ) -> ForwardResult<Self>
    where
        F: ForwardTransportFactory + ?Sized,
    {
        Self::start_with_host_fence(
            session_id,
            rule_id,
            rule_revision,
            rule,
            limits,
            factory,
            None,
        )
    }

    fn start_with_host_fence<F>(
        session_id: impl Into<String>,
        rule_id: Option<String>,
        rule_revision: Option<u64>,
        rule: PortForwardRule,
        limits: ForwardRuntimeLimits,
        factory: Arc<F>,
        expected_host_state_version: Option<u64>,
    ) -> ForwardResult<Self>
    where
        F: ForwardTransportFactory + ?Sized,
    {
        let session_id = bounded_id(session_id.into())?;
        rule.validate()?;
        let limits = limits.validate()?;
        let generation = ForwardGeneration::new(1)?;
        let summary = ForwardSessionSummary {
            session_id,
            host_ref: rule.host_ref().to_owned(),
            rule_id,
            rule_revision,
            rule_snapshot: rule.clone(),
            started_at_unix_ms: unix_time_ms(),
            generation,
            state_revision: 1,
            state: ForwardSessionState::Starting,
            actual_bind: None,
            child_count: 0,
            listener_to_target_bytes: 0,
            target_to_listener_bytes: 0,
            failure: None,
            last_child_failure: None,
            cleanup: ForwardCleanupFacts::default(),
        };
        let (summary_tx, summary_rx) = watch::channel(summary.clone());
        let (stop_tx, stop_rx) = mpsc::channel(1);
        tauri::async_runtime::spawn(run_session(
            summary,
            summary_tx,
            stop_rx,
            rule,
            limits,
            factory,
            expected_host_state_version,
        ));
        Ok(Self {
            stop_tx,
            summary_rx,
        })
    }

    pub(crate) fn snapshot(&self) -> ForwardSessionSummary {
        self.summary_rx.borrow().clone()
    }

    pub(crate) async fn changed(&mut self) -> ForwardSessionSummary {
        let _ = self.summary_rx.changed().await;
        self.snapshot()
    }

    pub(crate) async fn stop(
        &self,
        generation: ForwardGeneration,
    ) -> ForwardResult<ForwardSessionSummary> {
        if self.snapshot().generation != generation {
            return Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::Protocol,
                "generation",
                "stale forward generation",
            )));
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .stop_tx
            .send(StopRequest {
                generation,
                reply_tx,
            })
            .await
            .is_err()
        {
            let snapshot = self.snapshot();
            if matches!(
                snapshot.state,
                ForwardSessionState::Failed | ForwardSessionState::Stopped
            ) {
                return Ok(snapshot);
            }
            return Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::Protocol,
                "stop",
                "forward actor is unavailable",
            )));
        }
        reply_rx.await.map_err(|_| {
            ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::Protocol,
                "stop",
                "forward actor stopped before acknowledging cleanup",
            ))
        })
    }
}

/// Minimal process-level owner used by the Tauri command and lifecycle layers.
/// It does not define public DTOs: callers translate their Core API request to
/// a validated [`PortForwardRule`], then retain the returned generation fence.
#[derive(Clone)]
pub(crate) struct ForwardSessionService {
    factory: Arc<dyn ForwardTransportFactory>,
    sessions: Arc<Mutex<BTreeMap<String, ForwardSessionHandle>>>,
    retained_for_exit_once: Arc<Mutex<BTreeMap<String, (ForwardGeneration, u64)>>>,
}

impl ForwardSessionService {
    pub(crate) fn production(
        hosts: HostService,
        vault: VaultService,
        transient_credentials: TransientCredentialService,
        ssh_agent: SshAgentService,
    ) -> Self {
        Self::with_factory(Arc::new(ProductionForwardTransportFactory::new(
            hosts,
            vault,
            transient_credentials,
            ssh_agent,
        )))
    }

    fn with_factory(factory: Arc<dyn ForwardTransportFactory>) -> Self {
        Self {
            factory,
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            retained_for_exit_once: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) fn start_session(
        &self,
        session_id: impl Into<String>,
        rule_id: Option<String>,
        rule_revision: Option<u64>,
        rule: PortForwardRule,
        limits: ForwardRuntimeLimits,
    ) -> ForwardResult<ForwardSessionHandle> {
        self.start_session_with_host_fence(session_id, rule_id, rule_revision, rule, limits, None)
    }

    fn start_session_with_host_fence(
        &self,
        session_id: impl Into<String>,
        rule_id: Option<String>,
        rule_revision: Option<u64>,
        rule: PortForwardRule,
        limits: ForwardRuntimeLimits,
        expected_host_state_version: Option<u64>,
    ) -> ForwardResult<ForwardSessionHandle> {
        self.start_session_with_factory(
            session_id,
            rule_id,
            rule_revision,
            rule,
            limits,
            Arc::clone(&self.factory),
            expected_host_state_version,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_session_with_factory(
        &self,
        session_id: impl Into<String>,
        rule_id: Option<String>,
        rule_revision: Option<u64>,
        rule: PortForwardRule,
        limits: ForwardRuntimeLimits,
        factory: Arc<dyn ForwardTransportFactory>,
        expected_host_state_version: Option<u64>,
    ) -> ForwardResult<ForwardSessionHandle> {
        let session_id = bounded_id(session_id.into())?;
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sessions.contains_key(&session_id) {
            return Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::InvalidRule,
                "session-start",
                "the forward session id is already active",
            )));
        }
        let handle = ForwardSessionHandle::start_with_host_fence(
            session_id.clone(),
            rule_id,
            rule_revision,
            rule,
            limits,
            factory,
            expected_host_state_version,
        )?;
        sessions.insert(session_id, handle.clone());
        Ok(handle)
    }

    pub(crate) fn snapshots(&self) -> Vec<ForwardSessionSummary> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(ForwardSessionHandle::snapshot)
            .collect()
    }

    pub(crate) fn wire_summaries(&self) -> ForwardResult<Vec<wire::ForwardSessionSummary>> {
        self.snapshots()
            .into_iter()
            .map(map_forward_summary)
            .collect()
    }

    /// Starts a plugin-owned forward by opening child channels on an existing user SSH session.
    /// The supplied channel capability cannot authenticate, reconnect, or disconnect its parent.
    pub(crate) fn start_plugin_session(
        &self,
        session_id: wire::ForwardSessionId,
        rule: PortForwardRule,
        channels: SharedSessionChannels,
        cancelled: watch::Receiver<bool>,
    ) -> ForwardResult<ForwardSessionSummary> {
        let factory: Arc<dyn ForwardTransportFactory> = Arc::new(SharedForwardTransportFactory {
            channels,
            cancelled,
        });
        let handle = self.start_session_with_factory(
            session_id.to_string(),
            None,
            None,
            rule,
            ForwardRuntimeLimits::default(),
            factory,
            None,
        )?;
        Ok(handle.snapshot())
    }

    /// Stops only the exact session and generation selected by the owning broker.
    pub(crate) async fn stop_plugin_session(
        &self,
        session_id: &wire::ForwardSessionId,
        expected_generation: wire::WireSequence,
    ) -> ForwardResult<ForwardSessionSummary> {
        let generation = ForwardGeneration::new(expected_generation.get())?;
        self.stop_session(session_id.as_str(), generation).await
    }

    pub(crate) fn plugin_session_snapshot(
        &self,
        session_id: &wire::ForwardSessionId,
        expected_generation: wire::WireSequence,
    ) -> ForwardResult<ForwardSessionSummary> {
        let generation = ForwardGeneration::new(expected_generation.get())?;
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id.as_str())
            .map(ForwardSessionHandle::snapshot)
            .filter(|summary| summary.generation == generation)
            .ok_or_else(|| {
                ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::HostUnavailable,
                    "session-snapshot",
                    "the forward session does not exist",
                ))
            })
    }

    pub(crate) async fn stop_session(
        &self,
        session_id: &str,
        generation: ForwardGeneration,
    ) -> ForwardResult<ForwardSessionSummary> {
        let handle = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
            .ok_or_else(|| {
                ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::HostUnavailable,
                    "session-stop",
                    "the forward session does not exist",
                ))
            })?;
        let summary = handle.stop(generation).await?;
        if !summary.cleanup.uncertain {
            self.sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(session_id);
            self.retained_for_exit_once
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(session_id);
        }
        Ok(summary)
    }

    pub(crate) fn exit_blockers(&self) -> Vec<String> {
        let retained = self
            .retained_for_exit_once
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.snapshots()
            .into_iter()
            .filter(|summary| match summary.state {
                ForwardSessionState::Starting
                | ForwardSessionState::Running
                | ForwardSessionState::Stopping => true,
                ForwardSessionState::Failed if summary.cleanup.uncertain => retained
                    .get(&summary.session_id)
                    .is_none_or(|fence| *fence != (summary.generation, summary.state_revision)),
                ForwardSessionState::Failed | ForwardSessionState::Stopped => false,
            })
            .map(|summary| summary.session_id)
            .collect()
    }

    pub(crate) fn retain_uncertain_cleanup_for_exit_once(
        &self,
        session_id: &str,
        generation: ForwardGeneration,
        state_revision: u64,
        confirmed: bool,
    ) -> ForwardResult<ForwardSessionSummary> {
        if !confirmed {
            return Err(ForwardRuntimeError::invalid(
                "retaining uncertain forward cleanup requires explicit confirmation",
            ));
        }
        let summary = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .map(ForwardSessionHandle::snapshot)
            .ok_or_else(|| {
                ForwardRuntimeError::from_failure(ForwardFailure::new(
                    ForwardFailureCode::HostUnavailable,
                    "cleanup-retain",
                    "the forward session does not exist",
                ))
            })?;
        if summary.generation != generation
            || summary.state_revision != state_revision
            || summary.state != ForwardSessionState::Failed
            || !summary.cleanup.uncertain
        {
            return Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::CleanupUncertain,
                "cleanup-retain",
                "the uncertain cleanup confirmation fence is stale or invalid",
            )));
        }
        self.retained_for_exit_once
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_owned(), (generation, state_revision));
        Ok(summary)
    }

    fn consume_retained_exit(&self, summary: &ForwardSessionSummary) -> bool {
        let mut retained = self
            .retained_for_exit_once
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        matches!(
            retained.remove(&summary.session_id),
            Some(fence) if fence == (summary.generation, summary.state_revision)
        )
    }

    pub(crate) async fn shutdown_all(&self) -> ForwardResult<Vec<ForwardSessionSummary>> {
        let handles = {
            let sessions = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sessions
                .iter()
                .map(|(session_id, handle)| (session_id.clone(), handle.clone()))
                .collect::<Vec<_>>()
        };
        let mut summaries = Vec::with_capacity(handles.len());
        let mut first_error = None;
        for (session_id, handle) in handles {
            let before = handle.snapshot();
            if before.state == ForwardSessionState::Failed && before.cleanup.uncertain {
                if self.consume_retained_exit(&before) {
                    summaries.push(before);
                } else if first_error.is_none() {
                    first_error = Some(cleanup_uncertain_error());
                }
                continue;
            }
            if matches!(
                before.state,
                ForwardSessionState::Failed | ForwardSessionState::Stopped
            ) {
                self.sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&session_id);
                summaries.push(before);
                continue;
            }
            let generation = before.generation;
            match handle.stop(generation).await {
                Ok(summary) if summary.cleanup.uncertain => {
                    summaries.push(summary);
                    if first_error.is_none() {
                        first_error = Some(cleanup_uncertain_error());
                    }
                }
                Ok(summary) => {
                    self.sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&session_id);
                    summaries.push(summary);
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(summaries), Err)
    }
}

fn cleanup_uncertain_error() -> ForwardRuntimeError {
    ForwardRuntimeError::from_failure(ForwardFailure::new(
        ForwardFailureCode::CleanupUncertain,
        "cleanup",
        "forward cleanup remains uncertain and requires explicit user confirmation",
    ))
}

struct StopRequest {
    generation: ForwardGeneration,
    reply_tx: oneshot::Sender<ForwardSessionSummary>,
}

struct OpenRequest {
    target_host: String,
    target_port: u16,
    originator_address: String,
    originator_port: u16,
    reply_tx: oneshot::Sender<ForwardResult<BoxedForwardIo>>,
}

enum RunEnd {
    Stop(StopRequest),
    Failed(ForwardFailure),
}

async fn run_session<F>(
    mut summary: ForwardSessionSummary,
    summary_tx: watch::Sender<ForwardSessionSummary>,
    mut stop_rx: mpsc::Receiver<StopRequest>,
    rule: PortForwardRule,
    limits: ForwardRuntimeLimits,
    factory: Arc<F>,
    expected_host_state_version: Option<u64>,
) where
    F: ForwardTransportFactory + ?Sized,
{
    let generation = summary.generation;
    let traffic = Arc::new(ForwardTrafficCounters::default());
    let connect = timeout(
        limits.connect_timeout,
        factory.connect(rule.host_ref(), generation, expected_host_state_version),
    );
    tokio::pin!(connect);
    let transport = tokio::select! {
        biased;
        Some(stop) = stop_rx.recv() => {
            transition(&mut summary, ForwardSessionState::Stopping);
            publish(&summary_tx, &summary);
            transition(&mut summary, ForwardSessionState::Stopped);
            publish(&summary_tx, &summary);
            let _ = stop.reply_tx.send(summary);
            return;
        }
        result = &mut connect => result.unwrap_or_else(|_| Err(ForwardRuntimeError::from_failure(
            ForwardFailure::new(
                ForwardFailureCode::ConnectTimeout,
                "transport-connect",
                "connecting the forward SSH transport timed out",
            )
        ))),
    };
    let mut transport = match transport {
        Ok(transport) => transport,
        Err(error) => {
            fail_summary(&mut summary, error.failure);
            publish(&summary_tx, &summary);
            answer_pending_stop(&mut stop_rx, &summary).await;
            return;
        }
    };

    let (mut children, end, remote_binding) = match &rule {
        PortForwardRule::Local {
            local_bind_address,
            local_listen_port,
            remote_target_host,
            remote_target_port,
            ..
        } => {
            let listener = match bind_local_listener(
                &mut *transport,
                SocketAddr::new(*local_bind_address, *local_listen_port),
                "local-listener",
                "failed to bind the local loopback listener",
            )
            .await
            {
                Ok(listener) => listener,
                Err(failure) => {
                    let mut children = JoinSet::new();
                    cleanup(
                        &mut summary,
                        &summary_tx,
                        &mut children,
                        transport,
                        None,
                        limits.child_shutdown_timeout,
                    )
                    .await;
                    fail_summary(&mut summary, failure);
                    publish(&summary_tx, &summary);
                    answer_pending_stop(&mut stop_rx, &summary).await;
                    return;
                }
            };
            let actual = listener
                .local_addr()
                .expect("bound TCP listener has an address");
            summary.actual_bind = Some(ActualForwardBind::Local { address: actual });
            transition(&mut summary, ForwardSessionState::Running);
            publish(&summary_tx, &summary);
            let (open_tx, open_rx) = mpsc::channel(limits.max_concurrent_children);
            let mut children = JoinSet::new();
            let end = run_local_listener(
                &mut summary,
                &summary_tx,
                &mut stop_rx,
                &mut *transport,
                listener,
                &mut children,
                open_tx,
                open_rx,
                FixedTarget {
                    host: remote_target_host.clone(),
                    port: *remote_target_port,
                },
                Arc::clone(&traffic),
                limits,
            )
            .await;
            (children, end, None)
        }
        PortForwardRule::Dynamic {
            local_bind_address,
            local_listen_port,
            ..
        } => {
            let listener = match bind_local_listener(
                &mut *transport,
                SocketAddr::new(*local_bind_address, *local_listen_port),
                "dynamic-listener",
                "failed to bind the dynamic loopback listener",
            )
            .await
            {
                Ok(listener) => listener,
                Err(failure) => {
                    let mut children = JoinSet::new();
                    cleanup(
                        &mut summary,
                        &summary_tx,
                        &mut children,
                        transport,
                        None,
                        limits.child_shutdown_timeout,
                    )
                    .await;
                    fail_summary(&mut summary, failure);
                    publish(&summary_tx, &summary);
                    answer_pending_stop(&mut stop_rx, &summary).await;
                    return;
                }
            };
            let actual = listener
                .local_addr()
                .expect("bound TCP listener has an address");
            summary.actual_bind = Some(ActualForwardBind::Local { address: actual });
            transition(&mut summary, ForwardSessionState::Running);
            publish(&summary_tx, &summary);
            let (open_tx, open_rx) = mpsc::channel(limits.max_concurrent_children);
            let mut children = JoinSet::new();
            let end = run_dynamic_listener(
                &mut summary,
                &summary_tx,
                &mut stop_rx,
                &mut *transport,
                listener,
                &mut children,
                open_tx,
                open_rx,
                Arc::clone(&traffic),
                limits,
            )
            .await;
            (children, end, None)
        }
        PortForwardRule::Remote {
            remote_bind_address,
            remote_listen_port,
            local_target_host,
            local_target_port,
            ..
        } => {
            let actual_port = match transport
                .request_remote_forward(remote_bind_address, *remote_listen_port)
                .await
            {
                Ok(port) => port,
                Err(error) => {
                    if error.failure.code == ForwardFailureCode::RemoteRegistrationUncertain {
                        summary.cleanup.listener_closed_or_remote_cancelled =
                            CleanupStepState::Uncertain;
                        summary.cleanup.uncertain = true;
                    }
                    let mut children = JoinSet::new();
                    cleanup(
                        &mut summary,
                        &summary_tx,
                        &mut children,
                        transport,
                        None,
                        limits.child_shutdown_timeout,
                    )
                    .await;
                    fail_summary(&mut summary, error.failure);
                    publish(&summary_tx, &summary);
                    answer_pending_stop(&mut stop_rx, &summary).await;
                    return;
                }
            };
            summary.actual_bind = Some(ActualForwardBind::Remote {
                address: remote_bind_address.clone(),
                port: actual_port,
            });
            transition(&mut summary, ForwardSessionState::Running);
            publish(&summary_tx, &summary);
            let binding = (remote_bind_address.clone(), actual_port);
            let mut children = JoinSet::new();
            let end = run_remote_listener(
                &mut summary,
                &summary_tx,
                &mut stop_rx,
                &mut *transport,
                &mut children,
                FixedTarget {
                    host: local_target_host.clone(),
                    port: *local_target_port,
                },
                Arc::clone(&traffic),
                limits,
            )
            .await;
            (children, end, Some(binding))
        }
    };

    transition(&mut summary, ForwardSessionState::Stopping);
    publish(&summary_tx, &summary);
    cleanup(
        &mut summary,
        &summary_tx,
        &mut children,
        transport,
        remote_binding,
        limits.child_shutdown_timeout,
    )
    .await;

    match end {
        RunEnd::Stop(stop) => {
            if stop.generation != generation {
                fail_summary(
                    &mut summary,
                    ForwardFailure::new(
                        ForwardFailureCode::Protocol,
                        "generation",
                        "stale forward generation",
                    ),
                );
            } else if summary.cleanup.uncertain {
                fail_summary(
                    &mut summary,
                    ForwardFailure::new(
                        ForwardFailureCode::CleanupUncertain,
                        "cleanup",
                        "forward cleanup outcome is uncertain",
                    ),
                );
            } else {
                transition(&mut summary, ForwardSessionState::Stopped);
            }
            publish(&summary_tx, &summary);
            let _ = stop.reply_tx.send(summary);
        }
        RunEnd::Failed(failure) => {
            fail_summary(&mut summary, failure);
            publish(&summary_tx, &summary);
            answer_pending_stop(&mut stop_rx, &summary).await;
        }
    }
}

async fn bind_local_listener(
    transport: &mut dyn ForwardTransport,
    address: SocketAddr,
    stage: &'static str,
    detail: &'static str,
) -> Result<TcpListener, ForwardFailure> {
    tokio::select! {
        biased;
        failure = transport.wait_failure() => Err(failure.err().map_or_else(
            || ForwardFailure::new(
                ForwardFailureCode::TransportLost,
                "transport",
                "the forward transport stopped unexpectedly",
            ),
            |error| error.failure,
        )),
        listener = TcpListener::bind(address) => listener.map_err(|_| {
            ForwardFailure::new(ForwardFailureCode::Bind, stage, detail)
        }),
    }
}

#[derive(Clone)]
struct FixedTarget {
    host: String,
    port: u16,
}

#[allow(clippy::too_many_arguments)]
async fn run_local_listener(
    summary: &mut ForwardSessionSummary,
    summary_tx: &watch::Sender<ForwardSessionSummary>,
    stop_rx: &mut mpsc::Receiver<StopRequest>,
    transport: &mut dyn ForwardTransport,
    listener: TcpListener,
    children: &mut JoinSet<ForwardResult<()>>,
    open_tx: mpsc::Sender<OpenRequest>,
    mut open_rx: mpsc::Receiver<OpenRequest>,
    target: FixedTarget,
    traffic: Arc<ForwardTrafficCounters>,
    limits: ForwardRuntimeLimits,
) -> RunEnd {
    let mut traffic_tick = tokio::time::interval(TRAFFIC_PUBLISH_INTERVAL);
    traffic_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            Some(stop) = stop_rx.recv() => return RunEnd::Stop(stop),
            failure = transport.wait_failure() => {
                return RunEnd::Failed(failure.err().map_or_else(
                    || ForwardFailure::new(
                        ForwardFailureCode::TransportLost,
                        "transport",
                        "the forward transport stopped unexpectedly",
                    ),
                    |error| error.failure,
                ));
            }
            Some(joined) = children.join_next(), if !children.is_empty() => {
                record_child_result(summary, joined);
                publish(summary_tx, summary);
            }
            _ = traffic_tick.tick() => sync_traffic_summary(summary, summary_tx, &traffic),
            Some(request) = open_rx.recv() => {
                if let Some(stop) = service_open_request(transport, request, limits.connect_timeout, stop_rx).await {
                    return RunEnd::Stop(stop);
                }
            }
            accepted = listener.accept(), if children.len() < limits.max_concurrent_children => {
                match accepted {
                    Ok((client, peer)) => {
                        let target = target.clone();
                        let open_tx = open_tx.clone();
                        let traffic = Arc::clone(&traffic);
                        children.spawn(async move {
                            let channel = request_channel(&open_tx, target, peer).await?;
                            relay_with_idle_timeout(
                                client,
                                channel,
                                limits.idle_timeout,
                                traffic,
                                true,
                            ).await
                        });
                        summary.child_count = children.len();
                        summary.state_revision = summary.state_revision.saturating_add(1);
                        publish(summary_tx, summary);
                    }
                    Err(_) => return RunEnd::Failed(ForwardFailure::new(
                        ForwardFailureCode::Bind,
                        "local-accept",
                        "the local listener stopped accepting connections",
                    )),
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_dynamic_listener(
    summary: &mut ForwardSessionSummary,
    summary_tx: &watch::Sender<ForwardSessionSummary>,
    stop_rx: &mut mpsc::Receiver<StopRequest>,
    transport: &mut dyn ForwardTransport,
    listener: TcpListener,
    children: &mut JoinSet<ForwardResult<()>>,
    open_tx: mpsc::Sender<OpenRequest>,
    mut open_rx: mpsc::Receiver<OpenRequest>,
    traffic: Arc<ForwardTrafficCounters>,
    limits: ForwardRuntimeLimits,
) -> RunEnd {
    let mut traffic_tick = tokio::time::interval(TRAFFIC_PUBLISH_INTERVAL);
    traffic_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            Some(stop) = stop_rx.recv() => return RunEnd::Stop(stop),
            failure = transport.wait_failure() => {
                return RunEnd::Failed(failure.err().map_or_else(
                    || ForwardFailure::new(
                        ForwardFailureCode::TransportLost,
                        "transport",
                        "the forward transport stopped unexpectedly",
                    ),
                    |error| error.failure,
                ));
            }
            Some(joined) = children.join_next(), if !children.is_empty() => {
                record_child_result(summary, joined);
                publish(summary_tx, summary);
            }
            _ = traffic_tick.tick() => sync_traffic_summary(summary, summary_tx, &traffic),
            Some(request) = open_rx.recv() => {
                if let Some(stop) = service_open_request(transport, request, limits.connect_timeout, stop_rx).await {
                    return RunEnd::Stop(stop);
                }
            }
            accepted = listener.accept(), if children.len() < limits.max_concurrent_children => {
                match accepted {
                    Ok((client, peer)) => {
                        let open_tx = open_tx.clone();
                        let traffic = Arc::clone(&traffic);
                        children.spawn(run_socks5_child(
                            client,
                            peer,
                            open_tx,
                            limits.socks_handshake_timeout,
                            limits.idle_timeout,
                            traffic,
                        ));
                        summary.child_count = children.len();
                        summary.state_revision = summary.state_revision.saturating_add(1);
                        publish(summary_tx, summary);
                    }
                    Err(_) => return RunEnd::Failed(ForwardFailure::new(
                        ForwardFailureCode::Bind,
                        "dynamic-accept",
                        "the dynamic listener stopped accepting connections",
                    )),
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_remote_listener(
    summary: &mut ForwardSessionSummary,
    summary_tx: &watch::Sender<ForwardSessionSummary>,
    stop_rx: &mut mpsc::Receiver<StopRequest>,
    transport: &mut dyn ForwardTransport,
    children: &mut JoinSet<ForwardResult<()>>,
    target: FixedTarget,
    traffic: Arc<ForwardTrafficCounters>,
    limits: ForwardRuntimeLimits,
) -> RunEnd {
    let mut traffic_tick = tokio::time::interval(TRAFFIC_PUBLISH_INTERVAL);
    traffic_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            Some(stop) = stop_rx.recv() => return RunEnd::Stop(stop),
            Some(joined) = children.join_next(), if !children.is_empty() => {
                record_child_result(summary, joined);
                publish(summary_tx, summary);
            }
            _ = traffic_tick.tick() => sync_traffic_summary(summary, summary_tx, &traffic),
            incoming = transport.next_forwarded_tcpip(), if children.len() < limits.max_concurrent_children => {
                match incoming {
                    Ok(Some(incoming)) => {
                        let target = target.clone();
                        let traffic = Arc::clone(&traffic);
                        children.spawn(async move {
                            let local = timeout(
                                limits.connect_timeout,
                                TcpStream::connect((target.host.as_str(), target.port)),
                            )
                            .await
                            .map_err(|_| ForwardRuntimeError::from_failure(ForwardFailure::new(
                                ForwardFailureCode::ConnectTimeout,
                                "remote-local-target",
                                "connecting the remote forward to its local target timed out",
                            )))?
                            .map_err(|_| ForwardRuntimeError::from_failure(ForwardFailure::new(
                                ForwardFailureCode::TargetConnect,
                                "remote-local-target",
                                "failed to connect the remote forward to its local target",
                            )))?;
                            relay_with_idle_timeout(
                                local,
                                incoming.stream,
                                limits.idle_timeout,
                                traffic,
                                false,
                            ).await
                        });
                        summary.child_count = children.len();
                        summary.state_revision = summary.state_revision.saturating_add(1);
                        publish(summary_tx, summary);
                    }
                    Ok(None) => return RunEnd::Failed(ForwardFailure::new(
                        ForwardFailureCode::TransportLost,
                        "remote-listener",
                        "the SSH transport stopped delivering remote-forward channels",
                    )),
                    Err(error) => return RunEnd::Failed(error.failure),
                }
            }
        }
    }
}

async fn service_open_request(
    transport: &mut dyn ForwardTransport,
    request: OpenRequest,
    connect_timeout: Duration,
    stop_rx: &mut mpsc::Receiver<StopRequest>,
) -> Option<StopRequest> {
    let open = timeout(
        connect_timeout,
        transport.open_direct_tcpip(
            &request.target_host,
            request.target_port,
            &request.originator_address,
            request.originator_port,
        ),
    );
    tokio::pin!(open);
    tokio::select! {
        biased;
        Some(stop) = stop_rx.recv() => {
            let _ = request.reply_tx.send(Err(ForwardRuntimeError::from_failure(
                ForwardFailure::new(
                    ForwardFailureCode::CleanupUncertain,
                    "channel-open",
                    "forward stopped while a channel was opening",
                )
            )));
            Some(stop)
        }
        result = &mut open => {
            let result = result.unwrap_or_else(|_| Err(ForwardRuntimeError::from_failure(
                ForwardFailure::new(
                    ForwardFailureCode::ConnectTimeout,
                    "channel-open",
                    "opening the SSH forwarded channel timed out",
                )
            )));
            let _ = request.reply_tx.send(result);
            None
        }
    }
}

async fn request_channel(
    open_tx: &mpsc::Sender<OpenRequest>,
    target: FixedTarget,
    peer: SocketAddr,
) -> ForwardResult<BoxedForwardIo> {
    let (reply_tx, reply_rx) = oneshot::channel();
    open_tx
        .send(OpenRequest {
            target_host: target.host,
            target_port: target.port,
            originator_address: peer.ip().to_string(),
            originator_port: peer.port(),
            reply_tx,
        })
        .await
        .map_err(|_| {
            ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::Protocol,
                "channel-open",
                "the forward actor stopped before opening a channel",
            ))
        })?;
    reply_rx.await.map_err(|_| {
        ForwardRuntimeError::from_failure(ForwardFailure::new(
            ForwardFailureCode::Protocol,
            "channel-open",
            "the forward actor dropped the channel-open response",
        ))
    })?
}

async fn run_socks5_child(
    mut client: TcpStream,
    peer: SocketAddr,
    open_tx: mpsc::Sender<OpenRequest>,
    handshake_timeout: Duration,
    idle_timeout: Duration,
    traffic: Arc<ForwardTrafficCounters>,
) -> ForwardResult<()> {
    let target = match timeout(handshake_timeout, read_socks5_request(&mut client)).await {
        Ok(Ok(target)) => target,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::SocksTruncated,
                "socks5-handshake",
                "the SOCKS5 handshake timed out before it was complete",
            )));
        }
    };
    let (reply_tx, reply_rx) = oneshot::channel();
    open_tx
        .send(OpenRequest {
            target_host: target.host,
            target_port: target.port,
            originator_address: peer.ip().to_string(),
            originator_port: peer.port(),
            reply_tx,
        })
        .await
        .map_err(|_| {
            ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::Protocol,
                "socks5-connect",
                "the forward actor stopped before opening a SOCKS5 channel",
            ))
        })?;
    let channel = match reply_rx.await {
        Ok(Ok(channel)) => channel,
        Ok(Err(error)) => {
            let _ = write_socks5_reply(&mut client, socks_reply_for_failure(&error.failure)).await;
            return Err(error);
        }
        Err(_) => {
            let failure = ForwardFailure::new(
                ForwardFailureCode::Protocol,
                "socks5-connect",
                "the forward actor dropped the SOCKS5 channel response",
            );
            let _ = write_socks5_reply(&mut client, 1).await;
            return Err(ForwardRuntimeError::from_failure(failure));
        }
    };
    write_socks5_reply(&mut client, 0).await?;
    relay_with_idle_timeout(client, channel, idle_timeout, traffic, true).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SocksTarget {
    host: String,
    port: u16,
}

async fn read_socks5_request<S>(stream: &mut S) -> ForwardResult<SocksTarget>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut greeting = [0_u8; 2];
    read_exact_socks(stream, &mut greeting).await?;
    if greeting[0] != SOCKS_VERSION {
        return Err(socks_failure(
            ForwardFailureCode::Protocol,
            "invalid SOCKS5 greeting",
        ));
    }
    if greeting[1] == 0 {
        let _ = stream
            .write_all(&[SOCKS_VERSION, SOCKS_NO_ACCEPTABLE_AUTH])
            .await;
        return Err(socks_failure(
            ForwardFailureCode::SocksAuthenticationRejected,
            "SOCKS5 greeting offered no authentication method",
        ));
    }
    let mut methods = vec![0_u8; usize::from(greeting[1])];
    read_exact_socks(stream, &mut methods).await?;
    if !methods.contains(&SOCKS_NO_AUTH) {
        let _ = stream
            .write_all(&[SOCKS_VERSION, SOCKS_NO_ACCEPTABLE_AUTH])
            .await;
        return Err(socks_failure(
            ForwardFailureCode::SocksAuthenticationRejected,
            "SOCKS5 authentication method is not supported",
        ));
    }
    stream
        .write_all(&[SOCKS_VERSION, SOCKS_NO_AUTH])
        .await
        .map_err(|_| {
            socks_failure(
                ForwardFailureCode::Protocol,
                "failed to select SOCKS5 authentication",
            )
        })?;

    let mut request = [0_u8; 4];
    read_exact_socks(stream, &mut request).await?;
    if request[0] != SOCKS_VERSION || request[2] != 0 {
        let _ = write_socks5_reply(stream, 1).await;
        return Err(socks_failure(
            ForwardFailureCode::Protocol,
            "invalid SOCKS5 request header",
        ));
    }
    if request[1] != SOCKS_COMMAND_CONNECT {
        let detail = match request[1] {
            SOCKS_COMMAND_BIND => "SOCKS5 BIND is not supported",
            SOCKS_COMMAND_UDP_ASSOCIATE => "SOCKS5 UDP ASSOCIATE is not supported",
            _ => "unknown SOCKS5 command is not supported",
        };
        let _ = write_socks5_reply(stream, 7).await;
        return Err(socks_failure(
            ForwardFailureCode::SocksCommandRejected,
            detail,
        ));
    }

    let host = match request[3] {
        SOCKS_ADDRESS_IPV4 => {
            let mut address = [0_u8; 4];
            read_exact_socks(stream, &mut address).await?;
            IpAddr::from(address).to_string()
        }
        SOCKS_ADDRESS_IPV6 => {
            let mut address = [0_u8; 16];
            read_exact_socks(stream, &mut address).await?;
            IpAddr::from(address).to_string()
        }
        SOCKS_ADDRESS_DOMAIN => {
            let mut length = [0_u8; 1];
            read_exact_socks(stream, &mut length).await?;
            if length[0] == 0 {
                let _ = write_socks5_reply(stream, 8).await;
                return Err(socks_failure(
                    ForwardFailureCode::SocksAddressRejected,
                    "SOCKS5 domain target is empty",
                ));
            }
            let mut domain = vec![0_u8; usize::from(length[0])];
            read_exact_socks(stream, &mut domain).await?;
            if !domain.is_ascii()
                || domain
                    .iter()
                    .any(|byte| byte.is_ascii_control() || *byte == b' ')
            {
                let _ = write_socks5_reply(stream, 8).await;
                return Err(socks_failure(
                    ForwardFailureCode::SocksAddressRejected,
                    "SOCKS5 domain target is invalid",
                ));
            }
            String::from_utf8(domain).map_err(|_| {
                socks_failure(
                    ForwardFailureCode::SocksAddressRejected,
                    "SOCKS5 domain target is invalid",
                )
            })?
        }
        _ => {
            let _ = write_socks5_reply(stream, 8).await;
            return Err(socks_failure(
                ForwardFailureCode::SocksAddressRejected,
                "SOCKS5 address type is not supported",
            ));
        }
    };
    let mut port = [0_u8; 2];
    read_exact_socks(stream, &mut port).await?;
    let port = u16::from_be_bytes(port);
    if port == 0 {
        let _ = write_socks5_reply(stream, 8).await;
        return Err(socks_failure(
            ForwardFailureCode::SocksAddressRejected,
            "SOCKS5 target port must be non-zero",
        ));
    }
    Ok(SocksTarget { host, port })
}

async fn read_exact_socks<S>(stream: &mut S, buffer: &mut [u8]) -> ForwardResult<()>
where
    S: AsyncRead + Unpin,
{
    stream.read_exact(buffer).await.map(|_| ()).map_err(|_| {
        socks_failure(
            ForwardFailureCode::SocksTruncated,
            "SOCKS5 request ended before the declared fields were complete",
        )
    })
}

async fn write_socks5_reply<S>(stream: &mut S, reply: u8) -> ForwardResult<()>
where
    S: AsyncWrite + Unpin,
{
    stream
        .write_all(&[
            SOCKS_VERSION,
            reply,
            0,
            SOCKS_ADDRESS_IPV4,
            0,
            0,
            0,
            0,
            0,
            0,
        ])
        .await
        .map_err(|_| socks_failure(ForwardFailureCode::Protocol, "failed to write SOCKS5 reply"))
}

fn socks_failure(code: ForwardFailureCode, detail: &'static str) -> ForwardRuntimeError {
    ForwardRuntimeError::from_failure(ForwardFailure::new(code, "socks5", detail))
}

fn socks_reply_for_failure(failure: &ForwardFailure) -> u8 {
    match failure.code {
        ForwardFailureCode::RemoteRegistrationRejected => 2,
        ForwardFailureCode::TargetConnect => 5,
        ForwardFailureCode::ConnectTimeout => 4,
        _ => 1,
    }
}

async fn relay_with_idle_timeout<A, B>(
    mut first: A,
    mut second: B,
    idle_timeout: Duration,
    traffic: Arc<ForwardTrafficCounters>,
    first_is_listener: bool,
) -> ForwardResult<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let mut first_eof = false;
    let mut second_eof = false;
    let mut first_buffer = vec![0_u8; RELAY_BUFFER_BYTES];
    let mut second_buffer = vec![0_u8; RELAY_BUFFER_BYTES];
    while !first_eof || !second_eof {
        let activity = timeout(idle_timeout, async {
            tokio::select! {
                read = first.read(&mut first_buffer), if !first_eof => (true, read),
                read = second.read(&mut second_buffer), if !second_eof => (false, read),
            }
        })
        .await
        .map_err(|_| {
            ForwardRuntimeError::from_failure(ForwardFailure::new(
                ForwardFailureCode::Relay,
                "relay-idle",
                "forward child reached its idle timeout",
            ))
        })?;
        match activity {
            (true, Ok(0)) => {
                first_eof = true;
                second.shutdown().await.map_err(map_relay_error)?;
            }
            (false, Ok(0)) => {
                second_eof = true;
                first.shutdown().await.map_err(map_relay_error)?;
            }
            (true, Ok(read)) => {
                second
                    .write_all(&first_buffer[..read])
                    .await
                    .map_err(map_relay_error)?;
                traffic_counter(&traffic, first_is_listener)
                    .fetch_add(u64::try_from(read).unwrap_or(u64::MAX), Ordering::Relaxed);
            }
            (false, Ok(read)) => {
                first
                    .write_all(&second_buffer[..read])
                    .await
                    .map_err(map_relay_error)?;
                traffic_counter(&traffic, !first_is_listener)
                    .fetch_add(u64::try_from(read).unwrap_or(u64::MAX), Ordering::Relaxed);
            }
            (_, Err(error)) => return Err(map_relay_error(error)),
        }
    }
    Ok(())
}

fn map_relay_error(_: io::Error) -> ForwardRuntimeError {
    ForwardRuntimeError::from_failure(ForwardFailure::new(
        ForwardFailureCode::Relay,
        "relay",
        "forward child relay failed",
    ))
}

async fn cleanup(
    summary: &mut ForwardSessionSummary,
    summary_tx: &watch::Sender<ForwardSessionSummary>,
    children: &mut JoinSet<ForwardResult<()>>,
    mut transport: Box<dyn ForwardTransport>,
    remote_binding: Option<(String, u16)>,
    child_shutdown_timeout: Duration,
) {
    // Entering Stopping has already fenced all accept/open branches. A remote
    // listener is then cancelled by its exact server-assigned binding before
    // child channels are cleared and the owning SSH transport is disconnected.
    if let Some((address, port)) = remote_binding {
        summary.cleanup.listener_closed_or_remote_cancelled =
            match transport.cancel_remote_forward(&address, port).await {
                Ok(()) => CleanupStepState::Complete,
                Err(_) => {
                    summary.cleanup.uncertain = true;
                    CleanupStepState::Uncertain
                }
            };
    } else if summary.actual_bind.is_some() {
        summary.cleanup.listener_closed_or_remote_cancelled = CleanupStepState::Complete;
    }
    publish(summary_tx, summary);

    children.abort_all();
    let deadline = Instant::now() + child_shutdown_timeout;
    while !children.is_empty() {
        match timeout_at(deadline, children.join_next()).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                summary.cleanup.abandoned_child_count = children.len();
                summary.cleanup.uncertain = true;
                break;
            }
        }
    }
    summary.child_count = 0;
    summary.cleanup.children_cleared = if summary.cleanup.abandoned_child_count == 0 {
        CleanupStepState::Complete
    } else {
        CleanupStepState::Uncertain
    };
    publish(summary_tx, summary);

    summary.cleanup.transport_disconnected = match transport.disconnect().await {
        Ok(()) => CleanupStepState::Complete,
        Err(_) => {
            summary.cleanup.uncertain = true;
            CleanupStepState::Uncertain
        }
    };
}

fn record_child_result(
    summary: &mut ForwardSessionSummary,
    joined: Result<ForwardResult<()>, tokio::task::JoinError>,
) {
    summary.child_count = summary.child_count.saturating_sub(1);
    summary.state_revision = summary.state_revision.saturating_add(1);
    match joined {
        Ok(Ok(())) => {}
        Ok(Err(error)) => summary.last_child_failure = Some(error.failure),
        Err(_) => {
            summary.last_child_failure = Some(ForwardFailure::new(
                ForwardFailureCode::CleanupUncertain,
                "child-task",
                "a forward child task ended without a reliable outcome",
            ));
        }
    }
}

fn traffic_counter(traffic: &ForwardTrafficCounters, listener_to_target: bool) -> &AtomicU64 {
    if listener_to_target {
        &traffic.listener_to_target
    } else {
        &traffic.target_to_listener
    }
}

fn sync_traffic_summary(
    summary: &mut ForwardSessionSummary,
    summary_tx: &watch::Sender<ForwardSessionSummary>,
    traffic: &ForwardTrafficCounters,
) {
    let listener_to_target = traffic.listener_to_target.load(Ordering::Relaxed);
    let target_to_listener = traffic.target_to_listener.load(Ordering::Relaxed);
    if listener_to_target != summary.listener_to_target_bytes
        || target_to_listener != summary.target_to_listener_bytes
    {
        summary.listener_to_target_bytes = listener_to_target;
        summary.target_to_listener_bytes = target_to_listener;
        summary.state_revision = summary.state_revision.saturating_add(1);
        publish(summary_tx, summary);
    }
}

fn transition(summary: &mut ForwardSessionSummary, state: ForwardSessionState) {
    summary.state = state;
    summary.state_revision = summary.state_revision.saturating_add(1);
}

fn fail_summary(summary: &mut ForwardSessionSummary, failure: ForwardFailure) {
    summary.failure = Some(failure);
    transition(summary, ForwardSessionState::Failed);
}

fn publish(summary_tx: &watch::Sender<ForwardSessionSummary>, summary: &ForwardSessionSummary) {
    let _ = summary_tx.send(summary.clone());
}

async fn answer_pending_stop(
    stop_rx: &mut mpsc::Receiver<StopRequest>,
    summary: &ForwardSessionSummary,
) {
    if let Ok(Some(stop)) = timeout(Duration::from_millis(10), stop_rx.recv()).await {
        let _ = stop.reply_tx.send(summary.clone());
    }
}

fn incoming_remote_forward(channel: ForwardedTcpipChannel) -> IncomingRemoteForward {
    IncomingRemoteForward {
        connected_address: channel.connected_address.clone(),
        connected_port: channel.connected_port,
        originator_address: channel.originator_address.clone(),
        originator_port: channel.originator_port,
        stream: Box::new(channel),
    }
}

/// A locked Vault can only explain failures while acquiring or applying a
/// credential. Host-key and route failures must retain their protocol origin.
fn vault_locked_after_credential_failure(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::AuthenticationRejected | TransportError::InvalidPrivateKey
    ) || matches!(
        error,
        TransportError::RouteIngress(route)
            if route.kind == IngressFailureKind::CredentialLocked
    )
}

fn map_transport_error(error: TransportError) -> ForwardRuntimeError {
    let failure = match error {
        TransportError::HostKeyRejected => ForwardFailure::new(
            ForwardFailureCode::HostKeyReviewRequired,
            "host-key",
            "the forward requires explicit host-key review",
        ),
        TransportError::HostKeyMismatch { .. } => ForwardFailure::new(
            ForwardFailureCode::HostKeyMismatch,
            "host-key",
            "the observed host key does not match the trusted key",
        ),
        TransportError::AuthenticationRejected
        | TransportError::AuthenticationIncomplete
        | TransportError::AuthenticationTimeout => ForwardFailure::new(
            ForwardFailureCode::AuthenticationRejected,
            "authentication",
            "the forward SSH authentication did not complete",
        ),
        TransportError::ForwardRequestRejected => ForwardFailure::new(
            ForwardFailureCode::RemoteRegistrationRejected,
            "remote-registration",
            "the SSH server rejected the remote-forward request",
        ),
        TransportError::ForwardRequestTimeout => ForwardFailure::new(
            ForwardFailureCode::RemoteRegistrationUncertain,
            "remote-registration",
            "the remote-forward registration result is uncertain after timeout",
        ),
        TransportError::ForwardChannelOpenTimeout => ForwardFailure::new(
            ForwardFailureCode::ConnectTimeout,
            "channel-open",
            "opening the SSH forwarded channel timed out",
        ),
        TransportError::ForwardChannelOpenRejected => ForwardFailure::new(
            ForwardFailureCode::TargetConnect,
            "channel-open",
            "the SSH server rejected the forwarded channel target",
        ),
        TransportError::ConnectionLost => ForwardFailure::new(
            ForwardFailureCode::TransportLost,
            "transport",
            "the forward SSH transport was lost",
        ),
        TransportError::ConnectTimeout => ForwardFailure::new(
            ForwardFailureCode::ConnectTimeout,
            "transport-connect",
            "connecting the forward SSH transport timed out",
        ),
        TransportError::ConnectFailed => ForwardFailure::new(
            ForwardFailureCode::TransportConnect,
            "transport-connect",
            "connecting the forward SSH transport failed",
        ),
        TransportError::DisconnectTimeout => ForwardFailure::new(
            ForwardFailureCode::CleanupUncertain,
            "transport-disconnect",
            "disconnecting the forward SSH transport timed out",
        ),
        _ => ForwardFailure::new(
            ForwardFailureCode::Protocol,
            "ssh-transport",
            "the SSH forward protocol operation failed",
        ),
    };
    ForwardRuntimeError::from_failure(failure)
}

fn vault_locked_failure() -> ForwardRuntimeError {
    ForwardRuntimeError::from_failure(ForwardFailure::new(
        ForwardFailureCode::VaultLocked,
        "vault",
        "the saved Host credentials require an unlocked Vault",
    ))
}

fn profile_failure(error: ConnectionProfileError) -> ForwardRuntimeError {
    let failure = match error {
        ConnectionProfileError::InvalidTarget | ConnectionProfileError::StaleHost => {
            ForwardFailure::new(
                ForwardFailureCode::InvalidRule,
                "connection-profile",
                "the saved Host reference or state is invalid",
            )
        }
        ConnectionProfileError::CredentialUnavailable => ForwardFailure::new(
            ForwardFailureCode::CredentialUnavailable,
            "connection-profile",
            "the saved Host has no ready credential",
        ),
        ConnectionProfileError::UnsupportedConfiguration
        | ConnectionProfileError::LoginAutomationConfirmationRequired => ForwardFailure::new(
            ForwardFailureCode::Protocol,
            "connection-profile",
            "the saved Host connection configuration is not supported",
        ),
        ConnectionProfileError::PersistenceUnavailable => ForwardFailure::new(
            ForwardFailureCode::HostUnavailable,
            "connection-profile",
            "the saved Host connection profile is unavailable",
        ),
    };
    ForwardRuntimeError::from_failure(failure)
}

fn bounded_id(value: String) -> ForwardResult<String> {
    if value.trim().is_empty()
        || value.len() > MAX_ID_BYTES
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(ForwardRuntimeError::invalid("forward id is invalid"));
    }
    Ok(value)
}

fn validate_host_ref(value: &str) -> ForwardResult<()> {
    if value.trim().is_empty()
        || value.len() > MAX_ID_BYTES
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(ForwardRuntimeError::invalid("host reference is invalid"));
    }
    Ok(())
}

fn require_loopback(address: IpAddr) -> ForwardResult<()> {
    if !address.is_loopback() {
        return Err(ForwardRuntimeError::invalid(
            "local and dynamic forwards must bind a loopback address",
        ));
    }
    Ok(())
}

fn validate_remote_bind(address: &str) -> ForwardResult<()> {
    if address.trim().is_empty()
        || address.len() > MAX_REMOTE_BIND_BYTES
        || address
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(ForwardRuntimeError::invalid(
            "remote forward bind address is invalid",
        ));
    }
    Ok(())
}

fn validate_target(host: &str, port: u16) -> ForwardResult<()> {
    if host.trim().is_empty()
        || host.len() > MAX_HOST_BYTES
        || host
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control() || byte == b' ')
        || port == 0
    {
        return Err(ForwardRuntimeError::invalid("forward target is invalid"));
    }
    Ok(())
}

type CoreResult<T> = Result<T, Box<wire::CoreApiError>>;

fn require_mutation_key(request_id: &wire::RequestId, idempotency_key: &str) -> CoreResult<()> {
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 160 {
        return Err(forward_core_error(
            request_id.clone(),
            "forward.invalid_idempotency_key",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn forward_rule_list(
    request: wire::ForwardRuleListRequest,
    hosts: State<'_, HostService>,
) -> CoreResult<wire::ForwardRuleListResponse> {
    hosts
        .with_forward_repository(|repository| repository.list_forward_rules())
        .map(|rules| wire::ForwardRuleListResponse { rules })
        .map_err(|error| crate::host_service::map_persistence_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) fn forward_rule_create(
    request: wire::ForwardRuleCreateRequest,
    hosts: State<'_, HostService>,
) -> CoreResult<wire::ForwardRuleSummary> {
    require_mutation_key(&request.meta.request_id, &request.idempotency_key)?;
    wire_rule_to_runtime(request.rule.clone()).map_err(|_| {
        forward_core_error(
            request.meta.request_id.clone(),
            "forward.invalid_rule",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        )
    })?;
    hosts
        .with_forward_repository(|repository| {
            repository.create_forward_rule(&request.rule_id, &request.label, &request.rule)
        })
        .map_err(|error| crate::host_service::map_persistence_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) fn forward_rule_update(
    request: wire::ForwardRuleUpdateRequest,
    hosts: State<'_, HostService>,
) -> CoreResult<wire::ForwardRuleSummary> {
    require_mutation_key(&request.meta.request_id, &request.idempotency_key)?;
    wire_rule_to_runtime(request.rule.clone()).map_err(|_| {
        forward_core_error(
            request.meta.request_id.clone(),
            "forward.invalid_rule",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        )
    })?;
    hosts
        .with_forward_repository(|repository| {
            repository.update_forward_rule(
                &request.rule_id,
                request.expected_state_version,
                &request.label,
                &request.rule,
            )
        })
        .map_err(|error| crate::host_service::map_persistence_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) fn forward_rule_delete(
    request: wire::ForwardRuleDeleteRequest,
    hosts: State<'_, HostService>,
) -> CoreResult<()> {
    require_mutation_key(&request.meta.request_id, &request.idempotency_key)?;
    hosts
        .with_forward_repository(|repository| {
            repository.delete_forward_rule(&request.rule_id, request.expected_state_version)
        })
        .map_err(|error| crate::host_service::map_persistence_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) async fn forward_rule_preflight(
    request: wire::ForwardRulePreflightRequest,
) -> CoreResult<wire::ForwardRulePreflightResponse> {
    let request_id = request.meta.request_id;
    let rule = wire_rule_to_runtime(request.rule).map_err(|_| {
        forward_core_error(
            request_id.clone(),
            "forward.invalid_rule",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        )
    })?;
    let local_bind = match rule {
        PortForwardRule::Local {
            local_bind_address,
            local_listen_port,
            ..
        }
        | PortForwardRule::Dynamic {
            local_bind_address,
            local_listen_port,
            ..
        } => Some((local_bind_address, local_listen_port)),
        PortForwardRule::Remote { .. } => None,
    };
    let Some((address, port)) = local_bind else {
        return Ok(wire::ForwardRulePreflightResponse {
            local_bind_available_at_check: None,
            checked_address: None,
            checked_port: None,
            advisory_only: true,
        });
    };
    let available = TcpListener::bind(SocketAddr::new(address, port))
        .await
        .is_ok();
    Ok(wire::ForwardRulePreflightResponse {
        local_bind_available_at_check: Some(available),
        checked_address: Some(address.to_string()),
        checked_port: Some(port),
        advisory_only: true,
    })
}

#[tauri::command]
pub(crate) fn forward_session_start(
    request: wire::ForwardSessionStartRequest,
    on_event: Channel<wire::ForwardSessionEvent>,
    service: State<'_, ForwardSessionService>,
    hosts: State<'_, HostService>,
    lifecycle: State<'_, crate::lifecycle::LifecycleState>,
) -> CoreResult<wire::ForwardSessionSummary> {
    let request_id = request.meta.request_id.clone();
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    if request.idempotency_key.trim().is_empty() {
        return Err(forward_core_error(
            request_id,
            "forward.invalid_idempotency_key",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        ));
    }
    if request.rule_id.is_some() != request.rule_revision.is_some() {
        return Err(forward_core_error(
            request_id,
            "forward.invalid_rule_link",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        ));
    }
    if let (Some(rule_id), Some(rule_revision)) = (&request.rule_id, request.rule_revision) {
        let saved = hosts
            .with_forward_repository(|repository| repository.list_forward_rules())
            .map_err(|error| crate::host_service::map_persistence_error(request_id.clone(), error))?
            .into_iter()
            .find(|saved| &saved.rule_id == rule_id)
            .ok_or_else(|| {
                forward_core_error(
                    request_id.clone(),
                    "forward.rule_not_found",
                    wire::ErrorCategory::Unavailable,
                    wire::RetryStrategy::RefreshSnapshot,
                )
            })?;
        if saved.state_version != rule_revision || saved.rule != request.rule {
            return Err(forward_core_error(
                request_id,
                "forward.rule_snapshot_stale",
                wire::ErrorCategory::Conflict,
                wire::RetryStrategy::RefreshSnapshot,
            ));
        }
    }
    let rule = wire_rule_to_runtime(request.rule).map_err(|_| {
        forward_core_error(
            request_id.clone(),
            "forward.invalid_rule",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        )
    })?;
    let handle = service
        .start_session(
            request.session_id.to_string(),
            request.rule_id.map(|value| value.to_string()),
            request.rule_revision.map(wire::WireSequence::get),
            rule,
            ForwardRuntimeLimits::default(),
        )
        .map_err(|error| map_forward_runtime_error(request_id.clone(), error))?;
    let initial = map_forward_summary(handle.snapshot()).map_err(|_| {
        forward_core_error(
            request_id.clone(),
            "forward.invalid_runtime_fact",
            wire::ErrorCategory::Internal,
            wire::RetryStrategy::Never,
        )
    })?;
    tauri::async_runtime::spawn(async move {
        let mut handle = handle;
        loop {
            let summary = handle.changed().await;
            let terminal = matches!(
                summary.state,
                ForwardSessionState::Failed | ForwardSessionState::Stopped
            );
            let Ok(summary) = map_forward_summary(summary) else {
                break;
            };
            let event = wire::ForwardSessionEvent {
                schema_version: 1,
                event_seq: summary.state_revision,
                session: summary,
            };
            if on_event.send(event).is_err() || terminal {
                break;
            }
        }
    });
    Ok(initial)
}

#[tauri::command]
pub(crate) fn forward_session_snapshot(
    request: wire::ForwardSessionSnapshotRequest,
    service: State<'_, ForwardSessionService>,
) -> CoreResult<wire::ForwardSessionSnapshot> {
    let mut sessions = service
        .snapshots()
        .into_iter()
        .map(map_forward_summary)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            forward_core_error(
                request.meta.request_id,
                "forward.invalid_runtime_fact",
                wire::ErrorCategory::Internal,
                wire::RetryStrategy::Never,
            )
        })?;
    sessions.sort_by(|left, right| left.session_id.as_str().cmp(right.session_id.as_str()));
    let snapshot_revision = sessions
        .iter()
        .map(|summary| summary.state_revision.get())
        .max()
        .unwrap_or(0);
    Ok(wire::ForwardSessionSnapshot {
        snapshot_revision: wire::WireSequence::new(snapshot_revision),
        sessions,
    })
}

#[tauri::command]
pub(crate) async fn forward_session_stop(
    request: wire::ForwardSessionStopRequest,
    service: State<'_, ForwardSessionService>,
) -> CoreResult<wire::ForwardSessionSummary> {
    let request_id = request.meta.request_id;
    let generation = ForwardGeneration::new(request.expected_generation.get())
        .map_err(|error| map_forward_runtime_error(request_id.clone(), error))?;
    service
        .stop_session(request.session_id.as_str(), generation)
        .await
        .and_then(map_forward_summary)
        .map_err(|error| map_forward_runtime_error(request_id, error))
}

#[tauri::command]
pub(crate) fn forward_cleanup_retain(
    request: wire::ForwardCleanupRetainRequest,
    service: State<'_, ForwardSessionService>,
) -> CoreResult<wire::ForwardSessionSummary> {
    let request_id = request.meta.request_id;
    if request.idempotency_key.trim().is_empty() {
        return Err(forward_core_error(
            request_id,
            "forward.invalid_idempotency_key",
            wire::ErrorCategory::Validation,
            wire::RetryStrategy::Never,
        ));
    }
    let generation = ForwardGeneration::new(request.expected_generation.get())
        .map_err(|error| map_forward_runtime_error(request_id.clone(), error))?;
    service
        .retain_uncertain_cleanup_for_exit_once(
            request.session_id.as_str(),
            generation,
            request.expected_state_revision.get(),
            request.retain_uncertain_cleanup_for_exit_confirmed,
        )
        .and_then(map_forward_summary)
        .map_err(|error| map_forward_runtime_error(request_id, error))
}

fn wire_rule_to_runtime(rule: wire::PortForwardRule) -> ForwardResult<PortForwardRule> {
    let runtime = match rule {
        wire::PortForwardRule::Local {
            host_id,
            local_bind_address,
            local_listen_port,
            remote_target_host,
            remote_target_port,
        } => PortForwardRule::Local {
            host_ref: host_id.to_string(),
            local_bind_address: local_bind_address
                .parse()
                .map_err(|_| ForwardRuntimeError::invalid("local bind address is invalid"))?,
            local_listen_port,
            remote_target_host,
            remote_target_port,
        },
        wire::PortForwardRule::Remote {
            host_id,
            remote_bind_address,
            remote_listen_port,
            local_target_host,
            local_target_port,
        } => PortForwardRule::Remote {
            host_ref: host_id.to_string(),
            remote_bind_address,
            remote_listen_port,
            local_target_host,
            local_target_port,
        },
        wire::PortForwardRule::Dynamic {
            host_id,
            local_bind_address,
            local_listen_port,
        } => PortForwardRule::Dynamic {
            host_ref: host_id.to_string(),
            local_bind_address: local_bind_address
                .parse()
                .map_err(|_| ForwardRuntimeError::invalid("local bind address is invalid"))?,
            local_listen_port,
        },
    };
    runtime.validate()?;
    Ok(runtime)
}

fn runtime_rule_to_wire(rule: PortForwardRule) -> ForwardResult<wire::PortForwardRule> {
    Ok(match rule {
        PortForwardRule::Local {
            host_ref,
            local_bind_address,
            local_listen_port,
            remote_target_host,
            remote_target_port,
        } => wire::PortForwardRule::Local {
            host_id: wire::HostId::parse(host_ref)
                .map_err(|_| ForwardRuntimeError::invalid("forward host id is invalid"))?,
            local_bind_address: local_bind_address.to_string(),
            local_listen_port,
            remote_target_host,
            remote_target_port,
        },
        PortForwardRule::Remote {
            host_ref,
            remote_bind_address,
            remote_listen_port,
            local_target_host,
            local_target_port,
        } => wire::PortForwardRule::Remote {
            host_id: wire::HostId::parse(host_ref)
                .map_err(|_| ForwardRuntimeError::invalid("forward host id is invalid"))?,
            remote_bind_address,
            remote_listen_port,
            local_target_host,
            local_target_port,
        },
        PortForwardRule::Dynamic {
            host_ref,
            local_bind_address,
            local_listen_port,
        } => wire::PortForwardRule::Dynamic {
            host_id: wire::HostId::parse(host_ref)
                .map_err(|_| ForwardRuntimeError::invalid("forward host id is invalid"))?,
            local_bind_address: local_bind_address.to_string(),
            local_listen_port,
        },
    })
}

fn map_forward_summary(
    summary: ForwardSessionSummary,
) -> ForwardResult<wire::ForwardSessionSummary> {
    let session_id = wire::ForwardSessionId::parse(summary.session_id)
        .map_err(|_| ForwardRuntimeError::invalid("forward session id is invalid"))?;
    let host_id = wire::HostId::parse(summary.host_ref)
        .map_err(|_| ForwardRuntimeError::invalid("forward host id is invalid"))?;
    let rule_id = summary
        .rule_id
        .map(wire::ForwardRuleId::parse)
        .transpose()
        .map_err(|_| ForwardRuntimeError::invalid("forward rule id is invalid"))?;
    Ok(wire::ForwardSessionSummary {
        session_id,
        host_id,
        generation: wire::WireSequence::new(summary.generation.get()),
        state_revision: wire::WireSequence::new(summary.state_revision),
        state: match summary.state {
            ForwardSessionState::Starting => wire::ForwardSessionState::Starting,
            ForwardSessionState::Running => wire::ForwardSessionState::Running,
            ForwardSessionState::Failed => wire::ForwardSessionState::Failed,
            ForwardSessionState::Stopping => wire::ForwardSessionState::Stopping,
            ForwardSessionState::Stopped => wire::ForwardSessionState::Stopped,
        },
        rule_id,
        rule_revision: summary.rule_revision.map(wire::WireSequence::new),
        rule_snapshot: runtime_rule_to_wire(summary.rule_snapshot)?,
        started_at_unix_ms: summary.started_at_unix_ms,
        actual_bind: summary.actual_bind.map(|bind| match bind {
            ActualForwardBind::Local { address } => wire::ActualForwardBind::Local {
                address: address.ip().to_string(),
                port: address.port(),
            },
            ActualForwardBind::Remote { address, port } => {
                wire::ActualForwardBind::Remote { address, port }
            }
        }),
        child_count: u32::try_from(summary.child_count).unwrap_or(u32::MAX),
        listener_to_target_bytes: wire::WireSequence::new(summary.listener_to_target_bytes),
        target_to_listener_bytes: wire::WireSequence::new(summary.target_to_listener_bytes),
        failure: summary.failure.map(map_forward_failure),
        last_child_failure: summary.last_child_failure.map(map_forward_failure),
        cleanup: wire::ForwardCleanupFacts {
            listener_closed_or_remote_cancelled: map_cleanup_step(
                summary.cleanup.listener_closed_or_remote_cancelled,
            ),
            children_cleared: map_cleanup_step(summary.cleanup.children_cleared),
            transport_disconnected: map_cleanup_step(summary.cleanup.transport_disconnected),
            abandoned_child_count: u32::try_from(summary.cleanup.abandoned_child_count)
                .unwrap_or(u32::MAX),
            uncertain: summary.cleanup.uncertain,
        },
    })
}

fn map_cleanup_step(state: CleanupStepState) -> wire::CleanupStepState {
    match state {
        CleanupStepState::NotRequired => wire::CleanupStepState::NotRequired,
        CleanupStepState::Complete => wire::CleanupStepState::Complete,
        CleanupStepState::Uncertain => wire::CleanupStepState::Uncertain,
    }
}

fn map_forward_failure(failure: ForwardFailure) -> wire::ForwardFailure {
    let code = match failure.code {
        ForwardFailureCode::InvalidRule => wire::ForwardFailureCode::InvalidRule,
        ForwardFailureCode::HostUnavailable => wire::ForwardFailureCode::HostUnavailable,
        ForwardFailureCode::HostKeyReviewRequired => {
            wire::ForwardFailureCode::HostKeyReviewRequired
        }
        ForwardFailureCode::HostKeyMismatch => wire::ForwardFailureCode::HostKeyMismatch,
        ForwardFailureCode::VaultLocked => wire::ForwardFailureCode::VaultLocked,
        ForwardFailureCode::CredentialUnavailable => {
            wire::ForwardFailureCode::CredentialUnavailable
        }
        ForwardFailureCode::AuthenticationRejected => {
            wire::ForwardFailureCode::AuthenticationRejected
        }
        ForwardFailureCode::TransportConnect => wire::ForwardFailureCode::TransportConnect,
        ForwardFailureCode::Bind => wire::ForwardFailureCode::Bind,
        ForwardFailureCode::RemoteRegistrationRejected => {
            wire::ForwardFailureCode::RemoteRegistrationRejected
        }
        ForwardFailureCode::RemoteRegistrationUncertain => {
            wire::ForwardFailureCode::RemoteRegistrationUncertain
        }
        ForwardFailureCode::TransportLost => wire::ForwardFailureCode::TransportLost,
        ForwardFailureCode::Protocol => wire::ForwardFailureCode::Protocol,
        ForwardFailureCode::ResourceLimit => wire::ForwardFailureCode::ResourceLimit,
        ForwardFailureCode::SocksTruncated => wire::ForwardFailureCode::SocksTruncated,
        ForwardFailureCode::SocksAuthenticationRejected => {
            wire::ForwardFailureCode::SocksAuthenticationRejected
        }
        ForwardFailureCode::SocksCommandRejected => wire::ForwardFailureCode::SocksCommandRejected,
        ForwardFailureCode::SocksAddressRejected => wire::ForwardFailureCode::SocksAddressRejected,
        ForwardFailureCode::ConnectTimeout => wire::ForwardFailureCode::ConnectTimeout,
        ForwardFailureCode::TargetConnect => wire::ForwardFailureCode::TargetConnect,
        ForwardFailureCode::Relay => wire::ForwardFailureCode::Relay,
        ForwardFailureCode::CleanupUncertain => wire::ForwardFailureCode::CleanupUncertain,
    };
    wire::ForwardFailure {
        code,
        stage: failure.stage,
        message_key: "errors.forward.runtime".to_owned(),
    }
}

fn map_forward_runtime_error(
    request_id: wire::RequestId,
    error: ForwardRuntimeError,
) -> Box<wire::CoreApiError> {
    let (category, retry) = match error.failure.code {
        ForwardFailureCode::InvalidRule => {
            (wire::ErrorCategory::Validation, wire::RetryStrategy::Never)
        }
        ForwardFailureCode::HostKeyReviewRequired
        | ForwardFailureCode::VaultLocked
        | ForwardFailureCode::CredentialUnavailable => (
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::WaitForUser,
        ),
        ForwardFailureCode::ConnectTimeout => (
            wire::ErrorCategory::Timeout,
            wire::RetryStrategy::AfterMilliseconds(1_000),
        ),
        ForwardFailureCode::HostUnavailable
        | ForwardFailureCode::HostKeyMismatch
        | ForwardFailureCode::AuthenticationRejected
        | ForwardFailureCode::Bind
        | ForwardFailureCode::RemoteRegistrationRejected
        | ForwardFailureCode::RemoteRegistrationUncertain
        | ForwardFailureCode::TransportLost
        | ForwardFailureCode::TargetConnect
        | ForwardFailureCode::CleanupUncertain => (
            wire::ErrorCategory::Unavailable,
            wire::RetryStrategy::RefreshSnapshot,
        ),
        _ => (wire::ErrorCategory::Internal, wire::RetryStrategy::Never),
    };
    forward_core_error(request_id, "forward.operation_failed", category, retry)
}

fn forward_core_error(
    request_id: wire::RequestId,
    code: &str,
    category: wire::ErrorCategory,
    retry_strategy: wire::RetryStrategy,
) -> Box<wire::CoreApiError> {
    Box::new(wire::CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: "errors.forward.operationFailed".to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};

    use super::*;

    #[test]
    fn vault_recovery_classifies_only_credential_failures() {
        assert!(vault_locked_after_credential_failure(
            &TransportError::AuthenticationRejected
        ));
        assert!(vault_locked_after_credential_failure(
            &TransportError::InvalidPrivateKey
        ));
        assert!(vault_locked_after_credential_failure(
            &TransportError::RouteIngress(norishell_ssh_transport::RouteIngressError {
                stage: norishell_ssh_transport::IngressStage::Configuration,
                kind: IngressFailureKind::CredentialLocked,
            })
        ));
        assert!(!vault_locked_after_credential_failure(
            &TransportError::HostKeyRejected
        ));
        assert!(!vault_locked_after_credential_failure(
            &TransportError::ConnectFailed
        ));
    }

    #[derive(Default)]
    struct MockFacts {
        connects: Vec<(String, ForwardGeneration)>,
        events: Vec<String>,
        direct_targets: Vec<(String, u16)>,
        incoming: VecDeque<IncomingRemoteForward>,
        fail_cancel: bool,
        expected_host_state_versions: Vec<Option<u64>>,
    }

    struct MockFactory {
        facts: Arc<Mutex<MockFacts>>,
        remote_port: u16,
    }

    impl ForwardTransportFactory for MockFactory {
        fn connect<'a>(
            &'a self,
            host_ref: &'a str,
            generation: ForwardGeneration,
            expected_host_state_version: Option<u64>,
        ) -> BoxFuture<'a, ForwardResult<Box<dyn ForwardTransport>>> {
            Box::pin(async move {
                self.facts
                    .lock()
                    .unwrap()
                    .connects
                    .push((host_ref.to_owned(), generation));
                self.facts
                    .lock()
                    .unwrap()
                    .expected_host_state_versions
                    .push(expected_host_state_version);
                Ok(Box::new(MockTransport {
                    facts: Arc::clone(&self.facts),
                    remote_port: self.remote_port,
                }) as Box<dyn ForwardTransport>)
            })
        }
    }

    struct MockTransport {
        facts: Arc<Mutex<MockFacts>>,
        remote_port: u16,
    }

    impl ForwardTransport for MockTransport {
        fn open_direct_tcpip<'a>(
            &'a mut self,
            target_host: &'a str,
            target_port: u16,
            _originator_address: &'a str,
            _originator_port: u16,
        ) -> BoxFuture<'a, ForwardResult<BoxedForwardIo>> {
            Box::pin(async move {
                self.facts
                    .lock()
                    .unwrap()
                    .direct_targets
                    .push((target_host.to_owned(), target_port));
                let (client, mut peer) = duplex(4096);
                tokio::spawn(async move {
                    let mut data = vec![0_u8; 64];
                    while let Ok(read) = peer.read(&mut data).await {
                        if read == 0 {
                            break;
                        }
                        if peer.write_all(&data[..read]).await.is_err() {
                            break;
                        }
                    }
                });
                Ok(Box::new(client) as BoxedForwardIo)
            })
        }

        fn request_remote_forward<'a>(
            &'a mut self,
            bind_address: &'a str,
            bind_port: u16,
        ) -> BoxFuture<'a, ForwardResult<u16>> {
            Box::pin(async move {
                self.facts
                    .lock()
                    .unwrap()
                    .events
                    .push(format!("request:{bind_address}:{bind_port}"));
                Ok(if bind_port == 0 {
                    self.remote_port
                } else {
                    bind_port
                })
            })
        }

        fn cancel_remote_forward<'a>(
            &'a mut self,
            bind_address: &'a str,
            bind_port: u16,
        ) -> BoxFuture<'a, ForwardResult<()>> {
            Box::pin(async move {
                let mut facts = self.facts.lock().unwrap();
                facts
                    .events
                    .push(format!("cancel:{bind_address}:{bind_port}"));
                if facts.fail_cancel {
                    return Err(cleanup_uncertain_error());
                }
                Ok(())
            })
        }

        fn next_forwarded_tcpip(
            &mut self,
        ) -> BoxFuture<'_, ForwardResult<Option<IncomingRemoteForward>>> {
            Box::pin(async move {
                if let Some(incoming) = self.facts.lock().unwrap().incoming.pop_front() {
                    Ok(Some(incoming))
                } else {
                    std::future::pending().await
                }
            })
        }

        fn wait_failure(&mut self) -> BoxFuture<'_, ForwardResult<()>> {
            Box::pin(std::future::pending())
        }

        fn disconnect(self: Box<Self>) -> BoxFuture<'static, ForwardResult<()>> {
            Box::pin(async move {
                self.facts
                    .lock()
                    .unwrap()
                    .events
                    .push("disconnect".to_owned());
                Ok(())
            })
        }
    }

    struct DisabledProductionWrapperFactory {
        facts: Arc<Mutex<MockFacts>>,
        remote_port: u16,
    }

    impl ForwardTransportFactory for DisabledProductionWrapperFactory {
        fn connect<'a>(
            &'a self,
            host_ref: &'a str,
            generation: ForwardGeneration,
            expected_host_state_version: Option<u64>,
        ) -> BoxFuture<'a, ForwardResult<Box<dyn ForwardTransport>>> {
            Box::pin(async move {
                self.facts
                    .lock()
                    .unwrap()
                    .connects
                    .push((host_ref.to_owned(), generation));
                self.facts
                    .lock()
                    .unwrap()
                    .expected_host_state_versions
                    .push(expected_host_state_version);
                Ok(Box::new(ProductionForwardTransport {
                    transport: Some(MockTransport {
                        facts: Arc::clone(&self.facts),
                        remote_port: self.remote_port,
                    }),
                    heartbeat_tasks: ForwardHeartbeatTasks::start(None, Vec::new()),
                }) as Box<dyn ForwardTransport>)
            })
        }
    }

    fn local_rule() -> PortForwardRule {
        PortForwardRule::Local {
            host_ref: "host-1".to_owned(),
            local_bind_address: "127.0.0.1".parse().unwrap(),
            local_listen_port: 0,
            remote_target_host: "internal.example".to_owned(),
            remote_target_port: 443,
        }
    }

    async fn wait_for_state(
        handle: &mut ForwardSessionHandle,
        expected: ForwardSessionState,
    ) -> ForwardSessionSummary {
        loop {
            let snapshot = handle.snapshot();
            if snapshot.state == expected {
                return snapshot;
            }
            handle.changed().await;
        }
    }

    #[test]
    fn tagged_rules_enforce_loopback_and_nonzero_targets() {
        assert!(local_rule().validate().is_ok());
        assert!(
            PortForwardRule::Dynamic {
                host_ref: "host-1".to_owned(),
                local_bind_address: "0.0.0.0".parse().unwrap(),
                local_listen_port: 1080,
            }
            .validate()
            .is_err()
        );
        assert!(
            PortForwardRule::Remote {
                host_ref: "host-1".to_owned(),
                remote_bind_address: "127.0.0.1".to_owned(),
                remote_listen_port: 0,
                local_target_host: "localhost".to_owned(),
                local_target_port: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn start_from_a_synchronous_tauri_command_uses_the_global_async_runtime() {
        let facts = Arc::new(Mutex::new(MockFacts::default()));
        let factory = Arc::new(MockFactory {
            facts,
            remote_port: 42000,
        });
        let mut handle = ForwardSessionHandle::start(
            "forward-sync-command",
            None,
            None,
            local_rule(),
            ForwardRuntimeLimits::default(),
            factory,
        )
        .expect("a synchronous command must be able to spawn the session actor");

        tauri::async_runtime::block_on(async move {
            let running = timeout(
                Duration::from_secs(1),
                wait_for_state(&mut handle, ForwardSessionState::Running),
            )
            .await
            .expect("the actor should run on Tauri's global async runtime");
            let stopped = handle.stop(running.generation).await.unwrap();
            assert_eq!(stopped.state, ForwardSessionState::Stopped);
        });
    }

    #[tokio::test]
    async fn every_start_requests_a_distinct_transport_and_reports_actual_bind() {
        let facts = Arc::new(Mutex::new(MockFacts::default()));
        let factory = Arc::new(MockFactory {
            facts: Arc::clone(&facts),
            remote_port: 42000,
        });
        let mut first = ForwardSessionHandle::start(
            "forward-1",
            None,
            None,
            local_rule(),
            ForwardRuntimeLimits::default(),
            Arc::clone(&factory),
        )
        .unwrap();
        let mut second = ForwardSessionHandle::start(
            "forward-2",
            None,
            None,
            local_rule(),
            ForwardRuntimeLimits::default(),
            factory,
        )
        .unwrap();
        let first_summary = wait_for_state(&mut first, ForwardSessionState::Running).await;
        let second_summary = wait_for_state(&mut second, ForwardSessionState::Running).await;
        assert!(
            matches!(first_summary.actual_bind, Some(ActualForwardBind::Local { address }) if address.ip().is_loopback() && address.port() != 0)
        );
        assert!(
            matches!(second_summary.actual_bind, Some(ActualForwardBind::Local { address }) if address.ip().is_loopback() && address.port() != 0)
        );
        assert_eq!(facts.lock().unwrap().connects.len(), 2);
        first.stop(first_summary.generation).await.unwrap();
        second.stop(second_summary.generation).await.unwrap();
    }

    #[tokio::test]
    async fn production_wrapper_keeps_disabled_local_remote_and_dynamic_forwards_running() {
        let facts = Arc::new(Mutex::new(MockFacts::default()));
        let factory = Arc::new(DisabledProductionWrapperFactory {
            facts,
            remote_port: 42002,
        });
        let rules = [
            local_rule(),
            PortForwardRule::Remote {
                host_ref: "host-1".to_owned(),
                remote_bind_address: "127.0.0.1".to_owned(),
                remote_listen_port: 0,
                local_target_host: "127.0.0.1".to_owned(),
                local_target_port: 22,
            },
            PortForwardRule::Dynamic {
                host_ref: "host-1".to_owned(),
                local_bind_address: "127.0.0.1".parse().unwrap(),
                local_listen_port: 0,
            },
        ];

        for (index, rule) in rules.into_iter().enumerate() {
            let mut handle = ForwardSessionHandle::start(
                format!("forward-disabled-{index}"),
                None,
                None,
                rule,
                ForwardRuntimeLimits::default(),
                Arc::clone(&factory),
            )
            .unwrap();
            let running = timeout(
                Duration::from_secs(1),
                wait_for_state(&mut handle, ForwardSessionState::Running),
            )
            .await
            .expect("disabled production wrapper should reach Running");
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert_eq!(handle.snapshot().state, ForwardSessionState::Running);
            let stopped = handle.stop(running.generation).await.unwrap();
            assert_eq!(stopped.state, ForwardSessionState::Stopped);
        }
    }

    #[tokio::test]
    async fn service_owns_unique_sessions_and_removes_them_after_stop() {
        let facts = Arc::new(Mutex::new(MockFacts::default()));
        let factory: Arc<dyn ForwardTransportFactory> = Arc::new(MockFactory {
            facts,
            remote_port: 42000,
        });
        let service = ForwardSessionService::with_factory(factory);
        let mut handle = service
            .start_session(
                "forward-service",
                None,
                None,
                local_rule(),
                ForwardRuntimeLimits::default(),
            )
            .unwrap();
        assert!(
            service
                .start_session(
                    "forward-service",
                    None,
                    None,
                    local_rule(),
                    ForwardRuntimeLimits::default(),
                )
                .is_err()
        );
        let running = wait_for_state(&mut handle, ForwardSessionState::Running).await;
        assert_eq!(service.snapshots().len(), 1);
        let stopped = service
            .stop_session("forward-service", running.generation)
            .await
            .unwrap();
        assert_eq!(stopped.state, ForwardSessionState::Stopped);
        assert!(service.snapshots().is_empty());
    }

    #[tokio::test]
    async fn uncertain_cleanup_stays_blocking_until_one_explicit_exit_attempt() {
        let facts = Arc::new(Mutex::new(MockFacts {
            fail_cancel: true,
            ..MockFacts::default()
        }));
        let factory: Arc<dyn ForwardTransportFactory> = Arc::new(MockFactory {
            facts,
            remote_port: 42003,
        });
        let service = ForwardSessionService::with_factory(factory);
        let session_id = "forward-uncertain";
        let mut handle = service
            .start_session(
                session_id,
                None,
                None,
                PortForwardRule::Remote {
                    host_ref: "host-1".to_owned(),
                    remote_bind_address: "127.0.0.1".to_owned(),
                    remote_listen_port: 0,
                    local_target_host: "127.0.0.1".to_owned(),
                    local_target_port: 22,
                },
                ForwardRuntimeLimits::default(),
            )
            .unwrap();
        let running = wait_for_state(&mut handle, ForwardSessionState::Running).await;

        let failed = service
            .stop_session(session_id, running.generation)
            .await
            .unwrap();
        assert_eq!(failed.state, ForwardSessionState::Failed);
        assert!(failed.cleanup.uncertain);
        assert_eq!(service.exit_blockers(), [session_id]);
        assert!(service.shutdown_all().await.is_err());
        assert_eq!(service.snapshots().len(), 1);
        assert_eq!(service.exit_blockers(), [session_id]);

        assert!(
            service
                .retain_uncertain_cleanup_for_exit_once(
                    session_id,
                    failed.generation,
                    failed.state_revision.saturating_sub(1),
                    true,
                )
                .is_err()
        );
        service
            .retain_uncertain_cleanup_for_exit_once(
                session_id,
                failed.generation,
                failed.state_revision,
                true,
            )
            .unwrap();
        assert!(service.exit_blockers().is_empty());
        assert!(service.shutdown_all().await.is_ok());
        assert_eq!(service.snapshots().len(), 1);
        assert_eq!(
            service.exit_blockers(),
            [session_id],
            "the confirmation is consumed even if a sibling later keeps the app alive"
        );
    }

    #[tokio::test]
    async fn production_factory_rejects_invalid_host_reference_before_network_access() {
        let directory = tempfile::tempdir().unwrap();
        let factory = ProductionForwardTransportFactory::new(
            HostService::start(directory.path()).unwrap(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
            SshAgentService::default(),
        );
        let failure = factory
            .connect("not-a-host-id", ForwardGeneration::new(1).unwrap(), None)
            .await
            .err()
            .expect("invalid HostRef must fail");
        assert_eq!(failure.failure.code, ForwardFailureCode::InvalidRule);
    }

    #[tokio::test]
    async fn production_verifier_accepts_only_previously_trusted_exact_keys() {
        let directory = tempfile::tempdir().unwrap();
        let hosts = HostService::start(directory.path()).unwrap();
        let verifier = TrustedForwardHostKeyVerifier {
            hosts: hosts.clone(),
        };
        let endpoint = Endpoint::parse("example.com", 22).unwrap();
        let observed = ObservedHostKey {
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint_sha256: "SHA256:key-a".to_owned(),
            public_key_blob: b"key-a".to_vec(),
        };
        assert_eq!(
            verifier
                .verify(endpoint.clone(), observed.clone())
                .await
                .unwrap(),
            HostKeyDecision::Rejected
        );
        hosts
            .trust_known_host(
                endpoint.normalized_address(),
                endpoint.port(),
                &observed.algorithm,
                &observed.public_key_blob,
            )
            .unwrap();
        assert_eq!(
            verifier
                .verify(endpoint.clone(), observed.clone())
                .await
                .unwrap(),
            HostKeyDecision::Trusted
        );
        let changed = ObservedHostKey {
            algorithm: observed.algorithm,
            fingerprint_sha256: "SHA256:key-b".to_owned(),
            public_key_blob: b"key-b".to_vec(),
        };
        assert!(matches!(
            verifier.verify(endpoint, changed).await.unwrap(),
            HostKeyDecision::Mismatch { .. }
        ));
    }

    #[tokio::test]
    async fn local_forward_relays_to_the_fixed_server_resolved_target() {
        let facts = Arc::new(Mutex::new(MockFacts::default()));
        let factory = Arc::new(MockFactory {
            facts: Arc::clone(&facts),
            remote_port: 42000,
        });
        let mut handle = ForwardSessionHandle::start(
            "forward-local-relay",
            None,
            None,
            local_rule(),
            ForwardRuntimeLimits::default(),
            factory,
        )
        .unwrap();
        let running = wait_for_state(&mut handle, ForwardSessionState::Running).await;
        let Some(ActualForwardBind::Local { address }) = running.actual_bind else {
            panic!("expected local bind");
        };
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut echoed = [0_u8; 4];
        timeout(Duration::from_secs(1), client.read_exact(&mut echoed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&echoed, b"ping");
        assert_eq!(
            facts.lock().unwrap().direct_targets,
            [("internal.example".to_owned(), 443)]
        );
        let traffic = timeout(Duration::from_secs(2), async {
            loop {
                let summary = handle.changed().await;
                if summary.listener_to_target_bytes >= 4 && summary.target_to_listener_bytes >= 4 {
                    break summary;
                }
            }
        })
        .await
        .expect("traffic counters should publish while the child remains active");
        assert!(traffic.state_revision > running.state_revision);
        handle.stop(running.generation).await.unwrap();
    }

    #[tokio::test]
    async fn dynamic_forward_opens_only_after_a_complete_connect_request() {
        let facts = Arc::new(Mutex::new(MockFacts::default()));
        let factory = Arc::new(MockFactory {
            facts: Arc::clone(&facts),
            remote_port: 42000,
        });
        let rule = PortForwardRule::Dynamic {
            host_ref: "host-1".to_owned(),
            local_bind_address: "127.0.0.1".parse().unwrap(),
            local_listen_port: 0,
        };
        let mut handle = ForwardSessionHandle::start(
            "forward-dynamic",
            None,
            None,
            rule,
            ForwardRuntimeLimits::default(),
            factory,
        )
        .unwrap();
        let running = wait_for_state(&mut handle, ForwardSessionState::Running).await;
        let Some(ActualForwardBind::Local { address }) = running.actual_bind else {
            panic!("expected dynamic local bind");
        };
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut method = [0_u8; 2];
        client.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 0]);
        client
            .write_all(&[
                5, 1, 0, 3, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c', b'o', b'm',
                0, 80,
            ])
            .await
            .unwrap();
        let mut connected = [0_u8; 10];
        client.read_exact(&mut connected).await.unwrap();
        assert_eq!(&connected[..2], &[5, 0]);
        client.write_all(b"ok").await.unwrap();
        let mut echoed = [0_u8; 2];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ok");
        assert_eq!(
            facts.lock().unwrap().direct_targets,
            [("example.com".to_owned(), 80)]
        );
        handle.stop(running.generation).await.unwrap();
    }

    #[tokio::test]
    async fn remote_uses_assigned_bind_for_exact_cancel_before_disconnect() {
        let facts = Arc::new(Mutex::new(MockFacts::default()));
        let factory = Arc::new(MockFactory {
            facts: Arc::clone(&facts),
            remote_port: 42001,
        });
        let rule = PortForwardRule::Remote {
            host_ref: "host-1".to_owned(),
            remote_bind_address: "127.0.0.1".to_owned(),
            remote_listen_port: 0,
            local_target_host: "127.0.0.1".to_owned(),
            local_target_port: 22,
        };
        let mut handle = ForwardSessionHandle::start(
            "forward-remote",
            None,
            None,
            rule,
            ForwardRuntimeLimits::default(),
            factory,
        )
        .unwrap();
        let running = wait_for_state(&mut handle, ForwardSessionState::Running).await;
        assert_eq!(
            running.actual_bind,
            Some(ActualForwardBind::Remote {
                address: "127.0.0.1".to_owned(),
                port: 42001,
            })
        );
        let stopped = handle.stop(running.generation).await.unwrap();
        assert_eq!(stopped.state, ForwardSessionState::Stopped);
        assert_eq!(
            facts.lock().unwrap().events,
            [
                "request:127.0.0.1:0",
                "cancel:127.0.0.1:42001",
                "disconnect",
            ]
        );
    }

    async fn socks_exchange(input: &[u8]) -> (ForwardResult<SocksTarget>, Vec<u8>) {
        let (mut client, mut server) = duplex(1024);
        let input = input.to_vec();
        let client_task = tokio::spawn(async move {
            client.write_all(&input).await.unwrap();
            client.shutdown().await.unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            response
        });
        let result = read_socks5_request(&mut server).await;
        drop(server);
        (result, client_task.await.unwrap())
    }

    #[tokio::test]
    async fn socks5_accepts_only_connect_and_keeps_domain_for_server_resolution() {
        let request = [
            5, 1, 0, 5, 1, 0, 3, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c', b'o',
            b'm', 0x01, 0xbb,
        ];
        let (target, response) = socks_exchange(&request).await;
        assert_eq!(
            target.unwrap(),
            SocksTarget {
                host: "example.com".to_owned(),
                port: 443,
            }
        );
        assert_eq!(response, [5, 0]);
    }

    #[tokio::test]
    async fn socks5_rejects_unknown_auth_bind_udp_and_truncation() {
        let (unknown_auth, response) = socks_exchange(&[5, 1, 2]).await;
        assert_eq!(
            unknown_auth.unwrap_err().failure.code,
            ForwardFailureCode::SocksAuthenticationRejected
        );
        assert_eq!(response, [5, 0xff]);

        for command in [SOCKS_COMMAND_BIND, SOCKS_COMMAND_UDP_ASSOCIATE, 99] {
            let (result, response) =
                socks_exchange(&[5, 1, 0, 5, command, 0, 1, 127, 0, 0, 1, 0, 80]).await;
            assert_eq!(
                result.unwrap_err().failure.code,
                ForwardFailureCode::SocksCommandRejected
            );
            assert_eq!(&response[..2], &[5, 0]);
            assert_eq!(response[3], 7);
        }

        let (truncated, response) = socks_exchange(&[5, 1, 0, 5, 1, 0, 3, 9, b'e']).await;
        assert_eq!(
            truncated.unwrap_err().failure.code,
            ForwardFailureCode::SocksTruncated
        );
        assert_eq!(response, [5, 0]);
    }

    #[tokio::test]
    async fn idle_timeout_applies_to_children() {
        let (first, _first_peer): (DuplexStream, DuplexStream) = duplex(64);
        let (second, _second_peer): (DuplexStream, DuplexStream) = duplex(64);
        let failure = relay_with_idle_timeout(
            first,
            second,
            Duration::from_millis(5),
            Arc::new(ForwardTrafficCounters::default()),
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(failure.failure.code, ForwardFailureCode::Relay);
        assert_eq!(failure.failure.stage, "relay-idle");
    }
}
