use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use norishell_core_api::{
    AgentIdentityKind, AgentIdentitySource, CoreApiError, ErrorCategory, RequestId, RetryStrategy,
    SshAgentCredentialCreateRequest, SshAgentKeyListRequest, SshAgentKeySummary,
    SshCertificateCriticalOption, SshCertificateExtension, SshCertificateMetadata,
    SshCertificateType,
};
use norishell_ssh_domain::ssh_sha256_fingerprint;
use norishell_ssh_transport::{
    AgentCertificateType, AgentClient, AgentEcdsaCurve, AgentHashAlgorithm, AgentIdentity,
    AgentKeyAlgorithm, AgentPublicKey, AgentStream, Authentication, PublicKeyBase64,
};
use tauri::State;
use tokio::time::timeout;
use uuid::Uuid;

use crate::{host_service::HostService, time::unix_time_ms};

const AGENT_LIST_TIMEOUT: Duration = Duration::from_secs(5);
const AGENT_HANDLE_TTL_MILLIS: i64 = 60_000;
const MAX_AGENT_KEYS: usize = 64;
const MAX_HANDLES: usize = 256;
const MAX_SAFE_COMMENT_BYTES: usize = 80;

type DynamicAgentClient = AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>;
type CoreResult<T> = Result<T, Box<CoreApiError>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SshAgentServiceError {
    Unavailable,
    InvalidEndpoint,
    Protocol,
    Timeout,
    KeyUnavailable,
}

#[derive(Clone)]
struct AgentKeySelection {
    public_key_blob: Vec<u8>,
    public_key_algorithm: String,
    public_key_fingerprint: String,
    identity_kind: AgentIdentityKind,
    certificate: Option<SshCertificateMetadata>,
    hardware_key_application: Option<String>,
    expires_at: Instant,
    consumed_by: Option<AgentCredentialRequestFingerprint>,
}

#[derive(Clone, PartialEq, Eq)]
struct AgentCredentialRequestFingerprint {
    operation_id: String,
    idempotency_key: String,
    identity_id: String,
    priority: u32,
    label: String,
}

#[derive(Default)]
struct HandleStore {
    values: BTreeMap<String, AgentKeySelection>,
    order: VecDeque<String>,
}

#[derive(Clone, Default)]
pub struct SshAgentService {
    handles: Arc<Mutex<HandleStore>>,
}

impl SshAgentService {
    pub(crate) async fn list_keys(&self) -> Result<Vec<SshAgentKeySummary>, SshAgentServiceError> {
        let (_client, identities) = open_and_list_default_agent().await?;
        let now = unix_time_ms();
        let expires_at_unix_ms = now.saturating_add(AGENT_HANDLE_TTL_MILLIS);
        let expires_at = Instant::now() + Duration::from_millis(AGENT_HANDLE_TTL_MILLIS as u64);
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_handles(&mut handles, Instant::now());
        let mut summaries = Vec::new();
        for identity in identities {
            let Some(identity_projection) = project_agent_identity(&identity) else {
                continue;
            };
            let comment = identity.comment();
            let public_key_blob = identity_projection.public_key_blob;
            let public_key_algorithm = identity_projection.public_key_algorithm;
            let public_key_fingerprint = ssh_sha256_fingerprint(&public_key_blob);
            let key_handle = Uuid::now_v7().to_string();
            handles.order.push_back(key_handle.clone());
            handles.values.insert(
                key_handle.clone(),
                AgentKeySelection {
                    public_key_blob,
                    public_key_algorithm: public_key_algorithm.clone(),
                    public_key_fingerprint: public_key_fingerprint.clone(),
                    identity_kind: identity_projection.identity_kind,
                    certificate: identity_projection.certificate.clone(),
                    hardware_key_application: identity_projection.hardware_key_application.clone(),
                    expires_at,
                    consumed_by: None,
                },
            );
            summaries.push(SshAgentKeySummary {
                key_handle,
                public_key_algorithm,
                public_key_fingerprint,
                identity_kind: identity_projection.identity_kind,
                certificate: identity_projection.certificate,
                hardware_key_application: identity_projection.hardware_key_application,
                comment: redact_agent_comment(comment),
                expires_at_unix_ms,
            });
        }
        while handles.values.len() > MAX_HANDLES {
            let Some(oldest) = handles.order.pop_front() else {
                break;
            };
            handles.values.remove(&oldest);
        }
        Ok(summaries)
    }

