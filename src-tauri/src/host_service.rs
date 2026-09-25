use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Read as _,
    path::Path,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use norishell_app_persistence::{
    AppPersistenceError, AppRepository, CredentialImportState, CredentialRecord,
    CredentialRecordDetails, HostBatchCreateInput, HostConnectionSnapshot,
    HostCreatePasswordStageState, KnownHostObservation, LoginAutomationSecretStageState,
};
use norishell_core_api::{
    AgentIdentityKind, AlgorithmCompatibilityException, AlgorithmPolicyCatalog,
    AlgorithmPolicyCatalogGetRequest, AlgorithmPolicyReplaceRequest, AlgorithmPolicySummary,
    AuthenticationPlanReplaceRequest, AuthenticationPlanSummary, CoreApiError,
    CredentialImportRequest, CredentialKind, CredentialRefDetails, CredentialRefId,
    CredentialRefListRequest, CredentialRefSummary, ErrorCategory, HeartbeatPolicyReplaceRequest,
    HeartbeatPolicySummary, HostCatalogEntry, HostCatalogListRequest, HostConfiguredCreateRequest,
    HostConfiguredCreateResponse, HostConnectionConfigGetRequest, HostConnectionConfigSummary,
    HostCreatePasswordCancelRequest, HostCreatePasswordCancelResponse,
    HostCreatePasswordStageRequest, HostCreatePasswordStageResponse, HostCreateRequest,
    HostDeleteRequest, HostFavoriteUpdateRequest, HostGroupCreateRequest, HostGroupDeleteRequest,
    HostGroupListRequest, HostGroupSummary, HostGroupUpdateRequest, HostId, HostListRequest,
    HostOrganizationGetRequest, HostOrganizationReplaceRequest, HostOrganizationSummary,
    HostSummary, HostTagCreateRequest, HostTagDeleteRequest, HostTagListRequest, HostTagSummary,
    HostTagUpdateRequest, HostUpdateRequest, IdentityCreateRequest, IdentityDeleteImpact,
    IdentityDeleteImpactRequest, IdentityDeleteRequest, IdentityDeleteResponse, IdentityId,
    IdentityListRequest, IdentitySummary, IdentityUpdateRequest,
    KeyboardInteractiveCredentialCreateRequest, KnownHostDeleteRequest, KnownHostDeleteResponse,
    KnownHostListRequest, KnownHostSummary, LoginAutomationConfirmRequest,
    LoginAutomationReplaceRequest, LoginAutomationSecretCancelRequest,
    LoginAutomationSecretCancelResponse, LoginAutomationSecretCreateRequest,
    LoginAutomationSecretCreateResponse, LoginAutomationSummary, MonitoringPolicyReplaceRequest,
    MonitoringPolicyReplaceResponse, MonitoringPolicySummary, PrivateKeyFileImportRequest,
    RecentConnectionListRequest, RecentConnectionSummary, RequestId, RetryStrategy,
    RoutePlanReplaceRequest, RoutePlanSummary, SshCertificateMetadata,
    TerminalWorkspaceLayoutGetRequest, TerminalWorkspaceLayoutReplaceRequest,
    TerminalWorkspaceLayoutSnapshot, WireSequence,
};
use norishell_secret_vault::SecretKind;
use norishell_ssh_transport::inspect_private_key;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt as _;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    metrics_session_service::MetricsSessionService,
    vault_service::{
        VaultSecretInsert, VaultService, VaultServiceError, map_service_error as map_vault_error,
    },
};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

#[derive(Clone)]
pub struct HostService {
    repository: Arc<Mutex<AppRepository>>,
    login_automation_secret_lifecycle: Arc<Mutex<()>>,
    login_automation_secret_reconciler: Arc<LoginAutomationSecretReconciler>,
}

const LOGIN_AUTOMATION_SECRET_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const LOGIN_AUTOMATION_SECRET_RECONCILE_BATCH: usize = 64;
const MAX_PRIVATE_KEY_FILE_BYTES: u64 = 1024 * 1024;

struct LoginAutomationSecretReconciler {
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl LoginAutomationSecretReconciler {
    fn new() -> Self {
        Self {
            stop: Arc::new((Mutex::new(false), Condvar::new())),
            worker: Mutex::new(None),
        }
    }

    fn shutdown(&self) {
        let (stopped, wake) = &*self.stop;
        *stopped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for LoginAutomationSecretReconciler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Debug, thiserror::Error)]
enum LoginAutomationSecretReconcileError {
    #[error(transparent)]
    Persistence(#[from] AppPersistenceError),
    #[error(transparent)]
    Vault(#[from] VaultServiceError),
}

impl LoginAutomationSecretReconcileError {
    fn diagnostic_category(&self) -> &'static str {
        match self {
            Self::Persistence(_) => "persistence",
            Self::Vault(_) => "vault",
        }
    }
}

#[derive(Default)]
struct LoginAutomationSecretReconcileReporter {
    failure_category: Option<&'static str>,
}

impl LoginAutomationSecretReconcileReporter {
    fn record_failure(&mut self, category: &'static str) -> bool {
        if self.failure_category == Some(category) {
            return false;
        }
        self.failure_category = Some(category);
        true
    }

    fn record_success(&mut self) -> bool {
        self.failure_category.take().is_some()
    }
}

impl HostService {
    pub fn start(app_data_directory: impl AsRef<Path>) -> Result<Self, String> {
        let path = app_data_directory
            .as_ref()
            .join("ssh")
            .join("norishell.sqlite3");
        AppRepository::open(path)
            .map(|repository| Self {
                repository: Arc::new(Mutex::new(repository)),
                login_automation_secret_lifecycle: Arc::new(Mutex::new(())),
                login_automation_secret_reconciler: Arc::new(
                    LoginAutomationSecretReconciler::new(),
                ),
            })
            .map_err(|_| Uuid::new_v4().to_string())
    }

    fn repository(&self) -> std::sync::MutexGuard<'_, AppRepository> {
        self.repository
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn host_ids_referencing_identity(
        &self,
        identity_id: &IdentityId,
    ) -> Result<Vec<HostId>, AppPersistenceError> {
        self.repository()
            .identity_delete_impact(identity_id)
            .map(|impact| {
                impact
                    .referencing_hosts
                    .into_iter()
                    .map(|host| host.host_id)
                    .collect()
            })
    }

    fn login_automation_secret_lifecycle(&self) -> std::sync::MutexGuard<'_, ()> {
        self.login_automation_secret_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn start_login_automation_secret_reconciler(
        &self,
        vault: VaultService,
    ) -> Result<(), String> {
        self.start_login_automation_secret_reconciler_with_interval(
            vault,
            LOGIN_AUTOMATION_SECRET_RECONCILE_INTERVAL,
        )
    }

