//! Desktop connection routes. Engines receive a byte stream and, for direct routes only, its peer address.
//! Gateways, hosts, and Vault stay owned by Core.
use super::{DesktopService, Session, prompts::clear_decision};
use crate::{
    connection_profile::resolve_long_lived_connection_profile,
    ssh_connection_orchestrator::{
        ConnectionPhase, ConnectionRouteStage, KeyboardInteractiveRequest,
        SshConnectionInteraction, SshConnectionOrchestrator,
    },
};
use norishell_app_persistence::KnownHostObservation;
use norishell_core_api::DesktopPromptKind;
use norishell_desktop_protocol::{BoxedDesktopIo, EngineError, Result};
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    HostKeyDecision, HostKeyVerifier, ObservedHostKey, SshForwardTransport, TransportCloseHandle,
    TransportError, VerifyFuture,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpStream, task::JoinSet};
use zeroize::Zeroizing;

pub(super) struct GatewayCleanup {
    transport: Option<SshForwardTransport<GatewayVerifier>>,
    closed: Option<TransportCloseHandle>,
    heartbeats: JoinSet<()>,
}

impl GatewayCleanup {
    pub async fn close(mut self) -> Result<()> {
        self.heartbeats.abort_all();
        while self.heartbeats.join_next().await.is_some() {}
        if let Some(transport) = self.transport.take() {
            let result = transport.disconnect().await;
            if let Some(mut closed) = self.closed.take() {
                tokio::time::timeout(Duration::from_secs(5), closed.wait_closed())
                    .await
                    .map_err(|_| EngineError::Timeout)?;
            }
            result.map_err(|_| EngineError::ConnectionLost)?;
        }
        Ok(())
    }
}

pub(super) async fn connect(
    service: &DesktopService,
    session: &Arc<Session>,
) -> Result<(BoxedDesktopIo, Option<SocketAddr>, GatewayCleanup)> {
    let profile = session.summary().profile;
    let endpoint = Endpoint::parse(&profile.address, profile.port)
        .map_err(|_| EngineError::InvalidConfiguration)?;
    let mut cleanup = GatewayCleanup {
        transport: None,
        closed: None,
        heartbeats: JoinSet::new(),
    };
    let Some(host_id) = profile.gateway_host_id else {
        session.phase("desktopConnecting");
        let stream = tokio::time::timeout(
            Duration::from_secs(30),
            TcpStream::connect((endpoint.normalized_address(), endpoint.port())),
        )
        .await
        .map_err(|_| EngineError::Timeout)?
        .map_err(|_| EngineError::ConnectionLost)?;
        stream
            .set_nodelay(true)
            .map_err(|_| EngineError::ConnectionLost)?;
        // The UDP companion must target the peer of this established TCP route.
        let udp_peer = stream
            .peer_addr()
            .map_err(|_| EngineError::ConnectionLost)?;
        return Ok((Box::new(stream), Some(udp_peer), cleanup));
    };
    let snapshot = service
        .hosts
        .get_connection_snapshot(&host_id)
        .map_err(|_| EngineError::InvalidConfiguration)?;
    let resolved = resolve_long_lived_connection_profile(
        &service.hosts,
        &host_id,
        snapshot.host.state_version,
    )
    .map_err(|_| EngineError::InvalidConfiguration)?;
    if connection_uses_vault(&resolved.connection) {
        service.ensure_vault_unlocked(session).await?;
    }
    let interval = resolved
        .transport_keepalive
        .as_ref()
        .map(|policy| Duration::from_secs(u64::from(policy.interval_seconds)));
    let verifier = Arc::new(GatewayVerifier {
        service: service.clone(),
        session: session.clone(),
    });
    let mut interaction = GatewayInteraction {
        service: service.clone(),
        session: session.clone(),
        failure: None,
    };
    let connection =
        SshConnectionOrchestrator::new(&service.vault, &service.transient, &service.agent)
            .connect(resolved.connection, verifier, &mut interaction, interval)
            .await
            .map_err(|failure| {
                interaction
                    .failure
                    .unwrap_or_else(|| map_transport_error(failure.error))
            })?;
    cleanup.closed = Some(connection.transport.close_handle());
    session
        .transports
        .lock()
        .map_err(|_| EngineError::Protocol)?
        .push(connection.transport.close_handle());
    cleanup.transport = Some(connection.transport.into_forward_transport());
    if let Some(policy) = resolved.transport_keepalive {
        for heartbeat in connection.transport_heartbeats {
            let session = Arc::downgrade(session);
            cleanup.heartbeats.spawn(async move {
                let mut next_due = heartbeat.first_due;
                let mut failures = 0u8;
                loop {
                    tokio::time::sleep_until(next_due).await;
                    let reply = tokio::time::timeout(
                        Duration::from_secs(u64::from(policy.reply_timeout_seconds)),
                        heartbeat.handle.request_reply(),
                    )
                    .await;
                    failures = if matches!(reply, Ok(Ok(()))) {
                        0
                    } else {
                        failures.saturating_add(1)
                    };
                    if failures >= policy.failure_threshold {
                        if let Some(session) = session.upgrade() {
                            session.fail_and_stop("gatewayConnectionLost");
                        }
                        break;
                    }
                    next_due = tokio::time::Instant::now()
                        + Duration::from_secs(u64::from(policy.interval_seconds));
                }
            });
        }
    }
    session.phase("gatewayOpeningChannel");
    let channel = cleanup
        .transport
        .as_ref()
        .ok_or(EngineError::Protocol)?
        .open_direct_tcpip(
            endpoint.normalized_address(),
            endpoint.port(),
            "127.0.0.1",
            0,
        )
        .await;
    match channel {
        Ok(channel) => Ok((Box::new(channel), None, cleanup)),
        Err(_) => {
            cleanup.close().await?;
            Err(EngineError::ConnectionLost)
        }
    }
}