    fn key_for_request(
        &self,
        key_handle: &str,
        expected_identity_kind: AgentIdentityKind,
        fingerprint: AgentCredentialRequestFingerprint,
    ) -> Result<AgentKeySelection, SshAgentServiceError> {
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        prune_handles(&mut handles, now);
        let selection = handles
            .values
            .get_mut(key_handle)
            .ok_or(SshAgentServiceError::KeyUnavailable)?;
        if selection.expires_at <= now {
            return Err(SshAgentServiceError::KeyUnavailable);
        }
        if selection.identity_kind != expected_identity_kind
            || !selection_is_currently_usable(selection)
        {
            return Err(SshAgentServiceError::KeyUnavailable);
        }
        match &selection.consumed_by {
            Some(existing) if existing != &fingerprint => {
                return Err(SshAgentServiceError::KeyUnavailable);
            }
            Some(_) => {}
            None => selection.consumed_by = Some(fingerprint),
        }
        Ok(selection.clone())
    }

    /// Re-opens the standard Agent only after the SSH server identity was trusted, re-lists its
    /// keys, and returns a signer bound to the exact stored public blob on that same AgentClient.
    pub(crate) async fn authentication_for_key(
        &self,
        expected_public_key_blob: &[u8],
        expected_identity_kind: AgentIdentityKind,
        expected_certificate: Option<&SshCertificateMetadata>,
        expected_hardware_application: Option<&str>,
    ) -> Result<Authentication, SshAgentServiceError> {
        let (client, identities) = open_and_list_default_agent().await?;
        let identity = identities.into_iter().find(|identity| match identity {
            AgentIdentity::PublicKey { key, .. } => {
                let kind = classify_public_key(key);
                kind == Some(expected_identity_kind)
                    && key.public_key_bytes() == expected_public_key_blob
                    && hardware_key_application(key).as_deref() == expected_hardware_application
            }
            AgentIdentity::Certificate { certificate, .. } => {
                if expected_identity_kind != AgentIdentityKind::Certificate {
                    return false;
                }
                certificate_metadata(certificate).is_some_and(|metadata| {
                    expected_certificate == Some(&metadata)
                        && metadata.subject_public_key_blob == expected_public_key_blob
                        && certificate_is_currently_usable(&metadata)
                })
            }
        });
        match identity {
            Some(AgentIdentity::Certificate { certificate, .. }) => {
                Ok(Authentication::ssh_agent_certificate(certificate, client))
            }
            Some(identity @ AgentIdentity::PublicKey { .. }) => {
                Ok(Authentication::ssh_agent(identity, client))
            }
            None => Err(SshAgentServiceError::KeyUnavailable),
        }
    }
}

#[tauri::command]
pub async fn ssh_agent_key_list(
    request: SshAgentKeyListRequest,
    service: State<'_, SshAgentService>,
) -> CoreResult<Vec<SshAgentKeySummary>> {
    service
        .list_keys()
        .await
        .map_err(|error| map_agent_error(request.meta.request_id, error))
}