    fn start_login_automation_secret_reconciler_with_interval(
        &self,
        vault: VaultService,
        interval: Duration,
    ) -> Result<(), String> {
        let mut worker = self
            .login_automation_secret_reconciler
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.is_some() {
            return Ok(());
        }
        let repository = Arc::clone(&self.repository);
        let lifecycle = Arc::clone(&self.login_automation_secret_lifecycle);
        let stop = Arc::clone(&self.login_automation_secret_reconciler.stop);
        *worker = Some(
            thread::Builder::new()
                .name("login-automation-secret-reconciler".to_owned())
                .spawn(move || {
                    let mut reporter = LoginAutomationSecretReconcileReporter::default();
                    loop {
                        let (stopped, wake) = &*stop;
                        if *stopped
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                        {
                            break;
                        }
                        if vault.is_unlocked() {
                            let result = reconcile_login_automation_secrets(
                                &repository,
                                &lifecycle,
                                &vault,
                                LOGIN_AUTOMATION_SECRET_RECONCILE_BATCH,
                            )
                            .and_then(|_| {
                                reconcile_host_create_passwords(
                                    &repository,
                                    &lifecycle,
                                    &vault,
                                    LOGIN_AUTOMATION_SECRET_RECONCILE_BATCH,
                                )
                            });
                            match result {
                                Ok(_) => {
                                    if reporter.record_success() {
                                        eprintln!(
                                            "login automation secret reconciliation recovered"
                                        );
                                    }
                                }
                                Err(error) => {
                                    let category = error.diagnostic_category();
                                    if reporter.record_failure(category) {
                                        eprintln!(
                                            "login automation secret reconciliation failed ({category}); retry scheduled"
                                        );
                                    }
                                }
                            }
                        }
                        let guard = stopped
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let (guard, _) = wake
                            .wait_timeout_while(guard, interval, |stopped| !*stopped)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if *guard {
                            break;
                        }
                    }
                })
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    }

    pub(crate) fn shutdown_login_automation_secret_reconciler(&self) {
        self.login_automation_secret_reconciler.shutdown();
    }

    /// Plugin metadata shares the application's one SQLite repository and
    /// lock. The closure keeps the database handle inside this service so the
    /// Plugin Manager cannot open a competing metadata connection or bypass
    /// repository validation.
    pub(crate) fn with_plugin_repository<T>(
        &self,
        operation: impl FnOnce(&mut AppRepository) -> Result<T, AppPersistenceError>,
    ) -> Result<T, AppPersistenceError> {
        operation(&mut self.repository())
    }

    /// Saved forwarding rules share the application's one SQLite repository.
    /// Runtime sessions only receive validated immutable snapshots from here.
    pub(crate) fn with_forward_repository<T>(
        &self,
        operation: impl FnOnce(&mut AppRepository) -> Result<T, AppPersistenceError>,
    ) -> Result<T, AppPersistenceError> {
        operation(&mut self.repository())
    }

    pub(crate) fn with_desktop_repository<T>(
        &self,
        operation: impl FnOnce(&mut AppRepository) -> Result<T, AppPersistenceError>,
    ) -> Result<T, AppPersistenceError> {
        operation(&mut self.repository())
    }

    /// Password stages share the Host/Vault lifecycle lock with stage, cancel and reconcile.
    /// Holding it across the single repository transaction prevents a cancel from deleting the
    /// Vault secret after this desktop save has consumed its stage.
    pub(crate) fn with_desktop_password_stage_repository<T>(
        &self,
        operation: impl FnOnce(&mut AppRepository) -> Result<T, AppPersistenceError>,
    ) -> Result<T, AppPersistenceError> {
        let _lifecycle = self.login_automation_secret_lifecycle();
        operation(&mut self.repository())
    }

    pub(crate) fn get_ready_credential(
        &self,
        credential_ref_id: &CredentialRefId,
    ) -> Result<CredentialRecord, AppPersistenceError> {
        self.repository()
            .get_ready_credential_record(credential_ref_id)
    }

    pub(crate) fn get_identity_summary(
        &self,
        identity_id: &norishell_core_api::IdentityId,
    ) -> Result<IdentitySummary, AppPersistenceError> {
        self.repository().get_identity(identity_id)
    }

    pub(crate) fn get_connection_snapshot(
        &self,
        host_id: &HostId,
    ) -> Result<HostConnectionSnapshot, AppPersistenceError> {
        self.repository().get_host_connection_snapshot(host_id)
    }

    pub(crate) fn list_ssh_sync_snapshots(
        &self,
    ) -> Result<Vec<HostConnectionSnapshot>, AppPersistenceError> {
        let repository = self.repository();
        repository
            .list_hosts()?
            .into_iter()
            .map(|host| repository.get_host_connection_snapshot(&host.host_id))
            .collect()
    }

    pub(crate) fn with_ssh_sync_repository<T>(
        &self,
        action: impl FnOnce(&mut AppRepository) -> Result<T, AppPersistenceError>,
    ) -> Result<T, AppPersistenceError> {
        action(&mut self.repository())
    }

    pub(crate) fn record_successful_connection(
        &self,
        host_id: &HostId,
    ) -> Result<RecentConnectionSummary, AppPersistenceError> {
        self.repository().record_successful_connection(host_id)
    }

    pub(crate) fn list_overview_sources(
        &self,
        request_id: RequestId,
    ) -> CoreResult<Vec<(HostCatalogEntry, MonitoringPolicySummary)>> {
        let repository = self.repository();
        let entries = repository
            .list_host_catalog(norishell_core_api::HostCatalogSort::FavoriteThenLabel)
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        entries
            .into_iter()
            .map(|mut entry| {
                entry.host =
                    Self::with_ready_credential_fact_from_repository(&repository, entry.host)
                        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
                let policy = repository
                    .get_monitoring_policy(&entry.host.host_id)
                    .map_err(|error| map_persistence_error(request_id.clone(), error))?;
                Ok((entry, policy))
            })
            .collect()
    }

    fn replace_algorithm_policy(
        &self,
        host_id: &HostId,
        expected_revision: WireSequence,
        policy_id: &str,
        compatibility_exceptions: &[AlgorithmCompatibilityException],
    ) -> Result<AlgorithmPolicySummary, AppPersistenceError> {
        crate::algorithm_policy::resolve(policy_id, compatibility_exceptions).map_err(|()| {
            AppPersistenceError::InvalidInput(
                "algorithm policy selection is not in the Core catalog",
            )
        })?;
        self.repository().replace_algorithm_policy(
            host_id,
            expected_revision,
            policy_id,
            compatibility_exceptions,
        )
    }

    pub(crate) fn create_hosts_atomically(
        &self,
        inputs: &[HostBatchCreateInput],
        request_id: RequestId,
    ) -> CoreResult<Vec<HostSummary>> {
        self.repository()
            .create_hosts_atomically(inputs)
            .map_err(|error| map_persistence_error(request_id, error))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_agent_identity_credential(
        &self,
        operation_id: &norishell_core_api::OperationId,
        idempotency_key: &str,
        identity_id: &norishell_core_api::IdentityId,
        priority: u32,
        label: &str,
        identity_kind: AgentIdentityKind,
        public_key_blob: &[u8],
        public_key_algorithm: &str,
        public_key_fingerprint: &str,
        hardware_application: Option<&str>,
        certificate: Option<&SshCertificateMetadata>,
        request_id: RequestId,
    ) -> CoreResult<CredentialRefSummary> {
        self.repository()
            .create_agent_identity_credential(
                operation_id,
                idempotency_key,
                identity_id,
                priority,
                label,
                identity_kind,
                public_key_blob,
                public_key_algorithm,
                public_key_fingerprint,
                hardware_application,
                certificate,
            )
            .map(|record| credential_summary(&record))
            .map_err(|error| map_persistence_error(request_id, error))
    }

    pub(crate) fn create_keyboard_interactive_credential(
        &self,
        request: &KeyboardInteractiveCredentialCreateRequest,
    ) -> CoreResult<CredentialRefSummary> {
        self.repository()
            .create_keyboard_interactive_credential(
                &request.operation_id,
                &request.idempotency_key,
                &request.identity_id,
                request.priority,
                &request.label,
                request.max_rounds,
            )
            .map(|record| credential_summary(&record))
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))
    }

    fn with_ready_credential_fact(
        &self,
        host: HostSummary,
    ) -> Result<HostSummary, AppPersistenceError> {
        Self::with_ready_credential_fact_from_repository(&self.repository(), host)
    }

    fn with_ready_credential_fact_from_repository(
        repository: &AppRepository,
        mut host: HostSummary,
    ) -> Result<HostSummary, AppPersistenceError> {
        host.has_ready_credential = !repository
            .list_ready_credential_records_for_host(&host.host_id)?
            .is_empty();
        Ok(host)
    }

    pub(crate) fn observe_known_host(
        &self,
        address: &str,
        port: u16,
        algorithm: &str,
        public_key_blob: &[u8],
    ) -> Result<KnownHostObservation, AppPersistenceError> {
        self.repository()
            .observe_known_host(address, port, algorithm, public_key_blob)
    }

    pub(crate) fn trust_known_host(
        &self,
        address: &str,
        port: u16,
        algorithm: &str,
        public_key_blob: &[u8],
    ) -> Result<KnownHostSummary, AppPersistenceError> {
        self.repository()
            .trust_known_host(address, port, algorithm, public_key_blob)
    }

