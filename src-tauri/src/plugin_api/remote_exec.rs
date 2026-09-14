//! Independently connected, Core-authorized SSH exec resources for plugins.
//!
//! The protected service creates [`CoreRemoteExecGrant`] from an exact saved Host handle and an
//! approval-bound command. This module never parses a Host handle, asks a trust question, exposes
//! credentials, or turns a plugin string into a shell plan after admission.

use std::{sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_app_persistence::{CredentialRecordDetails, KnownHostObservation};
use norishell_core_api::{
    HostId, PluginApiErrorCode, PluginApiResourceEventKind, PluginProcessOutputStream,
    PluginRemoteExecEvent, PluginRemoteExecSendRequest, WireSequence,
};
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    HostKeyDecision as TransportHostKeyDecision, HostKeyVerifier, ObservedHostKey,
    RemoteExecStream, RemoteExecStreamEvent, RemoteExecStreamLimits, TransportError, VerifyFuture,
};
use tokio::sync::{oneshot, watch};
use zeroize::Zeroizing;

use crate::{
    connection_profile::{
        ConnectionCredential, ResolvedLongLivedConnectionProfile,
        resolve_long_lived_connection_profile,
    },
    host_service::HostService,
    ssh_agent_service::SshAgentService,
    ssh_connection_orchestrator::{
        ConnectionPhase, ConnectionRouteStage, KeyboardInteractiveRequest,
        SshConnectionInteraction, SshConnectionOrchestrator,
    },
    transient_credential_service::TransientCredentialService,
    vault_service::VaultService,
};

use super::{
    ResourceCommandReceiver, ResourceEventWriter, ResourceFence, ResourceOwner, ResourceRegistry,
};

const MAX_REMOTE_EXEC_COMMAND_BYTES: usize = 16 * 1024;
const MAX_REMOTE_EXEC_STDIN_BYTES: usize = 16 * 1024;
const MAX_REMOTE_EXEC_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_REMOTE_EXEC_EVENT_BYTES: usize = 8 * 1024;
const MIN_REMOTE_EXEC_TIMEOUT_MS: u32 = 100;
const MAX_REMOTE_EXEC_TIMEOUT_MS: u32 = 120_000;

/// Runtime dependencies for one fresh resource-private SSH transport.
#[derive(Clone)]
pub(crate) struct RemoteExecRuntime {
    pub(crate) hosts: HostService,
    pub(crate) vault: VaultService,
    pub(crate) transient_credentials: TransientCredentialService,
    pub(crate) ssh_agent: SshAgentService,
}

/// A non-serializable, one-use admission decision constructed by PluginService.
///
/// `admission_fence` is checked until the exact exec dispatch; `resource_fence` deliberately
/// survives document/action completion and controls only the lifetime of the resulting resource.
pub(crate) struct CoreRemoteExecGrant {
    pub(crate) owner: ResourceOwner,
    pub(crate) host_id: HostId,
    pub(crate) expected_host_revision: WireSequence,
    pub(crate) expected_connection_revision: String,
    pub(crate) command: Vec<u8>,
    pub(crate) timeout_ms: u32,
    pub(crate) admission_fence: ResourceFence,
    pub(crate) resource_fence: ResourceFence,
}

pub(crate) struct RemoteExecDriver;

impl RemoteExecDriver {
    /// Reserves the resource first, then resolves and connects a new independent SSH transport in
    /// the resource task. This prevents a dispatched channel from escaping the resource quota.
    pub(crate) async fn start(
        resources: &ResourceRegistry,
        grant: CoreRemoteExecGrant,
        runtime: RemoteExecRuntime,
    ) -> Result<String, PluginApiErrorCode> {
        validate_grant(&grant)?;
        if !(grant.admission_fence)() || !(grant.resource_fence)() {
            return Err(PluginApiErrorCode::Revoked);
        }
        let owner = grant.owner.clone();
        let timeout_ms = grant.timeout_ms;
        let (ready, received) = oneshot::channel();
        let handle = resources.spawn_with_commands(
            owner.clone(),
            "remote-exec",
            move |cancel, events, commands| async move {
                run_remote_exec(grant, runtime, cancel, events, commands, ready).await
            },
        )?;
        // Do not report a usable handle before `exec` crossed the SSH boundary. If this caller
        // times out or disappears during dispatch, close the independent transport and return
        // OutcomeUnknown because the remote side may already have started the command.
        match tokio::time::timeout(Duration::from_millis(u64::from(timeout_ms)), received).await {
            Ok(Ok(Ok(()))) => Ok(handle),
            Ok(Ok(Err(code))) => {
                let _ = resources.close(&owner, &handle).await;
                Err(code)
            }
            Ok(Err(_)) => {
                let _ = resources.close(&owner, &handle).await;
                Err(PluginApiErrorCode::Unavailable)
            }
            Err(_) => {
                let _ = resources.close(&owner, &handle).await;
                Err(PluginApiErrorCode::OutcomeUnknown)
            }
        }
    }

