use std::collections::BTreeSet;

use norishell_app_persistence::{
    AppPersistenceError, CredentialRecord, CredentialRecordDetails, HostConnectionSnapshot,
};
use norishell_core_api::{
    CredentialRefId, HeartbeatPolicy, LoginAutomationStepInput, MonitoringPolicy, ProxyDnsMode,
    ProxyEndpoint, RouteIngress, SshSessionEndpoint, SshSessionTarget, WireSequence,
};
use norishell_ssh_domain::Endpoint;
use norishell_ssh_transport::{
    ALGORITHM_POLICY_CATALOG_VERSION, AlgorithmPolicy, SECURE_DEFAULT_ALGORITHM_POLICY_ID,
};

use crate::{host_service::HostService, transient_credential_service::TransientCredentialService};

#[derive(Clone)]
pub(crate) enum ConnectionCredential {
    Stored(Box<CredentialRecord>),
    Transient(CredentialRefId),
}

impl ConnectionCredential {
    pub(crate) fn credential_ref_id(&self) -> &CredentialRefId {
        match self {
            Self::Stored(credential) => &credential.credential_ref_id,
            Self::Transient(credential_ref_id) => credential_ref_id,
        }
    }
}

pub(crate) struct ResolvedConnectionProfile {
    pub(crate) endpoint: SshSessionEndpoint,
    pub(crate) username: String,
    pub(crate) ingress: ResolvedRouteIngress,
    pub(crate) jump_hosts: Vec<ResolvedJumpHost>,
    pub(crate) credentials: Vec<ConnectionCredential>,
    pub(crate) algorithm_policy: ResolvedAlgorithmPolicy,
    pub(crate) heartbeat_policy: ResolvedHeartbeatPolicy,
    pub(crate) login_automation: Option<ResolvedLoginAutomation>,
    /// Opaque source-revision token captured before an operation starts. It is
    /// intentionally non-cryptographic and contains no secret references.
    pub(crate) revision_token: String,
}

pub(crate) struct ResolvedTerminalConnectionProfile {
    pub(crate) connection: ResolvedSshConnectionBase,
    pub(crate) heartbeat_policy: ResolvedHeartbeatPolicy,
    pub(crate) login_automation: Option<ResolvedLoginAutomation>,
    pub(crate) revision_token: String,
}

#[derive(Clone)]
pub(crate) struct ResolvedSshConnectionBase {
    pub(crate) endpoint: SshSessionEndpoint,
    pub(crate) username: String,
    pub(crate) ingress: ResolvedRouteIngress,
    pub(crate) jump_hosts: Vec<ResolvedJumpHost>,
    pub(crate) credentials: Vec<ConnectionCredential>,
    pub(crate) algorithm_policy: ResolvedAlgorithmPolicy,
    /// Contains only Host, route, authentication, algorithm and credential
    /// revisions. Resource policies are added by the owning profile.
    pub(crate) revision_token: String,
}

impl From<ResolvedTerminalConnectionProfile> for ResolvedConnectionProfile {
    fn from(profile: ResolvedTerminalConnectionProfile) -> Self {
        let ResolvedSshConnectionBase {
            endpoint,
            username,
            ingress,
            jump_hosts,
            credentials,
            algorithm_policy,
            revision_token: _,
        } = profile.connection;
        Self {
            endpoint,
            username,
            ingress,
            jump_hosts,
            credentials,
            algorithm_policy,
            heartbeat_policy: profile.heartbeat_policy,
            login_automation: profile.login_automation,
            revision_token: profile.revision_token,
        }
    }
}

pub(crate) struct ResolvedMetricsConnectionProfile {
    pub(crate) connection: ResolvedSshConnectionBase,
    pub(crate) monitoring_policy: ResolvedMonitoringPolicy,
    pub(crate) revision_token: String,
}