#[tauri::command]
pub fn ssh_agent_credential_create(
    request: SshAgentCredentialCreateRequest,
    service: State<'_, SshAgentService>,
    hosts: State<'_, HostService>,
) -> CoreResult<norishell_core_api::CredentialRefSummary> {
    let request_id = request.meta.request_id.clone();
    let fingerprint = AgentCredentialRequestFingerprint {
        operation_id: request.operation_id.as_str().to_owned(),
        idempotency_key: request.idempotency_key.clone(),
        identity_id: request.identity_id.as_str().to_owned(),
        priority: request.priority,
        label: request.label.clone(),
    };
    let selection = service
        .key_for_request(
            &request.key_handle,
            request.expected_identity_kind,
            fingerprint,
        )
        .map_err(|error| map_agent_error(request_id.clone(), error))?;
    hosts.create_agent_identity_credential(
        &request.operation_id,
        &request.idempotency_key,
        &request.identity_id,
        request.priority,
        &request.label,
        selection.identity_kind,
        &selection.public_key_blob,
        &selection.public_key_algorithm,
        &selection.public_key_fingerprint,
        selection.hardware_key_application.as_deref(),
        selection.certificate.as_ref(),
        request_id,
    )
}

struct AgentIdentityProjection {
    public_key_blob: Vec<u8>,
    public_key_algorithm: String,
    identity_kind: AgentIdentityKind,
    certificate: Option<SshCertificateMetadata>,
    hardware_key_application: Option<String>,
}

fn project_agent_identity(identity: &AgentIdentity) -> Option<AgentIdentityProjection> {
    match identity {
        AgentIdentity::PublicKey { key, .. } => {
            let identity_kind = classify_public_key(key)?;
            Some(AgentIdentityProjection {
                public_key_blob: key.public_key_bytes(),
                public_key_algorithm: key.algorithm().to_string(),
                identity_kind,
                certificate: None,
                hardware_key_application: hardware_key_application(key),
            })
        }
        AgentIdentity::Certificate { certificate, .. } => {
            let metadata = certificate_metadata(certificate)?;
            Some(AgentIdentityProjection {
                public_key_blob: metadata.subject_public_key_blob.clone(),
                public_key_algorithm: metadata.subject_public_key_algorithm.clone(),
                identity_kind: AgentIdentityKind::Certificate,
                certificate: Some(metadata),
                hardware_key_application: None,
            })
        }
    }
}

fn classify_public_key(key: &AgentPublicKey) -> Option<AgentIdentityKind> {
    if is_supported_agent_algorithm(key.algorithm()) {
        Some(AgentIdentityKind::Ordinary)
    } else if matches!(
        key.algorithm(),
        AgentKeyAlgorithm::SkEcdsaSha2NistP256 | AgentKeyAlgorithm::SkEd25519
    ) && hardware_key_application(key).is_some()
    {
        Some(AgentIdentityKind::HardwareKey)
    } else {
        None
    }
}

fn hardware_key_application(key: &AgentPublicKey) -> Option<String> {
    let application = key
        .key_data()
        .sk_ecdsa_p256()
        .map(|key| key.application())
        .or_else(|| key.key_data().sk_ed25519().map(|key| key.application()))?;
    (application.starts_with("ssh:")
        && application.len() <= 255
        && !application.chars().any(char::is_control))
    .then(|| application.to_owned())
}