fn connection_uses_vault(
    connection: &crate::connection_profile::ResolvedSshConnectionBase,
) -> bool {
    use crate::connection_profile::{ConnectionCredential, ResolvedRouteIngress};
    use norishell_app_persistence::CredentialRecordDetails;
    let secret = |credential: &ConnectionCredential| {
        matches!(credential,
        ConnectionCredential::Stored(record) if matches!(record.details,
            CredentialRecordDetails::Password { .. } | CredentialRecordDetails::PrivateKey { .. }))
    };
    let ingress = match &connection.ingress {
        ResolvedRouteIngress::DirectTcp => false,
        ResolvedRouteIngress::HttpConnectProxy { authentication, .. }
        | ResolvedRouteIngress::Socks5Proxy { authentication, .. } => authentication.is_some(),
    };
    ingress
        || connection.credentials.iter().any(secret)
        || connection
            .jump_hosts
            .iter()
            .any(|jump| jump.credentials.iter().any(secret))
}

struct GatewayVerifier {
    service: DesktopService,
    session: Arc<Session>,
}
impl HostKeyVerifier for GatewayVerifier {
    fn verify(&self, endpoint: Endpoint, observed: ObservedHostKey) -> VerifyFuture {
        let service = self.service.clone();
        let session = self.session.clone();
        Box::pin(async move {
            let host = service
                .hosts
                .observe_known_host(
                    endpoint.normalized_address(),
                    endpoint.port(),
                    &observed.algorithm,
                    &observed.public_key_blob,
                )
                .map_err(|_| TransportError::HostKeyVerificationFailed)?;
            match host {
                KnownHostObservation::Trusted(_) => Ok(HostKeyDecision::Trusted),
                KnownHostObservation::Mismatch { trusted, .. }
                | KnownHostObservation::AlgorithmChanged { trusted, .. } => {
                    Ok(HostKeyDecision::Mismatch {
                        trusted_fingerprint: trusted.fingerprint_sha256,
                    })
                }
                KnownHostObservation::Unknown(_) => {
                    let result = service
                        .prompt(
                            &session,
                            DesktopPromptKind::HostKey {
                                address: endpoint.normalized_address().to_owned(),
                                port: endpoint.port(),
                                algorithm: observed.algorithm.clone(),
                                fingerprint: observed.fingerprint_sha256,
                            },
                        )
                        .await;
                    if result.is_err() {
                        return Ok(HostKeyDecision::Rejected);
                    }
                    service
                        .hosts
                        .trust_known_host(
                            endpoint.normalized_address(),
                            endpoint.port(),
                            &observed.algorithm,
                            &observed.public_key_blob,
                        )
                        .map_err(|_| TransportError::HostKeyVerificationFailed)?;
                    Ok(HostKeyDecision::Trusted)
                }
            }
        })
    }
}