    /// A close-stdin request uses an empty internal command frame. Empty stdin bytes are rejected
    /// for ordinary writes, so a guest cannot accidentally turn a write into EOF.
    pub(crate) async fn send(
        resources: &ResourceRegistry,
        owner: &ResourceOwner,
        request: PluginRemoteExecSendRequest,
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        if request.handle.is_empty() || request.handle.len() > 120 {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let bytes = if request.close_stdin {
            if !request.data_base64.is_empty() {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            Vec::new()
        } else {
            if request.data_base64.is_empty()
                || request.data_base64.len() > MAX_REMOTE_EXEC_STDIN_BYTES.saturating_mul(2)
            {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            let bytes = BASE64
                .decode(request.data_base64)
                .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
            if bytes.is_empty() || bytes.len() > MAX_REMOTE_EXEC_STDIN_BYTES {
                return Err(PluginApiErrorCode::InvalidRequest);
            }
            bytes
        };
        resources
            .send(owner, &request.handle, bytes, fence.as_ref())
            .await
    }
}

async fn run_remote_exec(
    grant: CoreRemoteExecGrant,
    runtime: RemoteExecRuntime,
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut commands: ResourceCommandReceiver,
    ready: oneshot::Sender<Result<(), PluginApiErrorCode>>,
) -> Result<(), PluginApiErrorCode> {
    let mut ready = Some(ready);
    if !(grant.resource_fence)() || !(grant.admission_fence)() {
        let _ = ready
            .take()
            .expect("one bootstrap reply")
            .send(Err(PluginApiErrorCode::Revoked));
        return emit_cancelled(&events);
    }
    let profile = match resolve_profile(&grant, &runtime) {
        Ok(profile) => profile,
        Err(code) => {
            let _ = ready.take().expect("one bootstrap reply").send(Err(code));
            return emit_failure(&events, code);
        }
    };
    if profile_uses_keyboard_interactive(&profile) {
        // A plugin resource cannot cause an interactive credential prompt; a user must establish
        // a supported saved credential first.
        let _ = ready
            .take()
            .expect("one bootstrap reply")
            .send(Err(PluginApiErrorCode::InteractionRequired));
        return emit_failure(&events, PluginApiErrorCode::InteractionRequired);
    }
    if !(grant.resource_fence)() || !(grant.admission_fence)() {
        let _ = ready
            .take()
            .expect("one bootstrap reply")
            .send(Err(PluginApiErrorCode::Revoked));
        return emit_cancelled(&events);
    }

    let verifier = Arc::new(TrustedPluginRemoteExecHostKeyVerifier {
        hosts: runtime.hosts.clone(),
    });
    let mut interaction = NonInteractivePluginRemoteExecInteraction;
    let resource_fence = grant.resource_fence.clone();
    let admission_fence = grant.admission_fence.clone();
    let orchestrator = SshConnectionOrchestrator::new(
        &runtime.vault,
        &runtime.transient_credentials,
        &runtime.ssh_agent,
    );
    let connection = tokio::select! {
        result = orchestrator.connect(profile.connection, verifier, &mut interaction, None) => result,
        changed = cancel.changed() => {
            let _ = changed;
            let _ = ready.take().expect("one bootstrap reply").send(Err(PluginApiErrorCode::Revoked));
            return emit_cancelled(&events);
        }
        _ = wait_for_bootstrap_revocation(resource_fence.clone(), admission_fence.clone()) => {
            let _ = ready.take().expect("one bootstrap reply").send(Err(PluginApiErrorCode::Revoked));
            return emit_cancelled(&events);
        }
    };
    let connection = match connection {
        Ok(connection) => connection,
        Err(failure) => {
            let code = map_connection_failure(failure.error);
            let _ = ready.take().expect("one bootstrap reply").send(Err(code));
            return emit_failure(&events, code);
        }
    };
    if !(grant.resource_fence)() || !(grant.admission_fence)() {
        let _ = ready
            .take()
            .expect("one bootstrap reply")
            .send(Err(PluginApiErrorCode::Revoked));
        return connection
            .transport
            .disconnect()
            .await
            .map_err(|_| PluginApiErrorCode::CleanupIncomplete)
            .and_then(|()| emit_cancelled(&events));
    }

    let limits = RemoteExecStreamLimits {
        max_stdin_bytes: MAX_REMOTE_EXEC_STDIN_BYTES,
        max_stdout_bytes: MAX_REMOTE_EXEC_OUTPUT_BYTES,
        max_stderr_bytes: MAX_REMOTE_EXEC_OUTPUT_BYTES,
    };
    let stream = match connection
        .transport
        .into_exec_transport()
        .open_stream(&grant.command, limits, move || {
            resource_fence() && admission_fence()
        })
        .await
    {
        Ok(stream) => stream,
        Err(error) => {
            let code = map_dispatch_failure(error);
            let _ = ready.take().expect("one bootstrap reply").send(Err(code));
            return emit_failure(&events, code);
        }
    };
    // The action fence is intentionally absent from this point onward: a dispatched remote
    // command cannot be safely retried when its UI action finishes. The resource lifetime stays
    // bound to the installed package, generation, capability epoch, and Host scope fence.
    if ready
        .take()
        .expect("one bootstrap reply")
        .send(Ok(()))
        .is_err()
    {
        return finish_cancelled(stream, &mut commands, &events).await;
    }
    drive_stream(
        stream,
        grant.timeout_ms,
        grant.resource_fence,
        cancel,
        events,
        commands,
    )
    .await
}

async fn wait_for_bootstrap_revocation(
    resource_fence: ResourceFence,
    admission_fence: ResourceFence,
) {
    loop {
        if !resource_fence() || !admission_fence() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn validate_grant(grant: &CoreRemoteExecGrant) -> Result<(), PluginApiErrorCode> {
    if grant.command.is_empty()
        || grant.command.len() > MAX_REMOTE_EXEC_COMMAND_BYTES
        || grant.command.contains(&0)
        || !(MIN_REMOTE_EXEC_TIMEOUT_MS..=MAX_REMOTE_EXEC_TIMEOUT_MS).contains(&grant.timeout_ms)
        || grant.expected_connection_revision.is_empty()
    {
        return Err(PluginApiErrorCode::InvalidRequest);
    }
    Ok(())
}

fn resolve_profile(
    grant: &CoreRemoteExecGrant,
    runtime: &RemoteExecRuntime,
) -> Result<ResolvedLongLivedConnectionProfile, PluginApiErrorCode> {
    let profile = resolve_long_lived_connection_profile(
        &runtime.hosts,
        &grant.host_id,
        grant.expected_host_revision,
    )
    .map_err(|_| PluginApiErrorCode::InteractionRequired)?;
    if profile.connection.revision_token != grant.expected_connection_revision {
        return Err(PluginApiErrorCode::Conflict);
    }
    Ok(profile)
}

fn profile_uses_keyboard_interactive(profile: &ResolvedLongLivedConnectionProfile) -> bool {
    profile
        .connection
        .credentials
        .iter()
        .chain(
            profile
                .connection
                .jump_hosts
                .iter()
                .flat_map(|jump| jump.credentials.iter()),
        )
        .any(credential_is_keyboard_interactive)
}

fn credential_is_keyboard_interactive(credential: &ConnectionCredential) -> bool {
    matches!(
        credential,
        ConnectionCredential::Stored(record)
            if matches!(record.details, CredentialRecordDetails::KeyboardInteractive { .. })
    )
}

async fn drive_stream<V>(
    mut stream: RemoteExecStream<V>,
    timeout_ms: u32,
    resource_fence: ResourceFence,
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut commands: ResourceCommandReceiver,
) -> Result<(), PluginApiErrorCode>
where
    V: HostKeyVerifier,
{
    let deadline = tokio::time::Instant::now() + Duration::from_millis(u64::from(timeout_ms));
    let mut commands_open = true;
    let mut exit_emitted = false;
    loop {
        if *cancel.borrow_and_update() || !resource_fence() {
            return finish_cancelled(stream, &mut commands, &events).await;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let cleanup = stream.disconnect().await;
            drain_cancelled(&mut commands);
            let _ = emit_remote_event(
                &events,
                PluginRemoteExecEvent::RemoteExecExited { exit_code: None },
                &mut cancel,
                &resource_fence,
            )
            .await;
            return cleanup.map_err(|_| PluginApiErrorCode::CleanupIncomplete);
        }
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow_and_update() {
                    return finish_cancelled(stream, &mut commands, &events).await;
                }
            }
            command = commands.recv(), if commands_open => {
                match command {
                    Some(command) => {
                        let result = if command.payload().is_empty() {
                            stream.close_stdin().await.map_err(map_stream_failure)
                        } else {
                            stream.send_stdin(command.payload()).await.map_err(map_stream_failure)
                        };
                        let failed = result.is_err();
                        command.finish(result);
                        if failed {
                            let cleanup = stream.disconnect().await;
                            drain_cancelled(&mut commands);
                            let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
                            return cleanup.map_err(|_| PluginApiErrorCode::CleanupIncomplete);
                        }
                    }
                    None => commands_open = false,
                }
            }
            received = tokio::time::timeout(remaining, stream.next_event()) => {
                let event = match received {
                    Err(_) => {
                        let cleanup = stream.disconnect().await;
                        drain_cancelled(&mut commands);
                        let _ = emit_remote_event(&events, PluginRemoteExecEvent::RemoteExecExited { exit_code: None }, &mut cancel, &resource_fence).await;
                        return cleanup.map_err(|_| PluginApiErrorCode::CleanupIncomplete);
                    }
                    Ok(Err(_)) => return finish_cancelled(stream, &mut commands, &events).await,
                    Ok(Ok(None)) => RemoteExecStreamEvent::Eof,
                    Ok(Ok(Some(event))) => event,
                };
                match event {
                    RemoteExecStreamEvent::Stdout(bytes) => {
                        if emit_output(&events, PluginProcessOutputStream::Stdout, &bytes, &mut cancel, &resource_fence).await.is_err() {
                            return finish_cancelled(stream, &mut commands, &events).await;
                        }
                    }
                    RemoteExecStreamEvent::Stderr(bytes) => {
                        if emit_output(&events, PluginProcessOutputStream::Stderr, &bytes, &mut cancel, &resource_fence).await.is_err() {
                            return finish_cancelled(stream, &mut commands, &events).await;
                        }
                    }
                    RemoteExecStreamEvent::ExitStatus(status) => {
                        if emit_remote_event(&events, PluginRemoteExecEvent::RemoteExecExited { exit_code: status.map(i64::from) }, &mut cancel, &resource_fence).await.is_err() {
                            return finish_cancelled(stream, &mut commands, &events).await;
                        }
                        exit_emitted = true;
                    }
                    RemoteExecStreamEvent::Eof => {
                        if !exit_emitted && emit_remote_event(&events, PluginRemoteExecEvent::RemoteExecExited { exit_code: None }, &mut cancel, &resource_fence).await.is_err() {
                            return finish_cancelled(stream, &mut commands, &events).await;
                        }
                        drain_cancelled(&mut commands);
                        return stream.disconnect().await.map_err(|_| PluginApiErrorCode::CleanupIncomplete);
                    }
                }
            }
        }
    }
}

async fn finish_cancelled<V>(
    stream: RemoteExecStream<V>,
    commands: &mut ResourceCommandReceiver,
    events: &ResourceEventWriter,
) -> Result<(), PluginApiErrorCode>
where
    V: HostKeyVerifier,
{
    let cleanup = stream.disconnect().await;
    drain_cancelled(commands);
    let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
    cleanup.map_err(|_| PluginApiErrorCode::CleanupIncomplete)
}

async fn emit_output(
    events: &ResourceEventWriter,
    stream: PluginProcessOutputStream,
    bytes: &[u8],
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    for chunk in bytes.chunks(MAX_REMOTE_EXEC_EVENT_BYTES) {
        emit_remote_event(
            events,
            PluginRemoteExecEvent::RemoteExecOutput {
                stream,
                data_base64: BASE64.encode(chunk),
            },
            cancel,
            fence,
        )
        .await?;
    }
    Ok(())
}

async fn emit_remote_event(
    events: &ResourceEventWriter,
    event: PluginRemoteExecEvent,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    events
        .emit_backpressured(
            PluginApiResourceEventKind::RemoteExec { event },
            cancel,
            fence.as_ref(),
        )
        .await
}

fn drain_cancelled(commands: &mut ResourceCommandReceiver) {
    while let Some(command) = commands.try_recv() {
        command.finish(Err(PluginApiErrorCode::Cancelled));
    }
}

fn emit_cancelled(events: &ResourceEventWriter) -> Result<(), PluginApiErrorCode> {
    let _ = events.emit(PluginApiResourceEventKind::Cancelled {});
    Ok(())
}

fn emit_failure(
    events: &ResourceEventWriter,
    _failure: PluginApiErrorCode,
) -> Result<(), PluginApiErrorCode> {
    // Resource events deliberately do not carry upstream transport diagnostics. The secure broker
    // has already returned/recorded a stable admission outcome; a later resource failure is only
    // observable as cancellation so no endpoint, credential, or server text crosses the ABI.
    emit_cancelled(events)
}

fn map_connection_failure(error: TransportError) -> PluginApiErrorCode {
    match error {
        TransportError::HostKeyRejected => PluginApiErrorCode::InteractionRequired,
        TransportError::HostKeyMismatch { .. } => PluginApiErrorCode::PermissionDenied,
        TransportError::AuthenticationRejected
        | TransportError::AuthenticationIncomplete
        | TransportError::AuthenticationTimeout
        | TransportError::InvalidKeyboardInteractiveResponse
        | TransportError::SshAgentKeyUnavailable
        | TransportError::SshAgentUnavailable
        | TransportError::InvalidPrivateKey => PluginApiErrorCode::InteractionRequired,
        TransportError::ConnectTimeout | TransportError::HandshakeTimeout => {
            PluginApiErrorCode::TimedOut
        }
        _ => PluginApiErrorCode::Unavailable,
    }
}

fn map_dispatch_failure(error: TransportError) -> PluginApiErrorCode {
    match error {
        TransportError::RemoteExecRejected => PluginApiErrorCode::Revoked,
        TransportError::RemoteExecTimeout => PluginApiErrorCode::TimedOut,
        TransportError::InvalidRemoteExecCommand => PluginApiErrorCode::InvalidRequest,
        _ => PluginApiErrorCode::Unavailable,
    }
}

fn map_stream_failure(error: TransportError) -> PluginApiErrorCode {
    match error {
        TransportError::RemoteExecOutputTooLarge => PluginApiErrorCode::QuotaExceeded,
        TransportError::RemoteExecTimeout => PluginApiErrorCode::TimedOut,
        _ => PluginApiErrorCode::OutcomeUnknown,
    }
}

#[derive(Clone)]
struct TrustedPluginRemoteExecHostKeyVerifier {
    hosts: HostService,
}

impl HostKeyVerifier for TrustedPluginRemoteExecHostKeyVerifier {
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
                    .map(|_| TransportHostKeyDecision::Trusted)
                    .map_err(|_| TransportError::HostKeyVerificationFailed),
                Ok(KnownHostObservation::Unknown(_)) => Ok(TransportHostKeyDecision::Rejected),
                Ok(KnownHostObservation::Mismatch { trusted, .. })
                | Ok(KnownHostObservation::AlgorithmChanged { trusted, .. }) => {
                    Ok(TransportHostKeyDecision::Mismatch {
                        trusted_fingerprint: trusted.fingerprint_sha256,
                    })
                }
                Err(_) => Err(TransportError::HostKeyVerificationFailed),
            }
        })
    }
}

struct NonInteractivePluginRemoteExecInteraction;

impl SshConnectionInteraction for NonInteractivePluginRemoteExecInteraction {
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
