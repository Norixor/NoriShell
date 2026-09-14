use std::{sync::Arc, time::Duration};

use norishell_app_persistence::{CredentialRecord, CredentialRecordDetails};
use norishell_core_api::{CredentialRefId, HostId, SshSessionEndpoint, WireSequence};
use norishell_secret_vault::SecretKind;
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    AuthenticatedTransport, Authentication, ConnectRequest, HostKeyVerifier, IngressFailureKind,
    IngressStage, KeyboardInteractiveChallenge, KeyboardInteractiveOutcome, NegotiatedAlgorithms,
    ProxyCredentials, RouteIngress as TransportRouteIngress, RouteIngressError, Socks5DnsMode,
    TransportError, TransportHeartbeatHandle, VerifiedTransport,
};
use zeroize::Zeroizing;

use crate::{
    connection_profile::{
        ConnectionCredential, ResolvedAlgorithmPolicy, ResolvedJumpHost, ResolvedRouteIngress,
        ResolvedSshConnectionBase,
    },
    ssh_agent_service::{SshAgentService, SshAgentServiceError},
    time::unix_time_ms,
    transient_credential_service::TransientCredentialService,
    vault_service::{VaultService, VaultServiceError},
};

const JUMP_CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const KEYBOARD_INTERACTIVE_MAX_TOTAL_PROMPTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionRouteStage {
    Ingress,
    JumpHost {
        hop_index: u8,
        host_id: HostId,
        endpoint: SshSessionEndpoint,
    },
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionPhase {
    Connecting,
    Authenticating,
}

pub(crate) struct KeyboardInteractiveRequest {
    pub(crate) route_stage: ConnectionRouteStage,
    pub(crate) credential_ref_id: CredentialRefId,
    pub(crate) attempt_index: u8,
    pub(crate) round_index: u8,
    pub(crate) challenge: KeyboardInteractiveChallenge,
}

/// Resource-neutral interaction boundary for one SSH connection operation.
///
/// The connector owns protocol sequencing while the resource actor owns how a
/// host-key verifier and keyboard-interactive challenge are presented. No
/// Terminal attachment, Pane, view, or session-service DTO crosses this API.
pub(crate) trait SshConnectionInteraction: Send {
    async fn phase_changed(
        &mut self,
        phase: ConnectionPhase,
        route_stage: ConnectionRouteStage,
    ) -> Result<(), TransportError>;

    async fn answer_keyboard_interactive(
        &mut self,
        request: KeyboardInteractiveRequest,
    ) -> Result<Vec<Zeroizing<String>>, TransportError>;
}

pub(crate) struct NegotiatedAlgorithmFact {
    pub(crate) route_stage: ConnectionRouteStage,
    pub(crate) policy_id: String,
    pub(crate) policy_revision: Option<WireSequence>,
    pub(crate) policy_catalog_version: &'static str,
    pub(crate) negotiated: NegotiatedAlgorithms,
}

pub(crate) struct RoutedTransportHeartbeat {
    pub(crate) route_stage: ConnectionRouteStage,
    pub(crate) handle: TransportHeartbeatHandle,
    pub(crate) first_due: tokio::time::Instant,
    pub(crate) first_due_at_unix_ms: i64,
}

pub(crate) struct AuthenticatedConnection<V>
where
    V: HostKeyVerifier,
{
    pub(crate) transport: AuthenticatedTransport<V>,
    pub(crate) credential_ref_id: CredentialRefId,
    pub(crate) negotiated_algorithms: Vec<NegotiatedAlgorithmFact>,
    pub(crate) transport_heartbeats: Vec<RoutedTransportHeartbeat>,
}

#[derive(Debug)]
pub(crate) struct SshConnectionFailure {
    pub(crate) error: TransportError,
    pub(crate) route_stage: ConnectionRouteStage,
}

pub(crate) struct SshConnectionOrchestrator<'a> {
    vault: &'a VaultService,
    transient_credentials: &'a TransientCredentialService,
    ssh_agent: &'a SshAgentService,
}