struct GatewayInteraction {
    service: DesktopService,
    session: Arc<Session>,
    failure: Option<EngineError>,
}

fn map_transport_error(error: TransportError) -> EngineError {
    match error {
        TransportError::HostKeyRejected
        | TransportError::HostKeyMismatch { .. }
        | TransportError::HostKeyVerificationFailed => EngineError::CertificateRejected,
        TransportError::AuthenticationRejected
        | TransportError::AuthenticationIncomplete
        | TransportError::InvalidPrivateKey
        | TransportError::SshAgentKeyUnavailable
        | TransportError::SshAgentUnavailable => EngineError::AuthenticationRejected,
        TransportError::ConnectTimeout
        | TransportError::HandshakeTimeout
        | TransportError::HostKeyDecisionTimeout
        | TransportError::AuthenticationTimeout
        | TransportError::ChannelOpenTimeout
        | TransportError::ForwardChannelOpenTimeout
        | TransportError::ForwardRequestTimeout
        | TransportError::DisconnectTimeout => EngineError::Timeout,
        TransportError::InvalidEndpoint(_) | TransportError::InvalidUsername => {
            EngineError::InvalidConfiguration
        }
        TransportError::InvalidKeyboardInteractiveResponse => EngineError::Protocol,
        _ => EngineError::ConnectionLost,
    }
}
impl SshConnectionInteraction for GatewayInteraction {
    async fn phase_changed(
        &mut self,
        phase: ConnectionPhase,
        _stage: ConnectionRouteStage,
    ) -> std::result::Result<(), TransportError> {
        self.session.phase(match phase {
            ConnectionPhase::Connecting => "gatewayConnecting",
            ConnectionPhase::Authenticating => "gatewayAuthenticating",
        });
        Ok(())
    }
    async fn answer_keyboard_interactive(
        &mut self,
        request: KeyboardInteractiveRequest,
    ) -> std::result::Result<Vec<Zeroizing<String>>, TransportError> {
        let count = request.challenge.prompts.len();
        let mut decision = self
            .service
            .prompt(
                &self.session,
                DesktopPromptKind::KeyboardInteractive {
                    name: request.challenge.name,
                    instruction: request.challenge.instructions,
                    prompts: request
                        .challenge
                        .prompts
                        .iter()
                        .map(|prompt| prompt.text.clone())
                        .collect(),
                    echo: request
                        .challenge
                        .prompts
                        .iter()
                        .map(|prompt| prompt.echo)
                        .collect(),
                },
            )
            .await
            .map_err(|error| {
                self.failure = Some(error);
                TransportError::InvalidKeyboardInteractiveResponse
            })?;
        if decision.answers.len() != count {
            clear_decision(&mut decision);
            return Err(TransportError::InvalidKeyboardInteractiveResponse);
        }
        Ok(std::mem::take(&mut decision.answers)
            .into_iter()
            .map(Zeroizing::new)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_failure_preserves_identity_authentication_and_timeout_categories() {
        assert_eq!(
            map_transport_error(TransportError::HostKeyMismatch {
                trusted_fingerprint: "old-test-fingerprint".into(),
                observed_fingerprint: "new-test-fingerprint".into(),
            }),
            EngineError::CertificateRejected
        );
        assert_eq!(
            map_transport_error(TransportError::AuthenticationRejected),
            EngineError::AuthenticationRejected
        );
        assert_eq!(
            map_transport_error(TransportError::AuthenticationTimeout),
            EngineError::Timeout
        );
        assert_eq!(
            map_transport_error(TransportError::ForwardChannelOpenTimeout),
            EngineError::Timeout
        );
    }
}