    pub(crate) fn record_known_host_verified(
        &self,
        address: &str,
        port: u16,
        algorithm: &str,
        public_key_blob: &[u8],
    ) -> Result<KnownHostSummary, AppPersistenceError> {
        self.repository()
            .record_known_host_verified(address, port, algorithm, public_key_blob)
    }
}

fn reconcile_login_automation_secrets(
    repository: &Arc<Mutex<AppRepository>>,
    lifecycle: &Arc<Mutex<()>>,
    vault: &VaultService,
    maximum: usize,
) -> Result<usize, LoginAutomationSecretReconcileError> {
    let _lifecycle = lifecycle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !vault.is_unlocked() {
        return Ok(0);
    }
    let candidates = repository
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .list_login_automation_secret_cleanup_candidates(maximum)?;
    let mut reconciled = 0;
    for candidate in candidates {
        let staged = repository
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin_login_automation_secret_cleanup(
                &candidate.operation_id,
                &candidate.idempotency_key,
            )?
            .ok_or(AppPersistenceError::NotFound)?;
        if staged.state == LoginAutomationSecretStageState::Cancelled {
            continue;
        }
        vault.delete_secrets(std::slice::from_ref(&staged.secret_ref_id))?;
        repository
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finalize_login_automation_secret_cleanup(&staged.staged_secret_id)?;
        reconciled += 1;
    }
    Ok(reconciled)
}

fn reconcile_host_create_passwords(
    repository: &Arc<Mutex<AppRepository>>,
    lifecycle: &Arc<Mutex<()>>,
    vault: &VaultService,
    maximum: usize,
) -> Result<usize, LoginAutomationSecretReconcileError> {
    let _lifecycle = lifecycle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !vault.is_unlocked() {
        return Ok(0);
    }
    let candidates = repository
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .list_host_create_password_cleanup_candidates(maximum)?;
    let mut reconciled = 0;
    for candidate in candidates {
        let staged = repository
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin_host_create_password_cleanup(
                &candidate.operation_id,
                &candidate.idempotency_key,
            )?
            .ok_or(AppPersistenceError::NotFound)?;
        if staged.state == HostCreatePasswordStageState::Cancelled {
            continue;
        }
        vault.delete_secrets(std::slice::from_ref(&staged.secret_ref_id))?;
        repository
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finalize_host_create_password_cleanup(&staged.staged_password_id)?;
        reconciled += 1;
    }
    Ok(reconciled)
}

#[tauri::command]
pub fn host_list(
    request: HostListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<HostSummary>> {
    let request_id = request.meta.request_id;
    let hosts = service
        .repository()
        .list_hosts()
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    hosts
        .into_iter()
        .map(|host| service.with_ready_credential_fact(host))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn terminal_workspace_layout_get(
    request: TerminalWorkspaceLayoutGetRequest,
    service: State<'_, HostService>,
) -> CoreResult<TerminalWorkspaceLayoutSnapshot> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .get_terminal_workspace_layout()
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn terminal_workspace_layout_replace(
    request: TerminalWorkspaceLayoutReplaceRequest,
    service: State<'_, HostService>,
) -> CoreResult<TerminalWorkspaceLayoutSnapshot> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .replace_terminal_workspace_layout(request.expected_revision, &request.layout)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_catalog_list(
    request: HostCatalogListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<HostCatalogEntry>> {
    let request_id = request.meta.request_id;
    let entries = service
        .repository()
        .list_host_catalog(request.sort)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    entries
        .into_iter()
        .map(|mut entry| {
            entry.host = service.with_ready_credential_fact(entry.host)?;
            Ok(entry)
        })
        .collect::<Result<Vec<_>, AppPersistenceError>>()
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_create(
    request: HostCreateRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostSummary> {
    let request_id = request.meta.request_id.clone();
    let host = service
        .repository()
        .create_host(
            &request.label,
            &request.address,
            request.port,
            request.username.as_deref(),
            request.identity_id.as_ref(),
            request.favorite,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    service
        .with_ready_credential_fact(host)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub async fn host_configured_create(
    request: HostConfiguredCreateRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<HostConfiguredCreateResponse> {
    let request_id = request.meta.request_id.clone();
    crate::algorithm_policy::resolve(
        &request.algorithm_policy_id,
        &request.compatibility_exceptions,
    )
    .map_err(|()| {
        map_persistence_error(
            request_id.clone(),
            AppPersistenceError::InvalidInput(
                "algorithm policy selection is not in the Core catalog",
            ),
        )
    })?;
    let mut response = {
        let _lifecycle = service.login_automation_secret_lifecycle();
        service
            .repository()
            .create_host_configured(&request)
            .map_err(|error| map_persistence_error(request_id.clone(), error))?
    };
    response.host = service
        .with_ready_credential_fact(response.host)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    if response.connection_config.monitoring_policy.policy.enabled {
        let _ = metrics.reconcile_hosts(request_id).await;
    }
    Ok(response)
}

#[tauri::command]
pub fn host_create_password_stage(
    request: HostCreatePasswordStageRequest,
    service: State<'_, HostService>,
    vault: State<'_, VaultService>,
) -> CoreResult<HostCreatePasswordStageResponse> {
    stage_host_create_password(request, &service, &vault)
}

fn stage_host_create_password(
    mut request: HostCreatePasswordStageRequest,
    service: &HostService,
    vault: &VaultService,
) -> CoreResult<HostCreatePasswordStageResponse> {
    let request_id = request.meta.request_id.clone();
    let password = Zeroizing::new(std::mem::take(&mut request.password).into_bytes());
    if password.is_empty() || password.len() > 4_096 || password.contains(&0) {
        return Err(Box::new(CoreApiError {
            code: "host.invalid_staged_password".to_owned(),
            category: ErrorCategory::Validation,
            retry_strategy: RetryStrategy::Never,
            message_key: "errors.host.invalidStagedPassword".to_owned(),
            params: BTreeMap::new(),
            request_id: Some(request_id),
            diagnostic_id: None,
            conflict: None,
        }));
    }
    let _lifecycle = service.login_automation_secret_lifecycle();
    let begin = service
        .repository()
        .begin_host_create_password_stage_with_outcome(
            &request.operation_id,
            &request.idempotency_key,
            &request.identity_label,
            &request.credential_label,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    if !begin.created && begin.record.state == HostCreatePasswordStageState::PendingVault {
        return Err(map_persistence_error(
            request_id,
            AppPersistenceError::RequiresReload,
        ));
    }
    let staged = begin.record;
    if !begin.created {
        let existing = vault
            .read_secret(&staged.secret_ref_id, SecretKind::Password)
            .map_err(|error| map_vault_error(request_id.clone(), error))?;
        if !constant_time_secret_eq(existing.expose(), password.as_slice()) {
            return Err(map_persistence_error(
                request_id,
                AppPersistenceError::IdempotencyConflict,
            ));
        }
    }
    if begin.created
        && let Err(error) = vault.insert_secrets(&[VaultSecretInsert {
            secret_ref_id: staged.secret_ref_id.clone(),
            kind: SecretKind::Password,
            value: password,
        }])
    {
        if error.secret_insert_definitely_absent() {
            service
                .repository()
                .abort_pending_host_create_password_stage(&staged.staged_password_id)
                .map_err(|abort_error| map_persistence_error(request_id.clone(), abort_error))?;
        }
        return Err(map_vault_error(request_id, error));
    }
    let staged = service
        .repository()
        .mark_host_create_password_staged(&staged.staged_password_id)
        .map_err(|error| map_persistence_error(request_id, error))?;
    Ok(HostCreatePasswordStageResponse {
        staged_password_id: staged.staged_password_id,
        identity_label: staged.identity_label,
        credential_label: staged.credential_label,
        expires_at_unix_ms: staged.expires_at_unix_ms,
    })
}

fn constant_time_secret_eq(left: &[u8], right: &[u8]) -> bool {
    let maximum = left.len().max(right.len());
    let mut difference = left.len() ^ right.len();
    for index in 0..maximum {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

#[tauri::command]
pub fn host_create_password_cancel(
    request: HostCreatePasswordCancelRequest,
    service: State<'_, HostService>,
    vault: State<'_, VaultService>,
) -> CoreResult<HostCreatePasswordCancelResponse> {
    cancel_host_create_password(request, &service, &vault)
}

fn cancel_host_create_password(
    request: HostCreatePasswordCancelRequest,
    service: &HostService,
    vault: &VaultService,
) -> CoreResult<HostCreatePasswordCancelResponse> {
    let request_id = request.meta.request_id.clone();
    let _lifecycle = service.login_automation_secret_lifecycle();
    let Some(staged) = service
        .repository()
        .begin_host_create_password_cleanup(&request.operation_id, &request.idempotency_key)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?
    else {
        return Ok(HostCreatePasswordCancelResponse { cancelled: false });
    };
    if matches!(
        staged.state,
        HostCreatePasswordStageState::Cancelled | HostCreatePasswordStageState::Consumed
    ) {
        return Ok(HostCreatePasswordCancelResponse { cancelled: false });
    }
    vault
        .delete_secrets(std::slice::from_ref(&staged.secret_ref_id))
        .map_err(|error| map_vault_error(request_id.clone(), error))?;
    service
        .repository()
        .finalize_host_create_password_cleanup(&staged.staged_password_id)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub async fn host_update(
    request: HostUpdateRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<HostSummary> {
    let request_id = request.meta.request_id.clone();
    let host = service
        .repository()
        .update_host(
            &request.host_id,
            request.expected_state_version,
            &request.label,
            &request.address,
            request.port,
            request.username.as_deref(),
            request.identity_id.as_ref(),
            request.favorite,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let host = service
        .with_ready_credential_fact(host)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let _ = metrics.reconcile_hosts(request_id).await;
    Ok(host)
}

#[tauri::command]
pub async fn host_delete(
    request: HostDeleteRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<()> {
    let request_id = request.meta.request_id.clone();
    service
        .repository()
        .delete_host(&request.host_id, request.expected_state_version)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let _ = metrics.reconcile_hosts(request_id).await;
    Ok(())
}

#[tauri::command]
pub fn host_group_list(
    request: HostGroupListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<HostGroupSummary>> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .list_host_groups()
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_group_create(
    request: HostGroupCreateRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostGroupSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .create_host_group(&request.label)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_group_update(
    request: HostGroupUpdateRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostGroupSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .update_host_group(
            &request.group_id,
            request.expected_state_version,
            &request.label,
        )
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_group_delete(
    request: HostGroupDeleteRequest,
    service: State<'_, HostService>,
) -> CoreResult<()> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .delete_host_group(&request.group_id, request.expected_state_version)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_tag_list(
    request: HostTagListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<HostTagSummary>> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .list_host_tags()
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_tag_create(
    request: HostTagCreateRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostTagSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .create_host_tag(&request.label)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_tag_update(
    request: HostTagUpdateRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostTagSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .update_host_tag(
            &request.tag_id,
            request.expected_state_version,
            &request.label,
        )
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_tag_delete(
    request: HostTagDeleteRequest,
    service: State<'_, HostService>,
) -> CoreResult<()> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .delete_host_tag(&request.tag_id, request.expected_state_version)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_organization_get(
    request: HostOrganizationGetRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostOrganizationSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .get_host_organization(&request.host_id)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_organization_replace(
    request: HostOrganizationReplaceRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostOrganizationSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .replace_host_organization(
            &request.host_id,
            request.expected_host_state_version,
            request.group_id.as_ref(),
            &request.tag_ids,
        )
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_favorite_update(
    request: HostFavoriteUpdateRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostSummary> {
    let request_id = request.meta.request_id.clone();
    let host = service
        .repository()
        .update_host_favorite(
            &request.host_id,
            request.expected_state_version,
            request.favorite,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    service
        .with_ready_credential_fact(host)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn recent_connection_list(
    request: RecentConnectionListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<RecentConnectionSummary>> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .list_recent_connections(request.limit)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn host_connection_config_get(
    request: HostConnectionConfigGetRequest,
    service: State<'_, HostService>,
) -> CoreResult<HostConnectionConfigSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .get_host_connection_config(&request.host_id)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub async fn route_plan_replace(
    request: RoutePlanReplaceRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<RoutePlanSummary> {
    let request_id = request.meta.request_id.clone();
    let summary = service
        .repository()
        .replace_route_plan(
            &request.host_id,
            request.expected_revision,
            &request.ingress,
            &request.jump_host_ids,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let _ = metrics.reconcile_hosts(request_id).await;
    Ok(summary)
}

#[tauri::command]
pub async fn authentication_plan_replace(
    request: AuthenticationPlanReplaceRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<AuthenticationPlanSummary> {
    let request_id = request.meta.request_id.clone();
    let summary = service
        .repository()
        .replace_authentication_plan(
            &request.host_id,
            request.expected_revision,
            request.mode,
            &request.credential_ref_ids,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let _ = metrics.reconcile_hosts(request_id).await;
    Ok(summary)
}

#[tauri::command]
pub async fn algorithm_policy_replace(
    request: AlgorithmPolicyReplaceRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<AlgorithmPolicySummary> {
    let request_id = request.meta.request_id.clone();
    let summary = service
        .replace_algorithm_policy(
            &request.host_id,
            request.expected_revision,
            &request.policy_id,
            &request.compatibility_exceptions,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let _ = metrics.reconcile_hosts(request_id).await;
    Ok(summary)
}

#[tauri::command]
pub fn algorithm_policy_catalog_get(
    _request: AlgorithmPolicyCatalogGetRequest,
) -> CoreResult<AlgorithmPolicyCatalog> {
    Ok(crate::algorithm_policy::catalog())
}

#[tauri::command]
pub fn heartbeat_policy_replace(
    request: HeartbeatPolicyReplaceRequest,
    service: State<'_, HostService>,
) -> CoreResult<HeartbeatPolicySummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .replace_heartbeat_policy(&request.host_id, request.expected_revision, &request.policy)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub async fn monitoring_policy_replace(
    request: MonitoringPolicyReplaceRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<MonitoringPolicyReplaceResponse> {
    let request_id = request.meta.request_id.clone();
    let summary = service
        .repository()
        .replace_monitoring_policy(&request.host_id, request.expected_revision, &request.policy)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let runtime_reconciled = if summary.policy.enabled {
        metrics
            .reconcile_hosts(request_id.clone())
            .await
            .is_ok_and(|sessions| {
                sessions
                    .iter()
                    .any(|session| session.host_id == summary.host_id)
            })
    } else {
        metrics
            .disable_host(request_id.clone(), summary.host_id.clone())
            .await
            .is_ok()
    };
    if !summary.policy.enabled && runtime_reconciled {
        // Current-host shutdown is authoritative and independent of catalog
        // health. Reconcile the rest without weakening that result.
        let _ = metrics.reconcile_hosts(request_id).await;
    }
    Ok(MonitoringPolicyReplaceResponse {
        policy: summary,
        runtime_reconciled,
    })
}

#[tauri::command]
pub fn login_automation_replace(
    request: LoginAutomationReplaceRequest,
    service: State<'_, HostService>,
) -> CoreResult<LoginAutomationSummary> {
    let request_id = request.meta.request_id;
    let _lifecycle = service.login_automation_secret_lifecycle();
    service
        .repository()
        .replace_login_automation(
            &request.host_id,
            request.expected_revision,
            request.enabled,
            &request.steps,
        )
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn login_automation_confirm(
    request: LoginAutomationConfirmRequest,
    service: State<'_, HostService>,
) -> CoreResult<LoginAutomationSummary> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .confirm_login_automation(&request.host_id, request.expected_revision)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn login_automation_secret_create(
    request: LoginAutomationSecretCreateRequest,
    service: State<'_, HostService>,
    vault: State<'_, VaultService>,
) -> CoreResult<LoginAutomationSecretCreateResponse> {
    create_login_automation_secret(request, &service, &vault)
}

fn create_login_automation_secret(
    mut request: LoginAutomationSecretCreateRequest,
    service: &HostService,
    vault: &VaultService,
) -> CoreResult<LoginAutomationSecretCreateResponse> {
    let request_id = request.meta.request_id.clone();
    let label = request.label.trim().to_owned();
    let value = Zeroizing::new(std::mem::take(&mut request.value).into_bytes());
    if label.is_empty()
        || label.chars().count() > 120
        || value.is_empty()
        || value.len() > 4_096
        || value.contains(&0)
    {
        return Err(Box::new(CoreApiError {
            code: "host.invalid_login_automation_secret".to_owned(),
            category: ErrorCategory::Validation,
            retry_strategy: RetryStrategy::Never,
            message_key: "errors.host.invalidLoginAutomationSecret".to_owned(),
            params: BTreeMap::new(),
            request_id: Some(request_id),
            diagnostic_id: None,
            conflict: None,
        }));
    }
    let _lifecycle = service.login_automation_secret_lifecycle();
    let begin = service
        .repository()
        .begin_login_automation_secret_stage_with_outcome(
            &request.host_id,
            request.expected_automation_revision,
            &request.operation_id,
            &request.idempotency_key,
            &label,
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    if !begin.created && begin.record.state == LoginAutomationSecretStageState::PendingVault {
        return Err(map_persistence_error(
            request_id,
            AppPersistenceError::RequiresReload,
        ));
    }
    let staged = begin.record;
    if let Err(error) = vault.insert_secrets(&[VaultSecretInsert {
        secret_ref_id: staged.secret_ref_id.clone(),
        kind: SecretKind::LoginAutomation,
        value,
    }]) {
        if error.secret_insert_definitely_absent() {
            service
                .repository()
                .abort_pending_login_automation_secret_stage(&staged.staged_secret_id)
                .map_err(|abort_error| map_persistence_error(request_id.clone(), abort_error))?;
        }
        return Err(map_vault_error(request_id, error));
    }
    let staged = service
        .repository()
        .mark_login_automation_secret_staged(&staged.staged_secret_id)
        .map_err(|error| map_persistence_error(request_id, error))?;
    Ok(LoginAutomationSecretCreateResponse {
        staged_secret_id: staged.staged_secret_id,
        label: staged.label,
        expires_at_unix_ms: staged.expires_at_unix_ms,
    })
}

#[tauri::command]
pub fn login_automation_secret_cancel(
    request: LoginAutomationSecretCancelRequest,
    service: State<'_, HostService>,
    vault: State<'_, VaultService>,
) -> CoreResult<LoginAutomationSecretCancelResponse> {
    cancel_login_automation_secret(request, &service, &vault)
}

fn cancel_login_automation_secret(
    request: LoginAutomationSecretCancelRequest,
    service: &HostService,
    vault: &VaultService,
) -> CoreResult<LoginAutomationSecretCancelResponse> {
    let request_id = request.meta.request_id.clone();
    let _lifecycle = service.login_automation_secret_lifecycle();
    let Some(staged) = service
        .repository()
        .begin_login_automation_secret_cleanup(&request.operation_id, &request.idempotency_key)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?
    else {
        return Ok(LoginAutomationSecretCancelResponse { cancelled: false });
    };
    if matches!(
        staged.state,
        LoginAutomationSecretStageState::Cancelled | LoginAutomationSecretStageState::Consumed
    ) {
        return Ok(LoginAutomationSecretCancelResponse { cancelled: false });
    }
    vault
        .delete_secrets(std::slice::from_ref(&staged.secret_ref_id))
        .map_err(|error| map_vault_error(request_id.clone(), error))?;
    service
        .repository()
        .finalize_login_automation_secret_cleanup(&staged.staged_secret_id)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn identity_list(
    request: IdentityListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<IdentitySummary>> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .list_identities()
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn identity_create(
    request: IdentityCreateRequest,
    service: State<'_, HostService>,
) -> CoreResult<IdentitySummary> {
    let request_id = request.meta.request_id.clone();
    service
        .repository()
        .create_identity(&request.label, request.username.as_deref())
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub async fn identity_update(
    request: IdentityUpdateRequest,
    service: State<'_, HostService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<IdentitySummary> {
    let request_id = request.meta.request_id.clone();
    let identity = service
        .repository()
        .update_identity(
            &request.identity_id,
            request.expected_state_version,
            &request.label,
            request.username.as_deref(),
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let _ = metrics
        .recheck_hosts_referencing_identity(request_id, identity.identity_id.clone())
        .await;
    Ok(identity)
}

#[tauri::command]
pub fn identity_delete_impact(
    request: IdentityDeleteImpactRequest,
    service: State<'_, HostService>,
) -> CoreResult<IdentityDeleteImpact> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .identity_delete_impact(&request.identity_id)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn identity_delete(
    request: IdentityDeleteRequest,
    service: State<'_, HostService>,
) -> CoreResult<IdentityDeleteResponse> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .delete_identity(&request.identity_id, request.expected_state_version)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn credential_ref_list(
    request: CredentialRefListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<CredentialRefSummary>> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .list_credential_refs(&request.identity_id)
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn keyboard_interactive_credential_create(
    request: KeyboardInteractiveCredentialCreateRequest,
    service: State<'_, HostService>,
) -> CoreResult<CredentialRefSummary> {
    service.create_keyboard_interactive_credential(&request)
}

#[tauri::command]
pub async fn credential_import(
    mut request: CredentialImportRequest,
    service: State<'_, HostService>,
    vault: State<'_, VaultService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<CredentialRefSummary> {
    let request_id = request.meta.request_id.clone();
    let secret = Zeroizing::new(std::mem::take(&mut request.secret).into_bytes());
    let passphrase = request
        .passphrase
        .take()
        .map(|value| Zeroizing::new(value.into_bytes()));
    let credential = import_credential_material(request, secret, passphrase, &service, &vault)?;
    let _ = metrics
        .recheck_hosts_referencing_identity(request_id, credential.identity_id.clone())
        .await;
    Ok(credential)
}

/// Opens a native file chooser and imports only the selected private-key bytes
/// into the Vault. Neither the selected path nor the key material is returned
/// to the WebView or persisted outside the Vault.
#[tauri::command]
pub async fn private_key_file_import(
    request: PrivateKeyFileImportRequest,
    app: AppHandle,
    service: State<'_, HostService>,
    vault: State<'_, VaultService>,
    metrics: State<'_, MetricsSessionService>,
) -> CoreResult<Option<CredentialRefSummary>> {
    let request_id = request.meta.request_id.clone();
    let selected = app.dialog().file().blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| invalid_credential_error(request_id.clone()))?;
    let secret = read_selected_private_key_file(&path)
        .map_err(|_| invalid_credential_error(request_id.clone()))?;
    let mut import_request = CredentialImportRequest {
        meta: request.meta,
        operation_id: request.operation_id,
        idempotency_key: request.idempotency_key,
        identity_id: request.identity_id,
        kind: CredentialKind::PrivateKey,
        secret: String::new(),
        passphrase: request.passphrase,
        priority: request.priority,
        label: request.label,
    };
    let passphrase = import_request
        .passphrase
        .take()
        .map(|value| Zeroizing::new(value.into_bytes()));
    let credential =
        import_credential_material(import_request, secret, passphrase, &service, &vault)?;
    let _ = metrics
        .recheck_hosts_referencing_identity(request_id, credential.identity_id.clone())
        .await;
    Ok(Some(credential))
}

fn import_credential_material(
    request: CredentialImportRequest,
    secret: Zeroizing<Vec<u8>>,
    passphrase: Option<Zeroizing<Vec<u8>>>,
    service: &HostService,
    vault: &VaultService,
) -> CoreResult<CredentialRefSummary> {
    let request_id = request.meta.request_id.clone();
    let metadata = match request.kind {
        CredentialKind::Password => {
            if passphrase.is_some() {
                return Err(invalid_credential_error(request_id));
            }
            None
        }
        CredentialKind::PrivateKey => Some(
            inspect_private_key(
                secret.as_slice(),
                passphrase.as_ref().map(|value| value.as_slice()),
            )
            .map_err(|_| invalid_credential_error(request_id.clone()))?,
        ),
    };
    let record = service
        .repository()
        .begin_credential_import(
            &request.operation_id,
            &request.idempotency_key,
            &request.identity_id,
            request.kind,
            request.priority,
            &request.label,
            passphrase.is_some(),
        )
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    let (secret_ref_id, passphrase_secret_ref_id) = match &record.details {
        CredentialRecordDetails::Password { secret_ref_id }
        | CredentialRecordDetails::PrivateKey { secret_ref_id, .. } => (
            secret_ref_id.clone(),
            match &record.details {
                CredentialRecordDetails::PrivateKey {
                    passphrase_secret_ref_id,
                    ..
                } => passphrase_secret_ref_id.clone(),
                _ => None,
            },
        ),
        CredentialRecordDetails::KeyboardInteractive { .. }
        | CredentialRecordDetails::SshAgent { .. }
        | CredentialRecordDetails::Certificate { .. }
        | CredentialRecordDetails::HardwareKey { .. } => {
            return Err(invalid_credential_error(request_id));
        }
    };
    let mut inserts = vec![VaultSecretInsert {
        secret_ref_id,
        kind: match request.kind {
            CredentialKind::Password => SecretKind::Password,
            CredentialKind::PrivateKey => SecretKind::PrivateKey,
        },
        value: secret,
    }];
    match (passphrase_secret_ref_id, passphrase) {
        (Some(secret_ref_id), Some(value)) => inserts.push(VaultSecretInsert {
            secret_ref_id,
            kind: SecretKind::Passphrase,
            value,
        }),
        (None, None) => {}
        _ => return Err(invalid_credential_error(request_id)),
    }
    vault
        .insert_secrets(&inserts)
        .map_err(|error| map_vault_error(request_id.clone(), error))?;
    if record.import_state == CredentialImportState::Ready {
        return Ok(credential_summary(&record));
    }
    service
        .repository()
        .mark_credential_import_ready(
            &record.credential_ref_id,
            &request.operation_id,
            record.state_version,
            metadata.as_ref().map(|value| value.algorithm.as_str()),
            metadata
                .as_ref()
                .map(|value| value.fingerprint_sha256.as_str()),
        )
        .map(|record| credential_summary(&record))
        .map_err(|error| map_persistence_error(request_id, error))
}

pub(crate) fn replace_password_credential(
    request_id: RequestId,
    credential_ref_id: &CredentialRefId,
    identity_id: &IdentityId,
    expected_state_version: WireSequence,
    secret: Zeroizing<Vec<u8>>,
    service: &HostService,
    vault: &VaultService,
) -> CoreResult<CredentialRefSummary> {
    let record = service
        .repository()
        .get_ready_credential_record(credential_ref_id)
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    if record.identity_id != *identity_id || record.state_version != expected_state_version {
        return Err(map_persistence_error(
            request_id,
            AppPersistenceError::Conflict,
        ));
    }
    let CredentialRecordDetails::Password { secret_ref_id } = &record.details else {
        return Err(invalid_credential_error(request_id));
    };
    // Invalidate the old sync clock before the Vault write. If either write
    // fails, another device must review this secret instead of trusting an
    // earlier timestamp for new Vault contents.
    service
        .repository()
        .invalidate_ssh_sync_secret_time(secret_ref_id.as_str())
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    vault
        .replace_secret(&VaultSecretInsert {
            secret_ref_id: secret_ref_id.clone(),
            kind: SecretKind::Password,
            value: secret,
        })
        .map_err(|error| map_vault_error(request_id.clone(), error))?;
    service
        .repository()
        .record_ssh_sync_secret_time(secret_ref_id.as_str())
        .map_err(|error| map_persistence_error(request_id, error))?;
    Ok(credential_summary(&record))
}

fn read_selected_private_key_file(path: &Path) -> std::io::Result<Zeroizing<Vec<u8>>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut source = options.open(path)?;
    let metadata = source.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(std::io::Error::other("reparse points are not accepted"));
        }
    }
    if !metadata.is_file() || metadata.len() > MAX_PRIVATE_KEY_FILE_BYTES {
        return Err(std::io::Error::other(
            "private key must be a bounded regular file",
        ));
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
    source
        .by_ref()
        .take(MAX_PRIVATE_KEY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PRIVATE_KEY_FILE_BYTES {
        return Err(std::io::Error::other("private key exceeds the size limit"));
    }
    Ok(bytes)
}

pub(crate) fn credential_summary(record: &CredentialRecord) -> CredentialRefSummary {
    let details = match &record.details {
        CredentialRecordDetails::Password { .. } => CredentialRefDetails::Password,
        CredentialRecordDetails::PrivateKey {
            public_key_algorithm,
            public_key_fingerprint,
            ..
        } => CredentialRefDetails::PrivateKey {
            public_key_algorithm: public_key_algorithm.clone(),
            public_key_fingerprint: public_key_fingerprint.clone(),
        },
        CredentialRecordDetails::KeyboardInteractive { max_rounds } => {
            CredentialRefDetails::KeyboardInteractive {
                max_rounds: *max_rounds,
            }
        }
        CredentialRecordDetails::SshAgent {
            public_key_blob,
            public_key_algorithm,
            public_key_fingerprint,
            scope,
        } => CredentialRefDetails::SshAgent {
            public_key_algorithm: public_key_algorithm.clone(),
            public_key_fingerprint: public_key_fingerprint.clone(),
            public_key_blob: public_key_blob.clone(),
            scope: *scope,
        },
        CredentialRecordDetails::Certificate { certificate, scope } => {
            CredentialRefDetails::Certificate {
                certificate: certificate.clone(),
                scope: *scope,
            }
        }
        CredentialRecordDetails::HardwareKey {
            public_key_blob,
            public_key_algorithm,
            public_key_fingerprint,
            application,
            scope,
        } => CredentialRefDetails::HardwareKey {
            public_key_algorithm: public_key_algorithm.clone(),
            public_key_fingerprint: public_key_fingerprint.clone(),
            public_key_blob: public_key_blob.clone(),
            application: application.clone(),
            scope: *scope,
        },
    };
    CredentialRefSummary {
        credential_ref_id: record.credential_ref_id.clone(),
        identity_id: record.identity_id.clone(),
        method: record.method,
        priority: record.priority,
        label: record.label.clone(),
        details,
        state_version: record.state_version,
    }
}

fn invalid_credential_error(request_id: RequestId) -> Box<CoreApiError> {
    Box::new(CoreApiError {
        code: "credential.invalid_material".to_owned(),
        category: ErrorCategory::Validation,
        retry_strategy: RetryStrategy::Never,
        message_key: "errors.credential.invalidMaterial".to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: None,
    })
}

#[tauri::command]
pub fn known_host_list(
    request: KnownHostListRequest,
    service: State<'_, HostService>,
) -> CoreResult<Vec<KnownHostSummary>> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .list_known_hosts()
        .map_err(|error| map_persistence_error(request_id, error))
}

#[tauri::command]
pub fn known_host_delete(
    request: KnownHostDeleteRequest,
    service: State<'_, HostService>,
) -> CoreResult<KnownHostDeleteResponse> {
    let request_id = request.meta.request_id;
    service
        .repository()
        .delete_known_host(&request.known_host_id, request.expected_state_version)
        .map_err(|error| map_persistence_error(request_id, error))
}

pub(crate) fn map_persistence_error(
    request_id: RequestId,
    error: AppPersistenceError,
) -> Box<CoreApiError> {
    let (code, category, retry_strategy, message_key) = match error {
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::Endpoint(_) => (
            "ssh_metadata.invalid_input",
            ErrorCategory::Validation,
            RetryStrategy::Never,
            "errors.sshMetadata.invalidInput",
        ),
        AppPersistenceError::NotFound => (
            "ssh_metadata.not_found",
            ErrorCategory::Unavailable,
            RetryStrategy::RefreshSnapshot,
            "errors.sshMetadata.notFound",
        ),
        AppPersistenceError::Conflict
        | AppPersistenceError::IdempotencyConflict
        | AppPersistenceError::DatabaseNotFresh => (
            "ssh_metadata.conflict",
            ErrorCategory::Conflict,
            RetryStrategy::RefreshSnapshot,
            "errors.sshMetadata.conflict",
        ),
        AppPersistenceError::KnownHostMismatch { .. } => (
            "known_host.mismatch",
            ErrorCategory::Conflict,
            RetryStrategy::WaitForUser,
            "errors.knownHost.mismatch",
        ),
        AppPersistenceError::KnownHostAlgorithmChanged { .. } => (
            "known_host.algorithm_changed",
            ErrorCategory::Conflict,
            RetryStrategy::WaitForUser,
            "errors.knownHost.algorithmChanged",
        ),
        AppPersistenceError::UnsupportedSchema(_) => (
            "ssh_metadata.unsupported_schema",
            ErrorCategory::Incompatible,
            RetryStrategy::Upgrade,
            "errors.sshMetadata.unsupportedSchema",
        ),
        AppPersistenceError::RequiresReload => (
            "vault.requires_reload",
            ErrorCategory::NeedsReconciliation,
            RetryStrategy::Reconcile,
            "errors.vault.requiresReload",
        ),
        AppPersistenceError::InvalidStoredData
        | AppPersistenceError::RestoreCommitUnknown
        | AppPersistenceError::Database(_)
        | AppPersistenceError::Io(_) => {
            return Box::new(CoreApiError::safe_internal(
                request_id,
                Uuid::new_v4().to_string(),
            ));
        }
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
    use std::{
        sync::{Arc, Barrier, mpsc},
        time::Duration,
    };

    use norishell_app_persistence::{
        AppPersistenceError, CredentialRecordDetails, HostCreatePasswordStageState,
    };
    use norishell_core_api::{
        AlgorithmCategory, AlgorithmCompatibilityException, DesktopPasswordStage, DesktopProfile,
        DesktopProtocol, ErrorCategory, HostCreatePasswordCancelRequest,
        HostCreatePasswordStageRequest, HostId, LoginAutomationSecretCancelRequest,
        LoginAutomationSecretCreateRequest, LoginAutomationStepInput, OperationId, RequestId,
        RequestMeta, WireSequence,
    };
    use norishell_secret_vault::SecretKind;

    use super::{
        HostService, LoginAutomationSecretReconcileReporter, cancel_host_create_password,
        cancel_login_automation_secret, constant_time_secret_eq, create_login_automation_secret,
        read_selected_private_key_file, reconcile_host_create_passwords,
        reconcile_login_automation_secrets, stage_host_create_password,
    };
    use crate::vault_service::{VaultSecretInsert, VaultService, VaultServiceError};

    #[test]
    fn reconciler_reports_failure_transitions_without_repeating_each_retry() {
        let mut reporter = LoginAutomationSecretReconcileReporter::default();
        assert!(reporter.record_failure("persistence"));
        assert!(!reporter.record_failure("persistence"));
        assert!(reporter.record_failure("vault"));
        assert!(!reporter.record_failure("vault"));
        assert!(reporter.record_success());
        assert!(!reporter.record_success());
        assert!(reporter.record_failure("vault"));
    }

    #[test]
    fn starts_with_an_independent_application_database() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("start service");
        let repository = service.repository();
        assert!(repository.path().ends_with("ssh/norishell.sqlite3"));
        assert!(repository.list_hosts().expect("list hosts").is_empty());
        assert!(HostId::parse(uuid::Uuid::now_v7().to_string()).is_ok());
    }

    #[test]
    fn selected_private_key_reader_accepts_only_bounded_regular_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let key_path = directory.path().join("aws.pem");
        std::fs::write(&key_path, b"-----BEGIN RSA PRIVATE KEY-----\nfixture\n")
            .expect("write fixture");
        assert_eq!(
            read_selected_private_key_file(&key_path)
                .expect("read regular file")
                .as_slice(),
            b"-----BEGIN RSA PRIVATE KEY-----\nfixture\n"
        );
        assert!(read_selected_private_key_file(directory.path()).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let linked_path = directory.path().join("linked.pem");
            symlink(&key_path, &linked_path).expect("create symlink");
            assert!(read_selected_private_key_file(&linked_path).is_err());
        }
    }

    #[test]
    fn host_create_password_stage_is_bound_replayable_and_cancellable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("unlock vault");
        let operation_id = OperationId::new();
        let request = |credential_label: &str, password: &str| HostCreatePasswordStageRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: operation_id.clone(),
            idempotency_key: "host-create-password-once".to_owned(),
            identity_label: "Dedicated host identity".to_owned(),
            credential_label: credential_label.to_owned(),
            password: password.to_owned(),
        };
        let staged = stage_host_create_password(request("Password", "sen"), &service, &vault)
            .expect("stage password");
        let replay = stage_host_create_password(request("Password", "sen"), &service, &vault)
            .expect("response-loss replay");
        assert_eq!(replay.staged_password_id, staged.staged_password_id);
        let changed_secret = stage_host_create_password(
            request("Password", "different retry value"),
            &service,
            &vault,
        )
        .expect_err("changed secret conflicts");
        assert_eq!(changed_secret.category, ErrorCategory::Conflict);
        let record = service
            .repository()
            .begin_host_create_password_stage_with_outcome(
                &operation_id,
                "host-create-password-once",
                "Dedicated host identity",
                "Password",
            )
            .expect("stage record")
            .record;
        assert_eq!(
            vault
                .read_secret(&record.secret_ref_id, SecretKind::Password)
                .expect("staged Vault secret")
                .expose(),
            b"sen"
        );
        let conflict = stage_host_create_password(request("Changed", "sen"), &service, &vault)
            .expect_err("changed metadata conflicts");
        assert_eq!(conflict.category, ErrorCategory::Conflict);

        let cancelled = cancel_host_create_password(
            HostCreatePasswordCancelRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: operation_id.clone(),
                idempotency_key: "host-create-password-once".to_owned(),
            },
            &service,
            &vault,
        )
        .expect("cancel staged password");
        assert!(cancelled.cancelled);
        assert!(
            vault
                .read_secret(&record.secret_ref_id, SecretKind::Password)
                .is_err()
        );
        assert!(
            !cancel_host_create_password(
                HostCreatePasswordCancelRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id,
                    idempotency_key: "host-create-password-once".to_owned(),
                },
                &service,
                &vault,
            )
            .expect("cancel replay")
            .cancelled
        );
    }

    #[test]
    fn desktop_password_save_consumes_the_staged_vault_secret_without_allowing_cancel_cleanup() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("create Vault");
        let operation_id = OperationId::new();
        let idempotency_key = "desktop-rdp-password-save";
        let staged = stage_host_create_password(
            HostCreatePasswordStageRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: operation_id.clone(),
                idempotency_key: idempotency_key.to_owned(),
                identity_label: "Desktop RDP identity".to_owned(),
                credential_label: "Desktop RDP password".to_owned(),
                password: "desktop-fixture-password".to_owned(),
            },
            &service,
            &vault,
        )
        .expect("stage desktop password");
        let profile = DesktopProfile {
            id: uuid::Uuid::new_v4().to_string(),
            label: "Fixture RDP".to_owned(),
            protocol: DesktopProtocol::Rdp,
            address: "desktop.example.test".to_owned(),
            port: 3389,
            username: "fixture-user".to_owned(),
            domain: String::new(),
            host_id: None,
            gateway_host_id: None,
            credential_ref_id: None,
            width: 1280,
            height: 720,
            clipboard_enabled: false,
            audio_playback_enabled: false,
            vnc_protocol_version: norishell_core_api::VncProtocolVersion::Auto,
            revision: WireSequence::new(0),
        };
        let saved = service
            .with_desktop_password_stage_repository(|repository| {
                repository.save_desktop_profile_with_password_stage(
                    &profile,
                    &DesktopPasswordStage {
                        operation_id: operation_id.clone(),
                        idempotency_key: idempotency_key.to_owned(),
                        staged_password_id: staged.staged_password_id.clone(),
                    },
                )
            })
            .expect("save RDP profile with staged password");
        let credential = service
            .get_ready_credential(
                saved
                    .credential_ref_id
                    .as_ref()
                    .expect("dedicated ready credential"),
            )
            .expect("load ready credential");
        let secret_ref_id = match credential.details {
            CredentialRecordDetails::Password { secret_ref_id } => secret_ref_id,
            _ => panic!("desktop password save must create a password credential"),
        };
        let stored_password = vault
            .read_secret(&secret_ref_id, SecretKind::Password)
            .expect("desktop credential resolves in Vault");
        assert!(constant_time_secret_eq(
            stored_password.expose(),
            b"desktop-fixture-password"
        ));

        let cancelled = cancel_host_create_password(
            HostCreatePasswordCancelRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id,
                idempotency_key: idempotency_key.to_owned(),
            },
            &service,
            &vault,
        )
        .expect("cancel consumed stage is an idempotent no-op");
        assert!(!cancelled.cancelled);
        assert!(
            vault
                .read_secret(&secret_ref_id, SecretKind::Password)
                .is_ok()
        );
    }

    #[test]
    fn host_create_password_reconciler_cleans_pending_vault_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("unlock vault");
        let operation_id = OperationId::new();
        let pending = service
            .repository()
            .begin_host_create_password_stage_with_outcome(
                &operation_id,
                "pending-host-password",
                "Dedicated identity",
                "Password",
            )
            .expect("pending stage")
            .record;
        vault
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: pending.secret_ref_id.clone(),
                kind: SecretKind::Password,
                value: zeroize::Zeroizing::new(b"orphan candidate".to_vec()),
            }])
            .expect("simulate committed Vault write before response loss");
        assert_eq!(
            reconcile_host_create_passwords(
                &service.repository,
                &service.login_automation_secret_lifecycle,
                &vault,
                64,
            )
            .expect("reconcile"),
            1
        );
        assert!(
            vault
                .read_secret(&pending.secret_ref_id, SecretKind::Password)
                .is_err()
        );
        let cancelled = service
            .repository()
            .begin_host_create_password_cleanup(&operation_id, "pending-host-password")
            .expect("cancelled record")
            .expect("record");
        assert_eq!(cancelled.state, HostCreatePasswordStageState::Cancelled);
    }

    #[test]
    fn records_only_saved_host_connection_success_in_recent_history() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("start service");
        let host = service
            .repository()
            .create_host("Recent", "recent.example", 22, Some("ops"), None, false)
            .expect("create host");

        let recorded = service
            .record_successful_connection(&host.host_id)
            .expect("record success");
        assert_eq!(recorded.host_id, host.host_id);
        assert_eq!(recorded.successful_connection_count.get(), 1);

        let recent = service
            .repository()
            .list_recent_connections(10)
            .expect("list recent");
        assert_eq!(recent, vec![recorded]);
    }

    #[test]
    fn overview_sources_do_not_relock_the_repository_for_saved_hosts() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("start service");
        service
            .repository()
            .create_host("Overview", "overview.example", 22, Some("ops"), None, false)
            .expect("create host");

        let (completed_tx, completed_rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = service.list_overview_sources(RequestId::new());
            let _ = completed_tx.send(result);
        });
        let sources = completed_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("overview source projection must not deadlock")
            .expect("overview sources");

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].0.host.label, "Overview");
        assert!(!sources[0].0.host.has_ready_credential);
    }

    #[test]
    fn login_automation_secret_saga_recovers_from_locked_vault_and_cancels_idempotently() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        let host = service
            .repository()
            .create_host("Automation", "automation.example", 22, None, None, false)
            .expect("host");
        let locked_operation_id = OperationId::new();
        let locked_request = LoginAutomationSecretCreateRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: locked_operation_id.clone(),
            idempotency_key: "locked-login-secret".to_owned(),
            host_id: host.host_id.clone(),
            expected_automation_revision: WireSequence::new(1),
            label: "Root password".to_owned(),
            value: "never-written secret".to_owned(),
        };

        let locked = create_login_automation_secret(locked_request, &service, &vault)
            .expect_err("locked vault");
        assert_eq!(locked.code, "vault.locked");
        assert!(
            !cancel_login_automation_secret(
                LoginAutomationSecretCancelRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: locked_operation_id,
                    idempotency_key: "locked-login-secret".to_owned(),
                },
                &service,
                &vault,
            )
            .expect("locked create already safely aborted")
            .cancelled
        );

        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("unlock vault");
        let operation_id = OperationId::new();
        let make_request = |value: &str| LoginAutomationSecretCreateRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: operation_id.clone(),
            idempotency_key: "stage-login-secret-once".to_owned(),
            host_id: host.host_id.clone(),
            expected_automation_revision: WireSequence::new(1),
            label: "Root password".to_owned(),
            value: value.to_owned(),
        };
        let created =
            create_login_automation_secret(make_request("correct secret"), &service, &vault)
                .expect("create staged secret");
        let pending = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &operation_id,
                "stage-login-secret-once",
                "Root password",
            )
            .expect("read staged intent");
        assert_eq!(created.staged_secret_id, pending.staged_secret_id);
        assert_eq!(
            create_login_automation_secret(make_request("correct secret"), &service, &vault)
                .expect("staged exact replay")
                .staged_secret_id,
            created.staged_secret_id
        );
        let changed =
            create_login_automation_secret(make_request("changed secret"), &service, &vault)
                .expect_err("changed secret replay");
        assert_eq!(changed.code, "vault.secret_conflict");
        assert_eq!(
            vault
                .read_secret(&pending.secret_ref_id, SecretKind::LoginAutomation)
                .expect("staged secret")
                .expose(),
            b"correct secret"
        );
        for path in [
            directory.path().join("ssh/norishell.sqlite3"),
            directory.path().join("ssh/norishell.sqlite3-wal"),
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                assert!(
                    !bytes
                        .windows(b"correct secret".len())
                        .any(|window| window == b"correct secret")
                );
            }
        }

        let cancelled = cancel_login_automation_secret(
            LoginAutomationSecretCancelRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: operation_id.clone(),
                idempotency_key: "stage-login-secret-once".to_owned(),
            },
            &service,
            &vault,
        )
        .expect("cancel staged secret");
        assert!(cancelled.cancelled);
        assert!(matches!(
            vault.read_secret(&pending.secret_ref_id, SecretKind::LoginAutomation),
            Err(VaultServiceError::SecretNotFound)
        ));
        let replay = cancel_login_automation_secret(
            LoginAutomationSecretCancelRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: operation_id.clone(),
                idempotency_key: "stage-login-secret-once".to_owned(),
            },
            &service,
            &vault,
        )
        .expect("cancel replay");
        assert!(!replay.cancelled);

        assert!(
            !cancel_login_automation_secret(
                LoginAutomationSecretCancelRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: OperationId::new(),
                    idempotency_key: "missing-login-secret".to_owned(),
                },
                &service,
                &vault,
            )
            .expect("missing cancel")
            .cancelled
        );

        let pending_operation_id = OperationId::new();
        let _pending_before_write = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &pending_operation_id,
                "cancel-before-vault-write",
                "Unused password",
            )
            .expect("pending intent");
        assert!(
            cancel_login_automation_secret(
                LoginAutomationSecretCancelRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: pending_operation_id,
                    idempotency_key: "cancel-before-vault-write".to_owned(),
                },
                &service,
                &vault,
            )
            .expect("cancel missing vault entry")
            .cancelled
        );

        let consumed_operation_id = OperationId::new();
        let consumed = create_login_automation_secret(
            LoginAutomationSecretCreateRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: consumed_operation_id.clone(),
                idempotency_key: "consumed-response-lost".to_owned(),
                host_id: host.host_id.clone(),
                expected_automation_revision: WireSequence::new(1),
                label: "Consumed password".to_owned(),
                value: "must remain in vault".to_owned(),
            },
            &service,
            &vault,
        )
        .expect("stage consumed secret");
        let consumed_record = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &consumed_operation_id,
                "consumed-response-lost",
                "Consumed password",
            )
            .expect("read consumed candidate");
        service
            .repository()
            .replace_login_automation(
                &host.host_id,
                WireSequence::new(1),
                true,
                &[LoginAutomationStepInput::SendSecret {
                    secret_ref_id: consumed.staged_secret_id,
                    secret_label: "ignored".to_owned(),
                    append_enter: true,
                    timeout_seconds: 10,
                }],
            )
            .expect("consume secret before response is lost");
        assert!(
            !cancel_login_automation_secret(
                LoginAutomationSecretCancelRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: consumed_operation_id,
                    idempotency_key: "consumed-response-lost".to_owned(),
                },
                &service,
                &vault,
            )
            .expect("consumed cancel is a safe no-op")
            .cancelled
        );
        assert_eq!(
            vault
                .read_secret(&consumed_record.secret_ref_id, SecretKind::LoginAutomation,)
                .expect("consumed secret remains available")
                .expose(),
            b"must remain in vault"
        );
    }

    #[test]
    fn pending_vault_replay_requires_cleanup_before_any_secret_write() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("unlock vault");
        let host = service
            .repository()
            .create_host("Pending", "pending.example", 22, None, None, false)
            .expect("host");
        let operation_id = OperationId::new();
        let pending = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &operation_id,
                "pending-before-vault-write",
                "Pending password",
            )
            .expect("persist pending intent");
        let make_request = |value: &str| LoginAutomationSecretCreateRequest {
            meta: RequestMeta {
                request_id: RequestId::new(),
            },
            operation_id: operation_id.clone(),
            idempotency_key: "pending-before-vault-write".to_owned(),
            host_id: host.host_id.clone(),
            expected_automation_revision: WireSequence::new(1),
            label: "Pending password".to_owned(),
            value: value.to_owned(),
        };
        for value in ["original value", "changed value"] {
            let error = create_login_automation_secret(make_request(value), &service, &vault)
                .expect_err("pending replay must reconcile first");
            assert_eq!(error.category, ErrorCategory::NeedsReconciliation);
            assert!(matches!(
                vault.read_secret(&pending.secret_ref_id, SecretKind::LoginAutomation),
                Err(VaultServiceError::SecretNotFound)
            ));
        }
        assert!(
            cancel_login_automation_secret(
                LoginAutomationSecretCancelRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id,
                    idempotency_key: "pending-before-vault-write".to_owned(),
                },
                &service,
                &vault,
            )
            .expect("delete-first cleanup without a vault value")
            .cancelled
        );
        let retry_operation_id = OperationId::new();
        let retried = create_login_automation_secret(
            LoginAutomationSecretCreateRequest {
                meta: RequestMeta {
                    request_id: RequestId::new(),
                },
                operation_id: retry_operation_id,
                idempotency_key: "pending-after-cleanup".to_owned(),
                host_id: host.host_id,
                expected_automation_revision: WireSequence::new(1),
                label: "Pending password".to_owned(),
                value: "explicit retry value".to_owned(),
            },
            &service,
            &vault,
        )
        .expect("new operation succeeds after cleanup");
        assert_ne!(retried.staged_secret_id, pending.staged_secret_id);
    }

    #[test]
    fn create_cancel_race_is_serialized_across_database_and_vault() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("unlock vault");
        let host = service
            .repository()
            .create_host("Race", "race.example", 22, None, None, false)
            .expect("host");
        let operation_id = OperationId::new();
        let lifecycle = service.login_automation_secret_lifecycle();
        let staged = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &operation_id,
                "create-cancel-race",
                "Race password",
            )
            .expect("begin stage while lifecycle is owned");

        let barrier = Arc::new(Barrier::new(2));
        let cancel_barrier = Arc::clone(&barrier);
        let cancel_service = service.clone();
        let cancel_vault = vault.clone();
        let cancel_operation_id = operation_id.clone();
        let cancel = std::thread::spawn(move || {
            cancel_barrier.wait();
            cancel_login_automation_secret(
                LoginAutomationSecretCancelRequest {
                    meta: RequestMeta {
                        request_id: RequestId::new(),
                    },
                    operation_id: cancel_operation_id,
                    idempotency_key: "create-cancel-race".to_owned(),
                },
                &cancel_service,
                &cancel_vault,
            )
        });
        barrier.wait();
        std::thread::sleep(Duration::from_millis(20));
        assert!(!cancel.is_finished());

        vault
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: staged.secret_ref_id.clone(),
                kind: SecretKind::LoginAutomation,
                value: zeroize::Zeroizing::new(b"race secret".to_vec()),
            }])
            .expect("vault insert");
        service
            .repository()
            .mark_login_automation_secret_staged(&staged.staged_secret_id)
            .expect("mark staged");
        drop(lifecycle);

        assert!(
            cancel
                .join()
                .expect("cancel thread")
                .expect("cancel")
                .cancelled
        );
        assert!(matches!(
            vault.read_secret(&staged.secret_ref_id, SecretKind::LoginAutomation),
            Err(VaultServiceError::SecretNotFound)
        ));
    }