pub(crate) struct ResolvedLongLivedConnectionProfile {
    pub(crate) connection: ResolvedSshConnectionBase,
    pub(crate) transport_keepalive: Option<ResolvedTransportKeepalivePolicy>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedMonitoringPolicy {
    pub(crate) revision: WireSequence,
    pub(crate) policy: MonitoringPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedTransportKeepalivePolicy {
    pub(crate) revision: WireSequence,
    pub(crate) interval_seconds: u32,
    pub(crate) reply_timeout_seconds: u32,
    pub(crate) failure_threshold: u8,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedHeartbeatPolicy {
    pub(crate) revision: Option<WireSequence>,
    pub(crate) policy: HeartbeatPolicy,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedLoginAutomation {
    pub(crate) revision: WireSequence,
    pub(crate) steps: Vec<LoginAutomationStepInput>,
}

#[derive(Clone)]
pub(crate) struct ResolvedJumpHost {
    pub(crate) host_id: norishell_core_api::HostId,
    pub(crate) endpoint: SshSessionEndpoint,
    pub(crate) username: String,
    pub(crate) credentials: Vec<ConnectionCredential>,
    pub(crate) algorithm_policy: ResolvedAlgorithmPolicy,
    pub(crate) revision_token: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedAlgorithmPolicy {
    pub(crate) transport: AlgorithmPolicy,
    pub(crate) policy_id: String,
    pub(crate) policy_revision: Option<norishell_core_api::WireSequence>,
    pub(crate) catalog_version: &'static str,
}

#[derive(Clone)]
pub(crate) enum ResolvedRouteIngress {
    DirectTcp,
    HttpConnectProxy {
        endpoint: ProxyEndpoint,
        authentication: Option<ResolvedProxyAuthentication>,
    },
    Socks5Proxy {
        endpoint: ProxyEndpoint,
        dns_mode: ProxyDnsMode,
        authentication: Option<ResolvedProxyAuthentication>,
    },
}

#[derive(Clone)]
pub(crate) struct ResolvedProxyAuthentication {
    pub(crate) username: String,
    pub(crate) credential: Box<CredentialRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionProfileError {
    InvalidTarget,
    StaleHost,
    CredentialUnavailable,
    LoginAutomationConfirmationRequired,
    UnsupportedConfiguration,
    PersistenceUnavailable,
}

pub(crate) fn resolve_connection_profile(
    hosts: &HostService,
    transient_credentials: &TransientCredentialService,
    target: &SshSessionTarget,
    requested_credential_ref_id: Option<&CredentialRefId>,
) -> Result<ResolvedConnectionProfile, ConnectionProfileError> {
    resolve_terminal_connection_profile(
        hosts,
        transient_credentials,
        target,
        requested_credential_ref_id,
    )
    .map(ResolvedConnectionProfile::from)
}

/// Resolves the Host editor's direct connection probe without consulting any
/// saved Host, Identity or CredentialRef. The caller must provide a live
/// transient credential created for this exact form submission.
pub(crate) fn resolve_connection_test_profile(
    transient_credentials: &TransientCredentialService,
    endpoint: &SshSessionEndpoint,
    credential_ref_id: &CredentialRefId,
) -> Result<ResolvedSshConnectionBase, ConnectionProfileError> {
    let parsed_endpoint = Endpoint::parse(&endpoint.address, endpoint.port)
        .map_err(|_| ConnectionProfileError::InvalidTarget)?;
    let username = required_username(endpoint.username.as_deref())?;
    if !transient_credentials.contains(credential_ref_id) {
        return Err(ConnectionProfileError::CredentialUnavailable);
    }
    Ok(ResolvedSshConnectionBase {
        endpoint: endpoint.clone(),
        username,
        ingress: ResolvedRouteIngress::DirectTcp,
        jump_hosts: Vec::new(),
        credentials: vec![ConnectionCredential::Transient(credential_ref_id.clone())],
        algorithm_policy: ResolvedAlgorithmPolicy {
            transport: AlgorithmPolicy::secure_default(),
            policy_id: SECURE_DEFAULT_ALGORITHM_POLICY_ID.to_owned(),
            policy_revision: None,
            catalog_version: ALGORITHM_POLICY_CATALOG_VERSION,
        },
        revision_token: format!(
            "connection-test-v1:{}:{}:{}",
            parsed_endpoint.normalized_address(),
            parsed_endpoint.port(),
            credential_ref_id.as_str(),
        ),
    })
}

/// Resolves an explicit reconnect against the Host's latest committed snapshot.
///
/// The session target keeps the Host version used by its previous generation so
/// ordinary opens can still detect stale launch intents. A reconnect is a new
/// connection operation, however, and must pick up Host edits made after the
/// failed generation (including a newly attached Identity). The refreshed
/// target is returned so the Session actor can fence subsequent generations
/// against the snapshot that was actually used.
pub(crate) fn resolve_reconnect_connection_profile(
    hosts: &HostService,
    transient_credentials: &TransientCredentialService,
    target: &SshSessionTarget,
    requested_credential_ref_id: Option<&CredentialRefId>,
) -> Result<(ResolvedConnectionProfile, SshSessionTarget), ConnectionProfileError> {
    match target {
        SshSessionTarget::QuickConnect { .. } => resolve_connection_profile(
            hosts,
            transient_credentials,
            target,
            requested_credential_ref_id,
        )
        .map(|profile| (profile, target.clone())),
        SshSessionTarget::Host { host_id, .. } => {
            let snapshot = hosts
                .get_connection_snapshot(host_id)
                .map_err(map_persistence_error)?;
            let refreshed_target = SshSessionTarget::Host {
                host_id: host_id.clone(),
                expected_host_state_version: snapshot.host.state_version,
            };
            resolve_saved_host(
                hosts,
                snapshot,
                transient_credentials,
                requested_credential_ref_id,
            )
            .map(ResolvedConnectionProfile::from)
            .map(|profile| (profile, refreshed_target))
        }
    }
}

pub(crate) fn resolve_terminal_connection_profile(
    hosts: &HostService,
    transient_credentials: &TransientCredentialService,
    target: &SshSessionTarget,
    requested_credential_ref_id: Option<&CredentialRefId>,
) -> Result<ResolvedTerminalConnectionProfile, ConnectionProfileError> {
    match target {
        SshSessionTarget::QuickConnect { endpoint } => resolve_quick_connect(
            hosts,
            transient_credentials,
            endpoint,
            requested_credential_ref_id,
        ),
        SshSessionTarget::Host {
            host_id,
            expected_host_state_version,
        } => {
            let snapshot = hosts
                .get_connection_snapshot(host_id)
                .map_err(map_persistence_error)?;
            if snapshot.host.state_version != *expected_host_state_version {
                return Err(ConnectionProfileError::StaleHost);
            }
            resolve_saved_host(
                hosts,
                snapshot,
                transient_credentials,
                requested_credential_ref_id,
            )
        }
    }
}

/// Resolves a saved Host for the Metrics probe scheduler from one immutable
/// persistence snapshot. Terminal login automation and heartbeat policies are
/// neither parsed nor validated because each sample uses a bounded one-shot
/// exec probe rather than a long-lived Terminal or Metrics transport.
pub(crate) fn resolve_metrics_connection_profile(
    hosts: &HostService,
    host_id: &norishell_core_api::HostId,
    expected_host_state_version: WireSequence,
) -> Result<ResolvedMetricsConnectionProfile, ConnectionProfileError> {
    let snapshot = hosts
        .get_connection_snapshot(host_id)
        .map_err(map_persistence_error)?;
    if snapshot.host.state_version != expected_host_state_version {
        return Err(ConnectionProfileError::StaleHost);
    }
    resolve_saved_metrics_profile(hosts, &snapshot)
}

/// Resolves one saved Host for a newly created long-lived non-Terminal SSH
/// resource such as SFTP or forwarding. This profile is deliberately
/// independent from Monitoring and Terminal login automation.
pub(crate) fn resolve_long_lived_connection_profile(
    hosts: &HostService,
    host_id: &norishell_core_api::HostId,
    expected_host_state_version: WireSequence,
) -> Result<ResolvedLongLivedConnectionProfile, ConnectionProfileError> {
    let snapshot = hosts
        .get_connection_snapshot(host_id)
        .map_err(map_persistence_error)?;
    if snapshot.host.state_version != expected_host_state_version {
        return Err(ConnectionProfileError::StaleHost);
    }
    let connection = resolve_saved_host_base(
        hosts,
        &snapshot,
        select_stored_credentials(snapshot.credentials.clone())?,
    )?;
    let transport_keepalive = resolved_transport_keepalive(&snapshot);
    Ok(ResolvedLongLivedConnectionProfile {
        connection,
        transport_keepalive,
    })
}

/// Returns whether every available credential route for this connection needs
/// a Vault-backed secret. Agent and transient credentials keep a route usable
/// without opening the Vault.
pub(crate) fn connection_requires_vault(connection: &ResolvedSshConnectionBase) -> bool {
    let ingress_requires_vault = match &connection.ingress {
        ResolvedRouteIngress::HttpConnectProxy { authentication, .. }
        | ResolvedRouteIngress::Socks5Proxy { authentication, .. } => authentication.is_some(),
        ResolvedRouteIngress::DirectTcp => false,
    };
    ingress_requires_vault
        || credentials_require_vault(&connection.credentials)
        || connection
            .jump_hosts
            .iter()
            .any(|jump| credentials_require_vault(&jump.credentials))
}

/// Returns whether any route stage can use a Vault-backed secret. A mixed
/// credential plan can start through an Agent credential, then use this fact
/// to offer an explicit Vault recovery action after authentication fails.
pub(crate) fn connection_has_vault_credentials(connection: &ResolvedSshConnectionBase) -> bool {
    let ingress_has_vault = match &connection.ingress {
        ResolvedRouteIngress::HttpConnectProxy { authentication, .. }
        | ResolvedRouteIngress::Socks5Proxy { authentication, .. } => authentication.is_some(),
        ResolvedRouteIngress::DirectTcp => false,
    };
    ingress_has_vault
        || connection.credentials.iter().any(credential_requires_vault)
        || connection
            .jump_hosts
            .iter()
            .any(|jump| jump.credentials.iter().any(credential_requires_vault))
}

fn credentials_require_vault(credentials: &[ConnectionCredential]) -> bool {
    !credentials.is_empty() && credentials.iter().all(credential_requires_vault)
}

fn credential_requires_vault(credential: &ConnectionCredential) -> bool {
    matches!(
        credential,
        ConnectionCredential::Stored(record)
            if matches!(
                record.details,
                CredentialRecordDetails::Password { .. }
                    | CredentialRecordDetails::PrivateKey { .. }
            )
    )
}

fn resolve_quick_connect(
    hosts: &HostService,
    transient_credentials: &TransientCredentialService,
    endpoint: &SshSessionEndpoint,
    requested_credential_ref_id: Option<&CredentialRefId>,
) -> Result<ResolvedTerminalConnectionProfile, ConnectionProfileError> {
    let parsed_endpoint = Endpoint::parse(&endpoint.address, endpoint.port)
        .map_err(|_| ConnectionProfileError::InvalidTarget)?;
    let username = required_username(endpoint.username.as_deref())?;
    let credential_ref_id =
        requested_credential_ref_id.ok_or(ConnectionProfileError::CredentialUnavailable)?;
    let credential = match hosts.get_ready_credential(credential_ref_id) {
        Ok(credential) => ConnectionCredential::Stored(Box::new(credential)),
        Err(AppPersistenceError::NotFound) if transient_credentials.contains(credential_ref_id) => {
            ConnectionCredential::Transient(credential_ref_id.clone())
        }
        Err(AppPersistenceError::NotFound) => {
            return Err(ConnectionProfileError::CredentialUnavailable);
        }
        Err(error) => return Err(map_persistence_error(error)),
    };
    let revision_token = format!(
        "quick-v1:{}:{}:{}",
        parsed_endpoint.normalized_address(),
        parsed_endpoint.port(),
        credential.credential_ref_id().as_str(),
    );
    Ok(ResolvedTerminalConnectionProfile {
        connection: ResolvedSshConnectionBase {
            endpoint: endpoint.clone(),
            username,
            ingress: ResolvedRouteIngress::DirectTcp,
            jump_hosts: Vec::new(),
            credentials: vec![credential],
            algorithm_policy: ResolvedAlgorithmPolicy {
                transport: AlgorithmPolicy::secure_default(),
                policy_id: SECURE_DEFAULT_ALGORITHM_POLICY_ID.to_owned(),
                policy_revision: None,
                catalog_version: ALGORITHM_POLICY_CATALOG_VERSION,
            },
            revision_token: revision_token.clone(),
        },
        heartbeat_policy: ResolvedHeartbeatPolicy {
            revision: None,
            policy: HeartbeatPolicy::Disabled,
        },
        login_automation: None,
        revision_token,
    })
}

fn resolve_saved_host(
    hosts: &HostService,
    snapshot: HostConnectionSnapshot,
    transient_credentials: &TransientCredentialService,
    requested_credential_ref_id: Option<&CredentialRefId>,
) -> Result<ResolvedTerminalConnectionProfile, ConnectionProfileError> {
    let config = &snapshot.config;
    let heartbeat_policy = ResolvedHeartbeatPolicy {
        revision: Some(config.heartbeat_policy.revision),
        policy: config.heartbeat_policy.policy.clone(),
    };
    let login_automation = snapshot
        .login_automation
        .enabled
        .then(|| ResolvedLoginAutomation {
            revision: snapshot.login_automation.revision,
            steps: snapshot.login_automation.steps.clone(),
        });
    if snapshot.login_automation.enabled
        && snapshot.login_automation.confirmed_revision != Some(snapshot.login_automation.revision)
    {
        return Err(ConnectionProfileError::LoginAutomationConfirmationRequired);
    }
    if snapshot.login_automation.enabled
        && (snapshot.login_automation.steps.is_empty()
            || snapshot.login_automation.steps.iter().any(|step| {
                matches!(
                    step,
                    LoginAutomationStepInput::PreserveExistingSecret { .. }
                )
            }))
    {
        return Err(ConnectionProfileError::UnsupportedConfiguration);
    }

    let connection = resolve_saved_host_base(
        hosts,
        &snapshot,
        select_host_credentials(
            snapshot.credentials.clone(),
            transient_credentials,
            requested_credential_ref_id,
        )?,
    )?;
    let revision_token = format!(
        "terminal-v1:{}:heartbeat:{}:login:{}",
        connection.revision_token,
        config.heartbeat_policy.revision.get(),
        config.login_automation.revision.get(),
    );
    Ok(ResolvedTerminalConnectionProfile {
        connection,
        heartbeat_policy,
        login_automation,
        revision_token,
    })
}

fn resolve_saved_metrics_profile(
    hosts: &HostService,
    snapshot: &HostConnectionSnapshot,
) -> Result<ResolvedMetricsConnectionProfile, ConnectionProfileError> {
    let config = &snapshot.config;
    let connection = resolve_saved_host_base(
        hosts,
        snapshot,
        select_stored_credentials(snapshot.credentials.clone())?,
    )?;
    if !credentials_support_background_metrics(&connection.credentials)
        || connection
            .jump_hosts
            .iter()
            .any(|jump| !credentials_support_background_metrics(&jump.credentials))
    {
        return Err(ConnectionProfileError::UnsupportedConfiguration);
    }
    let monitoring_policy = ResolvedMonitoringPolicy {
        revision: config.monitoring_policy.revision,
        policy: config.monitoring_policy.policy.clone(),
    };
    let revision_token = format!(
        "metrics-probe-v1:{}:monitoring:{}",
        connection.revision_token,
        monitoring_policy.revision.get(),
    );
    Ok(ResolvedMetricsConnectionProfile {
        connection,
        monitoring_policy,
        revision_token,
    })
}

fn credentials_support_background_metrics(credentials: &[ConnectionCredential]) -> bool {
    credentials.iter().all(|credential| {
        matches!(
            credential,
            ConnectionCredential::Stored(record)
                if matches!(
                    record.details,
                    CredentialRecordDetails::Password { .. }
                        | CredentialRecordDetails::PrivateKey { .. }
                )
        )
    })
}

fn resolved_transport_keepalive(
    snapshot: &HostConnectionSnapshot,
) -> Option<ResolvedTransportKeepalivePolicy> {
    let config = &snapshot.config;
    match &config.heartbeat_policy.policy {
        HeartbeatPolicy::TransportKeepalive {
            interval_seconds,
            reply_timeout_seconds,
            failure_threshold,
        } => Some(ResolvedTransportKeepalivePolicy {
            revision: config.heartbeat_policy.revision,
            interval_seconds: *interval_seconds,
            reply_timeout_seconds: *reply_timeout_seconds,
            failure_threshold: *failure_threshold,
        }),
        HeartbeatPolicy::Disabled | HeartbeatPolicy::ShellHeartbeat { .. } => None,
    }
}

fn resolve_saved_host_base(
    hosts: &HostService,
    snapshot: &HostConnectionSnapshot,
    credentials: Vec<ConnectionCredential>,
) -> Result<ResolvedSshConnectionBase, ConnectionProfileError> {
    let config = &snapshot.config;
    let username = required_username(snapshot.host.username.as_deref().or_else(|| {
        snapshot
            .identity
            .as_ref()
            .and_then(|value| value.username.as_deref())
    }))?;
    let algorithm_policy = resolve_algorithm_policy(&config.algorithm_policy)?;
    let ingress = resolve_route_ingress(hosts, &config.route_plan.ingress)?;
    validate_jump_host_ids(&snapshot.host.host_id, &config.route_plan.jump_host_ids)?;
    let jump_hosts = config
        .route_plan
        .jump_host_ids
        .iter()
        .map(|host_id| resolve_jump_host(hosts, host_id))
        .collect::<Result<Vec<_>, _>>()?;
    let credential_token = connection_credentials_token(&credentials);
    let ingress_credential_token = ingress_credential_token(&ingress);
    let jump_token = jump_hosts
        .iter()
        .map(|jump| jump.revision_token.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let revision_token = format!(
        "base-v2:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        snapshot.host.host_id.as_str(),
        snapshot.host.state_version.get(),
        revision_token_component(&username),
        config.route_plan.revision.get(),
        config.authentication_plan.revision.get(),
        config.algorithm_policy.revision.get(),
        ingress_credential_token,
        jump_token,
        credential_token,
    );
    Ok(ResolvedSshConnectionBase {
        endpoint: SshSessionEndpoint {
            address: snapshot.host.address.clone(),
            port: snapshot.host.port,
            username: Some(username.clone()),
        },
        username,
        ingress,
        jump_hosts,
        credentials,
        algorithm_policy,
        revision_token,
    })
}

fn ingress_credential_token(ingress: &ResolvedRouteIngress) -> String {
    match ingress {
        ResolvedRouteIngress::DirectTcp
        | ResolvedRouteIngress::HttpConnectProxy {
            authentication: None,
            ..
        }
        | ResolvedRouteIngress::Socks5Proxy {
            authentication: None,
            ..
        } => "none".to_owned(),
        ResolvedRouteIngress::HttpConnectProxy {
            authentication: Some(authentication),
            ..
        }
        | ResolvedRouteIngress::Socks5Proxy {
            authentication: Some(authentication),
            ..
        } => format!(
            "{}:{}:{}",
            authentication.credential.credential_ref_id.as_str(),
            authentication.credential.state_version.get(),
            revision_token_component(&authentication.username),
        ),
    }
}

/// Encodes dynamic text without relying on a delimiter that could occur in a
/// user name. Tokens are opaque freshness fences and contain no secrets.
fn revision_token_component(value: &str) -> String {
    format!("{}:{value}", value.len())
}

fn validate_jump_host_ids(
    target_host_id: &norishell_core_api::HostId,
    jump_host_ids: &[norishell_core_api::HostId],
) -> Result<(), ConnectionProfileError> {
    let unique_jump_host_ids = jump_host_ids
        .iter()
        .map(|host_id| host_id.as_str())
        .collect::<BTreeSet<_>>();
    if jump_host_ids.len() > 5
        || unique_jump_host_ids.len() != jump_host_ids.len()
        || jump_host_ids
            .iter()
            .any(|host_id| host_id == target_host_id)
    {
        return Err(ConnectionProfileError::UnsupportedConfiguration);
    }
    Ok(())
}

fn resolve_jump_host(
    hosts: &HostService,
    host_id: &norishell_core_api::HostId,
) -> Result<ResolvedJumpHost, ConnectionProfileError> {
    let snapshot = hosts
        .get_connection_snapshot(host_id)
        .map_err(map_persistence_error)?;
    let config = &snapshot.config;
    // An explicit target chain owns its route order. A referenced hop with
    // another ingress/chain would otherwise be silently reinterpreted.
    if !matches!(config.route_plan.ingress, RouteIngress::DirectTcp)
        || !config.route_plan.jump_host_ids.is_empty()
    {
        return Err(ConnectionProfileError::UnsupportedConfiguration);
    }
    let username = required_username(snapshot.host.username.as_deref().or_else(|| {
        snapshot
            .identity
            .as_ref()
            .and_then(|value| value.username.as_deref())
    }))?;
    let credentials = select_stored_credentials(snapshot.credentials)?;
    let algorithm_policy = resolve_algorithm_policy(&config.algorithm_policy)?;
    let credential_token = connection_credentials_token(&credentials);
    let username_token = revision_token_component(&username);
    Ok(ResolvedJumpHost {
        host_id: snapshot.host.host_id.clone(),
        endpoint: SshSessionEndpoint {
            address: snapshot.host.address,
            port: snapshot.host.port,
            username: Some(username.clone()),
        },
        username,
        credentials,
        algorithm_policy,
        revision_token: format!(
            "jump-v2:{}:{}:{}:{}:{}:{}:{}",
            snapshot.host.host_id.as_str(),
            snapshot.host.state_version.get(),
            username_token,
            config.authentication_plan.revision.get(),
            config.algorithm_policy.revision.get(),
            config.route_plan.revision.get(),
            credential_token,
        ),
    })
}

fn resolve_algorithm_policy(
    summary: &norishell_core_api::AlgorithmPolicySummary,
) -> Result<ResolvedAlgorithmPolicy, ConnectionProfileError> {
    let transport =
        crate::algorithm_policy::resolve(&summary.policy_id, &summary.compatibility_exceptions)
            .map_err(|()| ConnectionProfileError::UnsupportedConfiguration)?;
    Ok(ResolvedAlgorithmPolicy {
        transport,
        policy_id: summary.policy_id.clone(),
        policy_revision: Some(summary.revision),
        catalog_version: ALGORITHM_POLICY_CATALOG_VERSION,
    })
}

fn connection_credentials_token(credentials: &[ConnectionCredential]) -> String {
    credentials
        .iter()
        .map(|credential| {
            format!(
                "{}:{}",
                credential.credential_ref_id().as_str(),
                credential_state_version(credential),
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn resolve_route_ingress(
    hosts: &HostService,
    ingress: &RouteIngress,
) -> Result<ResolvedRouteIngress, ConnectionProfileError> {
    match ingress {
        RouteIngress::DirectTcp => Ok(ResolvedRouteIngress::DirectTcp),
        RouteIngress::HttpConnectProxy {
            endpoint,
            proxy_auth_credential_ref_id,
        } => Ok(ResolvedRouteIngress::HttpConnectProxy {
            endpoint: endpoint.clone(),
            authentication: resolve_proxy_authentication(
                hosts,
                proxy_auth_credential_ref_id.as_ref(),
            )?,
        }),
        RouteIngress::Socks5Proxy {
            endpoint,
            dns_mode,
            proxy_auth_credential_ref_id,
        } => Ok(ResolvedRouteIngress::Socks5Proxy {
            endpoint: endpoint.clone(),
            dns_mode: *dns_mode,
            authentication: resolve_proxy_authentication(
                hosts,
                proxy_auth_credential_ref_id.as_ref(),
            )?,
        }),
    }
}

fn resolve_proxy_authentication(
    hosts: &HostService,
    credential_ref_id: Option<&CredentialRefId>,
) -> Result<Option<ResolvedProxyAuthentication>, ConnectionProfileError> {
    let Some(credential_ref_id) = credential_ref_id else {
        return Ok(None);
    };
    let credential = hosts
        .get_ready_credential(credential_ref_id)
        .map_err(map_persistence_error)?;
    if !matches!(credential.details, CredentialRecordDetails::Password { .. }) {
        return Err(ConnectionProfileError::CredentialUnavailable);
    }
    let identity = hosts
        .get_identity_summary(&credential.identity_id)
        .map_err(map_persistence_error)?;
    let username = required_username(identity.username.as_deref())?;
    Ok(Some(ResolvedProxyAuthentication {
        username,
        credential: Box::new(credential),
    }))
}

fn select_host_credentials(
    credentials: Vec<CredentialRecord>,
    transient_credentials: &TransientCredentialService,
    requested_credential_ref_id: Option<&CredentialRefId>,
) -> Result<Vec<ConnectionCredential>, ConnectionProfileError> {
    let Some(requested_credential_ref_id) = requested_credential_ref_id else {
        return select_stored_credentials(credentials);
    };
    if let Some(credential) = credentials
        .into_iter()
        .find(|credential| credential.credential_ref_id == *requested_credential_ref_id)
    {
        return Ok(vec![ConnectionCredential::Stored(Box::new(credential))]);
    }
    if transient_credentials.contains(requested_credential_ref_id) {
        return Ok(vec![ConnectionCredential::Transient(
            requested_credential_ref_id.clone(),
        )]);
    }
    Err(ConnectionProfileError::CredentialUnavailable)
}

fn select_stored_credentials(
    credentials: Vec<CredentialRecord>,
) -> Result<Vec<ConnectionCredential>, ConnectionProfileError> {
    let attempts = credentials
        .into_iter()
        .map(|value| ConnectionCredential::Stored(Box::new(value)))
        .collect::<Vec<_>>();
    (!attempts.is_empty())
        .then_some(attempts)
        .ok_or(ConnectionProfileError::CredentialUnavailable)
}

fn required_username(value: Option<&str>) -> Result<String, ConnectionProfileError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(ConnectionProfileError::InvalidTarget)
}

fn credential_state_version(credential: &ConnectionCredential) -> u64 {
    match credential {
        ConnectionCredential::Stored(credential) => credential.state_version.get(),
        ConnectionCredential::Transient(_) => 0,
    }
}

fn map_persistence_error(error: AppPersistenceError) -> ConnectionProfileError {
    match error {
        AppPersistenceError::NotFound | AppPersistenceError::Conflict => {
            ConnectionProfileError::StaleHost
        }
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::Endpoint(_) => {
            ConnectionProfileError::InvalidTarget
        }
        AppPersistenceError::IdempotencyConflict
        | AppPersistenceError::DatabaseNotFresh
        | AppPersistenceError::InvalidStoredData
        | AppPersistenceError::KnownHostMismatch { .. }
        | AppPersistenceError::KnownHostAlgorithmChanged { .. }
        | AppPersistenceError::RequiresReload
        | AppPersistenceError::RestoreCommitUnknown
        | AppPersistenceError::UnsupportedSchema(_)
        | AppPersistenceError::Database(_)
        | AppPersistenceError::Io(_) => ConnectionProfileError::PersistenceUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use norishell_app_persistence::AppRepository;
    use norishell_core_api::{
        AlgorithmCategory, AlgorithmCompatibilityException, AuthenticationPlanMode, CredentialKind,
        DiskResourceId, HeartbeatPolicy, NetworkResourceId, OperationId, ProxyDnsMode,
        ProxyEndpoint, RequestId, ShellHeartbeatLineEnding, SshSessionTarget,
        TransientCredentialPrepareRequest, WireSequence,
    };
    use norishell_ssh_domain::ssh_sha256_fingerprint;

    use super::*;

    fn ready_vault_credential(
        repository: &mut AppRepository,
        identity_id: &norishell_core_api::IdentityId,
        kind: CredentialKind,
        priority: u32,
        label: &str,
    ) -> CredentialRecord {
        let operation_id = OperationId::new();
        let pending = repository
            .begin_credential_import(
                &operation_id,
                &format!("resolver-{kind:?}-{priority}-{}", label.to_lowercase()),
                identity_id,
                kind,
                priority,
                label,
                false,
            )
            .expect("begin credential import");
        let public_key_metadata =
            (kind == CredentialKind::PrivateKey).then_some(("ssh-ed25519", "SHA256:public"));
        repository
            .mark_credential_import_ready(
                &pending.credential_ref_id,
                &operation_id,
                pending.state_version,
                public_key_metadata.map(|metadata| metadata.0),
                public_key_metadata.map(|metadata| metadata.1),
            )
            .expect("mark credential ready")
    }

    fn ready_password(
        repository: &mut AppRepository,
        identity_id: &norishell_core_api::IdentityId,
        priority: u32,
        label: &str,
    ) -> CredentialRecord {
        ready_vault_credential(
            repository,
            identity_id,
            CredentialKind::Password,
            priority,
            label,
        )
    }

    #[test]
    fn saved_host_defaults_resolve_through_one_revisioned_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        let credential = ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "Example.COM.",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        drop(repository);

        let hosts = HostService::start(directory.path()).expect("host service");
        let profile = resolve_connection_profile(
            &hosts,
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id.clone(),
                expected_host_state_version: host.state_version,
            },
            None,
        )
        .expect("resolve saved host");
        let long_lived =
            resolve_long_lived_connection_profile(&hosts, &host.host_id, host.state_version)
                .expect("resolve long-lived saved host");

        assert_eq!(profile.endpoint.address, "Example.COM.");
        assert_eq!(profile.username, "deploy");
        assert_eq!(
            profile.credentials[0].credential_ref_id(),
            &credential.credential_ref_id
        );
        assert!(profile.revision_token.starts_with("terminal-v1:"));
        assert!(connection_requires_vault(&long_lived.connection));
        assert!(connection_has_vault_credentials(&long_lived.connection));
    }

    #[test]
    fn private_key_requires_vault_and_agent_only_does_not() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let private_key_identity = repository
            .create_identity("Private key", Some("deploy"))
            .expect("private-key identity");
        ready_vault_credential(
            &mut repository,
            &private_key_identity.identity_id,
            CredentialKind::PrivateKey,
            0,
            "Private key",
        );
        let private_key_host = repository
            .create_host(
                "Private key host",
                "private.example",
                22,
                None,
                Some(&private_key_identity.identity_id),
                false,
            )
            .expect("private-key host");

        let agent_identity = repository
            .create_identity("Agent", Some("deploy"))
            .expect("agent identity");
        let agent_public_key = b"resolver-agent-public-key";
        repository
            .create_ssh_agent_credential(
                &OperationId::new(),
                "resolver-agent-only",
                &agent_identity.identity_id,
                0,
                "Agent key",
                agent_public_key,
                "ssh-ed25519",
                &ssh_sha256_fingerprint(agent_public_key),
            )
            .expect("agent credential");
        let agent_host = repository
            .create_host(
                "Agent host",
                "agent.example",
                22,
                None,
                Some(&agent_identity.identity_id),
                false,
            )
            .expect("agent host");
        drop(repository);

        let hosts = HostService::start(directory.path()).expect("host service");
        let private_key_profile = resolve_long_lived_connection_profile(
            &hosts,
            &private_key_host.host_id,
            private_key_host.state_version,
        )
        .expect("resolve private-key profile");
        assert!(connection_requires_vault(&private_key_profile.connection));
        assert!(connection_has_vault_credentials(
            &private_key_profile.connection
        ));

        let agent_profile = resolve_long_lived_connection_profile(
            &hosts,
            &agent_host.host_id,
            agent_host.state_version,
        )
        .expect("resolve agent profile");
        assert!(!connection_requires_vault(&agent_profile.connection));
        assert!(!connection_has_vault_credentials(&agent_profile.connection));
    }

    #[test]
    fn effective_identity_username_changes_connection_revision_token() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "identity.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        drop(repository);

        let hosts = HostService::start(directory.path()).expect("host service");
        let before =
            resolve_long_lived_connection_profile(&hosts, &host.host_id, host.state_version)
                .expect("resolve original profile")
                .connection
                .revision_token;
        drop(hosts);

        let mut repository = AppRepository::open(&database_path).expect("repository");
        repository
            .update_identity(
                &identity.identity_id,
                identity.state_version,
                "Operations",
                Some("release"),
            )
            .expect("update identity username");
        drop(repository);

        let after = resolve_long_lived_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &host.host_id,
            host.state_version,
        )
        .expect("resolve updated profile")
        .connection
        .revision_token;
        assert_ne!(before, after);
    }

    #[test]
    fn explicit_reconnect_refreshes_the_saved_host_version() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        let credential = ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        let updated_host = repository
            .update_host(
                &host.host_id,
                host.state_version,
                "Updated production",
                &host.address,
                host.port,
                host.username.as_deref(),
                host.identity_id.as_ref(),
                host.favorite,
            )
            .expect("update host");
        drop(repository);

        let hosts = HostService::start(directory.path()).expect("host service");
        let stale_target = SshSessionTarget::Host {
            host_id: host.host_id.clone(),
            expected_host_state_version: host.state_version,
        };
        assert!(matches!(
            resolve_connection_profile(
                &hosts,
                &TransientCredentialService::default(),
                &stale_target,
                Some(&credential.credential_ref_id),
            ),
            Err(ConnectionProfileError::StaleHost)
        ));

        let (profile, refreshed_target) = resolve_reconnect_connection_profile(
            &hosts,
            &TransientCredentialService::default(),
            &stale_target,
            Some(&credential.credential_ref_id),
        )
        .expect("resolve reconnect against latest host");

        assert_eq!(profile.endpoint.address, host.address);
        assert_eq!(
            refreshed_target,
            SshSessionTarget::Host {
                host_id: host.host_id,
                expected_host_state_version: updated_host.state_version,
            }
        );
    }

    #[test]
    fn host_override_order_controls_the_default_credential() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        let first = ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let second = ready_password(&mut repository, &identity.identity_id, 1, "Secondary");
        let foreign_identity = repository
            .create_identity("Foreign", Some("other"))
            .expect("foreign identity");
        let foreign = ready_password(&mut repository, &foreign_identity.identity_id, 0, "Foreign");
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        let initial = repository
            .get_authentication_plan(&host.host_id)
            .expect("authentication plan");
        assert!(matches!(
            repository.replace_authentication_plan(
                &host.host_id,
                initial.revision,
                AuthenticationPlanMode::HostOverride,
                &[foreign.credential_ref_id],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        repository
            .replace_authentication_plan(
                &host.host_id,
                initial.revision,
                AuthenticationPlanMode::HostOverride,
                &[
                    second.credential_ref_id.clone(),
                    first.credential_ref_id.clone(),
                ],
            )
            .expect("replace authentication plan");
        drop(repository);

        let profile = resolve_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id,
                expected_host_state_version: host.state_version,
            },
            None,
        )
        .expect("resolve override");
        assert_eq!(
            profile.credentials[0].credential_ref_id(),
            &second.credential_ref_id
        );
        assert_eq!(profile.credentials.len(), 2);
        assert_eq!(
            profile.credentials[1].credential_ref_id(),
            &first.credential_ref_id
        );
    }

    #[test]
    fn saved_host_resolves_socks5_ingress_without_silently_using_direct_tcp() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        let initial = repository
            .get_route_plan(&host.host_id)
            .expect("route plan");
        repository
            .replace_route_plan(
                &host.host_id,
                initial.revision,
                &RouteIngress::Socks5Proxy {
                    endpoint: ProxyEndpoint {
                        address: "proxy.example".to_owned(),
                        normalized_address: "proxy.example".to_owned(),
                        port: 1080,
                    },
                    dns_mode: ProxyDnsMode::Proxy,
                    proxy_auth_credential_ref_id: None,
                },
                &[],
            )
            .expect("replace route");
        drop(repository);

        let profile = resolve_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id,
                expected_host_state_version: host.state_version,
            },
            None,
        )
        .expect("resolve SOCKS5 profile");
        assert!(matches!(
            profile.ingress,
            ResolvedRouteIngress::Socks5Proxy {
                dns_mode: ProxyDnsMode::Proxy,
                authentication: None,
                ..
            }
        ));
    }

    #[test]
    fn proxy_authentication_uses_its_credential_identity_username() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let target_identity = repository
            .create_identity("Target", Some("deploy"))
            .expect("target identity");
        ready_password(
            &mut repository,
            &target_identity.identity_id,
            0,
            "Target password",
        );
        let proxy_identity = repository
            .create_identity("Proxy", Some("proxy-user"))
            .expect("proxy identity");
        let proxy_credential = ready_password(
            &mut repository,
            &proxy_identity.identity_id,
            0,
            "Proxy password",
        );
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&target_identity.identity_id),
                false,
            )
            .expect("host");
        let route = repository
            .get_route_plan(&host.host_id)
            .expect("route plan");
        repository
            .replace_route_plan(
                &host.host_id,
                route.revision,
                &RouteIngress::HttpConnectProxy {
                    endpoint: ProxyEndpoint {
                        address: "proxy.example".to_owned(),
                        normalized_address: "proxy.example".to_owned(),
                        port: 3128,
                    },
                    proxy_auth_credential_ref_id: Some(proxy_credential.credential_ref_id.clone()),
                },
                &[],
            )
            .expect("replace route");
        drop(repository);

