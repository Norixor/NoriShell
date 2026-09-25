use std::sync::Arc;

use norishell_app_persistence::KnownHostObservation;
use norishell_core_api::{
    CoreApiError, CredentialRefId, ErrorCategory, RequestId, RetryStrategy,
    SshConnectionTestRequest, SshConnectionTestResponse,
};
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    HostKeyDecision, HostKeyVerifier, ObservedHostKey, TransportError, VerifyFuture,
};
use tauri::State;
use zeroize::Zeroizing;

use crate::{
    connection_profile::{ConnectionProfileError, resolve_connection_test_profile},
    core_api_error::core_error,
    host_service::HostService,
    lifecycle::LifecycleState,
    ssh_agent_service::SshAgentService,
    ssh_connection_orchestrator::{
        ConnectionPhase, ConnectionRouteStage, KeyboardInteractiveRequest,
        SshConnectionInteraction, SshConnectionOrchestrator,
    },
    transient_credential_service::TransientCredentialService,
    vault_service::VaultService,
};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

#[derive(Clone)]
struct TrustedConnectionTestHostKeyVerifier {
    hosts: HostService,
}

impl HostKeyVerifier for TrustedConnectionTestHostKeyVerifier {
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

struct NonInteractiveConnectionTest;

impl SshConnectionInteraction for NonInteractiveConnectionTest {
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

struct TransientCredentialCleanup {
    service: TransientCredentialService,
    credential_ref_id: CredentialRefId,
}

impl Drop for TransientCredentialCleanup {
    fn drop(&mut self) {
        self.service.discard(&self.credential_ref_id);
    }
}

#[tauri::command]
pub async fn ssh_connection_test(
    request: SshConnectionTestRequest,
    hosts: State<'_, HostService>,
    vault: State<'_, VaultService>,
    transient_credentials: State<'_, TransientCredentialService>,
    ssh_agent: State<'_, SshAgentService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<SshConnectionTestResponse> {
    let request_id = request.meta.request_id;
    let cleanup = TransientCredentialCleanup {
        service: transient_credentials.inner().clone(),
        credential_ref_id: request.credential_ref_id.clone(),
    };
    let _creation_permit = lifecycle.acquire_resource_creation(request_id.clone())?;
    let profile = resolve_connection_test_profile(
        transient_credentials.inner(),
        &request.endpoint,
        &request.credential_ref_id,
    )
    .map_err(|error| map_profile_error(request_id.clone(), error))?;
    let verifier = Arc::new(TrustedConnectionTestHostKeyVerifier {
        hosts: hosts.inner().clone(),
    });
    let mut interaction = NonInteractiveConnectionTest;
    let connection = SshConnectionOrchestrator::new(
        vault.inner(),
        transient_credentials.inner(),
        ssh_agent.inner(),
    )
    .connect(profile, verifier, &mut interaction, None)
    .await
    .map_err(|failure| map_transport_error(request_id.clone(), failure.error))?;

    connection
        .transport
        .disconnect()
        .await
        .map_err(|error| map_transport_error(request_id, error))?;
    drop(cleanup);
    Ok(SshConnectionTestResponse { verified: true })
}

fn map_profile_error(request_id: RequestId, error: ConnectionProfileError) -> Box<CoreApiError> {
    match error {
        ConnectionProfileError::InvalidTarget
        | ConnectionProfileError::UnsupportedConfiguration
        | ConnectionProfileError::LoginAutomationConfirmationRequired => core_error(
            request_id,
            "ssh_connection_test.invalid_request",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.sshSession.invalidRequest",
        ),
        ConnectionProfileError::CredentialUnavailable => core_error(
            request_id,
            "ssh_connection_test.credential_unavailable",
            ErrorCategory::Conflict,
            RetryStrategy::WaitForUser,
            "errors.sshSession.credentialUnavailable",
        ),
        ConnectionProfileError::StaleHost | ConnectionProfileError::PersistenceUnavailable => {
            core_error(
                request_id,
                "ssh_connection_test.unavailable",
                ErrorCategory::Unavailable,
                RetryStrategy::RefreshSnapshot,
                "errors.sshSession.unavailable",
            )
        }
    }
}

fn map_transport_error(request_id: RequestId, error: TransportError) -> Box<CoreApiError> {
    let (code, category, retry_strategy, message_key) = match error {
        TransportError::InvalidEndpoint(_) => (
            "resolution_failed",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.sshSession.resolutionFailed",
        ),
        TransportError::ConnectTimeout => (
            "connection_timed_out",
            ErrorCategory::Timeout,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.sshSession.connectionTimedOut",
        ),
        TransportError::ConnectFailed => (
            "connection_failed",
            ErrorCategory::Unavailable,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.sshSession.connectionFailed",
        ),
        TransportError::HostKeyRejected => (
            "host_key_untrusted",
            ErrorCategory::Permission,
            RetryStrategy::WaitForUser,
            "errors.connectionTest.hostKeyUntrusted",
        ),
        TransportError::HostKeyMismatch { .. } => (
            "host_key_mismatch",
            ErrorCategory::Permission,
            RetryStrategy::WaitForUser,
            "errors.sshSession.hostKeyMismatch",
        ),
        TransportError::AuthenticationRejected
        | TransportError::InvalidPrivateKey
        | TransportError::InsecureRsaSignatureOnly => (
            "authentication_rejected",
            ErrorCategory::Permission,
            RetryStrategy::WaitForUser,
            "errors.sshSession.authenticationRejected",
        ),
        TransportError::AuthenticationTimeout => (
            "authentication_timed_out",
            ErrorCategory::Timeout,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.sshSession.authenticationTimedOut",
        ),
        TransportError::DisconnectTimeout => (
            "cleanup_timed_out",
            ErrorCategory::Timeout,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.connectionTest.cleanupTimedOut",
        ),
        TransportError::AlgorithmNegotiationFailed { .. } => (
            "algorithm_negotiation_failed",
            ErrorCategory::Incompatible,
            RetryStrategy::Never,
            "errors.sshSession.algorithmNegotiationFailed",
        ),
        TransportError::SshAgentKeyUnavailable => (
            "ssh_agent_key_unavailable",
            ErrorCategory::Unavailable,
            RetryStrategy::WaitForUser,
            "errors.sshSession.sshAgentKeyUnavailable",
        ),
        TransportError::SshAgentUnavailable => (
            "ssh_agent_unavailable",
            ErrorCategory::Unavailable,
            RetryStrategy::WaitForUser,
            "errors.sshSession.sshAgentUnavailable",
        ),
        _ => (
            "protocol_error",
            ErrorCategory::Unavailable,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.sshSession.protocolError",
        ),
    };
    core_error(
        request_id,
        &format!("ssh_connection_test.{code}"),
        category,
        retry_strategy,
        message_key,
    )
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{ErrorCategory, RequestId, RetryStrategy};
    use norishell_ssh_transport::TransportError;

    use super::map_transport_error;

    #[test]
    fn authentication_rejection_remains_an_actionable_test_failure() {
        let error = map_transport_error(RequestId::new(), TransportError::AuthenticationRejected);
        assert_eq!(error.code, "ssh_connection_test.authentication_rejected");
        assert_eq!(error.category, ErrorCategory::Permission);
        assert_eq!(error.retry_strategy, RetryStrategy::WaitForUser);
        assert_eq!(
            error.message_key,
            "errors.sshSession.authenticationRejected"
        );
    }

    #[test]
    fn unknown_host_key_requires_explicit_review() {
        let error = map_transport_error(RequestId::new(), TransportError::HostKeyRejected);
        assert_eq!(error.code, "ssh_connection_test.host_key_untrusted");
        assert_eq!(error.retry_strategy, RetryStrategy::WaitForUser);
        assert_eq!(error.message_key, "errors.connectionTest.hostKeyUntrusted");
    }

    #[test]
    fn cleanup_timeout_is_not_reported_as_a_success() {
        let error = map_transport_error(RequestId::new(), TransportError::DisconnectTimeout);
        assert_eq!(error.code, "ssh_connection_test.cleanup_timed_out");
        assert_eq!(error.category, ErrorCategory::Timeout);
        assert_eq!(error.message_key, "errors.connectionTest.cleanupTimedOut");
    }
}