fn certificate_metadata(
    certificate: &norishell_ssh_transport::OpenSshCertificate,
) -> Option<SshCertificateMetadata> {
    if certificate.cert_type() != AgentCertificateType::User
        || certificate.verify_signature().is_err()
    {
        return None;
    }
    let subject = AgentPublicKey::new(certificate.public_key().clone(), "");
    classify_public_key(&subject)?;
    let certificate_blob = certificate.to_bytes().ok()?;
    if certificate_blob.is_empty() || certificate_blob.len() > 64 * 1024 {
        return None;
    }
    let valid_after_unix_seconds = i64::try_from(certificate.valid_after()).ok()?;
    let valid_before_unix_seconds = (certificate.valid_before() != u64::MAX)
        .then(|| i64::try_from(certificate.valid_before()).ok())
        .flatten();
    if certificate.valid_before() != u64::MAX && valid_before_unix_seconds.is_none() {
        return None;
    }
    let valid_principals = bounded_certificate_strings(certificate.valid_principals(), 64, 256)?;
    let critical_options = certificate
        .critical_options()
        .iter()
        .take(33)
        .map(|(name, value)| SshCertificateCriticalOption {
            name: name.clone(),
            value: value.as_bytes().to_vec(),
            recognized: recognized_critical_option(name, value),
        })
        .collect::<Vec<_>>();
    if critical_options.len() != certificate.critical_options().len() {
        return None;
    }
    let extensions = certificate
        .extensions()
        .iter()
        .take(64)
        .map(|(name, value)| SshCertificateExtension {
            name: name.clone(),
            value: value.as_bytes().to_vec(),
            recognized: recognized_certificate_extension(name, value),
        })
        .collect::<Vec<_>>();
    if extensions.len() != certificate.extensions().len() {
        return None;
    }
    let subject_public_key_blob = subject.public_key_bytes();
    let ca_public_key_fingerprint = certificate
        .signature_key()
        .fingerprint(AgentHashAlgorithm::Sha256)
        .to_string();
    let certificate_algorithm = certificate.algorithm().to_certificate_type().to_string();
    Some(SshCertificateMetadata {
        source: AgentIdentitySource::SystemSshAgent,
        certificate_fingerprint: ssh_sha256_fingerprint(&certificate_blob),
        certificate_blob,
        certificate_algorithm,
        serial: certificate.serial().to_string(),
        subject_public_key_fingerprint: ssh_sha256_fingerprint(&subject_public_key_blob),
        subject_public_key_blob,
        subject_public_key_algorithm: subject.algorithm().to_string(),
        ca_public_key_fingerprint,
        key_id: bounded_certificate_text(certificate.key_id(), 1024)?,
        valid_principals,
        certificate_type: SshCertificateType::User,
        valid_after_unix_seconds,
        valid_before_unix_seconds,
        critical_options,
        extensions,
    })
}

fn bounded_certificate_strings(
    values: &[String],
    max_items: usize,
    max_bytes: usize,
) -> Option<Vec<String>> {
    if values.len() > max_items {
        return None;
    }
    values
        .iter()
        .map(|value| bounded_certificate_text(value, max_bytes))
        .collect()
}

fn bounded_certificate_text(value: &str, max_bytes: usize) -> Option<String> {
    (value.len() <= max_bytes && !value.chars().any(char::is_control)).then(|| value.to_owned())
}

fn recognized_critical_option(name: &str, value: &str) -> bool {
    match name {
        "force-command" => !value.is_empty() && value.len() <= 4096 && !value.contains('\0'),
        "source-address" => valid_source_address_option(value),
        "verify-required" => value.is_empty(),
        _ => false,
    }
}

fn valid_source_address_option(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value.split(',').all(|entry| {
            let (address, prefix) = entry.split_once('/').unwrap_or((entry, ""));
            let Ok(address) = address.parse::<std::net::IpAddr>() else {
                return false;
            };
            if prefix.is_empty() {
                return true;
            }
            prefix.parse::<u8>().is_ok_and(|prefix| match address {
                std::net::IpAddr::V4(_) => prefix <= 32,
                std::net::IpAddr::V6(_) => prefix <= 128,
            })
        })
}

fn recognized_certificate_extension(name: &str, value: &str) -> bool {
    value.is_empty()
        && matches!(
            name,
            "permit-X11-forwarding"
                | "permit-agent-forwarding"
                | "permit-port-forwarding"
                | "permit-pty"
                | "permit-user-rc"
                | "no-touch-required"
        )
}

fn certificate_is_currently_usable(metadata: &SshCertificateMetadata) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok());
    now.is_some_and(|now| {
        metadata.valid_after_unix_seconds <= now
            && metadata
                .valid_before_unix_seconds
                .is_none_or(|valid_before| now < valid_before)
            && !metadata.has_unknown_critical_options()
    })
}