        let profile = resolve_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id.clone(),
                expected_host_state_version: host.state_version,
            },
            None,
        )
        .expect("resolve HTTP CONNECT profile");
        let long_lived = resolve_long_lived_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &host.host_id,
            host.state_version,
        )
        .expect("resolve long-lived HTTP CONNECT profile");
        assert!(connection_requires_vault(&long_lived.connection));
        assert!(connection_has_vault_credentials(&long_lived.connection));
        let original_revision_token = long_lived.connection.revision_token.clone();
        let mut repository = AppRepository::open(&database_path).expect("repository");
        repository
            .update_identity(
                &proxy_identity.identity_id,
                proxy_identity.state_version,
                "Proxy",
                Some("proxy-release"),
            )
            .expect("update proxy identity username");
        drop(repository);
        let updated_revision_token = resolve_long_lived_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &host.host_id,
            host.state_version,
        )
        .expect("resolve updated HTTP CONNECT profile")
        .connection
        .revision_token;
        assert_ne!(original_revision_token, updated_revision_token);
        let ResolvedRouteIngress::HttpConnectProxy {
            authentication: Some(authentication),
            ..
        } = profile.ingress
        else {
            panic!("expected HTTP CONNECT authentication");
        };
        assert_eq!(authentication.username, "proxy-user");
        assert_eq!(
            authentication.credential.credential_ref_id,
            proxy_credential.credential_ref_id,
        );
    }

    #[test]
    fn explicit_jump_chain_resolves_each_hop_with_its_own_authentication() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let jump_identity = repository
            .create_identity("Jump", Some("jump-user"))
            .expect("jump identity");
        ready_password(
            &mut repository,
            &jump_identity.identity_id,
            0,
            "Jump password",
        );
        let target_identity = repository
            .create_identity("Target", Some("deploy"))
            .expect("target identity");
        ready_password(
            &mut repository,
            &target_identity.identity_id,
            0,
            "Target password",
        );
        let jump = repository
            .create_host(
                "Jump",
                "jump.example",
                22,
                None,
                Some(&jump_identity.identity_id),
                false,
            )
            .expect("jump host");
        let target = repository
            .create_host(
                "Target",
                "target.example",
                22,
                None,
                Some(&target_identity.identity_id),
                false,
            )
            .expect("target host");
        let route = repository
            .get_route_plan(&target.host_id)
            .expect("route plan");
        repository
            .replace_route_plan(
                &target.host_id,
                route.revision,
                &RouteIngress::DirectTcp,
                std::slice::from_ref(&jump.host_id),
            )
            .expect("replace route");
        repository
            .replace_algorithm_policy(
                &jump.host_id,
                WireSequence::new(1),
                "secure-default",
                &[AlgorithmCompatibilityException {
                    category: AlgorithmCategory::KeyExchange,
                    exception_id: "compat-kex-dh-group14-sha1".to_owned(),
                    reason: None,
                }],
            )
            .expect("jump algorithm policy");
        repository
            .replace_algorithm_policy(
                &target.host_id,
                WireSequence::new(1),
                "secure-default",
                &[AlgorithmCompatibilityException {
                    category: AlgorithmCategory::Mac,
                    exception_id: "compat-mac-hmac-sha1-etm".to_owned(),
                    reason: None,
                }],
            )
            .expect("target algorithm policy");
        let jump_heartbeat = repository
            .get_heartbeat_policy(&jump.host_id)
            .expect("jump heartbeat");
        repository
            .replace_heartbeat_policy(
                &jump.host_id,
                jump_heartbeat.revision,
                &HeartbeatPolicy::ShellHeartbeat {
                    payload_text: "echo jump".to_owned(),
                    line_ending: ShellHeartbeatLineEnding::Cr,
                    interval_seconds: 60,
                    user_idle_seconds: 30,
                },
            )
            .expect("jump heartbeat policy");
        let target_heartbeat = repository
            .get_heartbeat_policy(&target.host_id)
            .expect("target heartbeat");
        let target_heartbeat = repository
            .replace_heartbeat_policy(
                &target.host_id,
                target_heartbeat.revision,
                &HeartbeatPolicy::TransportKeepalive {
                    interval_seconds: 45,
                    reply_timeout_seconds: 10,
                    failure_threshold: 4,
                },
            )
            .expect("target heartbeat policy");
        drop(repository);

        let profile = resolve_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: target.host_id.clone(),
                expected_host_state_version: target.state_version,
            },
            None,
        )
        .expect("resolve jump chain");
        let long_lived = resolve_long_lived_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &target.host_id,
            target.state_version,
        )
        .expect("resolve long-lived jump chain");
        assert!(connection_requires_vault(&long_lived.connection));
        assert!(connection_has_vault_credentials(&long_lived.connection));
        let original_revision_token = long_lived.connection.revision_token.clone();
        let mut repository = AppRepository::open(&database_path).expect("repository");
        repository
            .update_identity(
                &jump_identity.identity_id,
                jump_identity.state_version,
                "Jump",
                Some("jump-release"),
            )
            .expect("update jump identity username");
        drop(repository);
        let updated_revision_token = resolve_long_lived_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &target.host_id,
            target.state_version,
        )
        .expect("resolve updated jump chain")
        .connection
        .revision_token;
        assert_ne!(original_revision_token, updated_revision_token);
        assert_eq!(profile.jump_hosts.len(), 1);
        assert_eq!(profile.jump_hosts[0].host_id, jump.host_id);
        assert_eq!(profile.jump_hosts[0].endpoint.address, "jump.example");
        assert_eq!(profile.jump_hosts[0].username, "jump-user");
        assert_eq!(profile.jump_hosts[0].credentials.len(), 1);
        assert_ne!(
            profile.jump_hosts[0].algorithm_policy.transport, profile.algorithm_policy.transport,
            "each route stage must retain its own Host algorithm policy",
        );
        assert_eq!(
            profile.heartbeat_policy.revision,
            Some(target_heartbeat.revision)
        );
        assert_eq!(
            profile.heartbeat_policy.policy,
            HeartbeatPolicy::TransportKeepalive {
                interval_seconds: 45,
                reply_timeout_seconds: 10,
                failure_threshold: 4,
            },
            "all transports in this resource use the target Host snapshot, not the hop policy",
        );
    }

    #[test]
    fn stale_host_and_transient_quick_connect_keep_distinct_fences() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Stale", Some("tester"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Stale",
                "stale.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        drop(repository);
        let hosts = HostService::start(directory.path()).expect("host service");
        let transient = TransientCredentialService::default();
        let prepared = transient
            .prepare(TransientCredentialPrepareRequest {
                meta: norishell_core_api::RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "resolver-quick-connect".to_owned(),
                kind: CredentialKind::Password,
                secret: "one-time-password".to_owned(),
                passphrase: None,
            })
            .expect("prepare transient credential");
        let quick = resolve_connection_profile(
            &hosts,
            &transient,
            &SshSessionTarget::QuickConnect {
                endpoint: SshSessionEndpoint {
                    address: "LOCALHOST".to_owned(),
                    port: 22,
                    username: Some("tester".to_owned()),
                },
            },
            Some(&prepared.credential_ref_id),
        )
        .expect("resolve quick connect");
        assert!(matches!(
            quick.credentials.as_slice(),
            [ConnectionCredential::Transient(_)]
        ));
        assert!(quick.revision_token.starts_with("quick-v1:localhost:22:"));
        assert_eq!(quick.heartbeat_policy.policy, HeartbeatPolicy::Disabled);
        assert_eq!(quick.heartbeat_policy.revision, None);

        let stale = resolve_connection_profile(
            &hosts,
            &transient,
            &SshSessionTarget::Host {
                host_id: host.host_id,
                expected_host_state_version: WireSequence::new(2),
            },
            None,
        );
        assert!(matches!(stale, Err(ConnectionProfileError::StaleHost)));
    }

    #[test]
    fn connection_test_profile_accepts_only_a_live_transient_credential() {
        let transient = TransientCredentialService::default();
        let endpoint = SshSessionEndpoint {
            address: "LOCALHOST".to_owned(),
            port: 22,
            username: Some("tester".to_owned()),
        };
        let missing = CredentialRefId::new();
        assert!(matches!(
            resolve_connection_test_profile(&transient, &endpoint, &missing),
            Err(ConnectionProfileError::CredentialUnavailable),
        ));

        let prepared = transient
            .prepare(TransientCredentialPrepareRequest {
                meta: norishell_core_api::RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: OperationId::new(),
                idempotency_key: "connection-test-profile".to_owned(),
                kind: CredentialKind::Password,
                secret: "one-time-password".to_owned(),
                passphrase: None,
            })
            .expect("prepare transient credential");
        let profile =
            resolve_connection_test_profile(&transient, &endpoint, &prepared.credential_ref_id)
                .expect("resolve connection test");
        assert!(matches!(
            profile.credentials.as_slice(),
            [ConnectionCredential::Transient(id)] if id == &prepared.credential_ref_id
        ));
        assert!(
            profile
                .revision_token
                .starts_with("connection-test-v1:localhost:22:")
        );
    }

    #[test]
    fn execution_boundary_rejects_oversized_duplicate_and_self_jump_chains() {
        let target = norishell_core_api::HostId::new();
        let unique = (0..6)
            .map(|_| norishell_core_api::HostId::new())
            .collect::<Vec<_>>();
        assert_eq!(
            validate_jump_host_ids(&target, &unique),
            Err(ConnectionProfileError::UnsupportedConfiguration),
        );
        let duplicate = norishell_core_api::HostId::new();
        assert_eq!(
            validate_jump_host_ids(&target, &[duplicate.clone(), duplicate]),
            Err(ConnectionProfileError::UnsupportedConfiguration),
        );
        assert_eq!(
            validate_jump_host_ids(&target, std::slice::from_ref(&target)),
            Err(ConnectionProfileError::UnsupportedConfiguration),
        );
    }

    #[test]
    fn execution_boundary_revalidates_catalog_ids_loaded_from_storage() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Tampered policy",
                "policy.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        repository
            .replace_algorithm_policy(
                &host.host_id,
                WireSequence::new(1),
                "secure-default",
                &[AlgorithmCompatibilityException {
                    category: AlgorithmCategory::Mac,
                    exception_id: "compat-mac-removed-from-current-catalog".to_owned(),
                    reason: None,
                }],
            )
            .expect("simulate older stored catalog selection");
        drop(repository);

        let hosts = HostService::start(directory.path()).expect("host service");
        let resolved = resolve_connection_profile(
            &hosts,
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id,
                expected_host_state_version: host.state_version,
            },
            None,
        );
        assert!(matches!(
            resolved,
            Err(ConnectionProfileError::UnsupportedConfiguration)
        ));
    }

    #[test]
    fn monitoring_revision_does_not_change_the_terminal_profile_token() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(&database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        drop(repository);

        let first = resolve_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id.clone(),
                expected_host_state_version: host.state_version,
            },
            None,
        )
        .expect("first terminal profile");

        let mut repository = AppRepository::open(&database_path).expect("repository");
        let monitoring = repository
            .get_monitoring_policy(&host.host_id)
            .expect("monitoring policy");
        repository
            .replace_monitoring_policy(
                &host.host_id,
                monitoring.revision,
                &MonitoringPolicy {
                    enabled: true,
                    sample_interval_millis: 30000,
                    sample_timeout_millis: 10000,
                    disk_mount_ids: vec![DiskResourceId::Root],
                    network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
                },
            )
            .expect("replace monitoring policy");
        drop(repository);

        let second = resolve_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id,
                expected_host_state_version: host.state_version,
            },
            None,
        )
        .expect("second terminal profile");

        assert_eq!(first.revision_token, second.revision_token);
        assert!(!second.revision_token.contains("monitoring"));
    }

    #[test]
    fn unconfirmed_login_automation_requires_confirmation_before_terminal_open() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        let automation = repository
            .get_login_automation(&host.host_id)
            .expect("login automation");
        repository
            .replace_login_automation(
                &host.host_id,
                automation.revision,
                true,
                &[LoginAutomationStepInput::SendText {
                    text: "echo ready".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                }],
            )
            .expect("replace login automation without confirming it");
        drop(repository);

        let hosts = HostService::start(directory.path()).expect("host service");
        let metrics = resolve_metrics_connection_profile(&hosts, &host.host_id, host.state_version)
            .expect("metrics profile must ignore terminal automation validity");
        assert_eq!(metrics.connection.endpoint.address, "host.example");
        assert!(!metrics.revision_token.contains("login"));

        let terminal = resolve_connection_profile(
            &hosts,
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id,
                expected_host_state_version: host.state_version,
            },
            None,
        );
        assert!(matches!(
            terminal,
            Err(ConnectionProfileError::LoginAutomationConfirmationRequired)
        ));
    }

    #[test]
    fn resource_profile_tokens_include_only_their_owned_policy_revisions() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");

        let heartbeat = repository
            .get_heartbeat_policy(&host.host_id)
            .expect("heartbeat policy");
        repository
            .replace_heartbeat_policy(
                &host.host_id,
                heartbeat.revision,
                &HeartbeatPolicy::TransportKeepalive {
                    interval_seconds: 45,
                    reply_timeout_seconds: 10,
                    failure_threshold: 4,
                },
            )
            .expect("replace heartbeat policy");
        let monitoring = repository
            .get_monitoring_policy(&host.host_id)
            .expect("monitoring policy");
        repository
            .replace_monitoring_policy(
                &host.host_id,
                monitoring.revision,
                &MonitoringPolicy {
                    enabled: true,
                    sample_interval_millis: 30000,
                    sample_timeout_millis: 10000,
                    disk_mount_ids: vec![DiskResourceId::Root],
                    network_interface_ids: vec![NetworkResourceId::AggregateNonLoopback],
                },
            )
            .expect("replace monitoring policy");
        let automation = repository
            .get_login_automation(&host.host_id)
            .expect("login automation");
        let automation = repository
            .replace_login_automation(
                &host.host_id,
                automation.revision,
                true,
                &[LoginAutomationStepInput::SendText {
                    text: "echo ready".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                }],
            )
            .expect("replace login automation");
        repository
            .confirm_login_automation(&host.host_id, automation.revision)
            .expect("confirm login automation");
        drop(repository);

        let hosts = HostService::start(directory.path()).expect("host service");
        let terminal = resolve_terminal_connection_profile(
            &hosts,
            &TransientCredentialService::default(),
            &SshSessionTarget::Host {
                host_id: host.host_id.clone(),
                expected_host_state_version: host.state_version,
            },
            None,
        )
        .expect("terminal profile");
        let metrics = resolve_metrics_connection_profile(&hosts, &host.host_id, host.state_version)
            .expect("metrics profile");

        assert!(terminal.revision_token.contains("heartbeat:2"));
        assert!(terminal.revision_token.contains("login:2"));
        assert!(!terminal.revision_token.contains("monitoring"));
        assert!(metrics.revision_token.contains("monitoring:2"));
        assert!(!metrics.revision_token.contains("heartbeat"));
        assert!(!metrics.revision_token.contains("login"));
        assert_eq!(metrics.monitoring_policy.revision, WireSequence::new(2));
        assert!(metrics.monitoring_policy.policy.enabled);
    }

    #[test]
    fn metrics_profile_excludes_terminal_shell_heartbeat() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Operations", Some("deploy"))
            .expect("identity");
        ready_password(&mut repository, &identity.identity_id, 0, "Primary");
        let host = repository
            .create_host(
                "Production",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        let heartbeat = repository
            .get_heartbeat_policy(&host.host_id)
            .expect("heartbeat policy");
        repository
            .replace_heartbeat_policy(
                &host.host_id,
                heartbeat.revision,
                &HeartbeatPolicy::ShellHeartbeat {
                    payload_text: "echo alive".to_owned(),
                    line_ending: ShellHeartbeatLineEnding::Cr,
                    interval_seconds: 60,
                    user_idle_seconds: 30,
                },
            )
            .expect("replace shell heartbeat policy");
        drop(repository);

        let metrics = resolve_metrics_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &host.host_id,
            host.state_version,
        )
        .expect("metrics profile");

        assert!(!metrics.revision_token.contains("heartbeat"));
    }

    #[test]
    fn metrics_probe_rejects_credentials_that_require_recurring_user_interaction() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("ssh/norishell.sqlite3");
        let mut repository = AppRepository::open(database_path).expect("repository");
        let identity = repository
            .create_identity("Interactive", Some("deploy"))
            .expect("identity");
        repository
            .create_keyboard_interactive_credential(
                &OperationId::new(),
                "metrics-interactive-credential",
                &identity.identity_id,
                0,
                "Interactive",
                3,
            )
            .expect("keyboard-interactive credential");
        let host = repository
            .create_host(
                "Interactive",
                "host.example",
                22,
                None,
                Some(&identity.identity_id),
                false,
            )
            .expect("host");
        drop(repository);

        let result = resolve_metrics_connection_profile(
            &HostService::start(directory.path()).expect("host service"),
            &host.host_id,
            host.state_version,
        );

        assert!(matches!(
            result,
            Err(ConnectionProfileError::UnsupportedConfiguration)
        ));
    }
}