impl<'a> SshConnectionOrchestrator<'a> {
    pub(crate) fn new(
        vault: &'a VaultService,
        transient_credentials: &'a TransientCredentialService,
        ssh_agent: &'a SshAgentService,
    ) -> Self {
        Self {
            vault,
            transient_credentials,
            ssh_agent,
        }
    }

    /// Creates a fresh ingress, SSH handshake, host-key verification and
    /// authentication chain for this call. No transport or negotiated fact is
    /// reused from another Terminal, Metrics, SFTP, or Forward resource.
    pub(crate) async fn connect<V, I>(
        &self,
        profile: ResolvedSshConnectionBase,
        verifier: Arc<V>,
        interaction: &mut I,
        transport_keepalive_interval: Option<Duration>,
    ) -> Result<AuthenticatedConnection<V>, SshConnectionFailure>
    where
        V: HostKeyVerifier,
        I: SshConnectionInteraction,
    {
        let ResolvedSshConnectionBase {
            endpoint,
            username,
            ingress,
            jump_hosts,
            credentials,
            algorithm_policy,
            revision_token,
        } = profile;
        debug_assert!(!revision_token.is_empty());
        debug_assert!(jump_hosts.len() <= 5);

        let initial_route_stage = jump_hosts
            .first()
            .map(|jump| jump_route_stage(0, jump))
            .unwrap_or(ConnectionRouteStage::Target);
        notify_phase(
            interaction,
            ConnectionPhase::Connecting,
            initial_route_stage.clone(),
        )
        .await?;
        let initial_algorithm_policy = jump_hosts
            .first()
            .map(|jump| &jump.algorithm_policy)
            .unwrap_or(&algorithm_policy);
        let ingress = transport_ingress_from(self.vault, ingress)
            .map_err(route_failure(ConnectionRouteStage::Ingress))?;
        let initial_endpoint = jump_hosts
            .first()
            .map(|jump| &jump.endpoint)
            .unwrap_or(&endpoint);
        let mut verified = VerifiedTransport::connect(
            ConnectRequest::new(&initial_endpoint.address, initial_endpoint.port)
                .map_err(route_failure(initial_route_stage.clone()))?
                .with_ingress(ingress)
                .with_algorithm_policy(initial_algorithm_policy.transport.clone()),
            verifier.clone(),
        )
        .await
        .map_err(|error| {
            let route_stage = if matches!(error, TransportError::RouteIngress(_)) {
                ConnectionRouteStage::Ingress
            } else {
                initial_route_stage.clone()
            };
            SshConnectionFailure { error, route_stage }
        })?;
        let mut negotiated_algorithms = vec![negotiated_algorithm_fact(
            initial_route_stage.clone(),
            initial_algorithm_policy,
            verified
                .negotiated_algorithms()
                .ok_or_else(|| SshConnectionFailure {
                    error: TransportError::Protocol,
                    route_stage: initial_route_stage.clone(),
                })?,
        )];
        let mut transport_heartbeats = Vec::new();

        for (hop_index, jump) in jump_hosts.iter().enumerate() {
            let jump_stage = jump_route_stage(hop_index, jump);
            notify_phase(
                interaction,
                ConnectionPhase::Authenticating,
                jump_stage.clone(),
            )
            .await?;
            let _jump_credential_ref_id = self
                .authenticate_verified_transport(
                    &mut verified,
                    &jump.username,
                    &jump.credentials,
                    &jump_stage,
                    interaction,
                )
                .await
                .map_err(route_failure(jump_stage.clone()))?;
            let authenticated = verified
                .into_authenticated()
                .map_err(route_failure(jump_stage.clone()))?;
            push_transport_heartbeat(
                &mut transport_heartbeats,
                jump_stage.clone(),
                &authenticated,
                transport_keepalive_interval,
            );
            let (next_endpoint, next_stage, next_algorithm_policy) = jump_hosts
                .get(hop_index + 1)
                .map(|next| {
                    (
                        &next.endpoint,
                        jump_route_stage(hop_index.saturating_add(1), next),
                        &next.algorithm_policy,
                    )
                })
                .unwrap_or((&endpoint, ConnectionRouteStage::Target, &algorithm_policy));
            let next_transport_endpoint =
                Endpoint::parse(&next_endpoint.address, next_endpoint.port)
                    .map_err(TransportError::from)
                    .map_err(route_failure(jump_stage.clone()))?;
            let stream = authenticated
                .into_direct_tcpip_stream(next_transport_endpoint, JUMP_CHANNEL_OPEN_TIMEOUT)
                .await
                .map_err(route_failure(jump_stage))?;
            notify_phase(interaction, ConnectionPhase::Connecting, next_stage.clone()).await?;
            verified = VerifiedTransport::connect_stream(
                ConnectRequest::new(&next_endpoint.address, next_endpoint.port)
                    .map_err(route_failure(next_stage.clone()))?
                    .with_algorithm_policy(next_algorithm_policy.transport.clone()),
                stream,
                verifier.clone(),
            )
            .await
            .map_err(route_failure(next_stage.clone()))?;
            negotiated_algorithms.push(negotiated_algorithm_fact(
                next_stage.clone(),
                next_algorithm_policy,
                verified
                    .negotiated_algorithms()
                    .ok_or(SshConnectionFailure {
                        error: TransportError::Protocol,
                        route_stage: next_stage,
                    })?,
            ));
        }

        notify_phase(
            interaction,
            ConnectionPhase::Authenticating,
            ConnectionRouteStage::Target,
        )
        .await?;
        let credential_ref_id = self
            .authenticate_verified_transport(
                &mut verified,
                &username,
                &credentials,
                &ConnectionRouteStage::Target,
                interaction,
            )
            .await
            .map_err(route_failure(ConnectionRouteStage::Target))?;
        let transport = verified
            .into_authenticated()
            .map_err(route_failure(ConnectionRouteStage::Target))?;
        push_transport_heartbeat(
            &mut transport_heartbeats,
            ConnectionRouteStage::Target,
            &transport,
            transport_keepalive_interval,
        );

        Ok(AuthenticatedConnection {
            transport,
            credential_ref_id,
            negotiated_algorithms,
            transport_heartbeats,
        })
    }