fn selection_is_currently_usable(selection: &AgentKeySelection) -> bool {
    match selection.identity_kind {
        AgentIdentityKind::Ordinary => selection.certificate.is_none(),
        AgentIdentityKind::Certificate => selection
            .certificate
            .as_ref()
            .is_some_and(certificate_is_currently_usable),
        AgentIdentityKind::HardwareKey => selection
            .hardware_key_application
            .as_deref()
            .is_some_and(|application| application.starts_with("ssh:")),
    }
}

async fn open_and_list_default_agent()
-> Result<(DynamicAgentClient, Vec<AgentIdentity>), SshAgentServiceError> {
    timeout(AGENT_LIST_TIMEOUT, async {
        let mut client = connect_default_agent().await?;
        let identities = client
            .request_identities()
            .await
            .map_err(|_| SshAgentServiceError::Protocol)?;
        if identities.len() > MAX_AGENT_KEYS {
            return Err(SshAgentServiceError::Protocol);
        }
        Ok((client, identities))
    })
    .await
    .map_err(|_| SshAgentServiceError::Timeout)?
}

#[cfg(target_os = "macos")]
async fn connect_default_agent() -> Result<DynamicAgentClient, SshAgentServiceError> {
    use std::{os::fd::AsRawFd, os::unix::fs::FileTypeExt, path::PathBuf};

    let path = std::env::var_os("SSH_AUTH_SOCK")
        .map(PathBuf::from)
        .ok_or(SshAgentServiceError::Unavailable)?;
    if !path.is_absolute() {
        return Err(SshAgentServiceError::InvalidEndpoint);
    }
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|_| SshAgentServiceError::InvalidEndpoint)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(SshAgentServiceError::InvalidEndpoint);
    }
    let stream = tokio::net::UnixStream::connect(&path)
        .await
        .map_err(|_| SshAgentServiceError::Unavailable)?;
    let mut peer_euid = 0;
    let mut peer_egid = 0;
    // SAFETY: getpeereid only writes the two valid uid/gid pointers for this live Unix socket fd.
    let peer_result =
        unsafe { libc::getpeereid(stream.as_raw_fd(), &mut peer_euid, &mut peer_egid) };
    // SAFETY: geteuid has no preconditions or borrowed memory.
    let current_euid = unsafe { libc::geteuid() };
    if peer_result != 0 || peer_euid != current_euid {
        return Err(SshAgentServiceError::InvalidEndpoint);
    }
    Ok(AgentClient::connect(stream).dynamic())
}