    #[test]
    fn reconciler_cleans_crash_states_and_shutdown_stops_the_worker() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("unlock vault");
        let host = service
            .repository()
            .create_host("Recovery", "recovery.example", 22, None, None, false)
            .expect("host");

        let pending_without_secret_operation = OperationId::new();
        service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &pending_without_secret_operation,
                "pending-without-secret",
                "Pending without secret",
            )
            .expect("pending without secret");
        assert_eq!(
            reconcile_login_automation_secrets(
                &service.repository,
                &service.login_automation_secret_lifecycle,
                &vault,
                64,
            )
            .expect("reconcile missing secret"),
            1
        );

        let pending_with_secret_operation = OperationId::new();
        let pending_with_secret = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &pending_with_secret_operation,
                "pending-with-secret",
                "Pending with secret",
            )
            .expect("pending with secret");
        vault
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: pending_with_secret.secret_ref_id.clone(),
                kind: SecretKind::LoginAutomation,
                value: zeroize::Zeroizing::new(b"crash secret".to_vec()),
            }])
            .expect("simulate vault commit before crash");
        assert_eq!(
            reconcile_login_automation_secrets(
                &service.repository,
                &service.login_automation_secret_lifecycle,
                &vault,
                64,
            )
            .expect("reconcile pending secret"),
            1
        );
        assert!(matches!(
            vault.read_secret(
                &pending_with_secret.secret_ref_id,
                SecretKind::LoginAutomation
            ),
            Err(VaultServiceError::SecretNotFound)
        ));

        let cleanup_operation = OperationId::new();
        let cleanup_pending = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &cleanup_operation,
                "cleanup-pending-resume",
                "Cleanup pending",
            )
            .expect("cleanup pending stage");
        vault
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: cleanup_pending.secret_ref_id.clone(),
                kind: SecretKind::LoginAutomation,
                value: zeroize::Zeroizing::new(b"cleanup secret".to_vec()),
            }])
            .expect("cleanup pending secret");
        service
            .repository()
            .begin_login_automation_secret_cleanup(&cleanup_operation, "cleanup-pending-resume")
            .expect("durable cleanup intent")
            .expect("cleanup record");
        assert_eq!(
            reconcile_login_automation_secrets(
                &service.repository,
                &service.login_automation_secret_lifecycle,
                &vault,
                64,
            )
            .expect("resume cleanup pending"),
            1
        );

        let worker_operation = OperationId::new();
        let worker_pending = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &worker_operation,
                "worker-startup-cleanup",
                "Worker cleanup",
            )
            .expect("worker pending");
        vault
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: worker_pending.secret_ref_id.clone(),
                kind: SecretKind::LoginAutomation,
                value: zeroize::Zeroizing::new(b"worker secret".to_vec()),
            }])
            .expect("worker secret");
        service
            .start_login_automation_secret_reconciler_with_interval(
                vault.clone(),
                Duration::from_millis(10),
            )
            .expect("start reconciler");
        for _ in 0..100 {
            if matches!(
                vault.read_secret(&worker_pending.secret_ref_id, SecretKind::LoginAutomation),
                Err(VaultServiceError::SecretNotFound)
            ) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            vault.read_secret(&worker_pending.secret_ref_id, SecretKind::LoginAutomation),
            Err(VaultServiceError::SecretNotFound)
        ));
        service.shutdown_login_automation_secret_reconciler();

        let after_shutdown_operation = OperationId::new();
        let after_shutdown = service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &after_shutdown_operation,
                "after-worker-shutdown",
                "After shutdown",
            )
            .expect("post-shutdown pending");
        vault
            .insert_secrets(&[VaultSecretInsert {
                secret_ref_id: after_shutdown.secret_ref_id.clone(),
                kind: SecretKind::LoginAutomation,
                value: zeroize::Zeroizing::new(b"post shutdown secret".to_vec()),
            }])
            .expect("post-shutdown secret");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            vault
                .read_secret(&after_shutdown.secret_ref_id, SecretKind::LoginAutomation)
                .expect("worker stayed stopped")
                .expose(),
            b"post shutdown secret"
        );
        reconcile_login_automation_secrets(
            &service.repository,
            &service.login_automation_secret_lifecycle,
            &vault,
            64,
        )
        .expect("final test cleanup");
    }

    #[test]
    fn reconciler_waits_for_vault_unlock_then_converges_pending_startup_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("host service");
        let vault = VaultService::start(directory.path());
        let host = service
            .repository()
            .create_host("Startup", "startup.example", 22, None, None, false)
            .expect("host");
        let operation_id = OperationId::new();
        service
            .repository()
            .begin_login_automation_secret_stage(
                &host.host_id,
                WireSequence::new(1),
                &operation_id,
                "startup-pending",
                "Startup pending",
            )
            .expect("startup pending");
        service
            .start_login_automation_secret_reconciler_with_interval(
                vault.clone(),
                Duration::from_millis(10),
            )
            .expect("start reconciler");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            service
                .repository()
                .list_login_automation_secret_cleanup_candidates(64)
                .expect("locked candidates")
                .len(),
            1
        );

        vault
            .create_for_tests(b"correct horse battery staple")
            .expect("unlock vault");
        for _ in 0..100 {
            if service
                .repository()
                .list_login_automation_secret_cleanup_candidates(64)
                .expect("startup candidates")
                .is_empty()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            service
                .repository()
                .list_login_automation_secret_cleanup_candidates(64)
                .expect("startup converged")
                .is_empty()
        );
        service.shutdown_login_automation_secret_reconciler();
    }

    #[test]
    fn public_algorithm_update_rejects_unknown_catalog_ids_without_advancing_revision() {
        let directory = tempfile::tempdir().expect("tempdir");
        let service = HostService::start(directory.path()).expect("start service");
        let host = service
            .repository()
            .create_host(
                "Algorithms",
                "algorithms.example",
                22,
                Some("ops"),
                None,
                false,
            )
            .expect("create host");
        let invalid = AlgorithmCompatibilityException {
            category: AlgorithmCategory::Mac,
            exception_id: "compat-mac-syntactically-valid-but-unknown".to_owned(),
            reason: None,
        };
        assert!(matches!(
            service.replace_algorithm_policy(
                &host.host_id,
                WireSequence::new(1),
                "secure-default",
                &[invalid],
            ),
            Err(AppPersistenceError::InvalidInput(_))
        ));
        let unchanged = service
            .repository()
            .get_algorithm_policy(&host.host_id)
            .expect("read unchanged policy");
        assert_eq!(unchanged.revision, WireSequence::new(1));
        assert!(unchanged.compatibility_exceptions.is_empty());

        let valid = AlgorithmCompatibilityException {
            category: AlgorithmCategory::Mac,
            exception_id: "compat-mac-hmac-sha1-etm".to_owned(),
            reason: Some("legacy appliance".to_owned()),
        };
        let updated = service
            .replace_algorithm_policy(
                &host.host_id,
                WireSequence::new(1),
                "secure-default",
                &[valid],
            )
            .expect("save catalog-owned exception");
        assert_eq!(updated.revision, WireSequence::new(2));
    }
}