    async fn authenticate_verified_transport<V, I>(
        &self,
        verified: &mut VerifiedTransport<V>,
        username: &str,
        credentials: &[ConnectionCredential],
        route_stage: &ConnectionRouteStage,
        interaction: &mut I,
    ) -> Result<CredentialRefId, TransportError>
    where
        V: HostKeyVerifier,
        I: SshConnectionInteraction,
    {
        let mut authenticated_credential_ref_id = None;
        let mut last_credential_error = TransportError::AuthenticationRejected;
        for (attempt_index, credential) in credentials.iter().enumerate() {
            let credential_ref_id = credential.credential_ref_id().clone();
            if let ConnectionCredential::Stored(record) = credential
                && let CredentialRecordDetails::KeyboardInteractive { max_rounds } = &record.details
            {
                match authenticate_keyboard_interactive(
                    verified,
                    username,
                    route_stage,
                    &credential_ref_id,
                    u8::try_from(attempt_index).unwrap_or(u8::MAX),
                    *max_rounds,
                    interaction,
                )
                .await
                {
                    Ok(()) => {
                        authenticated_credential_ref_id = Some(credential_ref_id);
                        break;
                    }
                    Err(TransportError::AuthenticationRejected) => {
                        last_credential_error = TransportError::AuthenticationRejected;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            let authentication = match credential {
                ConnectionCredential::Stored(credential) => {
                    authentication_from(self.vault, self.ssh_agent, credential).await
                }
                ConnectionCredential::Transient(credential_ref_id) => self
                    .transient_credentials
                    .take(credential_ref_id)
                    .ok_or(TransportError::AuthenticationRejected),
            };
            let authentication = match authentication {
                Ok(authentication) => authentication,
                Err(error) if authentication_error_allows_next_credential(&error, credential) => {
                    last_credential_error = error;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match verified
                .authenticate_attempt(username.to_owned(), authentication)
                .await
            {
                Ok(()) => {
                    authenticated_credential_ref_id = Some(credential_ref_id);
                    break;
                }
                Err(error) if authentication_error_allows_next_credential(&error, credential) => {
                    last_credential_error = error;
                }
                Err(error) => return Err(error),
            }
        }
        authenticated_credential_ref_id.ok_or(last_credential_error)
    }
}

fn authentication_error_allows_next_credential(
    error: &TransportError,
    credential: &ConnectionCredential,
) -> bool {
    let is_advanced_agent_identity = matches!(
        credential,
        ConnectionCredential::Stored(record)
            if matches!(
                record.details,
                CredentialRecordDetails::Certificate { .. }
                    | CredentialRecordDetails::HardwareKey { .. }
            )
    );
    if is_advanced_agent_identity {
        return matches!(error, TransportError::AuthenticationRejected);
    }
    matches!(
        error,
        TransportError::AuthenticationRejected
            | TransportError::InvalidPrivateKey
            | TransportError::InsecureRsaSignatureOnly
            | TransportError::SshAgentKeyUnavailable
            | TransportError::SshAgentUnavailable
    )
}

async fn authentication_from(
    vault: &VaultService,
    ssh_agent: &SshAgentService,
    credential: &CredentialRecord,
) -> Result<Authentication, TransportError> {
    match &credential.details {
        CredentialRecordDetails::Password { secret_ref_id } => {
            let password = vault
                .read_secret(secret_ref_id, SecretKind::Password)
                .map_err(|_| TransportError::AuthenticationRejected)?;
            Ok(Authentication::password(password.expose().to_vec()))
        }
        CredentialRecordDetails::PrivateKey {
            secret_ref_id,
            passphrase_secret_ref_id,
            ..
        } => {
            let private_key = vault
                .read_secret(secret_ref_id, SecretKind::PrivateKey)
                .map_err(|_| TransportError::InvalidPrivateKey)?;
            let passphrase = passphrase_secret_ref_id
                .as_ref()
                .map(|reference| vault.read_secret(reference, SecretKind::Passphrase))
                .transpose()
                .map_err(|_| TransportError::InvalidPrivateKey)?
                .map(|value| value.expose().to_vec());
            Ok(Authentication::private_key(
                private_key.expose().to_vec(),
                passphrase,
            ))
        }
        CredentialRecordDetails::SshAgent {
            public_key_blob, ..
        } => ssh_agent
            .authentication_for_key(
                public_key_blob,
                norishell_core_api::AgentIdentityKind::Ordinary,
                None,
                None,
            )
            .await
            .map_err(|error| match error {
                SshAgentServiceError::KeyUnavailable => TransportError::SshAgentKeyUnavailable,
                SshAgentServiceError::Unavailable
                | SshAgentServiceError::InvalidEndpoint
                | SshAgentServiceError::Protocol
                | SshAgentServiceError::Timeout => TransportError::SshAgentUnavailable,
            }),
        CredentialRecordDetails::Certificate { certificate, .. } => ssh_agent
            .authentication_for_key(
                &certificate.subject_public_key_blob,
                norishell_core_api::AgentIdentityKind::Certificate,
                Some(certificate),
                None,
            )
            .await
            .map_err(|error| match error {
                SshAgentServiceError::KeyUnavailable => TransportError::SshAgentKeyUnavailable,
                SshAgentServiceError::Unavailable
                | SshAgentServiceError::InvalidEndpoint
                | SshAgentServiceError::Protocol
                | SshAgentServiceError::Timeout => TransportError::SshAgentUnavailable,
            }),
        CredentialRecordDetails::HardwareKey {
            public_key_blob,
            application,
            ..
        } => ssh_agent
            .authentication_for_key(
                public_key_blob,
                norishell_core_api::AgentIdentityKind::HardwareKey,
                None,
                Some(application),
            )
            .await
            .map_err(|error| match error {
                SshAgentServiceError::KeyUnavailable => TransportError::SshAgentKeyUnavailable,
                SshAgentServiceError::Unavailable
                | SshAgentServiceError::InvalidEndpoint
                | SshAgentServiceError::Protocol
                | SshAgentServiceError::Timeout => TransportError::SshAgentUnavailable,
            }),
        CredentialRecordDetails::KeyboardInteractive { .. } => {
            Err(TransportError::AuthenticationRejected)
        }
    }
}

async fn authenticate_keyboard_interactive<V, I>(
    verified: &mut VerifiedTransport<V>,
    username: &str,
    route_stage: &ConnectionRouteStage,
    credential_ref_id: &CredentialRefId,
    attempt_index: u8,
    max_rounds: u8,
    interaction: &mut I,
) -> Result<(), TransportError>
where
    V: HostKeyVerifier,
    I: SshConnectionInteraction,
{
    if !(1..=32).contains(&max_rounds) {
        return Err(TransportError::InvalidKeyboardInteractiveResponse);
    }
    let mut outcome = verified
        .keyboard_interactive_start(username.to_owned())
        .await?;
    let mut round_index = 0_u8;
    let mut total_prompts = 0_usize;
    loop {
        match outcome {
            KeyboardInteractiveOutcome::Success => return Ok(()),
            KeyboardInteractiveOutcome::Rejected {
                partial_success: false,
                ..
            } => return Err(TransportError::AuthenticationRejected),
            KeyboardInteractiveOutcome::Rejected {
                partial_success: true,
                ..
            } => return Err(TransportError::AuthenticationIncomplete),
            KeyboardInteractiveOutcome::Challenge(challenge) => {
                round_index = round_index.saturating_add(1);
                total_prompts = total_prompts.saturating_add(challenge.prompts.len());
                if round_index > max_rounds
                    || total_prompts > KEYBOARD_INTERACTIVE_MAX_TOTAL_PROMPTS
                {
                    return Err(TransportError::InvalidKeyboardInteractiveResponse);
                }
                let responses = interaction
                    .answer_keyboard_interactive(KeyboardInteractiveRequest {
                        route_stage: route_stage.clone(),
                        credential_ref_id: credential_ref_id.clone(),
                        attempt_index,
                        round_index,
                        challenge,
                    })
                    .await?;
                outcome = verified.keyboard_interactive_respond(responses).await?;
            }
        }
    }
}

fn transport_ingress_from(
    vault: &VaultService,
    ingress: ResolvedRouteIngress,
) -> Result<TransportRouteIngress, TransportError> {
    match ingress {
        ResolvedRouteIngress::DirectTcp => Ok(TransportRouteIngress::DirectTcp),
        ResolvedRouteIngress::HttpConnectProxy {
            endpoint,
            authentication,
        } => Ok(TransportRouteIngress::HttpConnect {
            proxy: Endpoint::parse(&endpoint.address, endpoint.port)?,
            credentials: authentication
                .map(|authentication| proxy_credentials_from(vault, authentication))
                .transpose()?,
        }),
        ResolvedRouteIngress::Socks5Proxy {
            endpoint,
            dns_mode,
            authentication,
        } => Ok(TransportRouteIngress::Socks5 {
            proxy: Endpoint::parse(&endpoint.address, endpoint.port)?,
            dns_mode: match dns_mode {
                norishell_core_api::ProxyDnsMode::Local => Socks5DnsMode::Local,
                norishell_core_api::ProxyDnsMode::Proxy => Socks5DnsMode::Proxy,
            },
            credentials: authentication
                .map(|authentication| proxy_credentials_from(vault, authentication))
                .transpose()?,
        }),
    }
}

pub(crate) fn proxy_credentials_from(
    vault: &VaultService,
    authentication: crate::connection_profile::ResolvedProxyAuthentication,
) -> Result<ProxyCredentials, TransportError> {
    let CredentialRecordDetails::Password { secret_ref_id } = &authentication.credential.details
    else {
        return Err(TransportError::RouteIngress(RouteIngressError {
            stage: IngressStage::Configuration,
            kind: IngressFailureKind::CredentialUnavailable,
        }));
    };
    let password = vault
        .read_secret(secret_ref_id, SecretKind::Password)
        .map_err(|error| {
            TransportError::RouteIngress(RouteIngressError {
                stage: IngressStage::Configuration,
                kind: match error {
                    VaultServiceError::NotUnlocked => IngressFailureKind::CredentialLocked,
                    _ => IngressFailureKind::CredentialUnavailable,
                },
            })
        })?;
    ProxyCredentials::new(authentication.username, password.expose().to_vec())
        .map_err(|error| TransportError::RouteIngress(error.into()))
}

fn negotiated_algorithm_fact(
    route_stage: ConnectionRouteStage,
    policy: &ResolvedAlgorithmPolicy,
    negotiated: NegotiatedAlgorithms,
) -> NegotiatedAlgorithmFact {
    NegotiatedAlgorithmFact {
        route_stage,
        policy_id: policy.policy_id.clone(),
        policy_revision: policy.policy_revision,
        policy_catalog_version: policy.catalog_version,
        negotiated,
    }
}

fn push_transport_heartbeat<V: HostKeyVerifier>(
    facts: &mut Vec<RoutedTransportHeartbeat>,
    route_stage: ConnectionRouteStage,
    transport: &AuthenticatedTransport<V>,
    interval: Option<Duration>,
) {
    let Some(interval) = interval else {
        return;
    };
    facts.push(RoutedTransportHeartbeat {
        route_stage,
        handle: transport.heartbeat_handle(),
        first_due: tokio::time::Instant::now() + interval,
        first_due_at_unix_ms: unix_time_ms()
            .saturating_add(i64::try_from(interval.as_millis()).unwrap_or(i64::MAX)),
    });
}

fn route_failure(
    route_stage: ConnectionRouteStage,
) -> impl FnOnce(TransportError) -> SshConnectionFailure {
    move |error| SshConnectionFailure { error, route_stage }
}

async fn notify_phase<I: SshConnectionInteraction>(
    interaction: &mut I,
    phase: ConnectionPhase,
    route_stage: ConnectionRouteStage,
) -> Result<(), SshConnectionFailure> {
    interaction
        .phase_changed(phase, route_stage.clone())
        .await
        .map_err(route_failure(route_stage))
}

fn jump_route_stage(hop_index: usize, jump: &ResolvedJumpHost) -> ConnectionRouteStage {
    ConnectionRouteStage::JumpHost {
        hop_index: u8::try_from(hop_index).unwrap_or(u8::MAX),
        host_id: jump.host_id.clone(),
        endpoint: jump.endpoint.clone(),
    }
}

#[cfg(test)]
mod tests {
    use norishell_ssh_transport::AlgorithmPolicy;

    use super::*;

    #[derive(Default)]
    struct RecordingInteraction {
        phases: Vec<(ConnectionPhase, ConnectionRouteStage)>,
    }

    impl SshConnectionInteraction for RecordingInteraction {
        async fn phase_changed(
            &mut self,
            phase: ConnectionPhase,
            route_stage: ConnectionRouteStage,
        ) -> Result<(), TransportError> {
            self.phases.push((phase, route_stage));
            Ok(())
        }

        async fn answer_keyboard_interactive(
            &mut self,
            _request: KeyboardInteractiveRequest,
        ) -> Result<Vec<Zeroizing<String>>, TransportError> {
            panic!("stage routing test must not request authentication answers")
        }
    }

    fn jump(hostname: &str, port: u16) -> ResolvedJumpHost {
        ResolvedJumpHost {
            host_id: HostId::new(),
            endpoint: SshSessionEndpoint {
                address: hostname.to_owned(),
                port,
                username: Some("jump-user".to_owned()),
            },
            username: "jump-user".to_owned(),
            credentials: Vec::new(),
            algorithm_policy: ResolvedAlgorithmPolicy {
                transport: AlgorithmPolicy::secure_default(),
                policy_id: "secure-default".to_owned(),
                policy_revision: None,
                catalog_version: "test",
            },
            revision_token: format!("jump:{hostname}:{port}"),
        }
    }

    #[test]
    fn route_stages_are_resource_neutral_and_preserve_each_hop_identity() {
        let jumps = [jump("first.example", 22), jump("second.example", 2222)];

        let stages = jumps
            .iter()
            .enumerate()
            .map(|(index, jump)| jump_route_stage(index, jump))
            .chain(std::iter::once(ConnectionRouteStage::Target))
            .collect::<Vec<_>>();

        assert!(matches!(
            &stages[0],
            ConnectionRouteStage::JumpHost {
                hop_index: 0,
                host_id,
                endpoint,
            } if host_id == &jumps[0].host_id && endpoint.address == "first.example"
        ));
        assert!(matches!(
            &stages[1],
            ConnectionRouteStage::JumpHost {
                hop_index: 1,
                host_id,
                endpoint,
            } if host_id == &jumps[1].host_id && endpoint.port == 2222
        ));
        assert_eq!(stages[2], ConnectionRouteStage::Target);
    }

    #[test]
    fn partial_success_never_enters_the_ordered_credential_fallback_set() {
        let transient = ConnectionCredential::Transient(CredentialRefId::new());
        assert!(!authentication_error_allows_next_credential(
            &TransportError::AuthenticationIncomplete,
            &transient,
        ));
        assert!(authentication_error_allows_next_credential(
            &TransportError::AuthenticationRejected,
            &transient,
        ));
    }

    #[test]
    fn advanced_agent_identity_only_falls_back_after_an_ordinary_server_rejection() {
        let advanced = ConnectionCredential::Stored(Box::new(CredentialRecord {
            credential_ref_id: CredentialRefId::new(),
            identity_id: norishell_core_api::IdentityId::new(),
            method: norishell_core_api::AuthenticationMethodKind::HardwareKey,
            details: CredentialRecordDetails::HardwareKey {
                public_key_blob: vec![1, 2, 3],
                public_key_algorithm: "sk-ssh-ed25519@openssh.com".to_owned(),
                public_key_fingerprint: "SHA256:test".to_owned(),
                application: "ssh:test".to_owned(),
                scope: norishell_core_api::SshAgentScope::DefaultEnvironment,
            },
            priority: 0,
            label: "Security key".to_owned(),
            state_version: WireSequence::new(1),
            import_operation_id: None,
            import_idempotency_key: None,
            import_state: norishell_app_persistence::CredentialImportState::Ready,
        }));

        assert!(authentication_error_allows_next_credential(
            &TransportError::AuthenticationRejected,
            &advanced,
        ));
        for terminal_error in [
            TransportError::AuthenticationIncomplete,
            TransportError::AuthenticationTimeout,
            TransportError::SshAgentKeyUnavailable,
            TransportError::SshAgentUnavailable,
        ] {
            assert!(
                !authentication_error_allows_next_credential(&terminal_error, &advanced),
                "{terminal_error:?} must stop the advanced Agent authentication plan",
            );
        }
    }

    #[tokio::test]
    async fn phase_callback_keeps_the_exact_resource_neutral_route_stage() {
        let jump = jump("callback.example", 2200);
        let jump_stage = jump_route_stage(0, &jump);
        let mut interaction = RecordingInteraction::default();

        notify_phase(
            &mut interaction,
            ConnectionPhase::Connecting,
            jump_stage.clone(),
        )
        .await
        .expect("record connecting phase");
        notify_phase(
            &mut interaction,
            ConnectionPhase::Authenticating,
            ConnectionRouteStage::Target,
        )
        .await
        .expect("record target authentication phase");

        assert_eq!(
            interaction.phases,
            vec![
                (ConnectionPhase::Connecting, jump_stage),
                (
                    ConnectionPhase::Authenticating,
                    ConnectionRouteStage::Target,
                ),
            ]
        );
    }
}