#[cfg(windows)]
async fn connect_default_agent() -> Result<DynamicAgentClient, SshAgentServiceError> {
    const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    AgentClient::connect_named_pipe(OPENSSH_AGENT_PIPE)
        .await
        .map(AgentClient::dynamic)
        .map_err(|_| SshAgentServiceError::Unavailable)
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn connect_default_agent() -> Result<DynamicAgentClient, SshAgentServiceError> {
    Err(SshAgentServiceError::Unavailable)
}

fn is_supported_agent_algorithm(algorithm: AgentKeyAlgorithm) -> bool {
    matches!(
        algorithm,
        AgentKeyAlgorithm::Ed25519
            | AgentKeyAlgorithm::Ecdsa {
                curve: AgentEcdsaCurve::NistP256
                    | AgentEcdsaCurve::NistP384
                    | AgentEcdsaCurve::NistP521
            }
            | AgentKeyAlgorithm::Rsa { .. }
    )
}

fn redact_agent_comment(comment: &str) -> Option<String> {
    let trimmed = comment.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.contains("/Users/")
        || trimmed.contains("\\Users\\")
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    let mut value = String::new();
    for character in trimmed.chars() {
        if value.len().saturating_add(character.len_utf8()) > MAX_SAFE_COMMENT_BYTES {
            break;
        }
        value.push(character);
    }
    (!value.is_empty()).then_some(value)
}

fn prune_handles(handles: &mut HandleStore, now: Instant) {
    handles.values.retain(|_, value| value.expires_at > now);
    handles
        .order
        .retain(|handle| handles.values.contains_key(handle));
}

fn map_agent_error(request_id: RequestId, error: SshAgentServiceError) -> Box<CoreApiError> {
    let (code, category, retry_strategy, message_key) = match error {
        SshAgentServiceError::KeyUnavailable => (
            "ssh_agent.key_unavailable",
            ErrorCategory::Conflict,
            RetryStrategy::RefreshSnapshot,
            "errors.sshAgent.keyUnavailable",
        ),
        SshAgentServiceError::Timeout => (
            "ssh_agent.timeout",
            ErrorCategory::Unavailable,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.sshAgent.timeout",
        ),
        SshAgentServiceError::Unavailable
        | SshAgentServiceError::InvalidEndpoint
        | SshAgentServiceError::Protocol => (
            "ssh_agent.unavailable",
            ErrorCategory::Unavailable,
            RetryStrategy::AfterMilliseconds(1_000),
            "errors.sshAgent.unavailable",
        ),
    };
    Box::new(CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        AgentIdentityKind, AgentIdentitySource, SshCertificateCriticalOption,
        SshCertificateMetadata, SshCertificateType,
    };

    use super::{
        AgentCredentialRequestFingerprint, AgentKeySelection, SshAgentService,
        certificate_is_currently_usable, redact_agent_comment,
    };

    #[test]
    fn agent_comment_is_bounded_and_path_shaped_comments_are_redacted() {
        assert_eq!(redact_agent_comment("/Users/alice/.ssh/id_ed25519"), None);
        assert_eq!(
            redact_agent_comment("deploy key"),
            Some("deploy key".to_owned())
        );
        assert!(redact_agent_comment(&"x".repeat(200)).is_some_and(|value| value.len() == 80));
    }

    #[test]
    fn agent_key_handles_are_one_shot_and_expired_handles_fail_closed() {
        let service = SshAgentService::default();
        {
            let mut handles = service.handles.lock().expect("handle store");
            handles.order.push_back("one-shot".to_owned());
            handles.values.insert(
                "one-shot".to_owned(),
                AgentKeySelection {
                    public_key_blob: vec![1, 2, 3],
                    public_key_algorithm: "ssh-ed25519".to_owned(),
                    public_key_fingerprint: "SHA256:test".to_owned(),
                    identity_kind: AgentIdentityKind::Ordinary,
                    certificate: None,
                    hardware_key_application: None,
                    expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
                    consumed_by: None,
                },
            );
            handles.order.push_back("expired".to_owned());
            handles.values.insert(
                "expired".to_owned(),
                AgentKeySelection {
                    public_key_blob: vec![4, 5, 6],
                    public_key_algorithm: "ssh-ed25519".to_owned(),
                    public_key_fingerprint: "SHA256:expired".to_owned(),
                    identity_kind: AgentIdentityKind::Ordinary,
                    certificate: None,
                    hardware_key_application: None,
                    expires_at: std::time::Instant::now() - std::time::Duration::from_secs(1),
                    consumed_by: None,
                },
            );
        }

        let exact = AgentCredentialRequestFingerprint {
            operation_id: "operation".to_owned(),
            idempotency_key: "key".to_owned(),
            identity_id: "identity".to_owned(),
            priority: 100,
            label: "Agent key".to_owned(),
        };
        assert!(
            service
                .key_for_request("one-shot", AgentIdentityKind::Ordinary, exact.clone())
                .is_ok()
        );
        assert!(
            service
                .key_for_request("one-shot", AgentIdentityKind::Certificate, exact.clone())
                .is_err()
        );
        assert!(
            service
                .key_for_request("one-shot", AgentIdentityKind::Ordinary, exact.clone())
                .is_ok()
        );
        assert!(
            service
                .key_for_request(
                    "one-shot",
                    AgentIdentityKind::Ordinary,
                    AgentCredentialRequestFingerprint {
                        label: "changed".to_owned(),
                        ..exact.clone()
                    }
                )
                .is_err()
        );
        assert!(
            service
                .key_for_request("expired", AgentIdentityKind::Ordinary, exact)
                .is_err()
        );
    }

    #[test]
    fn certificate_validity_and_unknown_critical_options_fail_closed() {
        let now = super::unix_time_ms() / 1_000;
        let mut metadata = SshCertificateMetadata {
            source: AgentIdentitySource::SystemSshAgent,
            certificate_blob: vec![1],
            certificate_algorithm: "ssh-ed25519-cert-v01@openssh.com".to_owned(),
            certificate_fingerprint: "SHA256:cert".to_owned(),
            serial: "1".to_owned(),
            subject_public_key_blob: vec![2],
            subject_public_key_algorithm: "ssh-ed25519".to_owned(),
            subject_public_key_fingerprint: "SHA256:key".to_owned(),
            ca_public_key_fingerprint: "SHA256:ca".to_owned(),
            key_id: "qa".to_owned(),
            valid_principals: vec!["vincent".to_owned()],
            certificate_type: SshCertificateType::User,
            valid_after_unix_seconds: now - 1,
            valid_before_unix_seconds: Some(now + 60),
            critical_options: Vec::new(),
            extensions: Vec::new(),
        };
        assert!(certificate_is_currently_usable(&metadata));
        metadata.valid_after_unix_seconds = now + 1;
        assert!(!certificate_is_currently_usable(&metadata));
        metadata.valid_after_unix_seconds = now - 60;
        metadata.valid_before_unix_seconds = Some(now);
        assert!(!certificate_is_currently_usable(&metadata));
        metadata.valid_before_unix_seconds = Some(now + 60);
        metadata
            .critical_options
            .push(SshCertificateCriticalOption {
                name: "future-critical".to_owned(),
                value: Vec::new(),
                recognized: false,
            });
        assert!(!certificate_is_currently_usable(&metadata));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires an isolated NORISHELL_M4_AGENT_QA ssh-agent fixture"]
    async fn configured_macos_agent_qa_reparses_and_rebinds_the_exact_user_certificate() {
        assert!(std::env::var_os("NORISHELL_M4_AGENT_QA").is_some());
        let service = SshAgentService::default();
        let keys = service.list_keys().await.expect("list isolated QA Agent");
        let certificate = keys
            .iter()
            .find(|key| key.identity_kind == AgentIdentityKind::Certificate)
            .expect("isolated QA Agent exposes its user certificate");
        let metadata = certificate
            .certificate
            .as_ref()
            .expect("certificate metadata projection");
        assert_eq!(metadata.serial, "42");
        assert_eq!(metadata.key_id, "norishell-m4");
        assert_eq!(metadata.valid_principals, ["vincent"]);
        assert!(!metadata.has_unknown_critical_options());
        let authentication = service
            .authentication_for_key(
                &metadata.subject_public_key_blob,
                AgentIdentityKind::Certificate,
                Some(metadata),
                None,
            )
            .await
            .expect("re-open Agent and bind the exact certificate");
        assert!(matches!(
            authentication,
            norishell_ssh_transport::Authentication::SshAgentCertificate { .. }
        ));
        for blocked_key_id in ["future-cert", "unknown-critical"] {
            let blocked = keys
                .iter()
                .find(|key| {
                    key.certificate
                        .as_ref()
                        .is_some_and(|metadata| metadata.key_id == blocked_key_id)
                })
                .expect("isolated QA Agent exposes blocked certificate fixture");
            let blocked_metadata = blocked.certificate.as_ref().expect("certificate metadata");
            assert!(
                service
                    .authentication_for_key(
                        &blocked_metadata.subject_public_key_blob,
                        AgentIdentityKind::Certificate,
                        Some(blocked_metadata),
                        None,
                    )
                    .await
                    .is_err(),
                "{blocked_key_id} must fail closed before signing"
            );
        }
    }
}
