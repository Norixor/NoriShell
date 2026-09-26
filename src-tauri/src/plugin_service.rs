mod api;
mod api_credentials;
mod api_data;
mod api_file;
mod api_host;
mod api_invocation;
mod api_network;
mod api_permissions;
mod api_process;
mod api_remote_exec;
mod api_serial;
mod api_sftp;
mod api_subscriptions;
pub(crate) mod app_integration;
mod approval_policy;
mod broker_chain;
mod data_reads;
mod input;
pub(crate) mod isolated;
mod operations;
mod permissions;
mod protocol_driver;
mod protocol_launch;
pub(crate) mod protocol_terminal;
pub(crate) mod protocols;
pub(crate) mod settings;
mod tasks;
mod terminal;

pub(crate) use terminal::ApprovedPluginTerminalStartup;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_app_persistence::{
    AppPersistenceError, PluginActivationPermissions, PluginCapabilityGrantRecord,
    PluginHostScopeSetRecord, PluginInstalledRecord, PluginOperationPhase, PluginOperationRecord,
    PluginPermissionBinding, PluginPrivateStorageOwner,
};
use norishell_core_api::{
    CoreApiError, EVENT_PLUGIN_SSH_SYNC_BROWSER_INVALIDATED, ErrorCategory, ExitBlocker,
    InstalledPluginSummary, MAX_THEME_PACKAGE_ENTRIES, PLUGIN_PROTOCOL_MAJOR,
    PLUGIN_PROTOCOL_MAX_MINOR, PLUGIN_PROTOCOL_MINOR, PLUGIN_THEME_PROTOCOL_MINOR,
    PluginApprovalDecision, PluginApprovalId, PluginApprovedHostSessionLaunch, PluginAuditEntry,
    PluginAuditListRequest, PluginCapability, PluginCapabilityGrant,
    PluginCapabilityGrantsReplaceRequest, PluginContributionCopyRequest,
    PluginContributionCopyResponse, PluginContributionInvokeRequest, PluginContributionListRequest,
    PluginContributionNode, PluginContributionPanel, PluginContributionSlot, PluginErrorCode,
    PluginExtensionTargetContext, PluginExtensionTargetListRequest,
    PluginHostApprovalDecisionRequest, PluginHostApprovalDecisionResponse,
    PluginHostApprovalGetRequest, PluginHostApprovalKind, PluginHostApprovalSummary,
    PluginHostDomOperation, PluginHostDomOperationBatch, PluginHostDomSnapshot, PluginHostHandle,
    PluginHostMessageKind, PluginHostMetadataProjection, PluginHostMutationPatch,
    PluginHostRequest, PluginHostScopeListRequest, PluginHostScopeReplaceRequest,
    PluginHostScopeSnapshot, PluginHostScopeSummary, PluginHostSessionKind, PluginId,
    PluginInputApprovalId, PluginInstallState, PluginInstalledListRequest,
    PluginIsolatedSurfaceContent, PluginIsolatedSurfaceContentRequest,
    PluginIsolatedSurfaceOpenRequest, PluginLocalInstallRequest, PluginLocalPackageCancelRequest,
    PluginLocalPackagePrepareRequest, PluginLocalPackagePreview, PluginLocale,
    PluginLocaleSetRequest, PluginNavigationContribution, PluginNavigationItem,
    PluginNavigationListRequest, PluginObserverId, PluginOperationKind,
    PluginOperationPermissionList, PluginOperationPermissionListRequest,
    PluginOperationPermissionRevokeRequest, PluginOperationPermissionsClearRequest,
    PluginOperationRequest, PluginOperationState, PluginOperationSummary, PluginPackageKind,
    PluginPageContribution, PluginReadiness, PluginReadinessGetRequest,
    PluginSafeModeNextStartRequest, PluginSpecialPermissionDecisionRequest,
    PluginSpecialPermissionDecisionResponse, PluginSpecialPermissionGetRequest,
    PluginSpecialPermissionHost, PluginSpecialPermissionOpenRequest,
    PluginSpecialPermissionOutcome, PluginSpecialPermissionSnapshot, PluginSpecialPermissionTarget,
    PluginSshSyncBrowserInvalidated, PluginSshSyncBrowserReadRequest, PluginSshSyncBrowserSnapshot,
    PluginSshSyncBrowserState, PluginStateChangeRequest, PluginTargetContextCloseRequest,
    PluginTargetContextHandle, PluginTargetContextOpenRequest, PluginTerminalInputDecision,
    PluginTerminalInputDecisionRequest, PluginTerminalInputDecisionResponse,
    PluginTerminalInputGetRequest, PluginTerminalInputOpenRequest,
    PluginTerminalInputPendingListRequest, PluginTerminalInputProposal, PluginTerminalKind,
    PluginTerminalMetadataProjection, PluginTerminalObserveAttachRequest,
    PluginTerminalObserveAttachResponse, PluginTerminalObserveDetachRequest,
    PluginTerminalObserverBinding, PluginTerminalState, PluginThemeListRequest,
    PluginThemeListResponse, PluginUiActionRequest, PluginUiActionResponse, PluginUiContribution,
    PluginUiContributionListRequest, PluginUiDocument, PluginUiFieldValue, PluginUiTemplate,
    RequestId, RequestMeta, RetryStrategy, SafeConflictVersion, SshSessionTarget,
    TerminalInputFocusTarget, TerminalInputLease, ThemePackageEntry, WireSequence,
};
use norishell_plugin_platform::{
    PackageLimits, PluginInstaller, PluginPlatformError, PluginUiActionKind, inspect_local_package,
    validate_plugin_dialog_ui_action, validate_plugin_dom_operations, validate_plugin_dom_snapshot,
    validate_plugin_page_ui_action, validate_plugin_ui_action,
};

use semver::Version;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt as _;
use uuid::Uuid;

use crate::{
    host_service::HostService,
    lifecycle::LifecycleState,
    plugin_contribution::{
        ParsedPluginContribution, ParsedPluginUiOutputs, TemplateLifecycleActionAdmission,
        parse_plugin_ui_outputs, parse_ui_panel_outputs, template_lifecycle_action_admission,
        valid_action_id, validate_template_lifecycle,
    },
    plugin_extension_registry,
    plugin_host_process::{PluginHostProcess, PluginHostProcessError},
    ssh_session_service::{
        ApprovedPluginInput, ApprovedPluginObservationAttach, PluginTerminalObservation,
        SshSessionService,
    },
    time::unix_time_ms,
};

type CoreResult<T> = Result<T, Box<CoreApiError>>;

const PLUGIN_INPUT_APPROVAL_MILLIS: i64 = 30_000;
const PLUGIN_INPUT_MAX_BYTES: usize = 8 * 1024;
const PLUGIN_OBSERVATION_QUEUE_CAPACITY: usize = 32;
const PLUGIN_OBSERVER_GLOBAL_LIMIT: usize = 64;
const PLUGIN_OBSERVER_PER_PLUGIN_LIMIT: usize = 8;
const PLUGIN_OBSERVER_PER_SESSION_LIMIT: usize = 4;
const PLUGIN_OBSERVER_LEDGER_LIMIT: usize = 128;
const PLUGIN_TARGET_CONTEXT_LIMIT: usize = 2_048;
const PLUGIN_HOST_HANDLE_LIMIT: usize = 2_048;
const PLUGIN_HOST_APPROVAL_LIMIT: usize = 64;
const PLUGIN_HOST_APPROVAL_MILLIS: i64 = 5 * 60 * 1_000;
const PLUGIN_ISOLATED_SURFACE_MAX_BYTES: u64 = 2 * 1024 * 1024;
const PLUGIN_ISOLATED_SURFACE_GLOBAL_LIMIT: usize = 16;
const PLUGIN_ISOLATED_SURFACE_PER_PLUGIN_LIMIT: usize = 4;
const PLUGIN_PERMISSION_SURFACE_CONTRACT_REVISION: u64 = 1;
#[derive(Clone)]
pub(crate) struct PluginService {
    hosts: HostService,
    sessions: SshSessionService,
    installer: Arc<PluginInstaller>,
    ssh_sync: Option<crate::ssh_sync_exchange::SshSyncExchangeBroker>,
    operations: Option<crate::plugin_operations::PluginOperationsService>,
    resources: Option<crate::plugin_resources::PluginResourceService>,
    api: crate::plugin_api::PluginApi,
    api_sftp: Arc<Mutex<Option<crate::plugin_api::sftp::PluginSftpDriver>>>,
    protocol_slots: Arc<tokio::sync::Semaphore>,
    app_integrations: Arc<Mutex<app_integration::PluginAppIntegrationState>>,
    protocol_terminal:
        Arc<Mutex<Option<crate::plugin_terminal_session_service::PluginTerminalSessionService>>>,
    protocol_sessions: Arc<Mutex<BTreeMap<Uuid, protocol_terminal::ProtocolSessionContext>>>,
    protocol_launch_gate: Arc<tokio::sync::Mutex<()>>,
    protocol_snapshot_revision: Arc<std::sync::atomic::AtomicU64>,
    serial_candidates: Arc<Mutex<BTreeMap<String, api_serial::SerialCandidateRecord>>>,
    protocol_launches: Arc<Mutex<BTreeMap<String, protocol_launch::ProtocolLaunch>>>,
    operation_policies: crate::plugin_operation_policy::OperationPolicyService,
    package_limits: PackageLimits,
    local_import_root: Arc<PathBuf>,
    app_handle: Arc<Mutex<Option<AppHandle>>>,
    #[cfg(test)]
    test_lifecycle: Arc<Mutex<Option<LifecycleState>>>,
    safe_mode_next_marker: Arc<PathBuf>,
    safe_mode_active_marker: Arc<PathBuf>,
    safe_mode_active: bool,
    locale_path: Arc<PathBuf>,
    locale: Arc<Mutex<PluginLocale>>,
    runtime: Arc<Mutex<PluginRuntimeState>>,
}

#[derive(Default)]
struct PluginRuntimeState {
    active_instances: BTreeMap<String, ActivePluginInstance>,
    mutation_reservations: BTreeMap<String, Uuid>,
    pending_inputs: BTreeMap<String, PendingPluginInput>,
    observers: BTreeMap<String, PluginTerminalObserverBinding>,
    observer_attach_reservations: BTreeMap<String, PluginObserverAttachReservation>,
    observer_attach_ledger: BTreeMap<String, PluginObserverAttachLedgerEntry>,
    observer_attach_idempotency: BTreeMap<String, String>,
    observer_detach_reservations: BTreeMap<String, PluginObserverDetachReservation>,
    observer_detach_ledger: BTreeMap<String, String>,
    observer_detach_idempotency: BTreeMap<String, String>,
    prepared_local_packages: BTreeMap<String, PreparedLocalPackage>,
    target_contexts: BTreeMap<String, ActivePluginTargetContext>,
    target_context_identity: BTreeMap<String, String>,
    host_handles: BTreeMap<String, ActivePluginHostHandle>,
    pending_host_approvals: BTreeMap<String, PendingPluginHostApproval>,
    approved_host_sessions: BTreeMap<String, ApprovedPluginHostSession>,
    approved_terminal_channels: BTreeMap<String, terminal::PendingTerminalChannel>,
    pending_special_permissions: BTreeMap<String, PendingPluginSpecialPermission>,
    isolated_surfaces: BTreeMap<String, ActivePluginIsolatedSurface>,
}

struct PreparedLocalPackage {
    path: PathBuf,
    inspected: norishell_plugin_platform::InspectedPackage,
    preview: PluginLocalPackagePreview,
    permission_revision: u64,
    approved_special_permissions: Option<permissions::PreparedSpecialPermissionApproval>,
}

impl Drop for PreparedLocalPackage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct PluginObserverAttachReservation {
    fingerprint: String,
    idempotency_key: String,
    plugin_id: PluginId,
    session_id: norishell_core_api::SshSessionId,
}

struct PluginObserverAttachLedgerEntry {
    fingerprint: String,
    idempotency_key: String,
    binding: PluginTerminalObserverBinding,
}

struct PluginObserverDetachReservation {
    fingerprint: String,
    idempotency_key: String,
    plugin_id: PluginId,
}

#[derive(Clone)]
struct ActivePluginTargetContext {
    context: PluginExtensionTargetContext,
    identity_key: String,
    instance_key: String,
}

#[derive(Clone)]
struct ActivePluginHostHandle {
    plugin_id: PluginId,
    host_id: norishell_core_api::HostId,
    instance_generation: WireSequence,
    context_handle: PluginTargetContextHandle,
    scope_state_version: WireSequence,
    created_at_unix_ms: i64,
}

struct PendingPluginHostApproval {
    summary: PluginHostApprovalSummary,
    signer_fingerprint_sha256: String,
    package_sha256: String,
    instance_generation: WireSequence,
    host_id: norishell_core_api::HostId,
    host_handle: PluginHostHandle,
    scope_state_version: WireSequence,
    expected_host_state_version: Option<WireSequence>,
    action_label: String,
    mutation_patch: Option<PluginHostMutationPatch>,
    session_kind: Option<PluginHostSessionKind>,
    operation_policy: Option<crate::plugin_operation_policy::PreparedOperationPolicy>,
    approved_operation_policy: Option<crate::plugin_operation_policy::ApprovedOperationPolicy>,
}

struct PendingPluginSpecialPermission {
    snapshot: PluginSpecialPermissionSnapshot,
    /// Cancellation only restores the already-projected installed state. It
    /// never grants the secure surface any additional query capability.
    installed_snapshot: Option<InstalledPluginSummary>,
    expected_plugin_state_version: WireSequence,
    expected_grant_state_version: Option<WireSequence>,
    expected_scope_state_version: Option<WireSequence>,
    all_grants: Vec<PluginCapabilityGrantRecord>,
    prepared_baseline: Option<permissions::PreparedPermissionBaseline>,
}

#[derive(Clone)]
struct ApprovedPluginHostSession {
    plugin_id: PluginId,
    signer_fingerprint_sha256: String,
    package_sha256: String,
    instance_generation: WireSequence,
    host_id: norishell_core_api::HostId,
    scope_state_version: WireSequence,
    kind: PluginHostSessionKind,
    action_label: String,
    expires_at_unix_ms: i64,
    operation_policy: Option<crate::plugin_operation_policy::ApprovedOperationPolicy>,
}

use isolated::ActivePluginIsolatedSurface;

#[derive(Clone)]
struct ActivePluginInstance {
    plugin_name: String,
    signer_fingerprint_sha256: String,
    package_sha256: String,
    instance_generation: WireSequence,
    state_version: WireSequence,
    previously_crashed: bool,
    protocol_minor: u16,
    locale: norishell_core_api::PluginLocale,
    contributions: BTreeMap<PluginContributionSlot, ActivePluginContribution>,
    ui_templates: BTreeMap<String, PluginUiTemplate>,
    scoped_templates: BTreeMap<String, PluginUiTemplate>,
    scoped_states: BTreeMap<String, String>,
    operation_authority_revoked: bool,
    api_authority: Arc<std::sync::atomic::AtomicBool>,
    navigation: BTreeMap<String, PluginNavigationContribution>,
    pages: BTreeMap<String, PluginPageContribution>,
    contribution_revision: WireSequence,
    settings_revision: Option<WireSequence>,
    contribution_action_in_flight: bool,
    ui_state_json: String,
    process: Arc<Mutex<Option<PluginHostProcess>>>,
}

impl ActivePluginInstance {
    fn can_publish_contributions(&self) -> bool {
        !self.operation_authority_revoked
    }
}

#[derive(Clone)]
struct ActivePluginContribution {
    nodes: Vec<PluginContributionNode>,
    clipboard_values: BTreeMap<String, String>,
}

struct PendingPluginInput {
    proposal: PluginTerminalInputProposal,
    lease_id: norishell_core_api::SshInputLeaseId,
    bytes: Vec<u8>,
    remembered_policy: Option<crate::plugin_operation_policy::PreparedOperationPolicy>,
    authority: Option<crate::plugin_operations::OperationFence>,
}

fn contribution_map(
    parsed: Vec<ParsedPluginContribution>,
) -> Result<BTreeMap<PluginContributionSlot, ActivePluginContribution>, ()> {
    let mut contributions = BTreeMap::new();
    for contribution in parsed {
        if contributions
            .insert(
                contribution.slot,
                ActivePluginContribution {
                    nodes: contribution.nodes,
                    clipboard_values: contribution.clipboard_values,
                },
            )
            .is_some()
        {
            return Err(());
        }
    }
    Ok(contributions)
}

fn ui_template_map(
    templates: Vec<PluginUiTemplate>,
) -> Result<BTreeMap<String, PluginUiTemplate>, ()> {
    let mut mapped = BTreeMap::new();
    for template in templates {
        let target_id = template.target_id.as_str().to_owned();
        if plugin_extension_registry::find(&template.target_id).is_none()
            || mapped.insert(target_id, template).is_some()
        {
            return Err(());
        }
    }
    Ok(mapped)
}

fn template_on_open_action_id(
    template: &PluginUiTemplate,
) -> Option<norishell_core_api::PluginUiActionId> {
    template.on_open_action_id.clone()
}

fn template_auto_refresh(
    template: &PluginUiTemplate,
) -> Option<norishell_core_api::PluginUiAutoRefresh> {
    template.auto_refresh.clone()
}

fn retain_template_presentation(previous: &PluginUiTemplate, template: &mut PluginUiTemplate) {
    if template.route_paths.is_none() {
        template.route_paths = previous.route_paths.clone();
    }
    if template.icon.is_none() {
        template.icon = previous.icon.clone();
    }
    if template.on_open_action_id.is_none() {
        template.on_open_action_id = previous.on_open_action_id.clone();
    }
    if template.auto_refresh.is_none() {
        template.auto_refresh = previous.auto_refresh.clone();
    }
}

fn template_presentation_for_action<'a>(
    ui_templates: &'a BTreeMap<String, PluginUiTemplate>,
    scoped_templates: &'a BTreeMap<String, PluginUiTemplate>,
    contextual: bool,
    target_id: &str,
    context_handle: &str,
) -> Option<&'a PluginUiTemplate> {
    if contextual {
        scoped_templates
            .get(context_handle)
            .or_else(|| ui_templates.get(target_id))
    } else {
        ui_templates.get(target_id)
    }
}

fn ui_action_output_is_admissible(
    parsed: &ParsedPluginUiOutputs,
    action_kind: PluginUiActionKind,
    storage_available: bool,
) -> bool {
    parsed.panels.is_empty()
        && parsed.valid_initial_document_count()
        && parsed.navigation.is_empty()
        && parsed.pages.is_empty()
        && (parsed.storage_write.is_none()
            || (action_kind == PluginUiActionKind::Standard && storage_available))
        && match action_kind {
            PluginUiActionKind::Copy => parsed.clipboard_text.is_some(),
            PluginUiActionKind::Standard => parsed.clipboard_text.is_none(),
        }
}

fn auto_refresh_initial_output_is_admissible(parsed: &ParsedPluginUiOutputs) -> bool {
    parsed.panels.is_empty()
        && parsed.navigation.is_empty()
        && parsed.pages.is_empty()
        && parsed.clipboard_text.is_none()
        && parsed.storage_write.is_none()
        && parsed.host_mutation.is_none()
        && parsed.host_session.is_none()
        && parsed.host_dom_operations.is_none()
        && parsed.terminal_input_suggestion.is_none()
        && parsed.isolated_surface.is_none()
        && parsed.ssh_sync_request.is_none()
        && parsed.resource_operation.is_none()
        && parsed.api_call.is_none()
        && parsed.templates.len() <= 1
}

fn auto_refresh_callback_output_is_admissible(parsed: &ParsedPluginUiOutputs) -> bool {
    auto_refresh_initial_output_is_admissible(parsed)
        && parsed.remote_operation.is_none()
        && parsed.templates.len() == 1
}

fn auto_refresh_remote_operation_is_read_only(parsed: &ParsedPluginUiOutputs) -> bool {
    let Some(supplied) = parsed.remote_operation.as_ref() else {
        return true;
    };
    serde_json::from_str::<norishell_plugin_platform::operations::RemoteOperation>(
        &supplied.operation_json,
    )
    .ok()
    .and_then(|operation| norishell_plugin_platform::operations::plan(&operation).ok())
    .is_some_and(|plan| {
        plan.class == norishell_plugin_platform::operations::OperationClass::ReadOnly
            && plan.approval == norishell_plugin_platform::operations::ApprovalRequirement::None
    })
}

fn ssh_sync_browser_nodes_supported(document: &PluginUiDocument, ssh_sync_declared: bool) -> bool {
    document.nodes.iter().all(|node| {
        !matches!(
            node,
            norishell_core_api::PluginUiNode::SshSyncBrowser { .. }
        ) || (ssh_sync_declared)
    })
}

fn ssh_sync_browser_profile<'a>(
    document: &'a PluginUiDocument,
    node_id: &norishell_core_api::PluginUiNodeId,
) -> Option<&'a str> {
    document.nodes.iter().find_map(|node| match node {
        norishell_core_api::PluginUiNode::SshSyncBrowser {
            node_id: candidate,
            profile_id,
            ..
        } if candidate == node_id => Some(profile_id.as_str()),
        _ => None,
    })
}

fn ssh_sync_browser_empty_snapshot(
    state: PluginSshSyncBrowserState,
    profile_id: String,
) -> PluginSshSyncBrowserSnapshot {
    PluginSshSyncBrowserSnapshot {
        desktop_profile_count: 0,
        desktop_profile_rows_omitted: 0,
        desktop_profiles: Vec::new(),
        state,
        profile_id,
        cache_revision: WireSequence::new(0),
        host_count: 0,
        credential_count: 0,
        host_rows_omitted: 0,
        credential_rows_omitted: 0,
        remote_updated_at_unix_ms: None,
        verified_at_unix_ms: None,
        hosts: Vec::new(),
        credentials: Vec::new(),
    }
}

fn navigation_map(
    navigation: Vec<PluginNavigationContribution>,
) -> Result<BTreeMap<String, PluginNavigationContribution>, ()> {
    let mut mapped = BTreeMap::new();
    for item in navigation {
        if mapped
            .insert(item.navigation_id.as_str().to_owned(), item)
            .is_some()
        {
            return Err(());
        }
    }
    Ok(mapped)
}

fn page_map(
    pages: Vec<PluginPageContribution>,
) -> Result<BTreeMap<String, PluginPageContribution>, ()> {
    let mut mapped = BTreeMap::new();
    for page in pages {
        if mapped
            .insert(page.page_id.as_str().to_owned(), page)
            .is_some()
        {
            return Err(());
        }
    }
    Ok(mapped)
}

struct PluginMutationReservation {
    runtime: Arc<Mutex<PluginRuntimeState>>,
    plugin_id: String,
    operation_id: Uuid,
}

impl Drop for PluginMutationReservation {
    fn drop(&mut self) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime.mutation_reservations.get(&self.plugin_id) == Some(&self.operation_id) {
            runtime.mutation_reservations.remove(&self.plugin_id);
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeTerminalInputRequest {
    payload: String,
    append_enter: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginStorageSnapshot {
    revision: u64,
    value_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SshSyncRequestDisposition {
    Authorized,
    PermissionDenied,
    ProtocolViolation,
}

fn ssh_sync_request_disposition(
    action_kind: PluginUiActionKind,
    capability_granted: bool,
) -> SshSyncRequestDisposition {
    if action_kind != PluginUiActionKind::Standard {
        SshSyncRequestDisposition::ProtocolViolation
    } else if capability_granted {
        SshSyncRequestDisposition::Authorized
    } else {
        SshSyncRequestDisposition::PermissionDenied
    }
}

fn ssh_sync_request_profile_id(request: &norishell_core_api::PluginSshSyncRequest) -> &str {
    match request {
        norishell_core_api::PluginSshSyncRequest::Status { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::Authorize { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::Login { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::Register { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::VerifyEmail { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::CompleteMfa { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::Logout { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::Refresh { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::Sync { profile_id, .. }
        | norishell_core_api::PluginSshSyncRequest::ConfigureScope { profile_id }
        | norishell_core_api::PluginSshSyncRequest::ResetRemote { profile_id, .. } => profile_id,
    }
}

fn ssh_sync_result_payload(status: &norishell_core_api::PluginSshSyncStatus) -> serde_json::Value {
    serde_json::to_value(status).expect("SSH sync status is serializable")
}

fn plugin_host_payload(locale: &PluginLocale, mut payload: serde_json::Value) -> String {
    payload
        .as_object_mut()
        .expect("plugin host payloads are objects")
        .insert("locale".to_owned(), serde_json::json!(locale.as_str()));
    payload.to_string()
}

fn plugin_host_payload_with_settings(
    locale: &PluginLocale,
    mut payload: serde_json::Value,
    settings: Option<&serde_json::Value>,
) -> String {
    payload
        .as_object_mut()
        .expect("plugin host payloads are objects")
        .insert(
            "settings".to_owned(),
            settings.cloned().unwrap_or(serde_json::Value::Null),
        );
    plugin_host_payload(locale, payload)
}

fn plugin_visible_ui_fields(
    document: Option<&PluginUiDocument>,
    fields: &[PluginUiFieldValue],
) -> Vec<PluginUiFieldValue> {
    let password_field_ids = document
        .into_iter()
        .flat_map(|document| document.nodes.iter())
        .filter_map(|node| match node {
            norishell_core_api::PluginUiNode::TextField {
                field_id,
                field_kind: norishell_core_api::PluginUiFieldKind::Password,
                ..
            } => Some(field_id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    fields
        .iter()
        .cloned()
        .map(|mut field| {
            if password_field_ids.contains(field.field_id.as_str()) {
                field.value.clear();
            }
            field
        })
        .collect()
}

impl PluginService {
    fn reserve_plugin_mutation(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
    ) -> Option<PluginMutationReservation> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .mutation_reservations
            .contains_key(plugin_id.as_str())
        {
            return None;
        }
        runtime
            .mutation_reservations
            .insert(plugin_id.as_str().to_owned(), operation_id);
        Some(PluginMutationReservation {
            runtime: self.runtime.clone(),
            plugin_id: plugin_id.as_str().to_owned(),
            operation_id,
        })
    }

    fn commit_enable_instance(
        &self,
        plugin_id: &PluginId,
        operation_id: Uuid,
        instance: ActivePluginInstance,
    ) -> bool {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime.mutation_reservations.get(plugin_id.as_str()) != Some(&operation_id)
            || runtime.active_instances.contains_key(plugin_id.as_str())
        {
            return false;
        }
        runtime
            .active_instances
            .insert(plugin_id.as_str().to_owned(), instance);
        true
    }

    pub(crate) fn start(
        app_data_directory: impl AsRef<std::path::Path>,
        hosts: HostService,
        sessions: SshSessionService,
    ) -> Result<Self, String> {
        Self::start_with_dependencies(
            app_data_directory,
            hosts,
            sessions,
            PackageLimits::default(),
        )
    }

    pub(crate) fn start_with_sync(
        app_data_directory: impl AsRef<std::path::Path>,
        hosts: HostService,
        sessions: SshSessionService,
        ssh_sync: crate::ssh_sync_exchange::SshSyncExchangeBroker,
    ) -> Result<Self, String> {
        let mut service = Self::start(app_data_directory, hosts, sessions)?;
        service.ssh_sync = Some(ssh_sync);
        Ok(service)
    }

    pub(crate) fn with_operations(
        mut self,
        operations: crate::plugin_operations::PluginOperationsService,
    ) -> Self {
        self.operations = Some(operations);
        self
    }

    pub(crate) fn with_resources(
        mut self,
        resources: crate::plugin_resources::PluginResourceService,
    ) -> Self {
        self.resources = Some(resources);
        self
    }

    fn start_with_dependencies(
        app_data_directory: impl AsRef<Path>,
        hosts: HostService,
        sessions: SshSessionService,
        package_limits: PackageLimits,
    ) -> Result<Self, String> {
        let safe_mode_next_marker = app_data_directory
            .as_ref()
            .join("plugin-safe-mode-next-start");
        let safe_mode_active_marker = app_data_directory
            .as_ref()
            .join("plugin-safe-mode-active-startup");
        let safe_mode_active = std::env::var_os("NORISHELL_PLUGIN_SAFE_MODE")
            .is_some_and(|value| value == "1")
            || activate_safe_mode_marker(&safe_mode_next_marker, &safe_mode_active_marker)
                .map_err(|_| Uuid::new_v4().to_string())?;
        let local_import_root = app_data_directory.as_ref().join("plugin-imports");
        let locale_path = app_data_directory.as_ref().join("plugin-locale");
        let locale = fs::read_to_string(&locale_path)
            .ok()
            .and_then(|value| PluginLocale::parse(value.trim()).ok())
            .unwrap_or_else(|| PluginLocale::parse("zh-CN").expect("built-in plugin locale"));
        if local_import_root.exists() {
            fs::remove_dir_all(&local_import_root).map_err(|_| Uuid::new_v4().to_string())?;
        }
        fs::create_dir_all(&local_import_root).map_err(|_| Uuid::new_v4().to_string())?;
        let installer =
            PluginInstaller::new(app_data_directory.as_ref().join("plugins"), package_limits)
                .map_err(|_| Uuid::new_v4().to_string())?;
        let service = Self {
            operation_policies: crate::plugin_operation_policy::OperationPolicyService::new(
                hosts.clone(),
                &app_data_directory
                    .as_ref()
                    .join("plugin-policy")
                    .join("approval.key"),
            ),
            hosts,
            sessions,
            installer: Arc::new(installer),
            ssh_sync: None,
            operations: None,
            resources: None,
            api: crate::plugin_api::PluginApi::default(),
            api_sftp: Arc::new(Mutex::new(None)),
            protocol_slots: Arc::new(tokio::sync::Semaphore::new(32)),
            app_integrations: Arc::new(Mutex::new(
                app_integration::PluginAppIntegrationState::default(),
            )),
            protocol_terminal: Arc::new(Mutex::new(None)),
            protocol_sessions: Arc::new(Mutex::new(BTreeMap::new())),
            protocol_launch_gate: Arc::new(tokio::sync::Mutex::new(())),
            protocol_snapshot_revision: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            serial_candidates: Arc::new(Mutex::new(BTreeMap::new())),
            protocol_launches: Arc::new(Mutex::new(BTreeMap::new())),
            package_limits,
            local_import_root: Arc::new(local_import_root),
            app_handle: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            test_lifecycle: Arc::new(Mutex::new(None)),
            safe_mode_next_marker: Arc::new(safe_mode_next_marker),
            safe_mode_active_marker: Arc::new(safe_mode_active_marker),
            safe_mode_active,
            locale_path: Arc::new(locale_path),
            locale: Arc::new(Mutex::new(locale)),
            runtime: Arc::new(Mutex::new(PluginRuntimeState::default())),
        };
        service
            .reconcile_startup()
            .map_err(|_| Uuid::new_v4().to_string())?;
        Ok(service)
    }

    fn reconcile_startup(&self) -> Result<(), AppPersistenceError> {
        let operations = self.hosts.with_plugin_repository(|repository| {
            repository.list_reconcilable_plugin_operations()
        })?;
        for operation in operations {
            if operation.state == PluginOperationState::AwaitingCapabilities
                && operation.phase == PluginOperationPhase::AwaitingCapabilities
            {
                self.hosts.with_plugin_repository(|repository| {
                    repository.advance_plugin_operation(
                        &operation.operation_id,
                        operation.state_version,
                        PluginOperationState::Cancelled,
                        PluginOperationPhase::Completed,
                        Some("capability_rejected"),
                    )
                })?;
                continue;
            }
            match operation.kind {
                PluginOperationKind::CatalogRefresh => {
                    // Online catalogs are no longer supported. Finish legacy
                    // persisted rows without restoring network authority.
                    self.fail_operation_record(
                        operation,
                        PluginOperationPhase::Completed,
                        "install_conflict",
                    )?;
                }
                PluginOperationKind::Install | PluginOperationKind::Update => {
                    self.reconcile_install_operation(operation)?;
                }
                PluginOperationKind::Uninstall => {
                    self.reconcile_uninstall_operation(operation)?;
                }
                PluginOperationKind::Disable => {
                    self.fail_operation_record(
                        operation,
                        PluginOperationPhase::Completed,
                        "install_conflict",
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn enabled_plugins_for_startup(
        &self,
    ) -> Result<Vec<PluginInstalledRecord>, AppPersistenceError> {
        if self.safe_mode_active {
            return Ok(Vec::new());
        }
        self.hosts
            .with_plugin_repository(|repository| repository.list_plugin_installations())
            .map(|records| {
                records
                    .into_iter()
                    .filter(|installed| {
                        installed.state == PluginInstallState::Enabled
                            && matches!(
                                self.installer.active_package_kind(
                                    &installed.plugin_id,
                                    &installed.active_version,
                                    &installed.package_sha256,
                                ),
                                Ok(PluginPackageKind::Wasm)
                            )
                    })
                    .collect()
            })
    }

    pub(crate) fn attach_app_handle(&self, app: AppHandle) {
        *self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(app);
    }

    #[cfg(test)]
    pub(crate) fn attach_test_lifecycle(&self, lifecycle: LifecycleState) {
        *self
            .test_lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(lifecycle);
    }

    fn emit_ssh_sync_browser_invalidated(
        &self,
        plugin_id: Option<PluginId>,
        profile_id: Option<String>,
    ) {
        let app = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(app) = app {
            let _ = app.emit_to(
                "main",
                EVENT_PLUGIN_SSH_SYNC_BROWSER_INVALIDATED,
                PluginSshSyncBrowserInvalidated {
                    plugin_id,
                    profile_id,
                },
            );
        }
    }

    pub(crate) fn invalidate_all_ssh_sync_browser_snapshots(&self) {
        if let Some(ssh_sync) = self.ssh_sync.as_ref() {
            ssh_sync.invalidate_all_browser_snapshots();
        }
        self.emit_ssh_sync_browser_invalidated(None, None);
    }

    pub(crate) fn locale(&self) -> PluginLocale {
        self.locale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_locale(&self, locale: PluginLocale, request_id: RequestId) -> CoreResult<()> {
        let mut current = self
            .locale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fs::write(self.locale_path.as_ref(), locale.as_str()).map_err(|_| {
            Box::new(CoreApiError::safe_internal(
                request_id,
                Uuid::new_v4().to_string(),
            ))
        })?;
        *current = locale.clone();
        drop(current);
        for instance in self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .values_mut()
        {
            instance.locale = locale.clone();
        }
        Ok(())
    }

    pub(crate) fn complete_safe_mode_startup(&self) -> Result<(), String> {
        remove_regular_marker_if_present(self.safe_mode_active_marker.as_ref())
            .map_err(|_| Uuid::new_v4().to_string())
    }

    pub(crate) fn record_startup_restore_failure(
        &self,
        plugin_id: &PluginId,
        expected_state_version: WireSequence,
        error_code: &str,
    ) {
        let _ = self.hosts.with_plugin_repository(|repository| {
            let current = repository.get_plugin_installation(plugin_id)?;
            if current.state == PluginInstallState::Enabled
                && current.state_version == expected_state_version
            {
                let incompatible = matches!(
                    error_code,
                    "plugin.core_api_incompatible"
                        | "plugin.app_version_incompatible"
                        | "plugin.protocol_incompatible"
                );
                repository.set_plugin_install_state(
                    plugin_id,
                    expected_state_version,
                    if incompatible {
                        PluginInstallState::Incompatible
                    } else {
                        PluginInstallState::Crashed
                    },
                )?;
                repository.append_plugin_audit(
                    Some(plugin_id),
                    None,
                    "startup.restore",
                    "failed",
                    Some(
                        error_code
                            .strip_prefix("plugin.")
                            .filter(|code| parse_plugin_error_code(code).is_some())
                            .unwrap_or("runtime_rejected"),
                    ),
                )?;
            }
            Ok(())
        });
        self.emit_plugin_invalidated(plugin_id);
    }

    fn plugin_installation_fact(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Option<PluginInstalledRecord>, AppPersistenceError> {
        self.hosts.with_plugin_repository(|repository| {
            match repository.get_plugin_installation(plugin_id) {
                Ok(installed) => Ok(Some(installed)),
                Err(AppPersistenceError::NotFound) => Ok(None),
                Err(error) => Err(error),
            }
        })
    }

    fn set_plugin_state_convergent(
        &self,
        request_id: RequestId,
        current: &PluginInstalledRecord,
        target: PluginInstallState,
    ) -> CoreResult<PluginInstalledRecord> {
        let transition = || {
            self.hosts.with_plugin_repository(|repository| {
                repository.set_plugin_install_state(
                    &current.plugin_id,
                    current.state_version,
                    target,
                )
            })
        };
        match transition() {
            Ok(updated) => Ok(updated),
            Err(_) => {
                let expected_next = current.state_version.get().checked_add(1);
                let fact = self
                    .plugin_installation_fact(&current.plugin_id)
                    .map_err(|_| plugin_state_reconciliation_error(request_id.clone()))?;
                if let Some(updated) = fact.as_ref()
                    && updated.state == target
                    && Some(updated.state_version.get()) == expected_next
                {
                    return Ok(updated.clone());
                }
                if fact.as_ref() != Some(current) {
                    return Err(plugin_state_reconciliation_error(request_id));
                }
                match transition() {
                    Ok(updated) => Ok(updated),
                    Err(_) => {
                        let fact = self
                            .plugin_installation_fact(&current.plugin_id)
                            .map_err(|_| plugin_state_reconciliation_error(request_id.clone()))?;
                        if let Some(updated) = fact
                            && updated.state == target
                            && Some(updated.state_version.get()) == expected_next
                        {
                            Ok(updated)
                        } else {
                            Err(plugin_state_reconciliation_error(request_id))
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn replace_plugin_grants_convergent(
        &self,
        request_id: RequestId,
        installed: &PluginInstalledRecord,
        major_version: u64,
        expected_grant_version: Option<WireSequence>,
        current_grants: &[PluginCapabilityGrantRecord],
        decisions: &[(PluginCapability, bool)],
    ) -> CoreResult<Vec<PluginCapabilityGrantRecord>> {
        let binding = current_plugin_permission_binding(&installed.package_sha256);
        let replace = || {
            self.hosts.with_plugin_repository(|repository| {
                repository.replace_plugin_capability_grants(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major_version,
                    installed.state_version,
                    expected_grant_version,
                    &binding,
                    decisions,
                )
            })
        };
        let read_fact = || {
            self.hosts.with_plugin_repository(|repository| {
                let current = repository.get_plugin_installation(&installed.plugin_id)?;
                let grants = repository.list_plugin_capability_grants(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major_version,
                )?;
                Ok((current, grants))
            })
        };
        let next_grant_version = expected_grant_version
            .map(|version| version.get().checked_add(1))
            .unwrap_or(Some(1))
            .ok_or_else(|| plugin_grants_reconciliation_error(request_id.clone()))?;
        match replace() {
            Ok(grants) => Ok(grants),
            Err(_) => {
                let (current, grants) = read_fact()
                    .map_err(|_| plugin_grants_reconciliation_error(request_id.clone()))?;
                if current != *installed {
                    return Err(plugin_grants_reconciliation_error(request_id));
                }
                if capability_grant_records_match(&grants, decisions, next_grant_version) {
                    return Ok(grants);
                }
                if grants != current_grants {
                    return Err(plugin_grants_reconciliation_error(request_id));
                }
                match replace() {
                    Ok(grants) => Ok(grants),
                    Err(_) => {
                        let (current, grants) = read_fact()
                            .map_err(|_| plugin_grants_reconciliation_error(request_id.clone()))?;
                        if current == *installed
                            && capability_grant_records_match(
                                &grants,
                                decisions,
                                next_grant_version,
                            )
                        {
                            Ok(grants)
                        } else {
                            Err(plugin_grants_reconciliation_error(request_id))
                        }
                    }
                }
            }
        }
    }

    fn reconcile_install_operation(
        &self,
        operation: PluginOperationRecord,
    ) -> Result<(), AppPersistenceError> {
        let Some(plugin_id) = operation.plugin_id.as_ref() else {
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::Completed,
                "install_conflict",
            );
        };
        let Some(candidate_version) = operation.candidate_version.as_deref() else {
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::Completed,
                "install_conflict",
            );
        };
        let filesystem_active = match self.installer.read_active_version(plugin_id) {
            Ok(active) => active,
            Err(_) => {
                return self.fail_operation_record(
                    operation,
                    PluginOperationPhase::ReconcileRequired,
                    "install_conflict",
                );
            }
        };
        let database_active = self.plugin_installation_fact(plugin_id)?;
        if operation.expected_active_version.as_deref() == Some(candidate_version) {
            let filesystem_hash = self
                .installer
                .read_active_package_sha256(plugin_id)
                .ok()
                .flatten();
            if let Some(installed) = database_active
                .as_ref()
                .filter(|installed| installed.active_version == candidate_version)
            {
                if filesystem_hash.as_deref() == Some(installed.package_sha256.as_str()) {
                    let backup_exists = self
                        .installer
                        .replacement_backup_exists(plugin_id, &operation.operation_id)
                        .unwrap_or(false);
                    if !backup_exists
                        && operation.phase != PluginOperationPhase::DatabaseCommitted
                        && !(operation.phase == PluginOperationPhase::ReconcileRequired
                            && operation.state == PluginOperationState::Running)
                    {
                        if operation.phase == PluginOperationPhase::ReconcileRequired
                            && crate::plugin_oauth::discard_oauth_signer_transfer(
                                self.local_import_root
                                    .parent()
                                    .expect("import directory has app root"),
                                plugin_id.as_str(),
                                &operation.operation_id,
                            )
                            .is_err()
                        {
                            return self.fail_operation_record(
                                operation,
                                PluginOperationPhase::ReconcileRequired,
                                "install_conflict",
                            );
                        }
                        return self.fail_operation_record(
                            operation,
                            PluginOperationPhase::Completed,
                            "install_conflict",
                        );
                    }
                    // An interrupted operation may have committed SQLite but not
                    // removed the durable old-directory backup yet.
                    if self
                        .installer
                        .finalize_replacement(
                            plugin_id,
                            candidate_version,
                            &installed.package_sha256,
                            &operation.operation_id,
                        )
                        .is_ok()
                        && crate::plugin_oauth::finalize_oauth_signer_transfer(
                            self.local_import_root
                                .parent()
                                .expect("import directory has app root"),
                            plugin_id.as_str(),
                            &operation.operation_id,
                        )
                        .is_ok()
                    {
                        return self.complete_operation_record(operation);
                    }
                } else if self
                    .installer
                    .restore_replacement(
                        plugin_id,
                        candidate_version,
                        &installed.package_sha256,
                        filesystem_hash.as_deref().unwrap_or(&"0".repeat(64)),
                        &operation.operation_id,
                    )
                    .is_ok()
                    && crate::plugin_oauth::discard_oauth_signer_transfer(
                        self.local_import_root
                            .parent()
                            .expect("import directory has app root"),
                        plugin_id.as_str(),
                        &operation.operation_id,
                    )
                    .is_ok()
                {
                    return self.fail_operation_record(
                        operation,
                        PluginOperationPhase::Completed,
                        "install_conflict",
                    );
                }
            }
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::ReconcileRequired,
                "install_conflict",
            );
        }
        if filesystem_active.as_deref() == Some(candidate_version)
            && database_active
                .as_ref()
                .is_some_and(|installed| installed.active_version == candidate_version)
        {
            if crate::plugin_oauth::finalize_oauth_signer_transfer(
                self.local_import_root
                    .parent()
                    .expect("import directory has app root"),
                plugin_id.as_str(),
                &operation.operation_id,
            )
            .is_ok()
            {
                return self.complete_operation_record(operation);
            }
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::ReconcileRequired,
                "install_conflict",
            );
        }
        if filesystem_active.as_deref() == Some(candidate_version)
            && database_active.as_ref().is_none_or(|installed| {
                Some(installed.active_version.as_str())
                    == operation.expected_active_version.as_deref()
            })
        {
            if self
                .installer
                .restore_active_version(
                    plugin_id,
                    candidate_version,
                    operation.expected_active_version.as_deref(),
                    &operation.operation_id,
                )
                .is_ok()
                && crate::plugin_oauth::discard_oauth_signer_transfer(
                    self.local_import_root
                        .parent()
                        .expect("import directory has app root"),
                    plugin_id.as_str(),
                    &operation.operation_id,
                )
                .is_ok()
            {
                return self.fail_operation_record(
                    operation,
                    PluginOperationPhase::Completed,
                    "install_conflict",
                );
            }
        } else if filesystem_active.as_deref() == operation.expected_active_version.as_deref()
            && database_active.as_ref().is_none_or(|installed| {
                Some(installed.active_version.as_str())
                    == operation.expected_active_version.as_deref()
            })
            && crate::plugin_oauth::discard_oauth_signer_transfer(
                self.local_import_root
                    .parent()
                    .expect("import directory has app root"),
                plugin_id.as_str(),
                &operation.operation_id,
            )
            .is_ok()
        {
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::Completed,
                "install_conflict",
            );
        }
        if let Some(installed) = database_active {
            let _ = self.hosts.with_plugin_repository(|repository| {
                repository.set_plugin_install_state(
                    &installed.plugin_id,
                    installed.state_version,
                    PluginInstallState::Quarantined,
                )
            });
        }
        self.fail_operation_record(
            operation,
            PluginOperationPhase::ReconcileRequired,
            "install_conflict",
        )
    }

    fn reconcile_uninstall_operation(
        &self,
        operation: PluginOperationRecord,
    ) -> Result<(), AppPersistenceError> {
        let Some(plugin_id) = operation.plugin_id.as_ref() else {
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::Completed,
                "install_conflict",
            );
        };
        let Some(expected_version) = operation.expected_active_version.as_deref() else {
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::Completed,
                "install_conflict",
            );
        };
        let filesystem_active = match self.installer.read_active_version(plugin_id) {
            Ok(active) => active,
            Err(_) => {
                return self.fail_operation_record(
                    operation,
                    PluginOperationPhase::ReconcileRequired,
                    "install_conflict",
                );
            }
        };
        let database_active = self.plugin_installation_fact(plugin_id)?;
        if filesystem_active.is_none() && database_active.is_none() {
            if self
                .installer
                .finalize_uninstall(plugin_id, expected_version, &operation.operation_id)
                .is_err()
            {
                return self.fail_operation_record(
                    operation,
                    PluginOperationPhase::ReconcileRequired,
                    "install_conflict",
                );
            }
            return self.complete_operation_record(operation);
        }
        if filesystem_active.is_none()
            && database_active
                .as_ref()
                .is_some_and(|installed| installed.active_version == expected_version)
        {
            if self
                .installer
                .restore_uninstall(plugin_id, expected_version, &operation.operation_id)
                .is_err()
            {
                return self.fail_operation_record(
                    operation,
                    PluginOperationPhase::ReconcileRequired,
                    "install_conflict",
                );
            }
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::Completed,
                "install_conflict",
            );
        }
        if filesystem_active.as_deref() == operation.expected_active_version.as_deref()
            && database_active.is_some()
        {
            return self.fail_operation_record(
                operation,
                PluginOperationPhase::Completed,
                "install_conflict",
            );
        }
        if filesystem_active.as_deref() == Some(expected_version) && database_active.is_none() {
            if self
                .installer
                .prepare_uninstall(plugin_id, expected_version, &operation.operation_id)
                .and_then(|_| {
                    self.installer.finalize_uninstall(
                        plugin_id,
                        expected_version,
                        &operation.operation_id,
                    )
                })
                .is_err()
            {
                return self.fail_operation_record(
                    operation,
                    PluginOperationPhase::ReconcileRequired,
                    "install_conflict",
                );
            }
            return self.complete_operation_record(operation);
        }
        self.fail_operation_record(
            operation,
            PluginOperationPhase::ReconcileRequired,
            "install_conflict",
        )
    }

    fn complete_operation_record(
        &self,
        operation: PluginOperationRecord,
    ) -> Result<(), AppPersistenceError> {
        if operation.state == PluginOperationState::Succeeded
            && operation.phase == PluginOperationPhase::Completed
        {
            return Ok(());
        }
        self.hosts.with_plugin_repository(|repository| {
            repository.advance_plugin_operation(
                &operation.operation_id,
                operation.state_version,
                PluginOperationState::Succeeded,
                PluginOperationPhase::Completed,
                None,
            )
        })?;
        Ok(())
    }

    fn fail_operation_record(
        &self,
        operation: PluginOperationRecord,
        phase: PluginOperationPhase,
        error_code: &str,
    ) -> Result<(), AppPersistenceError> {
        if operation.state == PluginOperationState::Failed && operation.phase == phase {
            return Ok(());
        }
        self.hosts.with_plugin_repository(|repository| {
            repository.advance_plugin_operation(
                &operation.operation_id,
                operation.state_version,
                PluginOperationState::Failed,
                phase,
                Some(error_code),
            )
        })?;
        Ok(())
    }

    fn readiness(&self) -> PluginReadiness {
        PluginReadiness {
            ready: true,
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MAX_MINOR,
            safe_mode_active: self.safe_mode_active,
            safe_mode_next_start: self.safe_mode_next_marker.exists(),
        }
    }

    fn set_safe_mode_next_start(
        &self,
        enabled: bool,
        request_id: RequestId,
    ) -> CoreResult<PluginReadiness> {
        if enabled {
            if !self.safe_mode_next_marker.exists() {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt as _;
                    options.mode(0o600);
                }
                let mut file = options
                    .open(self.safe_mode_next_marker.as_ref())
                    .map_err(|_| plugin_validation_error(request_id.clone()))?;
                file.write_all(b"safe-mode\n")
                    .and_then(|()| file.sync_all())
                    .map_err(|_| plugin_validation_error(request_id.clone()))?;
            }
        } else if self.safe_mode_next_marker.exists() {
            remove_regular_marker_if_present(self.safe_mode_next_marker.as_ref())
                .map_err(|_| plugin_validation_error(request_id.clone()))?;
        }
        let _ = self.hosts.with_plugin_repository(|repository| {
            repository.append_plugin_audit(
                None,
                None,
                "safe_mode.next_start",
                if enabled { "enabled" } else { "disabled" },
                None,
            )
        });
        Ok(self.readiness())
    }

    fn require_ready(&self, request_id: RequestId) -> CoreResult<()> {
        let _ = request_id;
        Ok(())
    }

    fn prepare_local_package(
        &self,
        source_path: &Path,
        request_id: RequestId,
    ) -> CoreResult<PluginLocalPackagePreview> {
        if source_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("zip"))
        {
            return Err(platform_request_error(
                request_id,
                &PluginPlatformError::RejectedArchivePath,
            ));
        }
        let source = open_local_plugin_source(source_path)
            .map_err(|_| plugin_validation_error(request_id.clone()))?;
        let metadata = source
            .metadata()
            .map_err(|_| plugin_validation_error(request_id.clone()))?;
        if metadata.len() == 0 || metadata.len() > self.package_limits.max_archive_bytes {
            return Err(platform_request_error(
                request_id,
                &PluginPlatformError::PackageTooLarge,
            ));
        }
        let preparation_id = Uuid::new_v4().to_string();
        let private_path = self.local_import_root.join(format!("{preparation_id}.zip"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut target = options
            .open(&private_path)
            .map_err(|_| plugin_validation_error(request_id.clone()))?;
        let copied = std::io::copy(
            &mut source.take(self.package_limits.max_archive_bytes + 1),
            &mut target,
        )
        .map_err(|_| plugin_validation_error(request_id.clone()))?;
        if copied != metadata.len() || copied > self.package_limits.max_archive_bytes {
            let _ = fs::remove_file(&private_path);
            return Err(platform_request_error(
                request_id,
                &PluginPlatformError::PackageTooLarge,
            ));
        }
        target
            .sync_all()
            .map_err(|_| plugin_validation_error(request_id.clone()))?;
        drop(target);
        let inspected = match inspect_local_package(
            &private_path,
            &current_app_version(),
            std::env::consts::ARCH,
            self.package_limits,
        ) {
            Ok(inspected) => inspected,
            Err(error) => {
                let _ = fs::remove_file(&private_path);
                return Err(platform_request_error(request_id, &error));
            }
        };
        if !capabilities_match_protocol(
            &inspected.manifest.capabilities,
            inspected.manifest.protocol_minor,
        ) {
            let _ = fs::remove_file(&private_path);
            return Err(plugin_error(
                request_id,
                "plugin.protocol_incompatible",
                ErrorCategory::Incompatible,
                RetryStrategy::Never,
                "errors.plugin.protocolIncompatible",
                None,
            ));
        }
        let existing = match self.hosts.with_plugin_repository(|repository| {
            repository.get_plugin_installation(&inspected.manifest.plugin_id)
        }) {
            Ok(existing) => Some(existing),
            Err(AppPersistenceError::NotFound) => None,
            Err(error) => {
                let _ = fs::remove_file(&private_path);
                return Err(map_persistence_error(request_id, error));
            }
        };
        let package_sha256 = hex::encode(inspected.package_sha256);
        if existing.as_ref().is_some_and(|current| {
            !plugin_version_can_replace(
                &inspected.manifest.version,
                &package_sha256,
                &current.active_version,
                &current.package_sha256,
            )
        }) {
            let _ = fs::remove_file(&private_path);
            return Err(plugin_error(
                request_id,
                "plugin.install_conflict",
                ErrorCategory::Conflict,
                RetryStrategy::Never,
                "errors.plugin.installConflict",
                existing.as_ref().map(|value| value.state_version),
            ));
        }
        let prior_package_sha256 = match self.hosts.with_plugin_repository(|repository| {
            repository.plugin_installed_version_package_sha256(
                &inspected.manifest.plugin_id,
                &inspected.manifest.version,
            )
        }) {
            Ok(value) => value,
            Err(error) => {
                let _ = fs::remove_file(&private_path);
                return Err(map_persistence_error(request_id, error));
            }
        };
        let preview = PluginLocalPackagePreview {
            preparation_id: preparation_id.clone(),
            plugin_id: inspected.manifest.plugin_id.clone(),
            name: inspected.manifest.name.clone(),
            author: inspected.manifest.publisher.clone(),
            version: inspected.manifest.version.clone(),
            package_size: inspected.package_size,
            package_sha256,
            capabilities: inspected.manifest.capabilities.clone(),
            current_version: existing.as_ref().map(|value| value.active_version.clone()),
            current_state_version: existing.as_ref().map(|value| value.state_version),
            prior_package_sha256,
            retained_capability_grants: Vec::new(),
            approved_special_grants: Vec::new(),
            special_permission_expires_at_unix_ms: None,
            publisher_verified: false,
        };
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prepared_local_packages
            .insert(
                preparation_id,
                PreparedLocalPackage {
                    path: private_path,
                    inspected,
                    preview: preview.clone(),
                    permission_revision: 0,
                    approved_special_permissions: None,
                },
            );
        Ok(preview)
    }

    fn install_local_plugin(
        &self,
        request: PluginLocalInstallRequest,
    ) -> CoreResult<PluginOperationSummary> {
        let request_id = request.meta.request_id.clone();
        if request.idempotency_key.trim().is_empty()
            || request.preparation_id.len() > 80
            || request.expected_package_sha256.len() != 64
        {
            return Err(plugin_validation_error(request_id));
        }
        let prepared = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prepared_local_packages
            .remove(&request.preparation_id)
            .ok_or_else(|| plugin_validation_error(request_id.clone()))?;
        self.cancel_prepared_package(&request.preparation_id);
        let package_path = &prepared.path;
        let inspected = &prepared.inspected;
        let plugin_id = inspected.manifest.plugin_id.clone();
        let version = inspected.manifest.version.clone();
        let mutation_operation = Uuid::parse_str(request.operation_id.as_str())
            .map_err(|_| plugin_validation_error(request_id.clone()))?;
        let _mutation_reservation = self
            .reserve_plugin_mutation(&plugin_id, mutation_operation)
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        let existing = match self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_installation(&plugin_id))
        {
            Ok(existing) => Some(existing),
            Err(AppPersistenceError::NotFound) => None,
            Err(error) => return Err(map_persistence_error(request_id, error)),
        };
        let signer_fingerprint_sha256 = hex::encode(inspected.package_sha256);
        let publisher_key_base64 = String::new();
        let publisher_signature_base64 = String::new();
        let expected_state_version = existing.as_ref().map(|record| record.state_version);
        if expected_state_version != request.expected_state_version
            || request.expected_package_sha256 != hex::encode(inspected.package_sha256)
            || self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.plugin_installed_version_package_sha256(&plugin_id, &version)
                })
                .map_err(|error| map_persistence_error(request_id.clone(), error))?
                != prepared.preview.prior_package_sha256
            || existing.as_ref().is_some_and(|current| {
                !plugin_version_can_replace(
                    &version,
                    &request.expected_package_sha256,
                    &current.active_version,
                    &current.package_sha256,
                )
            })
        {
            return Err(plugin_conflict_error(request_id, expected_state_version));
        }
        let reconciled_settings = self.reconcile_package_settings(
            request_id.clone(),
            &plugin_id,
            inspected.settings.as_ref(),
        )?;
        // Re-import is the explicit local update action. A local package has
        // no verified publisher continuity, so a new artifact never inherits
        // grants or scoped approvals from the installed version.
        let retained_grants = Vec::new();
        let baseline = self.prepared_permission_baseline(&prepared, request_id.clone())?;
        let approval = prepared.approved_special_permissions.as_ref();
        if approval.is_some_and(|approval| !approval.valid_for(&prepared, &baseline)) {
            return Err(plugin_conflict_error(request_id, expected_state_version));
        }
        let Some(mut resolved_capability_grants) = resolve_install_grants(
            &inspected.manifest.capabilities,
            &request.capability_grants,
            &retained_grants,
        ) else {
            return Err(plugin_conflict_error(request_id, expected_state_version));
        };
        if let Some(approval) = approval {
            for (capability, granted) in &mut resolved_capability_grants {
                if special_plugin_capability(*capability) {
                    let approved = approval
                        .grants
                        .iter()
                        .find(|grant| grant.capability == *capability)
                        .ok_or_else(|| {
                            plugin_conflict_error(request_id.clone(), expected_state_version)
                        })?;
                    // Main may decline a prepared grant, but cannot expand it.
                    let requested = request
                        .capability_grants
                        .iter()
                        .find(|grant| grant.capability == *capability)
                        .is_some_and(|grant| grant.granted);
                    if requested && !approved.granted {
                        return Err(plugin_conflict_error(request_id, expected_state_version));
                    }
                    *granted = requested && approved.granted;
                }
            }
        } else if !newly_requested_special_capabilities_are_denied(
            &request.capability_grants,
            &retained_grants,
        ) {
            return Err(plugin_conflict_error(request_id, expected_state_version));
        }
        let approved_host_scopes = approval.map(|approval| {
            approval
                .host_scopes
                .iter()
                .filter(|(_, capability)| {
                    resolved_capability_grants
                        .iter()
                        .any(|(candidate, granted)| candidate == capability && *granted)
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        if existing
            .as_ref()
            .is_some_and(|record| record.state == PluginInstallState::Enabled)
            || self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active_instances
                .contains_key(plugin_id.as_str())
        {
            return Err(plugin_conflict_error(request_id, expected_state_version));
        }
        let package_sha256 = hex::encode(inspected.package_sha256);
        let fingerprint = local_install_request_fingerprint(
            &plugin_id,
            &version,
            &package_sha256,
            expected_state_version,
            &resolved_capability_grants
                .iter()
                .map(|(capability, granted)| PluginCapabilityGrant {
                    capability: *capability,
                    granted: *granted,
                })
                .collect::<Vec<_>>(),
            request_id.clone(),
        )?;
        let fingerprint =
            request_fingerprint(&(fingerprint, &approved_host_scopes), request_id.clone())?;
        match self.hosts.with_plugin_repository(|repository| {
            repository.get_plugin_operation(&request.operation_id)
        }) {
            Ok(operation) => {
                if operation.plugin_id.as_ref() != Some(&plugin_id)
                    || operation.idempotency_key != request.idempotency_key
                    || operation.request_fingerprint_sha256 != fingerprint
                    || operation.candidate_version.as_deref() != Some(version.as_str())
                {
                    return Err(plugin_conflict_error(request_id, expected_state_version));
                }
                if matches!(
                    operation.state,
                    PluginOperationState::Succeeded | PluginOperationState::Failed
                ) {
                    return Ok(operation_to_wire(operation));
                }
            }
            Err(AppPersistenceError::NotFound) => {}
            Err(error) => return Err(map_persistence_error(request_id, error)),
        }
        let kind = if existing.is_some() {
            PluginOperationKind::Update
        } else {
            PluginOperationKind::Install
        };
        let operation = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.begin_plugin_operation(
                    &request.operation_id,
                    Some(&plugin_id),
                    kind,
                    &request.idempotency_key,
                    &fingerprint,
                    Some(&version),
                    existing
                        .as_ref()
                        .map(|record| record.active_version.as_str()),
                )
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        if matches!(
            operation.state,
            PluginOperationState::Succeeded | PluginOperationState::Failed
        ) {
            return Ok(operation_to_wire(operation));
        }
        let operation = self.advance_operation(
            operation,
            PluginOperationState::Running,
            PluginOperationPhase::VerifyPackage,
            None,
            request_id.clone(),
        )?;
        let staged = match self
            .installer
            .stage(package_path, inspected, &request.operation_id)
        {
            Ok(staged) => staged,
            Err(error) => {
                return self.fail_operation(
                    operation,
                    PluginOperationPhase::Completed,
                    platform_error_code(&error),
                    request_id,
                );
            }
        };
        let operation = self.advance_operation(
            operation,
            PluginOperationState::Running,
            PluginOperationPhase::Staged,
            None,
            request_id.clone(),
        )?;
        let expected_active = existing
            .as_ref()
            .map(|record| record.active_version.as_str());
        let same_version_replacement = existing.as_ref().is_some_and(|record| {
            record.active_version == version && record.package_sha256 != package_sha256
        });
        let activation = match if same_version_replacement {
            self.installer.activate_replacement(
                staged,
                &existing
                    .as_ref()
                    .expect("replacement has existing package")
                    .package_sha256,
                &request.operation_id,
            )
        } else {
            self.installer
                .activate(staged, expected_active, &request.operation_id)
        } {
            Ok(activation) => activation,
            Err(error) => {
                return self.recover_or_require_reconciliation(
                    operation,
                    &plugin_id,
                    (&version, &package_sha256),
                    existing.as_ref().map(|record| {
                        (
                            record.active_version.as_str(),
                            record.package_sha256.as_str(),
                        )
                    }),
                    platform_error_code(&error),
                    request_id,
                );
            }
        };
        debug_assert_eq!(activation.active_version, version);
        if existing.is_some()
            && crate::plugin_oauth::record_oauth_signer_transfer(
                self.local_import_root
                    .parent()
                    .expect("import directory has app root"),
                plugin_id.as_str(),
                &existing
                    .as_ref()
                    .expect("replacement has existing package")
                    .package_sha256,
                &package_sha256,
                &request.operation_id,
            )
            .is_err()
        {
            return self.recover_or_require_reconciliation(
                operation,
                &plugin_id,
                (&version, &package_sha256),
                existing.as_ref().map(|record| {
                    (
                        record.active_version.as_str(),
                        record.package_sha256.as_str(),
                    )
                }),
                "install_conflict",
                request_id,
            );
        }
        let operation = match self.advance_operation(
            operation.clone(),
            PluginOperationState::Running,
            PluginOperationPhase::FilesystemActivated,
            None,
            request_id.clone(),
        ) {
            Ok(operation) => operation,
            Err(_) => {
                return self.recover_or_require_reconciliation(
                    operation,
                    &plugin_id,
                    (&version, &package_sha256),
                    existing.as_ref().map(|record| {
                        (
                            record.active_version.as_str(),
                            record.package_sha256.as_str(),
                        )
                    }),
                    "install_conflict",
                    request_id,
                );
            }
        };
        let now = unix_time_ms();
        let candidate = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: inspected.manifest.name.clone(),
            publisher: inspected.manifest.publisher.clone(),
            signer_fingerprint_sha256,
            active_version: version.clone(),
            package_sha256: package_sha256.clone(),
            capabilities: inspected.manifest.capabilities.clone(),
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(
                existing
                    .as_ref()
                    .map_or(1, |record| record.state_version.get().saturating_add(1)),
            ),
            installed_at_unix_ms: existing
                .as_ref()
                .map_or(now, |record| record.installed_at_unix_ms),
            updated_at_unix_ms: now,
        };
        let permission_binding = current_plugin_permission_binding(&package_sha256);
        let previous_binding = existing
            .as_ref()
            .map(|record| current_plugin_permission_binding(&record.package_sha256));
        let carried_host_scope_capabilities = Vec::new();
        let committed = self.hosts.with_plugin_repository(|repository| {
            repository.activate_plugin_installation_with_settings(
                expected_state_version,
                &candidate,
                inspected.package_size,
                &publisher_key_base64,
                &publisher_signature_base64,
                true,
                Some(PluginActivationPermissions {
                    protocol_major: u64::from(PLUGIN_PROTOCOL_MAJOR),
                    binding: &permission_binding,
                    previous_binding: previous_binding.as_ref(),
                    expected_previous_grant_state_version: baseline.grant_state_version(),
                    expected_previous_scope_state_version: baseline.scope_state_version(),
                    grants: &resolved_capability_grants,
                    carried_host_scope_capabilities: &carried_host_scope_capabilities,
                    approved_host_scopes: approved_host_scopes.as_deref(),
                }),
                reconciled_settings
                    .as_ref()
                    .map(|settings| settings.install()),
            )
        });
        let committed_record = match committed {
            Ok(record) => record,
            Err(error) => match self.plugin_installation_fact(&plugin_id) {
                Ok(Some(record))
                    if record == candidate
                        && self.package_settings_commit_matches(
                            &plugin_id,
                            &candidate.package_sha256,
                            reconciled_settings.as_ref(),
                        ) =>
                {
                    record
                }
                Ok(current) if current.as_ref() == existing.as_ref() => {
                    return self.recover_or_require_reconciliation(
                        operation,
                        &plugin_id,
                        (&version, &package_sha256),
                        existing.as_ref().map(|record| {
                            (
                                record.active_version.as_str(),
                                record.package_sha256.as_str(),
                            )
                        }),
                        persistence_error_code(&error),
                        request_id,
                    );
                }
                Ok(_) | Err(_) => {
                    return self.fail_operation(
                        operation,
                        PluginOperationPhase::ReconcileRequired,
                        persistence_error_code(&error),
                        request_id,
                    );
                }
            },
        };
        debug_assert_eq!(committed_record, candidate);
        self.publish_plugin_host_scope(&committed_record);
        let operation = self.advance_operation(
            operation,
            PluginOperationState::Running,
            PluginOperationPhase::DatabaseCommitted,
            None,
            request_id.clone(),
        )?;
        if same_version_replacement
            && self
                .installer
                .finalize_replacement(&plugin_id, &version, &package_sha256, &request.operation_id)
                .is_err()
        {
            return self
                .advance_operation(
                    operation,
                    PluginOperationState::Running,
                    PluginOperationPhase::ReconcileRequired,
                    Some("install_conflict"),
                    request_id,
                )
                .map(operation_to_wire);
        }
        if existing.is_some()
            && crate::plugin_oauth::finalize_oauth_signer_transfer(
                self.local_import_root
                    .parent()
                    .expect("import directory has app root"),
                plugin_id.as_str(),
                &request.operation_id,
            )
            .is_err()
        {
            return self
                .advance_operation(
                    operation,
                    PluginOperationState::Running,
                    PluginOperationPhase::ReconcileRequired,
                    Some("install_conflict"),
                    request_id,
                )
                .map(operation_to_wire);
        }
        self.advance_operation(
            operation,
            PluginOperationState::Succeeded,
            PluginOperationPhase::Completed,
            None,
            request_id,
        )
        .map(operation_to_wire)
    }

    fn advance_operation(
        &self,
        operation: PluginOperationRecord,
        state: PluginOperationState,
        phase: PluginOperationPhase,
        error_code: Option<&str>,
        request_id: RequestId,
    ) -> CoreResult<PluginOperationRecord> {
        self.hosts
            .with_plugin_repository(|repository| {
                repository.advance_plugin_operation(
                    &operation.operation_id,
                    operation.state_version,
                    state,
                    phase,
                    error_code,
                )
            })
            .map_err(|error| map_persistence_error(request_id, error))
    }

    fn fail_operation(
        &self,
        operation: PluginOperationRecord,
        phase: PluginOperationPhase,
        error_code: &str,
        request_id: RequestId,
    ) -> CoreResult<PluginOperationSummary> {
        self.advance_operation(
            operation,
            PluginOperationState::Failed,
            phase,
            Some(error_code),
            request_id,
        )
        .map(operation_to_wire)
    }

    fn recover_or_require_reconciliation(
        &self,
        operation: PluginOperationRecord,
        plugin_id: &PluginId,
        candidate: (&str, &str),
        previous: Option<(&str, &str)>,
        error_code: &str,
        request_id: RequestId,
    ) -> CoreResult<PluginOperationSummary> {
        let (candidate_version, candidate_hash) = candidate;
        let (previous_version, previous_hash) = previous
            .map(|(version, hash)| (Some(version), Some(hash)))
            .unwrap_or((None, None));
        if previous_version == Some(candidate_version) {
            if let Some(previous_hash) = previous_hash
                && self
                    .installer
                    .restore_replacement(
                        plugin_id,
                        candidate_version,
                        previous_hash,
                        candidate_hash,
                        &operation.operation_id,
                    )
                    .is_ok()
                && crate::plugin_oauth::discard_oauth_signer_transfer(
                    self.local_import_root
                        .parent()
                        .expect("import directory has app root"),
                    plugin_id.as_str(),
                    &operation.operation_id,
                )
                .is_ok()
            {
                return self.fail_operation(
                    operation,
                    PluginOperationPhase::Completed,
                    error_code,
                    request_id,
                );
            }
            return self.fail_operation(
                operation,
                PluginOperationPhase::ReconcileRequired,
                error_code,
                request_id,
            );
        }
        let active = self.installer.read_active_version(plugin_id);
        if active
            .as_ref()
            .is_ok_and(|active| active.as_deref() == previous_version)
            && crate::plugin_oauth::discard_oauth_signer_transfer(
                self.local_import_root
                    .parent()
                    .expect("import directory has app root"),
                plugin_id.as_str(),
                &operation.operation_id,
            )
            .is_ok()
        {
            return self.fail_operation(
                operation,
                PluginOperationPhase::Completed,
                error_code,
                request_id,
            );
        }
        if active
            .as_ref()
            .is_ok_and(|active| active.as_deref() == Some(candidate_version))
            && self
                .installer
                .restore_active_version(
                    plugin_id,
                    candidate_version,
                    previous_version,
                    &operation.operation_id,
                )
                .is_ok()
            && crate::plugin_oauth::discard_oauth_signer_transfer(
                self.local_import_root
                    .parent()
                    .expect("import directory has app root"),
                plugin_id.as_str(),
                &operation.operation_id,
            )
            .is_ok()
        {
            return self.fail_operation(
                operation,
                PluginOperationPhase::Completed,
                error_code,
                request_id,
            );
        }
        self.fail_operation(
            operation,
            PluginOperationPhase::ReconcileRequired,
            error_code,
            request_id,
        )
    }

    fn list_installed(&self, request_id: RequestId) -> CoreResult<Vec<InstalledPluginSummary>> {
        let records = self
            .hosts
            .with_plugin_repository(|repository| repository.list_plugin_installations())
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        records
            .into_iter()
            .map(|record| self.installed_summary(request_id.clone(), record))
            .collect()
    }

    fn list_themes(&self, request_id: RequestId) -> CoreResult<PluginThemeListResponse> {
        self.require_ready(request_id.clone())?;
        let records = self
            .hosts
            .with_plugin_repository(|repository| repository.list_plugin_installations())
            .map_err(|error| map_persistence_error(request_id, error))?;
        let mut themes = records
            .into_iter()
            .filter_map(|record| {
                let definition = self
                    .installer
                    .read_active_theme(
                        &record.plugin_id,
                        &record.active_version,
                        &record.package_sha256,
                    )
                    .ok()?;
                Some(ThemePackageEntry {
                    plugin_id: record.plugin_id,
                    package_hash: record.package_sha256,
                    version: record.active_version,
                    enabled: !self.safe_mode_active && record.state == PluginInstallState::Enabled,
                    definition,
                })
            })
            .collect::<Vec<_>>();
        themes.sort_by(|left, right| {
            left.plugin_id
                .cmp(&right.plugin_id)
                .then_with(|| left.definition.id.cmp(&right.definition.id))
        });
        themes.truncate(MAX_THEME_PACKAGE_ENTRIES);
        Ok(PluginThemeListResponse { themes })
    }

    fn list_contributions(
        &self,
        request_id: RequestId,
    ) -> CoreResult<Vec<PluginContributionPanel>> {
        self.require_ready(request_id.clone())?;
        let candidates = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .iter()
            .filter(|(_, instance)| {
                instance.can_publish_contributions() && !instance.contributions.is_empty()
            })
            .map(|(plugin_id, instance)| (plugin_id.clone(), instance.clone()))
            .collect::<Vec<_>>();
        Ok(candidates
            .into_iter()
            .flat_map(|(plugin_id, instance)| {
                let plugin_id = PluginId::parse(&plugin_id).ok()?;
                self.has_capability(
                    RequestId::new(),
                    &plugin_id,
                    &instance.signer_fingerprint_sha256,
                    PluginCapability::UiPanel,
                )
                .ok()?;
                Some(
                    instance
                        .contributions
                        .into_iter()
                        .filter(|(_, contribution)| !contribution.nodes.is_empty())
                        .map(|(slot, contribution)| PluginContributionPanel {
                            plugin_id: plugin_id.clone(),
                            plugin_name: instance.plugin_name.clone(),
                            signer_fingerprint_sha256: instance.signer_fingerprint_sha256.clone(),
                            package_sha256: instance.package_sha256.clone(),
                            instance_generation: instance.instance_generation,
                            state_version: instance.state_version,
                            contribution_revision: instance.contribution_revision,
                            slot,
                            nodes: contribution.nodes,
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect())
    }

    fn begin_contribution_action(
        &self,
        request: &PluginContributionInvokeRequest,
    ) -> CoreResult<ActivePluginInstance> {
        if !valid_action_id(&request.action_id) {
            return Err(plugin_validation_error(request.meta.request_id.clone()));
        }
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .mutation_reservations
            .contains_key(request.plugin_id.as_str())
        {
            return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
        }
        let Some(instance) = runtime.active_instances.get_mut(request.plugin_id.as_str()) else {
            return Err(plugin_not_found_error(request.meta.request_id.clone()));
        };
        if instance.signer_fingerprint_sha256 != request.signer_fingerprint_sha256
            || instance.package_sha256 != request.expected_package_sha256
            || instance.instance_generation != request.instance_generation
            || instance.state_version != request.expected_state_version
            || instance.contribution_revision != request.expected_contribution_revision
            || instance.contribution_action_in_flight
        {
            return Err(plugin_conflict_error(
                request.meta.request_id.clone(),
                Some(instance.contribution_revision),
            ));
        }
        if !instance
            .contributions
            .get(&request.slot)
            .is_some_and(|contribution| {
                contribution.nodes.iter().any(|node| {
                    matches!(
                        node,
                        PluginContributionNode::Action { action_id, .. }
                            if action_id == &request.action_id
                    )
                })
            })
        {
            return Err(plugin_validation_error(request.meta.request_id.clone()));
        }
        instance.contribution_action_in_flight = true;
        Ok(instance.clone())
    }

    fn commit_contribution_action(
        &self,
        request: &PluginContributionInvokeRequest,
        contributions: Vec<ParsedPluginContribution>,
    ) -> CoreResult<PluginContributionPanel> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .mutation_reservations
            .contains_key(request.plugin_id.as_str())
        {
            return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
        }
        let Some(instance) = runtime.active_instances.get_mut(request.plugin_id.as_str()) else {
            return Err(plugin_not_found_error(request.meta.request_id.clone()));
        };
        if instance.signer_fingerprint_sha256 != request.signer_fingerprint_sha256
            || instance.package_sha256 != request.expected_package_sha256
            || instance.instance_generation != request.instance_generation
            || instance.state_version != request.expected_state_version
            || instance.contribution_revision != request.expected_contribution_revision
            || !instance.contribution_action_in_flight
            || !instance
                .contributions
                .get(&request.slot)
                .is_some_and(|contribution| {
                    contribution.nodes.iter().any(|node| {
                        matches!(
                            node,
                            PluginContributionNode::Action { action_id, .. }
                                if action_id == &request.action_id
                        )
                    })
                })
        {
            return Err(plugin_conflict_error(
                request.meta.request_id.clone(),
                Some(instance.contribution_revision),
            ));
        }
        let mut next_contributions = contribution_map(contributions)
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        if next_contributions.len() != 1 {
            return Err(plugin_validation_error(request.meta.request_id.clone()));
        }
        let Some(contribution) = next_contributions.remove(&request.slot) else {
            return Err(plugin_validation_error(request.meta.request_id.clone()));
        };
        instance.contributions.insert(request.slot, contribution);
        instance.contribution_revision =
            WireSequence::new(instance.contribution_revision.get().saturating_add(1));
        instance.contribution_action_in_flight = false;
        Ok(PluginContributionPanel {
            plugin_id: request.plugin_id.clone(),
            plugin_name: instance.plugin_name.clone(),
            signer_fingerprint_sha256: instance.signer_fingerprint_sha256.clone(),
            package_sha256: instance.package_sha256.clone(),
            instance_generation: instance.instance_generation,
            state_version: instance.state_version,
            contribution_revision: instance.contribution_revision,
            slot: request.slot,
            nodes: instance
                .contributions
                .get(&request.slot)
                .map(|contribution| contribution.nodes.clone())
                .unwrap_or_default(),
        })
    }

    fn cancel_contribution_action(&self, request: &PluginContributionInvokeRequest) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(instance) = runtime.active_instances.get_mut(request.plugin_id.as_str())
            && instance.signer_fingerprint_sha256 == request.signer_fingerprint_sha256
            && instance.package_sha256 == request.expected_package_sha256
            && instance.instance_generation == request.instance_generation
            && instance.state_version == request.expected_state_version
            && instance.contribution_revision == request.expected_contribution_revision
        {
            instance.contribution_action_in_flight = false;
        }
    }

    async fn invoke_contribution(
        &self,
        request: PluginContributionInvokeRequest,
    ) -> CoreResult<PluginContributionPanel> {
        self.require_ready(request.meta.request_id.clone())?;
        let record = self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::UiPanel,
        )?;
        if record.package_sha256 != request.expected_package_sha256
            || record.state_version != request.expected_state_version
        {
            return Err(plugin_conflict_error(
                request.meta.request_id.clone(),
                Some(record.state_version),
            ));
        }
        let instance = self.begin_contribution_action(&request)?;
        let protocol_minor = instance.protocol_minor;
        let locale = instance.locale.clone();
        let host_request_id = Uuid::new_v4().to_string();
        let outputs = match self
            .execute_instance(
                request.meta.request_id.clone(),
                instance,
                PluginHostRequest {
                    protocol_major: PLUGIN_PROTOCOL_MAJOR,
                    protocol_minor,
                    request_id: host_request_id.clone(),
                    kind: PluginHostMessageKind::Invoke,
                    payload_json: plugin_host_payload(
                        &locale,
                        serde_json::json!({ "actionId": &request.action_id }),
                    ),
                },
            )
            .await
        {
            Ok(outputs) => outputs,
            Err(error) => {
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(error);
            }
        };
        let contributions = match parse_ui_panel_outputs(&host_request_id, outputs) {
            Ok(contributions) if !contributions.is_empty() => contributions,
            Ok(_) | Err(()) => {
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                let _ = self.stop_plugin(&request.plugin_id).await;
                return Err(plugin_runtime_error(
                    request.meta.request_id.clone(),
                    Some(PluginHostProcessError::Rejected),
                ));
            }
        };
        let current = self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::UiPanel,
        );
        match current {
            Ok(record)
                if record.package_sha256 == request.expected_package_sha256
                    && record.state_version == request.expected_state_version => {}
            Ok(record) => {
                self.cancel_contribution_action(&request);
                return Err(plugin_conflict_error(
                    request.meta.request_id.clone(),
                    Some(record.state_version),
                ));
            }
            Err(error) => {
                self.cancel_contribution_action(&request);
                return Err(error);
            }
        }
        self.commit_contribution_action(&request, contributions)
    }

    fn prepare_contribution_copy(
        &self,
        request: PluginContributionCopyRequest,
    ) -> CoreResult<PluginContributionCopyResponse> {
        if !valid_action_id(&request.copy_id) {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let record = self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::ClipboardWrite,
        )?;
        self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::UiPanel,
        )?;
        if record.package_sha256 != request.expected_package_sha256
            || record.state_version != request.expected_state_version
        {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(record.state_version),
            ));
        }
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .mutation_reservations
            .contains_key(request.plugin_id.as_str())
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let Some(instance) = runtime.active_instances.get(request.plugin_id.as_str()) else {
            return Err(plugin_not_found_error(request.meta.request_id));
        };
        if instance.signer_fingerprint_sha256 != request.signer_fingerprint_sha256
            || instance.package_sha256 != request.expected_package_sha256
            || instance.instance_generation != request.instance_generation
            || instance.state_version != request.expected_state_version
            || instance.contribution_revision != request.expected_contribution_revision
        {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(instance.contribution_revision),
            ));
        }
        let Some(contribution) = instance.contributions.get(&request.slot) else {
            return Err(plugin_validation_error(request.meta.request_id));
        };
        let node_exists = contribution.nodes.iter().any(|node| {
            matches!(node, PluginContributionNode::Copy { copy_id, .. } if copy_id == &request.copy_id)
        });
        let Some(text) = contribution.clipboard_values.get(&request.copy_id) else {
            return Err(plugin_validation_error(request.meta.request_id));
        };
        if !node_exists {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        Ok(PluginContributionCopyResponse { text: text.clone() })
    }

    fn installed_summary(
        &self,
        request_id: RequestId,
        record: PluginInstalledRecord,
    ) -> CoreResult<InstalledPluginSummary> {
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let grants = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_capability_grants(
                    &record.plugin_id,
                    &record.signer_fingerprint_sha256,
                    major,
                )
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        let has_settings = self.installed_has_settings(request_id, &record)?;
        let package_kind = self
            .installer
            .active_package_kind(
                &record.plugin_id,
                &record.active_version,
                &record.package_sha256,
            )
            .unwrap_or(PluginPackageKind::Wasm);
        let mut summary = installed_to_wire(record, grants, has_settings);
        summary.package_kind = package_kind;
        Ok(summary)
    }

    fn ensure_instance(
        &self,
        plugin_id: &PluginId,
        signer: &str,
        instance_generation: WireSequence,
        request_id: RequestId,
    ) -> CoreResult<()> {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .active_instances
            .get(plugin_id.as_str())
            .is_some_and(|instance| {
                instance.signer_fingerprint_sha256 == signer
                    && instance.instance_generation == instance_generation
            })
        {
            Ok(())
        } else {
            Err(plugin_error(
                request_id,
                "plugin.runtime_stale",
                ErrorCategory::Conflict,
                RetryStrategy::RefreshSnapshot,
                "errors.plugin.runtimeStale",
                None,
            ))
        }
    }

    fn has_capability(
        &self,
        request_id: RequestId,
        plugin_id: &PluginId,
        signer: &str,
        capability: PluginCapability,
    ) -> CoreResult<PluginInstalledRecord> {
        let record = self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_installation(plugin_id))
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        if record.state != PluginInstallState::Enabled
            || record.signer_fingerprint_sha256 != signer
            || !record.capabilities.contains(&capability)
        {
            return Err(plugin_permission_error(request_id));
        }
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let grants = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_capability_grants(plugin_id, signer, major)
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        if grants
            .iter()
            .any(|grant| grant.capability == capability && effective_plugin_grant(&record, grant))
        {
            Ok(record)
        } else {
            Err(plugin_permission_error(request_id))
        }
    }

    fn capability_granted_for_record(
        &self,
        request_id: RequestId,
        record: &PluginInstalledRecord,
        capability: PluginCapability,
    ) -> CoreResult<bool> {
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        self.hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_capability_grants(
                    &record.plugin_id,
                    &record.signer_fingerprint_sha256,
                    major,
                )
            })
            .map_err(|error| map_persistence_error(request_id, error))
            .map(|grants| {
                grants.iter().any(|grant| {
                    grant.capability == capability && effective_plugin_grant(record, grant)
                })
            })
    }

    fn capability_grant_epoch_for_record(
        &self,
        request_id: RequestId,
        record: &PluginInstalledRecord,
        capability: PluginCapability,
    ) -> CoreResult<WireSequence> {
        let grants = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_capability_grants(
                    &record.plugin_id,
                    &record.signer_fingerprint_sha256,
                    u64::from(PLUGIN_PROTOCOL_MAJOR),
                )
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        grants
            .iter()
            .find(|grant| grant.capability == capability && effective_plugin_grant(record, grant))
            .map(|grant| grant.state_version)
            .ok_or_else(|| plugin_permission_error(request_id))
    }

    fn plugin_storage_snapshot(
        &self,
        request_id: RequestId,
        installed: &PluginInstalledRecord,
    ) -> CoreResult<PluginStorageSnapshot> {
        let owner = PluginPrivateStorageOwner {
            plugin_id: installed.plugin_id.clone(),
            signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
        };
        self.hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_private_persistent_state(&owner, &|| true)
            })
            .map_err(|error| map_persistence_error(request_id, error))
            .map(|record| match record {
                Some(record) => PluginStorageSnapshot {
                    revision: record.revision,
                    value_json: record.value_json,
                },
                None => PluginStorageSnapshot {
                    revision: 0,
                    value_json: "{}".to_owned(),
                },
            })
    }

    fn active_instance(
        &self,
        request_id: RequestId,
        plugin_id: &PluginId,
        signer: &str,
        generation: WireSequence,
    ) -> CoreResult<ActivePluginInstance> {
        self.ensure_instance(plugin_id, signer, generation, request_id.clone())?;
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get(plugin_id.as_str())
            .cloned()
            .ok_or_else(|| plugin_not_found_error(request_id))
    }

    async fn execute_instance(
        &self,
        request_id: RequestId,
        instance: ActivePluginInstance,
        request: PluginHostRequest,
    ) -> CoreResult<Vec<norishell_core_api::PluginRuntimeOutput>> {
        let process = instance.process;
        tauri::async_runtime::spawn_blocking(move || {
            let mut process = process
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(host) = process.as_mut() else {
                return Err(PluginHostProcessError::Protocol);
            };
            let result = host.execute(request);
            if result.is_err() && host.shutdown().is_ok() {
                process.take();
            }
            result
        })
        .await
        .map_err(|_| plugin_runtime_error(request_id.clone(), None))?
        .map_err(|error| plugin_runtime_error(request_id, Some(error)))
    }

    async fn stop_plugin(&self, plugin_id: &PluginId) -> CoreResult<()> {
        if let Some(instance) = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get_mut(plugin_id.as_str())
        {
            instance.operation_authority_revoked = true;
            instance
                .api_authority
                .store(false, std::sync::atomic::Ordering::Release);
        }
        self.discard_terminal_channel_launches(plugin_id);
        let protocol_sessions_clean = self.stop_protocol_sessions(plugin_id, None).await;
        let tasks_clean = match self.task_service() {
            Ok(tasks) => tasks.stop_plugin(plugin_id, None).await.is_ok(),
            Err(_) => true,
        };
        self.emit_plugin_invalidated(plugin_id);
        let (operations_clean, resources_clean, api_resources_clean) = tokio::join!(
            async {
                match &self.operations {
                    Some(operations) => operations.stop_plugin(plugin_id).await,
                    None => true,
                }
            },
            async {
                match &self.resources {
                    Some(resources) => resources.stop_plugin(plugin_id).await.is_ok(),
                    None => true,
                }
            },
            async { self.api.resources.stop_plugin(plugin_id).await.is_ok() }
        );
        self.api.blobs.remove_plugin(plugin_id);
        self.api.data_states.remove_plugin(plugin_id);
        // Every owner below still needs a bounded cleanup attempt when an earlier owner fails.
        // The failing owner retains its own record for a later reconciliation retry.
        self.emit_ssh_sync_browser_invalidated(Some(plugin_id.clone()), None);
        self.close_isolated_surfaces(plugin_id);
        if let Some(ssh_sync) = self.ssh_sync.as_ref() {
            ssh_sync.stop_plugin(plugin_id.as_str()).await;
        }
        let (instance, observers) = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let instance = runtime.active_instances.get(plugin_id.as_str()).cloned();
            runtime
                .pending_inputs
                .retain(|_, pending| pending.proposal.plugin_id != *plugin_id);
            clear_observer_reservations_for_plugin(&mut runtime, plugin_id);
            runtime
                .host_handles
                .retain(|_, handle| handle.plugin_id != *plugin_id);
            runtime
                .pending_host_approvals
                .retain(|_, pending| pending.summary.plugin_id != *plugin_id);
            runtime
                .approved_host_sessions
                .retain(|_, pending| pending.plugin_id != *plugin_id);
            runtime.pending_special_permissions.retain(|_, pending| {
                pending.snapshot.plugin_id != *plugin_id
                    || matches!(
                        pending.snapshot.target,
                        PluginSpecialPermissionTarget::PreparedPackage { .. }
                    )
            });
            let observer_ids = runtime
                .observers
                .iter()
                .filter(|(_, binding)| binding.plugin_id == *plugin_id)
                .map(|(observer_id, _)| (observer_id.clone(), Uuid::parse_str(observer_id).ok()))
                .collect::<Vec<_>>();
            (instance, observer_ids)
        };
        let mut observers_clean = true;
        for (observer_key, observer_id) in observers {
            let Some(observer_id) = observer_id else {
                observers_clean = false;
                continue;
            };
            if self
                .sessions
                .detach_plugin_observer(RequestId::new(), observer_id)
                .await
                .is_ok()
            {
                self.runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observers
                    .remove(&observer_key);
            } else {
                observers_clean = false;
            }
        }
        let host_clean = if let Some(instance) = instance {
            self.stop_plugin_host_generation(plugin_id, &instance).await
        } else {
            true
        };
        self.emit_plugin_invalidated(plugin_id);
        if let Some(code) = plugin_cleanup_failure_code(
            operations_clean,
            resources_clean,
            api_resources_clean && protocol_sessions_clean && tasks_clean,
            observers_clean,
            host_clean,
        ) {
            return Err(plugin_cleanup_reconciliation_error(RequestId::new(), code));
        }
        Ok(())
    }

    async fn stop_plugin_host_generation(
        &self,
        plugin_id: &PluginId,
        instance: &ActivePluginInstance,
    ) -> bool {
        let generation = instance.instance_generation;
        let process = instance.process.clone();
        let failed_process = process.clone();
        let clean = tauri::async_runtime::spawn_blocking(move || {
            let mut process = process
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(host) = process.as_mut() {
                let result = host.shutdown();
                if result.is_ok() {
                    process.take();
                }
                result
            } else {
                Ok(())
            }
        })
        .await
        .is_ok_and(|result| result.is_ok());
        if clean {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if runtime
                .active_instances
                .get(plugin_id.as_str())
                .is_some_and(|current| {
                    current.instance_generation == generation
                        && Arc::ptr_eq(&current.process, &failed_process)
                })
            {
                runtime.active_instances.remove(plugin_id.as_str());
            }
        }
        clean
    }

    async fn delete_plugin_ssh_sync_data(
        &self,
        request_id: RequestId,
        plugin_id: &PluginId,
    ) -> CoreResult<()> {
        let Some(ssh_sync) = self.ssh_sync.as_ref() else {
            return Ok(());
        };
        ssh_sync
            .delete_plugin_data(plugin_id.as_str())
            .await
            .map_err(|_| plugin_sensitive_data_delete_error(request_id))
    }

    pub(crate) async fn shutdown_all(&self) -> CoreResult<()> {
        let mut plugin_ids = {
            let runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            runtime
                .active_instances
                .keys()
                .filter_map(|value| PluginId::parse(value).ok())
                .chain(
                    runtime
                        .observers
                        .values()
                        .map(|binding| binding.plugin_id.clone()),
                )
                .collect::<Vec<_>>()
        };
        if let Some(operations) = &self.operations {
            plugin_ids.extend(operations.active_plugin_ids());
            plugin_ids.sort();
            plugin_ids.dedup();
        }
        if let Some(resources) = &self.resources {
            plugin_ids.extend(resources.active_plugin_ids());
        }
        plugin_ids.extend(self.api.resources.plugin_ids());
        if let Ok(tasks) = self.task_service() {
            plugin_ids.extend(tasks.active_plugin_ids());
        }
        if let Ok(actor) = self.protocol_actor() {
            plugin_ids.extend(
                actor
                    .snapshot()
                    .await
                    .into_iter()
                    .filter_map(|session| PluginId::parse(session.provider.plugin_id).ok()),
            );
        }
        plugin_ids.sort();
        plugin_ids.dedup();
        let mut first_error = None;
        for plugin_id in plugin_ids {
            if let Err(error) = self.stop_plugin(&plugin_id).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Ok(actor) = self.protocol_actor()
            && actor.shutdown_all().await.is_err()
            && first_error.is_none()
        {
            first_error = Some(plugin_cleanup_reconciliation_error(
                RequestId::new(),
                "api_resources_cleanup_incomplete",
            ));
        }
        if let Ok(tasks) = self.task_service()
            && tasks.shutdown_all().await.is_err()
            && first_error.is_none()
        {
            first_error = Some(plugin_cleanup_reconciliation_error(
                RequestId::new(),
                "api_resources_cleanup_incomplete",
            ));
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(crate) fn exit_blockers(&self) -> Vec<ExitBlocker> {
        let mut blockers: Vec<_> = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .iter()
            .filter_map(|(plugin_id, instance)| {
                let process_id = instance
                    .process
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()?
                    .process_id();
                Some(ExitBlocker::PluginHost {
                    plugin_id: PluginId::parse(plugin_id).ok()?,
                    process_id,
                })
            })
            .collect();
        blockers.extend(self.api.resources.exit_blockers());
        if let Ok(tasks) = self.task_service() {
            blockers.extend(tasks.exit_blockers());
        }
        if let Ok(actor) = self.protocol_actor() {
            let contexts = self
                .protocol_sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            blockers.extend(actor.exit_blockers().into_iter().filter_map(|id| {
                contexts
                    .get(&id)
                    .map(|context| ExitBlocker::PluginResource {
                        plugin_id: context.profile.plugin_id.clone(),
                        resource_kind: "terminal".to_owned(),
                        resource_id: id.to_string(),
                    })
            }));
        }
        blockers
    }

    fn record_runtime_failure(&self, plugin_id: &PluginId, generation: WireSequence) {
        let failed_instance = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(instance) = runtime
                .active_instances
                .get_mut(plugin_id.as_str())
                .filter(|instance| instance.instance_generation == generation)
            else {
                return;
            };
            if instance.operation_authority_revoked {
                return;
            }
            instance.operation_authority_revoked = true;
            instance.contribution_action_in_flight = false;
            instance
                .api_authority
                .store(false, std::sync::atomic::Ordering::Release);
            instance.clone()
        };
        if let Some(operations) = &self.operations {
            operations.cancel_plugin_generation(plugin_id, generation);
        }
        self.discard_terminal_channel_launches(plugin_id);
        if let Some(resources) = self.resources.clone() {
            let plugin_id = plugin_id.clone();
            tauri::async_runtime::spawn(async move {
                let _ = resources
                    .stop_plugin_generation(&plugin_id, generation)
                    .await;
            });
        }
        let protocol_service = self.clone();
        let protocol_plugin = plugin_id.clone();
        tauri::async_runtime::spawn(async move {
            let _ = protocol_service
                .stop_protocol_sessions(&protocol_plugin, Some(generation))
                .await;
            if let Ok(tasks) = protocol_service.task_service() {
                let _ = tasks.stop_plugin(&protocol_plugin, Some(generation)).await;
            }
        });
        let api = self.api.clone();
        let plugin_id_for_api_cleanup = plugin_id.clone();
        tauri::async_runtime::spawn(async move {
            let _ = api
                .resources
                .stop_plugin_generation(&plugin_id_for_api_cleanup, generation)
                .await;
        });
        self.emit_plugin_invalidated(plugin_id);
        if let Some(ssh_sync) = self.ssh_sync.as_ref() {
            ssh_sync.invalidate_browser_plugin(plugin_id.as_str());
        }
        self.emit_ssh_sync_browser_invalidated(Some(plugin_id.clone()), None);
        self.close_isolated_surfaces(plugin_id);
        let previous_crash = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(instance) = runtime.active_instances.get_mut(plugin_id.as_str()) else {
                return;
            };
            if instance.instance_generation != generation {
                return;
            }
            let previous_crash = instance.previously_crashed;
            instance.contributions.clear();
            runtime
                .pending_inputs
                .retain(|_, pending| pending.proposal.plugin_id != *plugin_id);
            clear_observer_reservations_for_plugin(&mut runtime, plugin_id);
            runtime
                .host_handles
                .retain(|_, handle| handle.plugin_id != *plugin_id);
            runtime
                .pending_host_approvals
                .retain(|_, pending| pending.summary.plugin_id != *plugin_id);
            runtime
                .approved_host_sessions
                .retain(|_, pending| pending.plugin_id != *plugin_id);
            runtime.pending_special_permissions.retain(|_, pending| {
                pending.snapshot.plugin_id != *plugin_id
                    || matches!(
                        pending.snapshot.target,
                        PluginSpecialPermissionTarget::PreparedPackage { .. }
                    )
            });
            runtime
                .observers
                .retain(|_, binding| binding.plugin_id != *plugin_id);
            previous_crash
        };
        if let Ok(record) = self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_installation(plugin_id))
            && record.state == PluginInstallState::Enabled
            && record.state_version == failed_instance.state_version
            && record.package_sha256 == failed_instance.package_sha256
        {
            let _ = self.hosts.with_plugin_repository(|repository| {
                repository.set_plugin_install_state(
                    plugin_id,
                    record.state_version,
                    if previous_crash {
                        PluginInstallState::Quarantined
                    } else {
                        PluginInstallState::Crashed
                    },
                )
            });
        }

        // Persist the old failure before cleanup can remove its runtime record.
        // The captured process and generation cannot retarget a later enable.
        let cleanup_service = self.clone();
        let cleanup_plugin = plugin_id.clone();
        tauri::async_runtime::spawn(async move {
            cleanup_service
                .stop_plugin_host_generation(&cleanup_plugin, &failed_instance)
                .await;
        });
        self.emit_plugin_invalidated(plugin_id);
    }
}

impl PluginService {
    fn open_target_context(
        &self,
        request: PluginTargetContextOpenRequest,
    ) -> CoreResult<PluginExtensionTargetContext> {
        self.require_ready(request.meta.request_id.clone())?;
        let Some(definition) = plugin_extension_registry::find(&request.target_id) else {
            return Err(plugin_validation_error(request.meta.request_id));
        };
        if request.target_instance_key.is_empty()
            || request.target_instance_key.len() > 240
            || request.target_instance_key.chars().any(char::is_control)
            || (!definition.contextual && request.target_instance_key != "global")
            || request.display_label.as_ref().is_some_and(|label| {
                label.trim().is_empty()
                    || label.len() > 160
                    || label.chars().any(|character| {
                        character.is_control()
                            || matches!(
                                character,
                                '\u{061C}'
                                    | '\u{200E}'
                                    | '\u{200F}'
                                    | '\u{202A}'..='\u{202E}'
                                    | '\u{2066}'..='\u{2069}'
                            )
                    })
            })
        {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let identity_key = hex::encode(Sha256::digest(
            format!(
                "{}\0{}",
                request.target_id.as_str(),
                request.target_instance_key
            )
            .as_bytes(),
        ));
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(handle) = runtime.target_context_identity.get(&identity_key)
            && let Some(active) = runtime.target_contexts.get(handle)
        {
            return Ok(active.context.clone());
        }
        if runtime.target_contexts.len() >= PLUGIN_TARGET_CONTEXT_LIMIT {
            return Err(plugin_error(
                request.meta.request_id,
                "plugin.target_context_limit_reached",
                ErrorCategory::Unavailable,
                RetryStrategy::AfterMilliseconds(1_000),
                "errors.plugin.targetContextLimitReached",
                None,
            ));
        }
        let context_handle = PluginTargetContextHandle::parse(Uuid::new_v4().to_string())
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        let context = PluginExtensionTargetContext {
            target_id: request.target_id,
            surface_kind: definition.surface_kind,
            context_handle: context_handle.clone(),
            target_revision: WireSequence::new(1),
            display_label: request.display_label,
        };
        runtime
            .target_context_identity
            .insert(identity_key.clone(), context_handle.as_str().to_owned());
        runtime.target_contexts.insert(
            context_handle.as_str().to_owned(),
            ActivePluginTargetContext {
                context: context.clone(),
                identity_key,
                instance_key: request.target_instance_key,
            },
        );
        Ok(context)
    }

    fn close_target_context(&self, request: PluginTargetContextCloseRequest) -> CoreResult<()> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(active) = runtime.target_contexts.get(request.context_handle.as_str()) else {
            return Ok(());
        };
        if active.context.target_revision != request.expected_target_revision {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(active.context.target_revision),
            ));
        }
        let identity_key = active.identity_key.clone();
        runtime
            .target_contexts
            .remove(request.context_handle.as_str());
        runtime
            .host_handles
            .retain(|_, handle| handle.context_handle != request.context_handle);
        for instance in runtime.active_instances.values_mut() {
            instance
                .scoped_templates
                .remove(request.context_handle.as_str());
            instance
                .scoped_states
                .remove(request.context_handle.as_str());
        }
        if runtime.target_context_identity.get(&identity_key)
            == Some(&request.context_handle.as_str().to_owned())
        {
            runtime.target_context_identity.remove(&identity_key);
        }
        Ok(())
    }

    fn canonical_target_context(
        &self,
        request_id: RequestId,
        supplied: &PluginExtensionTargetContext,
    ) -> CoreResult<PluginExtensionTargetContext> {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(active) = runtime
            .target_contexts
            .get(supplied.context_handle.as_str())
        else {
            return Err(plugin_conflict_error(request_id, None));
        };
        if active.context.target_id != supplied.target_id
            || active.context.surface_kind != supplied.surface_kind
            || active.context.target_revision != supplied.target_revision
        {
            return Err(plugin_conflict_error(
                request_id,
                Some(active.context.target_revision),
            ));
        }
        Ok(active.context.clone())
    }

    fn list_ui_contributions(
        &self,
        request: PluginUiContributionListRequest,
    ) -> CoreResult<Vec<PluginUiContribution>> {
        self.require_ready(request.meta.request_id.clone())?;
        let target =
            self.canonical_target_context(request.meta.request_id.clone(), &request.target)?;
        let definition = plugin_extension_registry::find(&target.target_id)
            .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
        if target.target_id.as_str() == "app.page" {
            let binding = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .target_contexts
                .get(target.context_handle.as_str())
                .and_then(|active| active.instance_key.split_once('|'))
                .map(|(plugin_id, page_id)| (plugin_id.to_owned(), page_id.to_owned()))
                .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
            let plugin_id = PluginId::parse(binding.0)
                .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
            let instance = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active_instances
                .get(plugin_id.as_str())
                .cloned()
                .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
            if !instance.can_publish_contributions() {
                return Ok(Vec::new());
            }
            let page = instance
                .pages
                .get(&binding.1)
                .cloned()
                .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
            let installed = self.has_capability(
                request.meta.request_id.clone(),
                &plugin_id,
                &instance.signer_fingerprint_sha256,
                PluginCapability::UiPage,
            )?;
            if !self.settings_target_visible(
                request.meta.request_id,
                &installed,
                instance.settings_revision,
                target.target_id.as_str(),
            )? {
                return Ok(Vec::new());
            }
            return Ok(vec![PluginUiContribution {
                plugin_id,
                plugin_name: instance.plugin_name,
                signer_fingerprint_sha256: instance.signer_fingerprint_sha256,
                package_sha256: instance.package_sha256,
                instance_generation: instance.instance_generation,
                state_version: instance.state_version,
                contribution_revision: instance.contribution_revision,
                target,
                on_open_action_id: page.on_open_action_id,
                auto_refresh: None,
                route_paths: None,
                icon: page.icon,
                document: page.document,
            }]);
        }
        let candidates = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .iter()
            .filter_map(|(plugin_id, instance)| {
                instance
                    .can_publish_contributions()
                    .then(|| {
                        instance
                            .scoped_templates
                            .get(target.context_handle.as_str())
                            .or_else(|| instance.ui_templates.get(target.target_id.as_str()))
                            .map(|template| (plugin_id.clone(), instance.clone(), template.clone()))
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        let mut contributions = Vec::new();
        for (plugin_id, instance, template) in candidates {
            let plugin_id = PluginId::parse(plugin_id)
                .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
            let installed = self.has_capability(
                request.meta.request_id.clone(),
                &plugin_id,
                &instance.signer_fingerprint_sha256,
                definition.required_capability,
            )?;
            if !self.settings_target_visible(
                request.meta.request_id.clone(),
                &installed,
                instance.settings_revision,
                target.target_id.as_str(),
            )? {
                continue;
            }
            contributions.push(PluginUiContribution {
                plugin_id,
                plugin_name: instance.plugin_name,
                signer_fingerprint_sha256: instance.signer_fingerprint_sha256,
                package_sha256: instance.package_sha256,
                instance_generation: instance.instance_generation,
                state_version: instance.state_version,
                contribution_revision: instance.contribution_revision,
                target: target.clone(),
                on_open_action_id: template_on_open_action_id(&template),
                auto_refresh: template_auto_refresh(&template),
                route_paths: template.route_paths,
                icon: template.icon,
                document: template.document,
            });
        }
        Ok(contributions)
    }

    fn read_ssh_sync_browser(
        &self,
        request: PluginSshSyncBrowserReadRequest,
    ) -> CoreResult<PluginSshSyncBrowserSnapshot> {
        self.require_ready(request.meta.request_id.clone())?;
        if request.target_id.as_str() != "app.page" {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let supplied_target = PluginExtensionTargetContext {
            target_id: request.target_id.clone(),
            surface_kind: norishell_core_api::PluginExtensionSurfaceKind::Page,
            context_handle: request.context_handle.clone(),
            target_revision: request.expected_target_revision,
            display_label: None,
        };
        let target =
            self.canonical_target_context(request.meta.request_id.clone(), &supplied_target)?;
        if target.surface_kind != norishell_core_api::PluginExtensionSurfaceKind::Page {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let profile_id = {
            let runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if runtime
                .mutation_reservations
                .contains_key(request.plugin_id.as_str())
            {
                return Err(plugin_conflict_error(request.meta.request_id, None));
            }
            let page_id = runtime
                .target_contexts
                .get(request.context_handle.as_str())
                .and_then(|active| active.instance_key.split_once('|'))
                .filter(|(plugin_id, _)| *plugin_id == request.plugin_id.as_str())
                .map(|(_, page_id)| page_id)
                .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
            let instance = runtime
                .active_instances
                .get(request.plugin_id.as_str())
                .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
            if instance.signer_fingerprint_sha256 != request.signer_fingerprint_sha256
                || instance.package_sha256 != request.expected_package_sha256
                || instance.instance_generation != request.instance_generation
                || instance.state_version != request.expected_state_version
                || instance.contribution_revision != request.expected_contribution_revision
            {
                return Err(plugin_conflict_error(
                    request.meta.request_id,
                    Some(instance.contribution_revision),
                ));
            }
            let page = instance
                .pages
                .get(page_id)
                .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
            ssh_sync_browser_profile(&page.document, &request.node_id)
                .map(str::to_owned)
                .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?
        };
        let installed = self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::UiPage,
        )?;
        if installed.package_sha256 != request.expected_package_sha256
            || installed.state_version != request.expected_state_version
            || !installed.capabilities.contains(&PluginCapability::SshSync)
        {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(installed.state_version),
            ));
        }
        let grants = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_capability_grants(
                    &request.plugin_id,
                    &request.signer_fingerprint_sha256,
                    u64::from(PLUGIN_PROTOCOL_MAJOR),
                )
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let authorization_epoch = grants.iter().find_map(|grant| {
            (grant.capability == PluginCapability::SshSync
                && effective_plugin_grant(&installed, grant))
            .then_some(grant.state_version)
        });
        let Some(authorization_epoch) = authorization_epoch else {
            if let Some(ssh_sync) = self.ssh_sync.as_ref() {
                ssh_sync.invalidate_browser_profile(request.plugin_id.as_str(), &profile_id);
            }
            return Ok(ssh_sync_browser_empty_snapshot(
                PluginSshSyncBrowserState::PermissionDenied,
                profile_id,
            ));
        };
        let Some(ssh_sync) = self.ssh_sync.as_ref() else {
            return Ok(ssh_sync_browser_empty_snapshot(
                PluginSshSyncBrowserState::Failed,
                profile_id,
            ));
        };
        let invocation = crate::ssh_sync_exchange::SshSyncInvocationBinding {
            package_sha256: request.expected_package_sha256.clone(),
            instance_generation: request.instance_generation,
            authorization_epoch,
        };
        let service = self.clone();
        let fence_request = request.clone();
        let fence_profile_id = profile_id.clone();
        let fence = move || {
            service.ssh_sync_browser_read_fence_current(
                &fence_request,
                &fence_profile_id,
                authorization_epoch,
            )
        };
        Ok(ssh_sync.browser_snapshot(
            request.plugin_id.as_str(),
            &request.signer_fingerprint_sha256,
            &profile_id,
            &invocation,
            &fence,
        ))
    }

    fn ssh_sync_browser_read_fence_current(
        &self,
        request: &PluginSshSyncBrowserReadRequest,
        expected_profile_id: &str,
        expected_authorization_epoch: WireSequence,
    ) -> bool {
        let Ok(installed) = self.has_capability(
            RequestId::new(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::UiPage,
        ) else {
            return false;
        };
        if installed.package_sha256 != request.expected_package_sha256
            || installed.state_version != request.expected_state_version
            || self
                .capability_grant_epoch_for_record(
                    RequestId::new(),
                    &installed,
                    PluginCapability::SshSync,
                )
                .ok()
                != Some(expected_authorization_epoch)
        {
            return false;
        }
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .mutation_reservations
            .contains_key(request.plugin_id.as_str())
        {
            return false;
        }
        let Some(active_target) = runtime.target_contexts.get(request.context_handle.as_str())
        else {
            return false;
        };
        let Some((plugin_id, page_id)) = active_target.instance_key.split_once('|') else {
            return false;
        };
        let Some(instance) = runtime.active_instances.get(request.plugin_id.as_str()) else {
            return false;
        };
        plugin_id == request.plugin_id.as_str()
            && active_target.context.target_id == request.target_id
            && active_target.context.surface_kind
                == norishell_core_api::PluginExtensionSurfaceKind::Page
            && active_target.context.target_revision == request.expected_target_revision
            && instance.signer_fingerprint_sha256 == request.signer_fingerprint_sha256
            && instance.package_sha256 == request.expected_package_sha256
            && instance.instance_generation == request.instance_generation
            && instance.state_version == request.expected_state_version
            && instance.contribution_revision == request.expected_contribution_revision
            && instance.pages.get(page_id).is_some_and(|page| {
                ssh_sync_browser_profile(&page.document, &request.node_id)
                    == Some(expected_profile_id)
            })
    }

    fn list_navigation(&self, request_id: RequestId) -> CoreResult<Vec<PluginNavigationItem>> {
        self.require_ready(request_id.clone())?;
        let candidates = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .iter()
            .filter(|(_, instance)| {
                instance.can_publish_contributions() && !instance.navigation.is_empty()
            })
            .map(|(plugin_id, instance)| (plugin_id.clone(), instance.clone()))
            .collect::<Vec<_>>();
        let mut items = Vec::new();
        for (plugin_id, instance) in candidates {
            let plugin_id = PluginId::parse(plugin_id)
                .map_err(|_| plugin_validation_error(request_id.clone()))?;
            self.has_capability(
                request_id.clone(),
                &plugin_id,
                &instance.signer_fingerprint_sha256,
                PluginCapability::UiNavigation,
            )?;
            self.has_capability(
                request_id.clone(),
                &plugin_id,
                &instance.signer_fingerprint_sha256,
                PluginCapability::UiPage,
            )?;
            items.extend(instance.navigation.values().cloned().map(|navigation| {
                PluginNavigationItem {
                    plugin_id: plugin_id.clone(),
                    plugin_name: instance.plugin_name.clone(),
                    signer_fingerprint_sha256: instance.signer_fingerprint_sha256.clone(),
                    package_sha256: instance.package_sha256.clone(),
                    instance_generation: instance.instance_generation,
                    state_version: instance.state_version,
                    contribution_revision: instance.contribution_revision,
                    navigation,
                }
            }));
        }
        items.sort_by(|left, right| {
            left.navigation
                .order
                .cmp(&right.navigation.order)
                .then_with(|| left.navigation.label.cmp(&right.navigation.label))
                .then_with(|| left.plugin_id.cmp(&right.plugin_id))
        });
        Ok(items)
    }

    fn list_host_scopes(
        &self,
        request: PluginHostScopeListRequest,
    ) -> CoreResult<PluginHostScopeSnapshot> {
        self.require_ready(request.meta.request_id.clone())?;
        let installed = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&request.plugin_id)
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let (scope, mut grants) = self
            .hosts
            .with_plugin_repository(|repository| {
                Ok((
                    repository.plugin_host_scope_set(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?,
                    repository.list_plugin_host_scope_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?,
                ))
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        if scope
            .as_ref()
            .is_none_or(|scope| !effective_plugin_scope(&installed, scope))
        {
            grants.clear();
        }
        let state_version = scope.map(|scope| scope.state_version);
        let mut by_host = BTreeMap::<String, Vec<PluginCapability>>::new();
        for grant in grants {
            by_host
                .entry(grant.host_id.as_str().to_owned())
                .or_default()
                .push(grant.capability);
        }
        let mut hosts = Vec::with_capacity(by_host.len());
        for (host_id, mut capabilities) in by_host {
            let host_id = norishell_core_api::HostId::parse(host_id)
                .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
            let host = self
                .hosts
                .with_plugin_repository(|repository| repository.get_host(&host_id))
                .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
            capabilities.sort();
            hosts.push(PluginHostScopeSummary {
                host_id,
                host_label: host.label,
                endpoint: format!("{}:{}", host.address, host.port),
                capabilities,
                scope_state_version: state_version.unwrap_or(WireSequence::new(1)),
            });
        }
        hosts.sort_by(|left, right| {
            left.host_label
                .cmp(&right.host_label)
                .then_with(|| left.host_id.as_str().cmp(right.host_id.as_str()))
        });
        Ok(PluginHostScopeSnapshot {
            scope_state_version: state_version,
            hosts,
        })
    }

    fn publish_plugin_host_scope(&self, installed: &PluginInstalledRecord) {
        let scope = self.hosts.with_plugin_repository(|repository| {
            repository.plugin_host_scope_set(
                &installed.plugin_id,
                &installed.signer_fingerprint_sha256,
                u64::from(PLUGIN_PROTOCOL_MAJOR),
            )
        });
        if let Ok(scope) = scope {
            self.api.subscriptions.publish_host_scope(
                crate::plugin_api::subscriptions::HostScopeSourceEvent {
                    plugin_id: installed.plugin_id.clone(),
                    signer: installed.signer_fingerprint_sha256.clone(),
                    scope_state_version: scope.map(|scope| scope.state_version),
                },
            );
        }
    }

    fn replace_host_scopes(
        &self,
        request: PluginHostScopeReplaceRequest,
    ) -> CoreResult<PluginHostScopeSnapshot> {
        self.require_ready(request.meta.request_id.clone())?;
        let installed = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&request.plugin_id)
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        if installed.state_version != request.expected_plugin_state_version {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(installed.state_version),
            ));
        }
        if request.selections.len() > 256 {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let binding = current_plugin_permission_binding(&installed.package_sha256);
        let mut flattened = Vec::new();
        for selection in &request.selections {
            if selection.capabilities.is_empty() || selection.capabilities.len() > 6 {
                return Err(plugin_validation_error(request.meta.request_id));
            }
            for capability in &selection.capabilities {
                if !matches!(
                    capability,
                    PluginCapability::HostMetadataRead
                        | PluginCapability::HostMutationPropose
                        | PluginCapability::HostSessionRequest
                ) || !installed.capabilities.contains(capability)
                {
                    return Err(plugin_validation_error(request.meta.request_id));
                }
                self.has_capability(
                    request.meta.request_id.clone(),
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    *capability,
                )?;
                flattened.push((selection.host_id.clone(), *capability));
            }
        }
        self.hosts
            .with_plugin_repository(|repository| {
                repository.replace_plugin_host_scope_grants(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major,
                    installed.state_version,
                    request.expected_scope_state_version,
                    &binding,
                    &flattened,
                )
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .host_handles
            .retain(|_, handle| handle.plugin_id != request.plugin_id);
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_host_approvals
            .retain(|_, pending| pending.summary.plugin_id != request.plugin_id);
        let snapshot = self.list_host_scopes(PluginHostScopeListRequest {
            meta: request.meta,
            plugin_id: request.plugin_id.clone(),
        })?;
        self.api.subscriptions.publish_host_scope(
            crate::plugin_api::subscriptions::HostScopeSourceEvent {
                plugin_id: request.plugin_id,
                signer: installed.signer_fingerprint_sha256,
                scope_state_version: snapshot.scope_state_version,
            },
        );
        Ok(snapshot)
    }

    fn prepare_special_permission(
        &self,
        request: PluginSpecialPermissionOpenRequest,
    ) -> CoreResult<PluginSpecialPermissionSnapshot> {
        self.require_ready(request.meta.request_id.clone())?;
        let (plugin_id, expected_plugin_state_version) = match &request.target {
            PluginSpecialPermissionTarget::Installed {
                plugin_id,
                expected_plugin_state_version,
            } => (plugin_id.clone(), *expected_plugin_state_version),
            PluginSpecialPermissionTarget::PreparedPackage { .. } => {
                return self.prepare_package_special_permission(request);
            }
        };
        let installed = self
            .hosts
            .with_plugin_repository(|repository| repository.get_plugin_installation(&plugin_id))
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        if installed.state_version != expected_plugin_state_version {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(installed.state_version),
            ));
        }
        let mut declared_special = installed
            .capabilities
            .iter()
            .copied()
            .filter(|capability| special_plugin_capability(*capability))
            .collect::<Vec<_>>();
        declared_special.sort();
        if declared_special.is_empty()
            || request
                .requested_capability
                .is_some_and(|capability| !declared_special.contains(&capability))
        {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let (all_grants, scope, mut scope_grants, hosts) = self
            .hosts
            .with_plugin_repository(|repository| {
                Ok((
                    repository.list_plugin_capability_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?,
                    repository.plugin_host_scope_set(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?,
                    repository.list_plugin_host_scope_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?,
                    repository.list_hosts()?,
                ))
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let scope_state_version = scope.as_ref().map(|scope| scope.state_version);
        if scope
            .as_ref()
            .is_none_or(|scope| !effective_plugin_scope(&installed, scope))
        {
            scope_grants.clear();
        }
        let grant_state_version = all_grants.first().map(|grant| grant.state_version);
        if all_grants
            .iter()
            .any(|grant| Some(grant.state_version) != grant_state_version)
        {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let special_grants = declared_special
            .iter()
            .map(|capability| PluginCapabilityGrant {
                capability: *capability,
                granted: all_grants.iter().any(|grant| {
                    grant.capability == *capability && effective_plugin_grant(&installed, grant)
                }),
            })
            .collect::<Vec<_>>();
        let mut permission_hosts = hosts
            .into_iter()
            .map(|host| {
                let mut granted_capabilities = scope_grants
                    .iter()
                    .filter(|grant| grant.host_id == host.host_id)
                    .map(|grant| grant.capability)
                    .collect::<Vec<_>>();
                granted_capabilities.sort();
                PluginSpecialPermissionHost {
                    host_id: host.host_id,
                    label: host.label,
                    endpoint: format!("{}:{}", host.address, host.port),
                    granted_capabilities,
                }
            })
            .collect::<Vec<_>>();
        permission_hosts.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| left.host_id.as_str().cmp(right.host_id.as_str()))
        });
        let approval_id = PluginApprovalId::new();
        let publisher_verified =
            self.installed_publisher_is_verified(&installed, request.meta.request_id.clone())?;
        let snapshot = PluginSpecialPermissionSnapshot {
            target: request.target,
            requested_capability: request.requested_capability,
            publisher_verified,
            approval_id: approval_id.clone(),
            approval_state_version: WireSequence::new(1),
            expires_at_unix_ms: unix_time_ms().saturating_add(PLUGIN_HOST_APPROVAL_MILLIS),
            plugin_id: installed.plugin_id.clone(),
            plugin_name: installed.name.clone(),
            publisher: installed.publisher.clone(),
            signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
            version: installed.active_version.clone(),
            package_sha256: installed.package_sha256.clone(),
            special_grants,
            hosts: permission_hosts,
        };
        let has_settings =
            self.installed_has_settings(request.meta.request_id.clone(), &installed)?;
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = unix_time_ms();
        runtime
            .pending_special_permissions
            .retain(|_, pending| pending.snapshot.expires_at_unix_ms > now);
        if runtime.pending_special_permissions.len() >= PLUGIN_HOST_APPROVAL_LIMIT {
            return Err(plugin_error(
                request.meta.request_id,
                "plugin.permission_queue_full",
                ErrorCategory::Unavailable,
                RetryStrategy::AfterMilliseconds(1_000),
                "errors.plugin.permissionQueueFull",
                None,
            ));
        }
        runtime.pending_special_permissions.insert(
            approval_id.as_str().to_owned(),
            PendingPluginSpecialPermission {
                snapshot: snapshot.clone(),
                installed_snapshot: Some(installed_to_wire(
                    installed.clone(),
                    all_grants.clone(),
                    has_settings,
                )),
                expected_plugin_state_version: installed.state_version,
                expected_grant_state_version: grant_state_version,
                expected_scope_state_version: scope_state_version,
                all_grants,
                prepared_baseline: None,
            },
        );
        Ok(snapshot)
    }

    fn special_permission_snapshot(
        &self,
        request: PluginSpecialPermissionGetRequest,
    ) -> CoreResult<PluginSpecialPermissionSnapshot> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = unix_time_ms();
        runtime
            .pending_special_permissions
            .retain(|_, pending| pending.snapshot.expires_at_unix_ms > now);
        runtime
            .pending_special_permissions
            .get(request.approval_id.as_str())
            .map(|pending| pending.snapshot.clone())
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id))
    }

    async fn decide_special_permission(
        &self,
        request: PluginSpecialPermissionDecisionRequest,
    ) -> CoreResult<PluginSpecialPermissionDecisionResponse> {
        let pending = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_special_permissions
            .remove(request.approval_id.as_str())
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        if pending.snapshot.approval_state_version != request.expected_approval_state_version
            || pending.snapshot.expires_at_unix_ms <= unix_time_ms()
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        if matches!(
            pending.snapshot.target,
            PluginSpecialPermissionTarget::PreparedPackage { .. }
        ) {
            return self.decide_package_special_permission(pending, request);
        }
        let mut installed = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&pending.snapshot.plugin_id)
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        if installed.state_version != pending.expected_plugin_state_version
            || installed.signer_fingerprint_sha256 != pending.snapshot.signer_fingerprint_sha256
            || installed.package_sha256 != pending.snapshot.package_sha256
            || installed.active_version != pending.snapshot.version
        {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(installed.state_version),
            ));
        }
        if request.decision == PluginApprovalDecision::Reject {
            let grants = self
                .hosts
                .with_plugin_repository(|repository| {
                    repository.list_plugin_capability_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        u64::from(PLUGIN_PROTOCOL_MAJOR),
                    )
                })
                .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
            let has_settings =
                self.installed_has_settings(request.meta.request_id.clone(), &installed)?;
            return Ok(PluginSpecialPermissionDecisionResponse {
                approval_id: request.approval_id,
                decision: request.decision,
                target: PluginSpecialPermissionOutcome::Installed {
                    plugin: installed_to_wire(installed, grants, has_settings),
                },
            });
        }
        let (requested_special, flattened_scopes) = permissions::validate_special_decisions(
            &installed.capabilities,
            &request,
            &pending.snapshot.hosts,
        )?;
        let current_grants = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.list_plugin_capability_grants(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    u64::from(PLUGIN_PROTOCOL_MAJOR),
                )
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        if current_grants != pending.all_grants {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let mut decisions = current_grants
            .iter()
            .map(|grant| (grant.capability, grant.granted))
            .collect::<BTreeMap<_, _>>();
        for grant in requested_special {
            decisions.insert(grant.capability, grant.granted);
        }
        let decisions = installed
            .capabilities
            .iter()
            .map(|capability| {
                (
                    *capability,
                    decisions.get(capability).copied().unwrap_or(false),
                )
            })
            .collect::<Vec<_>>();
        let was_enabled = installed.state == PluginInstallState::Enabled;
        if was_enabled {
            self.stop_plugin(&installed.plugin_id).await?;
            installed = self.set_plugin_state_convergent(
                request.meta.request_id.clone(),
                &installed,
                PluginInstallState::Disabled,
            )?;
        }
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let binding = current_plugin_permission_binding(&installed.package_sha256);
        let (grants, _) = self
            .hosts
            .with_plugin_repository(|repository| {
                repository.replace_plugin_special_permissions(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major,
                    installed.state_version,
                    pending.expected_grant_state_version,
                    pending.expected_scope_state_version,
                    &binding,
                    &decisions,
                    &flattened_scopes,
                )
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runtime
            .host_handles
            .retain(|_, handle| handle.plugin_id != installed.plugin_id);
        runtime
            .pending_host_approvals
            .retain(|_, approval| approval.summary.plugin_id != installed.plugin_id);
        drop(runtime);
        let has_settings =
            self.installed_has_settings(request.meta.request_id.clone(), &installed)?;
        Ok(PluginSpecialPermissionDecisionResponse {
            approval_id: request.approval_id,
            decision: request.decision,
            target: PluginSpecialPermissionOutcome::Installed {
                plugin: installed_to_wire(installed, grants, has_settings),
            },
        })
    }

    fn host_metadata_for_action(
        &self,
        request: &PluginUiActionRequest,
        target: &PluginExtensionTargetContext,
        installed: &PluginInstalledRecord,
    ) -> CoreResult<Option<PluginHostMetadataProjection>> {
        if !installed
            .capabilities
            .contains(&PluginCapability::HostMetadataRead)
            || !self.capability_granted_for_record(
                request.meta.request_id.clone(),
                installed,
                PluginCapability::HostMetadataRead,
            )?
        {
            return Ok(None);
        }
        let instance_key = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .target_contexts
            .get(target.context_handle.as_str())
            .map(|active| active.instance_key.clone())
            .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
        let host_id_text = match target.target_id.as_str() {
            "host.detail.tools" | "overview.card.actions" => instance_key.as_str(),
            "terminal.toolbar" | "terminal.sidebar" | "terminal.contextMenu" => instance_key
                .split_once('|')
                .map_or("", |(host_id, _)| host_id),
            _ => return Ok(None),
        };
        let Ok(host_id) = norishell_core_api::HostId::parse(host_id_text) else {
            return Ok(None);
        };
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let (scope_state_version, scope_granted) = self
            .hosts
            .with_plugin_repository(|repository| {
                let scope = repository.plugin_host_scope_set(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major,
                )?;
                let granted = repository
                    .list_plugin_host_scope_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?
                    .into_iter()
                    .any(|grant| {
                        grant.host_id == host_id
                            && grant.capability == PluginCapability::HostMetadataRead
                    });
                Ok((
                    scope
                        .as_ref()
                        .filter(|scope| effective_plugin_scope(installed, scope))
                        .map(|scope| scope.state_version),
                    granted
                        && scope
                            .as_ref()
                            .is_some_and(|scope| effective_plugin_scope(installed, scope)),
                ))
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let Some(scope_state_version) = scope_state_version.filter(|_| scope_granted) else {
            return Ok(None);
        };
        let host = self
            .hosts
            .with_plugin_repository(|repository| repository.get_host(&host_id))
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let existing_handle = runtime.host_handles.iter().find_map(|(handle, record)| {
            (record.plugin_id == request.plugin_id
                && record.host_id == host_id
                && record.instance_generation == request.instance_generation
                && record.context_handle == request.context_handle
                && record.scope_state_version == scope_state_version)
                .then(|| handle.clone())
        });
        let handle =
            PluginHostHandle::parse(existing_handle.unwrap_or_else(|| Uuid::new_v4().to_string()))
                .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        if !runtime.host_handles.contains_key(handle.as_str())
            && runtime.host_handles.len() >= PLUGIN_HOST_HANDLE_LIMIT
            && let Some(oldest) = runtime
                .host_handles
                .iter()
                .min_by_key(|(_, record)| record.created_at_unix_ms)
                .map(|(handle, _)| handle.clone())
        {
            runtime.host_handles.remove(&oldest);
        }
        runtime
            .host_handles
            .entry(handle.as_str().to_owned())
            .or_insert_with(|| ActivePluginHostHandle {
                plugin_id: request.plugin_id.clone(),
                host_id,
                instance_generation: request.instance_generation,
                context_handle: request.context_handle.clone(),
                scope_state_version,
                created_at_unix_ms: unix_time_ms(),
            });
        Ok(Some(PluginHostMetadataProjection {
            host_handle: handle,
            label: host.label,
            address: host.address,
            port: host.port,
            username: host.username,
            favorite: host.favorite,
            host_state_version: host.state_version,
            scope_state_version,
        }))
    }

    async fn terminal_metadata_for_action(
        &self,
        request: &PluginUiActionRequest,
        target: &PluginExtensionTargetContext,
        installed: &PluginInstalledRecord,
    ) -> CoreResult<Option<PluginTerminalMetadataProjection>> {
        if !target.target_id.as_str().starts_with("terminal.")
            || !installed
                .capabilities
                .contains(&PluginCapability::TerminalMetadata)
            || !self.capability_granted_for_record(
                request.meta.request_id.clone(),
                installed,
                PluginCapability::TerminalMetadata,
            )?
        {
            return Ok(None);
        }
        let instance_key = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .target_contexts
            .get(target.context_handle.as_str())
            .map(|active| active.instance_key.clone())
            .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
        let parts = instance_key.split('|').collect::<Vec<_>>();
        if parts.len() != 5 {
            return Ok(None);
        }
        let expected_generation = parts[4]
            .parse::<u64>()
            .map(WireSequence::new)
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        let mut connection_key = None;
        let (kind, session_id, label, state, generation, attachment_count) = match parts[2] {
            "ssh" => {
                let session_id = norishell_core_api::SshSessionId::parse(parts[3])
                    .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
                let snapshot = self
                    .sessions
                    .snapshot(request.meta.request_id.clone())
                    .await?;
                let session = snapshot
                    .sessions
                    .into_iter()
                    .find(|session| {
                        session.session_id == session_id
                            && session.generation == expected_generation
                    })
                    .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
                if session.state == norishell_core_api::SshSessionState::Running
                    && self
                        .sessions
                        .plugin_channel_lease(
                            request.meta.request_id.clone(),
                            session.session_id.clone(),
                            session.generation,
                        )
                        .await
                        .is_err()
                {
                    return Ok(None);
                }
                if let Some(endpoint) = session.endpoint.as_ref() {
                    connection_key = Some(hex::encode(Sha256::digest(
                        format!(
                            "{}\0{}\0{}\0{}",
                            request.plugin_id.as_str(),
                            endpoint.address.to_ascii_lowercase(),
                            endpoint.port,
                            endpoint.username.as_deref().unwrap_or("")
                        )
                        .as_bytes(),
                    )));
                }
                let label = match &session.target {
                    SshSessionTarget::Host { host_id, .. } => self
                        .hosts
                        .with_plugin_repository(|repository| repository.get_host(host_id))
                        .map(|host| host.label)
                        .unwrap_or_else(|_| {
                            session.endpoint.as_ref().map_or_else(
                                || session.session_id.to_string(),
                                |endpoint| {
                                    endpoint.username.as_ref().map_or_else(
                                        || endpoint.address.clone(),
                                        |username| format!("{username}@{}", endpoint.address),
                                    )
                                },
                            )
                        }),
                    SshSessionTarget::QuickConnect { endpoint } => {
                        endpoint.username.as_ref().map_or_else(
                            || endpoint.address.clone(),
                            |username| format!("{username}@{}", endpoint.address),
                        )
                    }
                };
                (
                    PluginTerminalKind::Ssh,
                    session.session_id.as_str().to_owned(),
                    label,
                    plugin_ssh_terminal_state(session.state),
                    session.generation,
                    session.attachment_count,
                )
            }
            "local" => {
                let session_id = norishell_core_api::LocalSessionId::parse(parts[3])
                    .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
                let snapshot = self
                    .sessions
                    .local_snapshot(request.meta.request_id.clone())
                    .await?;
                let session = snapshot
                    .sessions
                    .into_iter()
                    .find(|session| {
                        session.session_id == session_id
                            && session.generation == expected_generation
                    })
                    .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
                (
                    PluginTerminalKind::Local,
                    session.session_id.as_str().to_owned(),
                    session.shell_name,
                    plugin_local_terminal_state(session.state),
                    session.generation,
                    session.attachment_count,
                )
            }
            _ => return Ok(None),
        };
        let terminal_handle = hex::encode(Sha256::digest(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                request.plugin_id.as_str(),
                request.signer_fingerprint_sha256,
                request.context_handle.as_str(),
                session_id,
                generation.get()
            )
            .as_bytes(),
        ));
        Ok(Some(PluginTerminalMetadataProjection {
            terminal_handle,
            connection_key,
            kind,
            label,
            state,
            generation,
            attachment_count,
        }))
    }

    fn prepare_host_approval(
        &self,
        request: &PluginUiActionRequest,
        parsed: &ParsedPluginUiOutputs,
        installed: &PluginInstalledRecord,
    ) -> CoreResult<Option<PendingPluginHostApproval>> {
        let (host_handle, capability, kind, reason, mutation, session_kind) =
            if let Some(proposal) = &parsed.host_mutation {
                (
                    &proposal.host_handle,
                    PluginCapability::HostMutationPropose,
                    PluginHostApprovalKind::Mutation,
                    proposal.reason.clone(),
                    Some(proposal.clone()),
                    None,
                )
            } else if let Some(proposal) = &parsed.host_session {
                (
                    &proposal.host_handle,
                    PluginCapability::HostSessionRequest,
                    PluginHostApprovalKind::Session,
                    proposal.reason.clone(),
                    None,
                    Some(proposal.kind),
                )
            } else {
                return Ok(None);
            };
        self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            capability,
        )?;
        let handle_record = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .host_handles
            .get(host_handle.as_str())
            .cloned()
            .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
        if handle_record.plugin_id != request.plugin_id
            || handle_record.instance_generation != request.instance_generation
            || handle_record.context_handle != request.context_handle
        {
            return Err(plugin_conflict_error(request.meta.request_id.clone(), None));
        }
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let (scope_state_version, scope_granted) = self
            .hosts
            .with_plugin_repository(|repository| {
                let scope = repository.plugin_host_scope_set(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major,
                )?;
                let granted = repository
                    .list_plugin_host_scope_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?
                    .into_iter()
                    .any(|grant| {
                        grant.host_id == handle_record.host_id && grant.capability == capability
                    });
                Ok((
                    scope
                        .as_ref()
                        .filter(|scope| effective_plugin_scope(installed, scope))
                        .map(|scope| scope.state_version),
                    granted
                        && scope
                            .as_ref()
                            .is_some_and(|scope| effective_plugin_scope(installed, scope)),
                ))
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        if scope_state_version != Some(handle_record.scope_state_version) || !scope_granted {
            return Err(plugin_conflict_error(
                request.meta.request_id.clone(),
                scope_state_version,
            ));
        }
        let host = self
            .hosts
            .with_plugin_repository(|repository| repository.get_host(&handle_record.host_id))
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        let (expected_host_state_version, mutation_patch) = if let Some(proposal) = mutation {
            if host.state_version != proposal.expected_host_state_version {
                return Err(plugin_conflict_error(
                    request.meta.request_id.clone(),
                    Some(host.state_version),
                ));
            }
            (
                Some(proposal.expected_host_state_version),
                Some(proposal.patch),
            )
        } else {
            (None, None)
        };
        let operation = match kind {
            PluginHostApprovalKind::Mutation => {
                norishell_core_api::PluginApprovalOperation::HostMutation
            }
            PluginHostApprovalKind::Session => {
                norishell_core_api::PluginApprovalOperation::HostSession
            }
        };
        let operation_policy = self.host_operation_policy(
            installed,
            capability,
            operation,
            request.action_id.as_str(),
            &handle_record.host_id,
            mutation_patch.as_ref(),
            session_kind,
        );
        let approved_operation_policy = operation_policy
            .as_ref()
            .and_then(|prepared| prepared.find().ok().flatten());
        let approval_id = PluginApprovalId::new();
        let expires_at_unix_ms = unix_time_ms().saturating_add(PLUGIN_HOST_APPROVAL_MILLIS);
        Ok(Some(PendingPluginHostApproval {
            summary: PluginHostApprovalSummary {
                approval_id: approval_id.clone(),
                plugin_id: request.plugin_id.clone(),
                plugin_name: installed.name.clone(),
                kind,
                host_label: host.label,
                endpoint: format!("{}:{}", host.address, host.port),
                reason,
                mutation_patch: mutation_patch.clone(),
                session_kind,
                expires_at_unix_ms,
                state_version: WireSequence::new(1),
                remember_policy: if operation_policy.is_some() {
                    norishell_core_api::PluginRememberPolicy::ExactOperation
                } else {
                    norishell_core_api::PluginRememberPolicy::StorageUnavailable
                },
            },
            signer_fingerprint_sha256: request.signer_fingerprint_sha256.clone(),
            package_sha256: request.expected_package_sha256.clone(),
            instance_generation: request.instance_generation,
            host_id: handle_record.host_id,
            host_handle: host_handle.clone(),
            scope_state_version: handle_record.scope_state_version,
            expected_host_state_version,
            mutation_patch,
            action_label: request.action_id.as_str().to_owned(),
            session_kind,
            operation_policy,
            approved_operation_policy,
        }))
    }

    fn authorized_host_dom_snapshot(
        &self,
        request: &PluginUiActionRequest,
        installed: &PluginInstalledRecord,
    ) -> CoreResult<Option<PluginHostDomSnapshot>> {
        let Some(snapshot) = request.host_dom_snapshot.as_ref() else {
            return Ok(None);
        };
        if !installed
            .capabilities
            .contains(&PluginCapability::UiHostDomObserve)
            || !self.capability_granted_for_record(
                request.meta.request_id.clone(),
                installed,
                PluginCapability::UiHostDomObserve,
            )?
        {
            // Core, rather than the renderer, is the disclosure boundary.
            // An ungranted snapshot is never forwarded to third-party code.
            return Ok(None);
        }
        if snapshot.context_handle != request.context_handle
            || validate_plugin_dom_snapshot(snapshot).is_err()
        {
            return Err(plugin_validation_error(request.meta.request_id.clone()));
        }
        Ok(Some(snapshot.clone()))
    }

    fn authorize_host_dom_operations(
        &self,
        request: &PluginUiActionRequest,
        snapshot: Option<&PluginHostDomSnapshot>,
        operations: Option<PluginHostDomOperationBatch>,
    ) -> CoreResult<Option<PluginHostDomOperationBatch>> {
        let Some(operations) = operations else {
            return Ok(None);
        };
        let Some(snapshot) = snapshot else {
            return Err(plugin_runtime_error(
                request.meta.request_id.clone(),
                Some(PluginHostProcessError::Rejected),
            ));
        };
        let requires_css = operations
            .operations
            .iter()
            .any(|operation| matches!(operation, PluginHostDomOperation::SetStyle { .. }));
        let requires_mutation = operations
            .operations
            .iter()
            .any(|operation| !matches!(operation, PluginHostDomOperation::SetStyle { .. }));
        if requires_css {
            self.has_capability(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                PluginCapability::UiHostCss,
            )?;
        }
        if requires_mutation {
            self.has_capability(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                PluginCapability::UiHostDomMutate,
            )?;
        }
        let owner_digest = hex::encode(Sha256::digest(request.plugin_id.as_str().as_bytes()));
        let owner_class_prefix = format!("norishell-plugin-{}-", &owner_digest[..12]);
        validate_plugin_dom_operations(snapshot, &operations, &owner_class_prefix).map_err(
            |_| {
                plugin_runtime_error(
                    request.meta.request_id.clone(),
                    Some(PluginHostProcessError::Rejected),
                )
            },
        )?;
        let _ = self.hosts.with_plugin_repository(|repository| {
            repository.append_plugin_audit(
                Some(&request.plugin_id),
                None,
                "ui.host_dom",
                "authorized",
                Some(if requires_mutation && requires_css {
                    "mutation_and_css"
                } else if requires_css {
                    "css"
                } else {
                    "mutation"
                }),
            )
        });
        Ok(Some(operations))
    }

    fn prepare_isolated_surface(
        &self,
        request: &PluginUiActionRequest,
        installed: &PluginInstalledRecord,
        surface: Option<PluginIsolatedSurfaceOpenRequest>,
    ) -> CoreResult<Option<(PluginIsolatedSurfaceOpenRequest, String)>> {
        let Some(surface) = surface else {
            return Ok(None);
        };
        self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::UiWebviewIsolated,
        )?;
        if installed.package_sha256 != request.expected_package_sha256
            || installed.state_version != request.expected_state_version
        {
            return Err(plugin_conflict_error(
                request.meta.request_id.clone(),
                Some(installed.state_version),
            ));
        }
        let bytes = self
            .installer
            .read_active_isolated_surface(
                &installed.plugin_id,
                &installed.active_version,
                &installed.package_sha256,
                &surface.surface_id,
                PLUGIN_ISOLATED_SURFACE_MAX_BYTES,
            )
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        let html = String::from_utf8(bytes)
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        if html.is_empty() || html.contains('\0') {
            return Err(plugin_validation_error(request.meta.request_id.clone()));
        }
        Ok(Some((surface, html)))
    }

    fn emit_plugin_invalidated(&self, plugin_id: &PluginId) {
        if let Some(app) = self
            .app_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            let _ = app.emit_to(
                "main",
                "plugin-runtime-invalidated",
                serde_json::json!({ "pluginId": plugin_id.as_str() }),
            );
        }
    }

    fn host_approval_snapshot(
        &self,
        request: PluginHostApprovalGetRequest,
    ) -> CoreResult<PluginHostApprovalSummary> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = unix_time_ms();
        runtime
            .pending_host_approvals
            .retain(|_, pending| pending.summary.expires_at_unix_ms > now);
        runtime
            .pending_host_approvals
            .get(request.approval_id.as_str())
            .map(|pending| pending.summary.clone())
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id))
    }

    fn decide_host_approval(
        &self,
        request: PluginHostApprovalDecisionRequest,
    ) -> CoreResult<PluginHostApprovalDecisionResponse> {
        let pending = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_host_approvals
            .remove(request.approval_id.as_str())
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        if pending.summary.state_version != request.expected_state_version
            || pending.summary.expires_at_unix_ms <= unix_time_ms()
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        if request.decision == PluginApprovalDecision::Reject {
            let _ = self.hosts.with_plugin_repository(|repository| {
                repository.append_plugin_audit(
                    Some(&pending.summary.plugin_id),
                    None,
                    "host.approval",
                    "rejected",
                    Some(match pending.summary.kind {
                        PluginHostApprovalKind::Mutation => "mutation",
                        PluginHostApprovalKind::Session => "session",
                    }),
                )
            });
            return Ok(PluginHostApprovalDecisionResponse {
                approval_id: request.approval_id,
                decision: request.decision,
                host_updated: false,
                session_launch: None,
            });
        }
        let capability = match pending.summary.kind {
            PluginHostApprovalKind::Mutation => PluginCapability::HostMutationPropose,
            PluginHostApprovalKind::Session => PluginCapability::HostSessionRequest,
        };
        let installed = self.has_capability(
            request.meta.request_id.clone(),
            &pending.summary.plugin_id,
            &pending.signer_fingerprint_sha256,
            capability,
        )?;
        if installed.package_sha256 != pending.package_sha256 {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(installed.state_version),
            ));
        }
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let instance_current = runtime
            .active_instances
            .get(pending.summary.plugin_id.as_str())
            .is_some_and(|instance| {
                instance.instance_generation == pending.instance_generation
                    && instance.package_sha256 == pending.package_sha256
                    && instance.signer_fingerprint_sha256 == pending.signer_fingerprint_sha256
            });
        let handle_current = runtime
            .host_handles
            .get(pending.host_handle.as_str())
            .is_some_and(|handle| {
                handle.plugin_id == pending.summary.plugin_id
                    && handle.host_id == pending.host_id
                    && handle.instance_generation == pending.instance_generation
                    && handle.scope_state_version == pending.scope_state_version
            });
        drop(runtime);
        if !instance_current || !handle_current {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let scope_current = self
            .hosts
            .with_plugin_repository(|repository| {
                let scope = repository.plugin_host_scope_set(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major,
                )?;
                let granted = repository
                    .list_plugin_host_scope_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?
                    .into_iter()
                    .any(|grant| {
                        grant.host_id == pending.host_id && grant.capability == capability
                    });
                Ok(scope.as_ref().is_some_and(|scope| {
                    effective_plugin_scope(&installed, scope)
                        && scope.state_version == pending.scope_state_version
                        && granted
                }))
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        if !scope_current {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let operation = match pending.summary.kind {
            PluginHostApprovalKind::Mutation => {
                norishell_core_api::PluginApprovalOperation::HostMutation
            }
            PluginHostApprovalKind::Session => {
                norishell_core_api::PluginApprovalOperation::HostSession
            }
        };
        let current_policy = self.host_operation_policy(
            &installed,
            capability,
            operation,
            &pending.action_label,
            &pending.host_id,
            pending.mutation_patch.as_ref(),
            pending.session_kind,
        );
        if pending.operation_policy.as_ref().is_some_and(|prepared| {
            !current_policy
                .as_ref()
                .is_some_and(|current| prepared.same_operation(current))
        }) {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let approved_policy = match request.policy {
            norishell_core_api::PluginApprovalPolicy::Once => None,
            norishell_core_api::PluginApprovalPolicy::Always => {
                match pending.approved_operation_policy {
                    Some(ref approved) => Some(approved.clone()),
                    None => Some(
                        pending
                            .operation_policy
                            .as_ref()
                            .ok_or_else(|| {
                                plugin_conflict_error(request.meta.request_id.clone(), None)
                            })?
                            .remember(request.expiry)
                            .map_err(|_| {
                                plugin_conflict_error(request.meta.request_id.clone(), None)
                            })?,
                    ),
                }
            }
        };
        if approved_policy
            .as_ref()
            .is_some_and(|policy| !policy.admit_dispatch())
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let mut host_updated = false;
        let mut session_launch = None;
        match pending.summary.kind {
            PluginHostApprovalKind::Mutation => {
                let patch = pending
                    .mutation_patch
                    .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
                let expected = pending
                    .expected_host_state_version
                    .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
                let host = self
                    .hosts
                    .with_plugin_repository(|repository| repository.get_host(&pending.host_id))
                    .map_err(|error| {
                        map_persistence_error(request.meta.request_id.clone(), error)
                    })?;
                if host.state_version != expected {
                    return Err(plugin_conflict_error(
                        request.meta.request_id,
                        Some(host.state_version),
                    ));
                }
                let username = if patch.clear_username {
                    None
                } else {
                    patch.username.as_deref().or(host.username.as_deref())
                };
                self.hosts
                    .with_plugin_repository(|repository| {
                        repository.update_host(
                            &pending.host_id,
                            expected,
                            patch.label.as_deref().unwrap_or(&host.label),
                            patch.address.as_deref().unwrap_or(&host.address),
                            patch.port.unwrap_or(host.port),
                            username,
                            host.identity_id.as_ref(),
                            patch.favorite.unwrap_or(host.favorite),
                        )
                    })
                    .map_err(|error| {
                        map_persistence_error(request.meta.request_id.clone(), error)
                    })?;
                host_updated = true;
            }
            PluginHostApprovalKind::Session => {
                let kind = pending
                    .session_kind
                    .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
                if kind != PluginHostSessionKind::Terminal {
                    return Err(plugin_validation_error(request.meta.request_id));
                }
                let authorization_token = PluginApprovalId::new();
                self.runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .approved_host_sessions
                    .insert(
                        authorization_token.as_str().to_owned(),
                        ApprovedPluginHostSession {
                            plugin_id: pending.summary.plugin_id.clone(),
                            signer_fingerprint_sha256: pending.signer_fingerprint_sha256.clone(),
                            package_sha256: pending.package_sha256.clone(),
                            instance_generation: pending.instance_generation,
                            host_id: pending.host_id.clone(),
                            scope_state_version: pending.scope_state_version,
                            kind,
                            action_label: pending.action_label.clone(),
                            expires_at_unix_ms: unix_time_ms().saturating_add(30_000),
                            operation_policy: approved_policy,
                        },
                    );
                session_launch = Some(PluginApprovedHostSessionLaunch {
                    operation_id: request.approval_id.clone(),
                    authorization_token,
                    host_id: pending.host_id,
                    kind,
                });
            }
        }
        let _ = self.hosts.with_plugin_repository(|repository| {
            repository.append_plugin_audit(
                Some(&pending.summary.plugin_id),
                None,
                "host.approval",
                "approved",
                Some(match pending.summary.kind {
                    PluginHostApprovalKind::Mutation => "mutation",
                    PluginHostApprovalKind::Session => "session",
                }),
            )
        });
        Ok(PluginHostApprovalDecisionResponse {
            approval_id: request.approval_id,
            decision: request.decision,
            host_updated,
            session_launch,
        })
    }

    pub(crate) fn consume_host_session_authorization(
        &self,
        request_id: RequestId,
        token: &PluginApprovalId,
        host_id: &norishell_core_api::HostId,
        kind: PluginHostSessionKind,
    ) -> CoreResult<()> {
        let pending = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .approved_host_sessions
            .get(token.as_str())
            .cloned()
            .ok_or_else(|| plugin_not_found_error(request_id.clone()))?;
        let operation_id = Uuid::new_v4();
        let _reservation = self
            .reserve_plugin_mutation(&pending.plugin_id, operation_id)
            .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
        if self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .approved_host_sessions
            .remove(token.as_str())
            .is_none()
        {
            return Err(plugin_conflict_error(request_id, None));
        }
        if pending.expires_at_unix_ms <= unix_time_ms()
            || &pending.host_id != host_id
            || pending.kind != kind
        {
            return Err(plugin_conflict_error(request_id, None));
        }
        self.host_session_grant_current(request_id, &pending)
    }

    fn host_session_grant_current(
        &self,
        request_id: RequestId,
        pending: &ApprovedPluginHostSession,
    ) -> CoreResult<()> {
        let capability = PluginCapability::HostSessionRequest;
        let installed = self.has_capability(
            request_id.clone(),
            &pending.plugin_id,
            &pending.signer_fingerprint_sha256,
            capability,
        )?;
        if installed.package_sha256 != pending.package_sha256 {
            return Err(plugin_conflict_error(
                request_id,
                Some(installed.state_version),
            ));
        }
        if let Some(approved) = &pending.operation_policy {
            let current = self.host_operation_policy(
                &installed,
                capability,
                norishell_core_api::PluginApprovalOperation::HostSession,
                &pending.action_label,
                &pending.host_id,
                None,
                Some(pending.kind),
            );
            if !current.is_some_and(|current| approved.matches_prepared(&current))
                || !approved.admit_dispatch()
            {
                return Err(plugin_conflict_error(request_id, None));
            }
        }
        let instance_current = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .get(pending.plugin_id.as_str())
            .is_some_and(|instance| {
                instance.instance_generation == pending.instance_generation
                    && instance.package_sha256 == pending.package_sha256
                    && instance.signer_fingerprint_sha256 == pending.signer_fingerprint_sha256
            });
        if !instance_current {
            return Err(plugin_conflict_error(request_id, None));
        }
        let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
        let scope_current = self
            .hosts
            .with_plugin_repository(|repository| {
                let scope = repository.plugin_host_scope_set(
                    &installed.plugin_id,
                    &installed.signer_fingerprint_sha256,
                    major,
                )?;
                let granted = repository
                    .list_plugin_host_scope_grants(
                        &installed.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        major,
                    )?
                    .into_iter()
                    .any(|grant| {
                        grant.host_id == pending.host_id && grant.capability == capability
                    });
                Ok(scope.as_ref().is_some_and(|scope| {
                    effective_plugin_scope(&installed, scope)
                        && scope.state_version == pending.scope_state_version
                        && granted
                }))
            })
            .map_err(|error| map_persistence_error(request_id.clone(), error))?;
        if !scope_current {
            return Err(plugin_conflict_error(request_id, None));
        }
        Ok(())
    }

    fn clear_ui_action_in_flight(&self, request: &PluginUiActionRequest) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(instance) = runtime.active_instances.get_mut(request.plugin_id.as_str())
            && instance.signer_fingerprint_sha256 == request.signer_fingerprint_sha256
            && instance.package_sha256 == request.expected_package_sha256
            && instance.instance_generation == request.instance_generation
            && instance.state_version == request.expected_state_version
            && instance.contribution_revision == request.expected_contribution_revision
        {
            instance.contribution_action_in_flight = false;
        }
    }

    fn ssh_sync_action_fence_current(&self, request: &PluginUiActionRequest) -> bool {
        let installed = self.has_capability(
            RequestId::new(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            PluginCapability::SshSync,
        );
        let Ok(installed) = installed else {
            return false;
        };
        if installed.package_sha256 != request.expected_package_sha256
            || installed.state_version != request.expected_state_version
        {
            return false;
        }
        self.broker_action_current(request)
    }

    async fn invoke_ui_action(
        &self,
        request: PluginUiActionRequest,
    ) -> CoreResult<PluginUiActionResponse> {
        self.require_ready(request.meta.request_id.clone())?;
        let supplied_target = PluginExtensionTargetContext {
            target_id: request.target_id.clone(),
            surface_kind: plugin_extension_registry::find(&request.target_id)
                .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?
                .surface_kind,
            context_handle: request.context_handle.clone(),
            target_revision: request.expected_target_revision,
            display_label: None,
        };
        let target =
            self.canonical_target_context(request.meta.request_id.clone(), &supplied_target)?;
        let definition = plugin_extension_registry::find(&target.target_id)
            .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
        let installed = self.has_capability(
            request.meta.request_id.clone(),
            &request.plugin_id,
            &request.signer_fingerprint_sha256,
            definition.required_capability,
        )?;
        if installed.package_sha256 != request.expected_package_sha256
            || installed.state_version != request.expected_state_version
        {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(installed.state_version),
            ));
        }
        let settings_snapshot =
            self.settings_snapshot_for_installation(request.meta.request_id.clone(), &installed)?;
        let settings_revision = settings_snapshot.as_ref().map(|snapshot| snapshot.revision);
        if settings_snapshot.as_ref().is_some_and(|snapshot| {
            !norishell_plugin_platform::settings_target_is_visible(
                &snapshot.schema,
                &snapshot.values,
                request.target_id.as_str(),
            )
        }) {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                settings_revision,
            ));
        }
        let settings_projection = settings_snapshot.map(|snapshot| {
            serde_json::json!({
                "revision": snapshot.revision,
                "values": snapshot.values,
            })
        });
        let page_id = if request.target_id.as_str() == "app.page" {
            let binding = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .target_contexts
                .get(request.context_handle.as_str())
                .and_then(|active| active.instance_key.split_once('|'))
                .map(|(plugin_id, page_id)| (plugin_id.to_owned(), page_id.to_owned()))
                .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
            if binding.0 != request.plugin_id.as_str() {
                return Err(plugin_conflict_error(request.meta.request_id, None));
            }
            Some(binding.1)
        } else {
            None
        };
        let (
            instance,
            action_kind,
            plugin_visible_fields,
            auto_refresh_lifecycle,
            explicit_user_action,
        ) = {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if runtime
                .mutation_reservations
                .contains_key(request.plugin_id.as_str())
            {
                return Err(plugin_conflict_error(request.meta.request_id, None));
            }
            let Some(instance) = runtime.active_instances.get_mut(request.plugin_id.as_str())
            else {
                return Err(plugin_not_found_error(request.meta.request_id));
            };
            let template = if page_id.is_none() {
                instance
                    .scoped_templates
                    .get(request.context_handle.as_str())
                    .or_else(|| instance.ui_templates.get(request.target_id.as_str()))
            } else {
                None
            };
            let document = page_id
                .as_ref()
                .and_then(|page_id| instance.pages.get(page_id).map(|page| &page.document))
                .or_else(|| template.map(|template| &template.document));
            let page_lifecycle = page_id
                .as_ref()
                .and_then(|page_id| instance.pages.get(page_id))
                .map(|page| {
                    norishell_plugin_platform::plugin_page_lifecycle_action_admission(
                        page,
                        &request.action_id,
                        &request.fields,
                    )
                })
                .transpose()
                .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?
                .unwrap_or(false);
            let (action_kind, auto_refresh_lifecycle, explicit_user_action) = if page_lifecycle {
                // Core derives background authority from its admitted Page declaration.
                // A renderer-supplied background=false cannot upgrade this hook.
                (Some(PluginUiActionKind::Standard), false, false)
            } else {
                match template
                    .map(|template| {
                        template_lifecycle_action_admission(
                            template,
                            &request.action_id,
                            &request.fields,
                        )
                    })
                    .unwrap_or(TemplateLifecycleActionAdmission::NotLifecycle)
                {
                    TemplateLifecycleActionAdmission::Admitted => {
                        (Some(PluginUiActionKind::Standard), false, false)
                    }
                    TemplateLifecycleActionAdmission::AutoRefreshAdmitted => {
                        (Some(PluginUiActionKind::Standard), true, false)
                    }
                    TemplateLifecycleActionAdmission::Rejected => {
                        return Err(plugin_validation_error(request.meta.request_id));
                    }
                    TemplateLifecycleActionAdmission::NotLifecycle => (
                        document.and_then(|document| {
                            if page_id.is_some() {
                                validate_plugin_page_ui_action(
                                    document,
                                    &request.action_id,
                                    &request.fields,
                                )
                                .ok()
                            } else if request.target_id.as_str() == "app.header.actions" {
                                validate_plugin_dialog_ui_action(
                                    document,
                                    &request.action_id,
                                    &request.fields,
                                )
                                .ok()
                            } else {
                                validate_plugin_ui_action(
                                    document,
                                    &request.action_id,
                                    &request.fields,
                                )
                                .ok()
                            }
                        }),
                        false,
                        true,
                    ),
                }
            };
            let plugin_visible_fields = plugin_visible_ui_fields(document, &request.fields);
            if instance.signer_fingerprint_sha256 != request.signer_fingerprint_sha256
                || instance.package_sha256 != request.expected_package_sha256
                || instance.instance_generation != request.instance_generation
                || instance.state_version != request.expected_state_version
                || instance.contribution_revision != request.expected_contribution_revision
                || instance.settings_revision != settings_revision
                || instance.contribution_action_in_flight
                || action_kind.is_none()
            {
                return Err(plugin_conflict_error(
                    request.meta.request_id,
                    Some(instance.contribution_revision),
                ));
            }
            instance.contribution_action_in_flight = true;
            (
                instance.clone(),
                action_kind.expect("validated action kind must be present"),
                plugin_visible_fields,
                auto_refresh_lifecycle,
                explicit_user_action && !request.background.unwrap_or(false),
            )
        };
        if action_kind == PluginUiActionKind::Copy
            && let Err(error) = self.has_capability(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                PluginCapability::ClipboardWrite,
            )
        {
            self.clear_ui_action_in_flight(&request);
            return Err(error);
        }
        let host_metadata = match self.host_metadata_for_action(&request, &target, &installed) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.clear_ui_action_in_flight(&request);
                return Err(error);
            }
        };
        let terminal_metadata = match self
            .terminal_metadata_for_action(&request, &target, &installed)
            .await
        {
            Ok(metadata) => metadata,
            Err(error) => {
                self.clear_ui_action_in_flight(&request);
                return Err(error);
            }
        };
        let host_dom_snapshot = match self.authorized_host_dom_snapshot(&request, &installed) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.clear_ui_action_in_flight(&request);
                return Err(error);
            }
        };
        let storage_snapshot = if installed
            .capabilities
            .contains(&PluginCapability::StoragePlugin)
        {
            if let Err(error) = self.has_capability(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                PluginCapability::StoragePlugin,
            ) {
                self.clear_ui_action_in_flight(&request);
                return Err(error);
            }
            Some(self.plugin_storage_snapshot(request.meta.request_id.clone(), &installed)?)
        } else {
            None
        };
        let host_request_id = Uuid::new_v4().to_string();
        let mut storage_write_token = host_request_id.clone();
        let protocol_minor = instance.protocol_minor;
        let locale = instance.locale.clone();
        let state_snapshot = if definition.contextual {
            instance
                .scoped_states
                .get(request.context_handle.as_str())
                .cloned()
                .unwrap_or_else(|| "{}".to_owned())
        } else {
            instance.ui_state_json.clone()
        };
        let mut action_payload = serde_json::json!({
            "targetId": request.target_id.as_str(),
            "contextHandle": request.context_handle.as_str(),
            "targetRevision": request.expected_target_revision.get().to_string(),
            "actionId": request.action_id.as_str(),
            "fields": plugin_visible_fields,
            "hostMetadata": host_metadata,
            "hostDomSnapshot": host_dom_snapshot,
            "terminalMetadata": terminal_metadata,
            "storage": storage_snapshot.clone(),
        });
        action_payload["nowUnixMs"] = serde_json::json!(unix_time_ms());
        action_payload["state"] = serde_json::json!({"valueJson": state_snapshot});
        let action_request = PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor,
            request_id: host_request_id.clone(),
            kind: PluginHostMessageKind::UiAction,
            payload_json: plugin_host_payload_with_settings(
                &locale,
                action_payload,
                settings_projection.as_ref(),
            ),
        };
        if serde_json::to_vec(&action_request).map_or(true, |bytes| {
            bytes.len() > norishell_plugin_platform::RuntimeLimits::default().max_request_bytes
        }) {
            self.clear_ui_action_in_flight(&request);
            return Err(plugin_error(
                request.meta.request_id,
                "plugin.broker_input_too_large",
                ErrorCategory::Unavailable,
                RetryStrategy::Never,
                "plugins.remoteApproval.resultTooLarge",
                None,
            ));
        }
        // Freeze the target before awaiting guest code; a late reply cannot follow a new focus.
        let input_focus = if explicit_user_action
            && installed
                .capabilities
                .contains(&PluginCapability::TerminalRequestInput)
        {
            self.sessions
                .terminal_focus_snapshot_internal(request.meta.request_id.clone())
                .await
                .ok()
        } else {
            None
        };
        let outputs = match self
            .execute_instance(request.meta.request_id.clone(), instance, action_request)
            .await
        {
            Ok(outputs) => outputs,
            Err(error) => {
                self.clear_ui_action_in_flight(&request);
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(error);
            }
        };
        let mut parsed = match parse_plugin_ui_outputs(&host_request_id, outputs) {
            Ok(parsed)
                if ui_action_output_is_admissible(
                    &parsed,
                    action_kind,
                    storage_snapshot.is_some(),
                ) && (!auto_refresh_lifecycle
                    || (auto_refresh_initial_output_is_admissible(&parsed)
                        && auto_refresh_remote_operation_is_read_only(&parsed))) =>
            {
                parsed
            }
            _ => {
                self.clear_ui_action_in_flight(&request);
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(plugin_runtime_error(
                    request.meta.request_id,
                    Some(PluginHostProcessError::Rejected),
                ));
            }
        };
        if parsed.has_operation_broker_request() {
            if action_kind != PluginUiActionKind::Standard {
                self.clear_ui_action_in_flight(&request);
                return Err(plugin_runtime_error(
                    request.meta.request_id,
                    Some(PluginHostProcessError::Rejected),
                ));
            }
            match self
                .invoke_operation_broker(
                    &request,
                    &installed,
                    &parsed,
                    &plugin_visible_fields,
                    &storage_snapshot,
                    &state_snapshot,
                    &settings_projection,
                    explicit_user_action,
                    input_focus,
                )
                .await
            {
                Ok((callback, callback_id)) => {
                    if auto_refresh_lifecycle
                        && !auto_refresh_callback_output_is_admissible(&callback)
                    {
                        self.clear_ui_action_in_flight(&request);
                        self.record_runtime_failure(
                            &request.plugin_id,
                            request.instance_generation,
                        );
                        return Err(plugin_runtime_error(
                            request.meta.request_id,
                            Some(PluginHostProcessError::Rejected),
                        ));
                    }
                    parsed = callback;
                    storage_write_token = callback_id;
                }
                Err(error) => {
                    self.clear_ui_action_in_flight(&request);
                    return Err(error);
                }
            }
        }
        let ssh_sync_status = if let Some(sync_request) = parsed.ssh_sync_request.clone() {
            if matches!(
                sync_request,
                norishell_core_api::PluginSshSyncRequest::Refresh { .. }
                    | norishell_core_api::PluginSshSyncRequest::Sync { .. }
                    | norishell_core_api::PluginSshSyncRequest::ConfigureScope { .. }
                    | norishell_core_api::PluginSshSyncRequest::ResetRemote { .. }
            ) {
                self.clear_ui_action_in_flight(&request);
                return Err(plugin_error(
                    request.meta.request_id,
                    "plugin.core_api_incompatible",
                    ErrorCategory::Incompatible,
                    RetryStrategy::Never,
                    "errors.plugin.coreApiIncompatible",
                    None,
                ));
            }
            let ssh_sync_profile_id = ssh_sync_request_profile_id(&sync_request).to_owned();
            let capability = self.has_capability(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                PluginCapability::SshSync,
            );
            match ssh_sync_request_disposition(action_kind, capability.is_ok()) {
                SshSyncRequestDisposition::ProtocolViolation => {
                    self.clear_ui_action_in_flight(&request);
                    self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                    let _ = self.stop_plugin(&request.plugin_id).await;
                    return Err(plugin_runtime_error(
                        request.meta.request_id,
                        Some(PluginHostProcessError::Rejected),
                    ));
                }
                SshSyncRequestDisposition::PermissionDenied => {
                    // A valid high-risk request without a user grant is an expected
                    // authorization state. It must not crash the plugin or discard
                    // its navigation/page contributions.
                    self.clear_ui_action_in_flight(&request);
                    return Err(capability.expect_err("denied capability must carry its error"));
                }
                SshSyncRequestDisposition::Authorized => {}
            }
            let Some(coordinator) = self.ssh_sync.as_ref() else {
                self.clear_ui_action_in_flight(&request);
                return Err(plugin_error(
                    request.meta.request_id,
                    "plugin.ssh_sync_unavailable",
                    ErrorCategory::Unavailable,
                    RetryStrategy::Never,
                    "errors.plugin.sshSyncUnavailable",
                    None,
                ));
            };
            let ssh_sync_authorization_epoch = self.capability_grant_epoch_for_record(
                request.meta.request_id.clone(),
                &installed,
                PluginCapability::SshSync,
            )?;
            let service = self.clone();
            let fence_request = request.clone();
            let fence: crate::ssh_sync_exchange::ActionFence = Arc::new(move || {
                service.ssh_sync_action_fence_current(&fence_request)
                    && service
                        .runtime
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .active_instances
                        .get(fence_request.plugin_id.as_str())
                        .is_some_and(|instance| instance.settings_revision == settings_revision)
            });
            if parsed.storage_write.is_some() {
                self.clear_ui_action_in_flight(&request);
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(plugin_runtime_error(
                    request.meta.request_id,
                    Some(PluginHostProcessError::Rejected),
                ));
            }
            let status = coordinator
                .clone()
                .invoke(
                    request.plugin_id.as_str(),
                    &request.signer_fingerprint_sha256,
                    sync_request,
                    &request.fields,
                    crate::ssh_sync_exchange::SshSyncActionRevision {
                        authorization: request.expected_state_version,
                        configuration: request.expected_contribution_revision,
                    },
                    crate::ssh_sync_exchange::SshSyncInvocationBinding {
                        package_sha256: request.expected_package_sha256.clone(),
                        instance_generation: request.instance_generation,
                        authorization_epoch: ssh_sync_authorization_epoch,
                    },
                    !explicit_user_action,
                    fence,
                )
                .await;
            self.emit_ssh_sync_browser_invalidated(
                Some(request.plugin_id.clone()),
                Some(ssh_sync_profile_id),
            );
            if !self.ssh_sync_action_fence_current(&request) {
                self.clear_ui_action_in_flight(&request);
                return Err(plugin_conflict_error(request.meta.request_id, None));
            }
            let callback_request_id = Uuid::new_v4().to_string();
            let callback_instance = self.active_instance(
                request.meta.request_id.clone(),
                &request.plugin_id,
                &request.signer_fingerprint_sha256,
                request.instance_generation,
            )?;
            let callback_protocol_minor = callback_instance.protocol_minor;
            let callback_locale = callback_instance.locale.clone();
            let callback_outputs = self
                .execute_instance(
                    request.meta.request_id.clone(),
                    callback_instance,
                    PluginHostRequest {
                        protocol_major: PLUGIN_PROTOCOL_MAJOR,
                        protocol_minor: callback_protocol_minor,
                        request_id: callback_request_id.clone(),
                        kind: PluginHostMessageKind::SshSyncResult,
                        payload_json: plugin_host_payload(&callback_locale, serde_json::json!({
                            "targetId": request.target_id.as_str(),
                            "contextHandle": request.context_handle.as_str(),
                            "targetRevision": request.expected_target_revision.get().to_string(),
                            "actionId": request.action_id.as_str(),
                            "result": ssh_sync_result_payload(&status),
                            "storage": storage_snapshot.clone(),
                        })),
                    },
                )
                .await?;
            let callback = parse_plugin_ui_outputs(&callback_request_id, callback_outputs)
                .map_err(|_| {
                    self.clear_ui_action_in_flight(&request);
                    self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                    plugin_runtime_error(
                        request.meta.request_id.clone(),
                        Some(PluginHostProcessError::Rejected),
                    )
                })?;
            if !callback.panels.is_empty()
                || callback.templates.len() != 1
                || !callback.navigation.is_empty()
                || !callback.pages.is_empty()
                || callback.clipboard_text.is_some()
                || callback.host_mutation.is_some()
                || callback.host_session.is_some()
                || callback.host_dom_operations.is_some()
                || callback.terminal_input_suggestion.is_some()
                || callback.isolated_surface.is_some()
                || callback.ssh_sync_request.is_some()
                || callback.remote_operation.is_some()
                || callback.resource_operation.is_some()
                || callback.api_call.is_some()
                || callback.ui_state.is_some()
                || (callback.storage_write.is_some() && storage_snapshot.is_none())
            {
                self.clear_ui_action_in_flight(&request);
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(plugin_runtime_error(
                    request.meta.request_id,
                    Some(PluginHostProcessError::Rejected),
                ));
            }
            parsed.templates = callback.templates;
            parsed.storage_write = callback.storage_write;
            storage_write_token = callback_request_id;
            Some(status)
        } else {
            None
        };
        let host_dom_operations = match self.authorize_host_dom_operations(
            &request,
            host_dom_snapshot.as_ref(),
            parsed.host_dom_operations.clone(),
        ) {
            Ok(operations) => operations,
            Err(error) => {
                self.clear_ui_action_in_flight(&request);
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(error);
            }
        };
        let terminal_input_suggestion =
            if let Some(suggestion) = parsed.terminal_input_suggestion.clone() {
                if !request.target_id.as_str().starts_with("terminal.")
                    || self
                        .has_capability(
                            request.meta.request_id.clone(),
                            &request.plugin_id,
                            &request.signer_fingerprint_sha256,
                            PluginCapability::TerminalProposeInput,
                        )
                        .is_err()
                {
                    self.clear_ui_action_in_flight(&request);
                    self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                    return Err(plugin_permission_error(request.meta.request_id));
                }
                Some(suggestion)
            } else {
                None
            };
        let isolated_surface = match self.prepare_isolated_surface(
            &request,
            &installed,
            parsed.isolated_surface.clone(),
        ) {
            Ok(surface) => surface,
            Err(error) => {
                self.clear_ui_action_in_flight(&request);
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(error);
            }
        };
        let pending_host_approval = match self.prepare_host_approval(&request, &parsed, &installed)
        {
            Ok(approval) => approval,
            Err(error) => {
                self.clear_ui_action_in_flight(&request);
                return Err(error);
            }
        };
        let mut template = parsed
            .templates
            .into_iter()
            .next()
            .ok_or_else(|| plugin_validation_error(request.meta.request_id.clone()))?;
        if template.target_id != request.target_id {
            self.clear_ui_action_in_flight(&request);
            return Err(plugin_validation_error(request.meta.request_id));
        }
        if !ssh_sync_browser_nodes_supported(
            &template.document,
            installed.capabilities.contains(&PluginCapability::SshSync),
        ) {
            self.clear_ui_action_in_flight(&request);
            self.record_runtime_failure(&request.plugin_id, request.instance_generation);
            return Err(plugin_runtime_error(
                request.meta.request_id,
                Some(PluginHostProcessError::Rejected),
            ));
        }
        if !self.settings_target_visible(
            request.meta.request_id.clone(),
            &installed,
            settings_revision,
            request.target_id.as_str(),
        )? {
            self.clear_ui_action_in_flight(&request);
            return Err(plugin_conflict_error(
                request.meta.request_id,
                settings_revision,
            ));
        }
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .mutation_reservations
            .contains_key(request.plugin_id.as_str())
        {
            drop(runtime);
            self.clear_ui_action_in_flight(&request);
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let Some(active_target) = runtime.target_contexts.get(request.context_handle.as_str())
        else {
            drop(runtime);
            self.clear_ui_action_in_flight(&request);
            return Err(plugin_conflict_error(request.meta.request_id, None));
        };
        if active_target.context.target_id != request.target_id
            || active_target.context.target_revision != request.expected_target_revision
        {
            let revision = active_target.context.target_revision;
            drop(runtime);
            self.clear_ui_action_in_flight(&request);
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(revision),
            ));
        }
        let target = active_target.context.clone();
        if pending_host_approval.is_some() {
            if !explicit_user_action {
                drop(runtime);
                self.clear_ui_action_in_flight(&request);
                return Err(plugin_error(
                    request.meta.request_id,
                    "plugin.host_approval_interaction_required",
                    ErrorCategory::Permission,
                    RetryStrategy::Never,
                    "plugins.remoteApproval.interactionRequired",
                    None,
                ));
            }
            let now = unix_time_ms();
            runtime
                .pending_host_approvals
                .retain(|_, pending| pending.summary.expires_at_unix_ms > now);
            if runtime.pending_host_approvals.len() >= PLUGIN_HOST_APPROVAL_LIMIT {
                drop(runtime);
                self.clear_ui_action_in_flight(&request);
                return Err(plugin_error(
                    request.meta.request_id,
                    "plugin.host_approval_queue_full",
                    ErrorCategory::Unavailable,
                    RetryStrategy::AfterMilliseconds(1_000),
                    "errors.plugin.hostApprovalQueueFull",
                    None,
                ));
            }
        }
        let Some(instance) = runtime.active_instances.get_mut(request.plugin_id.as_str()) else {
            return Err(plugin_not_found_error(request.meta.request_id));
        };
        if instance.signer_fingerprint_sha256 != request.signer_fingerprint_sha256
            || instance.package_sha256 != request.expected_package_sha256
            || instance.instance_generation != request.instance_generation
            || instance.state_version != request.expected_state_version
            || instance.contribution_revision != request.expected_contribution_revision
            || !instance.contribution_action_in_flight
        {
            return Err(plugin_conflict_error(
                request.meta.request_id,
                Some(instance.contribution_revision),
            ));
        }
        if page_id.is_none()
            && let Some(previous) = template_presentation_for_action(
                &instance.ui_templates,
                &instance.scoped_templates,
                definition.contextual,
                request.target_id.as_str(),
                request.context_handle.as_str(),
            )
        {
            retain_template_presentation(previous, &mut template);
        }
        if validate_template_lifecycle(&template).is_err() {
            instance.contribution_action_in_flight = false;
            drop(runtime);
            self.record_runtime_failure(&request.plugin_id, request.instance_generation);
            return Err(plugin_runtime_error(
                request.meta.request_id,
                Some(PluginHostProcessError::Rejected),
            ));
        }
        if let Some(page_id) = page_id.as_ref() {
            let Some(previous) = instance.pages.get(page_id) else {
                instance.contribution_action_in_flight = false;
                return Err(plugin_conflict_error(request.meta.request_id, None));
            };
            let mut replacement = previous.clone();
            replacement.document = template.document.clone();
            if norishell_plugin_platform::validate_plugin_page_lifecycle(&replacement).is_err() {
                instance.contribution_action_in_flight = false;
                drop(runtime);
                self.record_runtime_failure(&request.plugin_id, request.instance_generation);
                return Err(plugin_runtime_error(
                    request.meta.request_id,
                    Some(PluginHostProcessError::Rejected),
                ));
            }
        }
        if let Some(write) = parsed.storage_write.as_ref() {
            let Some(expected_revision) =
                storage_snapshot.as_ref().map(|snapshot| snapshot.revision)
            else {
                instance.contribution_action_in_flight = false;
                return Err(plugin_permission_error(request.meta.request_id.clone()));
            };
            if write.write_token != storage_write_token {
                instance.contribution_action_in_flight = false;
                return Err(plugin_conflict_error(request.meta.request_id, None));
            }
            if let Err(error) = self.hosts.with_plugin_repository(|repository| {
                repository.replace_plugin_private_persistent_state(
                    &PluginPrivateStorageOwner {
                        plugin_id: request.plugin_id.clone(),
                        signer_fingerprint_sha256: request.signer_fingerprint_sha256.clone(),
                    },
                    expected_revision,
                    &write.value_json,
                    &|| {
                        instance
                            .api_authority
                            .load(std::sync::atomic::Ordering::Acquire)
                    },
                )
            }) {
                instance.contribution_action_in_flight = false;
                return Err(map_persistence_error(
                    request.meta.request_id.clone(),
                    error,
                ));
            }
        }
        if let Some(page_id) = page_id.as_ref() {
            let Some(page) = instance.pages.get_mut(page_id) else {
                return Err(plugin_conflict_error(request.meta.request_id, None));
            };
            page.document = template.document.clone();
        } else {
            if definition.contextual {
                instance
                    .scoped_templates
                    .insert(request.context_handle.as_str().to_owned(), template.clone());
            } else {
                instance
                    .ui_templates
                    .insert(request.target_id.as_str().to_owned(), template.clone());
            }
        }
        let on_open_action_id = if let Some(page_id) = page_id.as_ref() {
            instance
                .pages
                .get(page_id)
                .and_then(|page| page.on_open_action_id.clone())
        } else {
            template_on_open_action_id(&template)
        };
        instance.contribution_revision =
            WireSequence::new(instance.contribution_revision.get().saturating_add(1));
        if let Some(state) = parsed.ui_state {
            if definition.contextual {
                instance
                    .scoped_states
                    .insert(request.context_handle.as_str().to_owned(), state.value_json);
            } else {
                instance.ui_state_json = state.value_json;
            }
        }
        instance.contribution_action_in_flight = false;
        let contribution = PluginUiContribution {
            plugin_id: request.plugin_id.clone(),
            plugin_name: instance.plugin_name.clone(),
            signer_fingerprint_sha256: instance.signer_fingerprint_sha256.clone(),
            package_sha256: instance.package_sha256.clone(),
            instance_generation: instance.instance_generation,
            state_version: instance.state_version,
            contribution_revision: instance.contribution_revision,
            target,
            on_open_action_id,
            auto_refresh: template_auto_refresh(&template),
            route_paths: template.route_paths,
            icon: template.icon,
            document: template.document,
        };
        let mut host_approval_id = pending_host_approval
            .as_ref()
            .map(|approval| approval.summary.approval_id.clone());
        let automatically_approved = pending_host_approval
            .as_ref()
            .is_some_and(|approval| approval.approved_operation_policy.is_some());
        if let Some(approval) = pending_host_approval {
            runtime
                .pending_host_approvals
                .insert(approval.summary.approval_id.as_str().to_owned(), approval);
        }
        drop(runtime);
        let mut auto_session_launch = None;
        if automatically_approved {
            if !explicit_user_action {
                return Err(plugin_error(
                    request.meta.request_id,
                    "plugin.host_approval_interaction_required",
                    ErrorCategory::Permission,
                    RetryStrategy::Never,
                    "plugins.remoteApproval.interactionRequired",
                    None,
                ));
            }
            let approval_id = host_approval_id
                .as_ref()
                .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
            let decision = self.decide_host_approval(PluginHostApprovalDecisionRequest {
                meta: RequestMeta {
                    request_id: request.meta.request_id.clone(),
                },
                approval_id: approval_id.clone(),
                decision: PluginApprovalDecision::Approve,
                expected_state_version: WireSequence::new(1),
                policy: norishell_core_api::PluginApprovalPolicy::Always,
                expiry: norishell_core_api::PluginApprovalExpiry::Unlimited,
            })?;
            host_approval_id = None;
            auto_session_launch = decision.session_launch;
        }
        if parsed.resource_navigation.is_some() || parsed.terminal_launch.is_some() {
            let app = self
                .app_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .ok_or_else(|| plugin_runtime_error(request.meta.request_id.clone(), None))?;
            if let Some(navigation) = parsed.resource_navigation.as_ref() {
                app.emit_to("main", "plugin-host-navigation-requested", navigation)
                    .map_err(|_| plugin_runtime_error(request.meta.request_id.clone(), None))?;
            }
            if let Some(launch) = parsed.terminal_launch.as_ref() {
                app.emit_to("main", "plugin-terminal-channel-approved", launch)
                    .map_err(|_| plugin_runtime_error(request.meta.request_id.clone(), None))?;
            }
            if let Some(launch) = auto_session_launch.as_ref() {
                app.emit_to("main", "plugin-host-session-approved", launch)
                    .map_err(|_| plugin_runtime_error(request.meta.request_id.clone(), None))?;
            }
        }
        if parsed.resource_navigation.is_none()
            && parsed.terminal_launch.is_none()
            && let Some(launch) = auto_session_launch.as_ref()
        {
            let app = self
                .app_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .ok_or_else(|| plugin_runtime_error(request.meta.request_id.clone(), None))?;
            app.emit_to("main", "plugin-host-session-approved", launch)
                .map_err(|_| plugin_runtime_error(request.meta.request_id.clone(), None))?;
        }

        let isolated_surface_opened = if let Some((surface, html)) = isolated_surface {
            self.open_isolated_surface(&request, surface, html)?;
            true
        } else {
            false
        };
        Ok(PluginUiActionResponse {
            contribution,
            clipboard_text: parsed.clipboard_text,
            host_approval_id,
            host_dom_operations,
            terminal_input_suggestion,
            isolated_surface_opened,
            ssh_sync_status,
        })
    }
}

impl PluginService {
    fn list_operation_permissions(
        &self,
        request: PluginOperationPermissionListRequest,
    ) -> CoreResult<PluginOperationPermissionList> {
        self.require_ready(request.meta.request_id.clone())?;
        self.operation_policies
            .list(&request.plugin_id)
            .map_err(|error| map_persistence_error(request.meta.request_id, error))
    }

    fn revoke_operation_permission(
        &self,
        request: PluginOperationPermissionRevokeRequest,
    ) -> CoreResult<()> {
        self.require_ready(request.meta.request_id.clone())?;
        self.operation_policies
            .revoke(
                &request.plugin_id,
                &request.permission_id,
                request.expected_policy_revision,
            )
            .map(|_| ())
            .map_err(|error| map_persistence_error(request.meta.request_id, error))
    }

    fn clear_operation_permissions(
        &self,
        request: PluginOperationPermissionsClearRequest,
    ) -> CoreResult<()> {
        self.require_ready(request.meta.request_id.clone())?;
        self.operation_policies
            .clear(&request.plugin_id, request.expected_policy_revision)
            .map_err(|error| map_persistence_error(request.meta.request_id, error))
    }
}

fn require_main_plugin_management_window<R: tauri::Runtime>(
    window: &WebviewWindow<R>,
    request_id: RequestId,
) -> CoreResult<()> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(plugin_permission_error(request_id))
    }
}

#[tauri::command]
pub(crate) fn plugin_installed_list(
    request: PluginInstalledListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<InstalledPluginSummary>> {
    service.list_installed(request.meta.request_id)
}

#[tauri::command]
pub(crate) fn plugin_theme_list<R: tauri::Runtime>(
    request: PluginThemeListRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<PluginThemeListResponse> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    service.list_themes(request.meta.request_id)
}

#[tauri::command]
pub(crate) fn plugin_readiness_get(
    request: PluginReadinessGetRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginReadiness> {
    service.require_ready(request.meta.request_id)?;
    Ok(service.readiness())
}

#[tauri::command]
pub(crate) fn plugin_audit_list(
    request: PluginAuditListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<PluginAuditEntry>> {
    service.require_ready(request.meta.request_id.clone())?;
    service
        .hosts
        .with_plugin_repository(|repository| repository.list_plugin_audit(request.limit))
        .map_err(|error| map_persistence_error(request.meta.request_id, error))
        .map(|records| {
            records
                .into_iter()
                .map(|record| PluginAuditEntry {
                    audit_id: record.audit_id,
                    plugin_id: record.plugin_id,
                    operation_id: record.operation_id,
                    action: record.action,
                    outcome: record.outcome,
                    detail_code: record.detail_code,
                    created_at_unix_ms: record.created_at_unix_ms,
                })
                .collect()
        })
}

#[tauri::command]
pub(crate) fn plugin_safe_mode_next_start(
    request: PluginSafeModeNextStartRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginReadiness> {
    service.set_safe_mode_next_start(request.enabled, request.meta.request_id)
}

#[tauri::command]
pub(crate) fn plugin_safe_mode_startup_complete(
    request: PluginReadinessGetRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginReadiness> {
    service
        .complete_safe_mode_startup()
        .map_err(|_| plugin_validation_error(request.meta.request_id))?;
    Ok(service.readiness())
}

#[tauri::command]
pub(crate) fn plugin_contribution_list(
    request: PluginContributionListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<PluginContributionPanel>> {
    service.list_contributions(request.meta.request_id)
}

#[tauri::command]
pub(crate) async fn plugin_contribution_invoke(
    request: PluginContributionInvokeRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginContributionPanel> {
    service.invoke_contribution(request).await
}

#[tauri::command]
pub(crate) fn plugin_contribution_copy(
    request: PluginContributionCopyRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginContributionCopyResponse> {
    service.prepare_contribution_copy(request)
}

#[tauri::command]
pub(crate) fn plugin_extension_target_list(
    request: PluginExtensionTargetListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<norishell_core_api::PluginExtensionTargetDefinition>> {
    service.require_ready(request.meta.request_id)?;
    Ok(plugin_extension_registry::definitions())
}

#[tauri::command]
pub(crate) fn plugin_target_context_open(
    request: PluginTargetContextOpenRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginExtensionTargetContext> {
    service.open_target_context(request)
}

#[tauri::command]
pub(crate) fn plugin_target_context_close(
    request: PluginTargetContextCloseRequest,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    service.close_target_context(request)
}

#[tauri::command]
pub(crate) fn plugin_ui_contribution_list(
    request: PluginUiContributionListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<PluginUiContribution>> {
    service.list_ui_contributions(request)
}

#[tauri::command]
pub(crate) fn plugin_navigation_list(
    request: PluginNavigationListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<PluginNavigationItem>> {
    service.list_navigation(request.meta.request_id)
}

#[tauri::command]
pub(crate) fn plugin_host_scope_list(
    request: PluginHostScopeListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginHostScopeSnapshot> {
    service.list_host_scopes(request)
}

#[tauri::command]
pub(crate) fn plugin_host_scope_replace(
    request: PluginHostScopeReplaceRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginHostScopeSnapshot> {
    let operation_id = Uuid::new_v4();
    let _reservation = service
        .reserve_plugin_mutation(&request.plugin_id, operation_id)
        .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    if service
        .runtime
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .active_instances
        .get(request.plugin_id.as_str())
        .is_some_and(|instance| instance.contribution_action_in_flight)
    {
        return Err(plugin_conflict_error(request.meta.request_id, None));
    }
    service.replace_host_scopes(request)
}

fn secure_host_approval_window_label(approval_id: &PluginApprovalId) -> String {
    format!("secure-plugin-host-{}", approval_id.as_str())
}

fn require_secure_host_approval_window(
    window: &WebviewWindow,
    approval_id: &PluginApprovalId,
    request_id: RequestId,
) -> CoreResult<()> {
    if window.label() == secure_host_approval_window_label(approval_id) {
        Ok(())
    } else {
        Err(plugin_permission_error(request_id))
    }
}

fn secure_special_permission_window_label(approval_id: &PluginApprovalId) -> String {
    format!("secure-plugin-permission-{}", approval_id.as_str())
}

fn require_secure_special_permission_window(
    window: &WebviewWindow,
    approval_id: &PluginApprovalId,
    request_id: RequestId,
) -> CoreResult<()> {
    if window.label() == secure_special_permission_window_label(approval_id) {
        Ok(())
    } else {
        Err(plugin_permission_error(request_id))
    }
}

#[tauri::command]
// Creating a WebView2 window on Windows must not run on the synchronous IPC main thread, or it deadlocks.
pub(crate) async fn plugin_host_approval_open(
    request: PluginHostApprovalGetRequest,
    app: AppHandle,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    service.host_approval_snapshot(request.clone())?;
    let label = secure_host_approval_window_label(&request.approval_id);
    if let Some(window) = app.get_webview_window(&label) {
        crate::window_first_show::show_if_revealed(&window)
            .map_err(|_| plugin_runtime_error(request.meta.request_id, None))?;
        return Ok(());
    }
    crate::secure_window_frame::apply_secure_window_frame(
        &app,
        &label,
        WebviewWindowBuilder::new(
            &app,
            label.clone(),
            WebviewUrl::App(
                format!(
                    "secure-plugin-host-approval.html?approvalId={}",
                    request.approval_id.as_str()
                )
                .into(),
            ),
        ),
    )
    .title("NoriShell")
    .resizable(false)
    .center()
    .build()
    .map_err(|_| plugin_runtime_error(request.meta.request_id, None))?;
    Ok(())
}

#[tauri::command]
pub(crate) fn plugin_host_approval_get(
    request: PluginHostApprovalGetRequest,
    window: WebviewWindow,
    service: State<'_, PluginService>,
) -> CoreResult<PluginHostApprovalSummary> {
    require_secure_host_approval_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    service.host_approval_snapshot(request)
}

#[tauri::command]
pub(crate) fn plugin_host_approval_decide(
    request: PluginHostApprovalDecisionRequest,
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, PluginService>,
) -> CoreResult<PluginHostApprovalDecisionResponse> {
    require_secure_host_approval_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    let summary = service.host_approval_snapshot(PluginHostApprovalGetRequest {
        meta: RequestMeta {
            request_id: request.meta.request_id.clone(),
        },
        approval_id: request.approval_id.clone(),
    })?;
    let operation_id = Uuid::new_v4();
    let _reservation = service
        .reserve_plugin_mutation(&summary.plugin_id, operation_id)
        .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    let response = service.decide_host_approval(request)?;
    if let Some(launch) = &response.session_launch {
        app.emit_to("main", "plugin-host-session-approved", launch)
            .map_err(|_| plugin_runtime_error(RequestId::new(), None))?;
    }
    Ok(response)
}

#[tauri::command]
// Creating a WebView2 window on Windows must not run on the synchronous IPC main thread, or it deadlocks.
pub(crate) async fn plugin_special_permission_open(
    request: PluginSpecialPermissionOpenRequest,
    app: AppHandle,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    service
        .open_special_permission_window(request, app)
        .map(|_| ())
}

#[tauri::command]
pub(crate) fn plugin_special_permission_get(
    request: PluginSpecialPermissionGetRequest,
    window: WebviewWindow,
    service: State<'_, PluginService>,
) -> CoreResult<PluginSpecialPermissionSnapshot> {
    require_secure_special_permission_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    service.special_permission_snapshot(request)
}

#[tauri::command]
pub(crate) async fn plugin_special_permission_decide(
    request: PluginSpecialPermissionDecisionRequest,
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, PluginService>,
) -> CoreResult<PluginSpecialPermissionDecisionResponse> {
    require_secure_special_permission_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    let snapshot = service.special_permission_snapshot(PluginSpecialPermissionGetRequest {
        meta: RequestMeta {
            request_id: request.meta.request_id.clone(),
        },
        approval_id: request.approval_id.clone(),
    })?;
    let operation_id = Uuid::new_v4();
    let _reservation = service
        .reserve_plugin_mutation(&snapshot.plugin_id, operation_id)
        .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    let response = service.decide_special_permission(request).await?;
    let _ = app.emit_to(
        "main",
        "plugin-special-permission-changed",
        &response.target,
    );
    Ok(response)
}

#[tauri::command]
pub(crate) async fn plugin_ui_action(
    request: PluginUiActionRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginUiActionResponse> {
    // Keep the full broker state machine out of Tauri's IPC dispatch future.
    // Moving that future through the Windows UI thread exhausted its 1 MiB stack.
    Box::pin(service.invoke_ui_action(request)).await
}

#[tauri::command]
pub(crate) async fn plugin_ssh_sync_browser_read(
    request: PluginSshSyncBrowserReadRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginSshSyncBrowserSnapshot> {
    let request_id = request.meta.request_id.clone();
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.read_ssh_sync_browser(request))
        .await
        .map_err(|_| plugin_runtime_error(request_id, None))?
}

#[tauri::command]
pub(crate) fn plugin_locale_set(
    request: PluginLocaleSetRequest,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    service.set_locale(request.locale, request.meta.request_id)
}

#[tauri::command]
pub(crate) fn plugin_isolated_surface_content(
    request: PluginIsolatedSurfaceContentRequest,
    window: WebviewWindow,
    service: State<'_, PluginService>,
) -> CoreResult<PluginIsolatedSurfaceContent> {
    service.isolated_surface_content(request, &window)
}

#[tauri::command]
pub(crate) async fn plugin_local_package_prepare(
    request: PluginLocalPackagePrepareRequest,
    app: AppHandle,
    service: State<'_, PluginService>,
) -> CoreResult<Option<PluginLocalPackagePreview>> {
    // Let the Core importer validate the selected bytes and ZIP structure.
    let selected = app.dialog().file().blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
    service
        .prepare_local_package(&path, request.meta.request_id)
        .map(Some)
}

#[tauri::command]
pub(crate) fn plugin_local_install(
    request: PluginLocalInstallRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginOperationSummary> {
    service.install_local_plugin(request)
}

#[tauri::command]
pub(crate) fn plugin_local_package_cancel(
    request: PluginLocalPackageCancelRequest,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    if request.preparation_id.len() > 80 {
        return Err(plugin_validation_error(request.meta.request_id));
    }
    service.cancel_prepared_package(&request.preparation_id);
    Ok(())
}

#[tauri::command]
pub(crate) async fn plugin_capability_grants_replace(
    request: PluginCapabilityGrantsReplaceRequest,
    service: State<'_, PluginService>,
) -> CoreResult<InstalledPluginSummary> {
    let request_id = request.meta.request_id.clone();
    service.require_ready(request_id.clone())?;
    let mutation_operation = Uuid::new_v4();
    let _mutation_reservation = service
        .reserve_plugin_mutation(&request.plugin_id, mutation_operation)
        .ok_or_else(|| plugin_conflict_error(request_id.clone(), None))?;
    let (mut installed, current_grants, major) = service
        .hosts
        .with_plugin_repository(|repository| {
            let installed = repository.get_plugin_installation(&request.plugin_id)?;
            if installed.state_version != request.expected_state_version {
                return Err(AppPersistenceError::Conflict);
            }
            let major = u64::from(PLUGIN_PROTOCOL_MAJOR);
            let current = repository.list_plugin_capability_grants(
                &request.plugin_id,
                &installed.signer_fingerprint_sha256,
                major,
            )?;
            Ok((installed, current, major))
        })
        .map_err(|error| map_persistence_error(request_id.clone(), error))?;
    if !explicit_capability_decision(&installed.capabilities, &request.grants) {
        return Err(plugin_validation_error(request_id));
    }
    if !requested_special_grants_do_not_expand(&installed, &current_grants, &request.grants) {
        return Err(plugin_permission_error(request_id));
    }
    let unchanged =
        plugin_grant_request_is_effectively_unchanged(&installed, &current_grants, &request.grants);
    if unchanged {
        return service.installed_summary(request_id, installed);
    }
    if installed.state == PluginInstallState::Enabled {
        service.stop_plugin(&installed.plugin_id).await?;
        installed = service.set_plugin_state_convergent(
            request_id.clone(),
            &installed,
            PluginInstallState::Disabled,
        )?;
    }
    let expected_grant_version = current_grants.first().map(|grant| grant.state_version);
    let decisions = request
        .grants
        .iter()
        .map(|grant| (grant.capability, grant.granted))
        .collect::<Vec<_>>();
    let grants = service.replace_plugin_grants_convergent(
        request_id.clone(),
        &installed,
        major,
        expected_grant_version,
        &current_grants,
        &decisions,
    )?;
    let _ = grants;
    service.installed_summary(request_id, installed)
}

#[tauri::command]
pub(crate) async fn plugin_enable(
    request: PluginStateChangeRequest,
    service: State<'_, PluginService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<InstalledPluginSummary> {
    service.require_ready(request.meta.request_id.clone())?;
    let enable_operation = Uuid::new_v4();
    let _mutation_reservation = service
        .reserve_plugin_mutation(&request.plugin_id, enable_operation)
        .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    let installed = service
        .hosts
        .with_plugin_repository(|repository| repository.get_plugin_installation(&request.plugin_id))
        .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
    let restoring_enabled = installed.state == PluginInstallState::Enabled;
    if installed.state_version != request.expected_state_version
        || service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_instances
            .contains_key(installed.plugin_id.as_str())
    {
        return Err(plugin_conflict_error(
            request.meta.request_id,
            Some(installed.state_version),
        ));
    }
    let package_kind = service
        .installer
        .active_package_kind(
            &installed.plugin_id,
            &installed.active_version,
            &installed.package_sha256,
        )
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
    if package_kind == PluginPackageKind::Theme {
        let manifest = service
            .installer
            .read_active_manifest(
                &installed.plugin_id,
                &installed.active_version,
                &installed.package_sha256,
            )
            .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
        if !norishell_plugin_platform::plugin_core_api_compatible(&manifest) {
            return Err(platform_request_error(
                request.meta.request_id,
                &PluginPlatformError::CoreApiIncompatible,
            ));
        }
        if manifest.name != installed.name
            || manifest.publisher != installed.publisher
            || manifest.capabilities != installed.capabilities
        {
            return Err(plugin_validation_error(request.meta.request_id));
        }
        let record = if restoring_enabled {
            installed
        } else {
            service.set_plugin_state_convergent(
                request.meta.request_id.clone(),
                &installed,
                PluginInstallState::Enabled,
            )?
        };
        return service.installed_summary(request.meta.request_id, record);
    }
    // Hold the application-exit creation gate through spawn, initialization,
    // durable state change, and active-instance registration. Cleanup cannot
    // authorize exit while an unregistered Plugin Host can still appear.
    let _creation_permit = lifecycle.acquire_resource_creation(request.meta.request_id.clone())?;
    let module = service
        .installer
        .read_active_module(
            &installed.plugin_id,
            &installed.active_version,
            &installed.package_sha256,
            16 * 1024 * 1024,
        )
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
    let manifest = service
        .installer
        .read_active_manifest(
            &installed.plugin_id,
            &installed.active_version,
            &installed.package_sha256,
        )
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
    if !norishell_plugin_platform::plugin_core_api_compatible(&manifest) {
        return Err(platform_request_error(
            request.meta.request_id,
            &PluginPlatformError::CoreApiIncompatible,
        ));
    }
    if manifest.name != installed.name
        || manifest.publisher != installed.publisher
        || manifest.capabilities != installed.capabilities
    {
        return Err(plugin_validation_error(request.meta.request_id));
    }
    if !protocol_is_compatible(manifest.protocol_major, manifest.protocol_minor)
        || !capabilities_match_protocol(&manifest.capabilities, manifest.protocol_minor)
    {
        return Err(platform_request_error(
            request.meta.request_id,
            &PluginPlatformError::InvalidProtocolCatalog,
        ));
    }
    let protocol_minor = manifest.protocol_minor;
    let locale = service.locale();
    let settings_snapshot =
        service.settings_snapshot_for_installation(request.meta.request_id.clone(), &installed)?;
    let settings_revision = settings_snapshot.as_ref().map(|snapshot| snapshot.revision);
    let settings_projection = settings_snapshot.map(|snapshot| {
        serde_json::json!({
            "revision": snapshot.revision,
            "values": snapshot.values,
        })
    });
    let instance_generation = if restoring_enabled {
        installed.state_version
    } else {
        WireSequence::new(installed.state_version.get().saturating_add(1))
    };
    let previously_crashed = installed.state == PluginInstallState::Crashed;
    let initialize_request_id = Uuid::new_v4().to_string();
    let ui_panel_granted = service.capability_granted_for_record(
        request.meta.request_id.clone(),
        &installed,
        PluginCapability::UiPanel,
    )?;
    let ui_navigation_granted = service.capability_granted_for_record(
        request.meta.request_id.clone(),
        &installed,
        PluginCapability::UiNavigation,
    )?;
    let ui_page_granted = service.capability_granted_for_record(
        request.meta.request_id.clone(),
        &installed,
        PluginCapability::UiPage,
    )?;
    let terminal_annotation_granted = service.capability_granted_for_record(
        request.meta.request_id.clone(),
        &installed,
        PluginCapability::TerminalAnnotation,
    )?;
    let storage_granted = service.capability_granted_for_record(
        request.meta.request_id.clone(),
        &installed,
        PluginCapability::StoragePlugin,
    )?;
    let ssh_sync_declared = installed.capabilities.contains(&PluginCapability::SshSync);
    let storage = if storage_granted {
        Some(service.plugin_storage_snapshot(request.meta.request_id.clone(), &installed)?)
    } else {
        None
    };
    let initialize = PluginHostRequest {
        protocol_major: PLUGIN_PROTOCOL_MAJOR,
        protocol_minor,
        request_id: initialize_request_id.clone(),
        kind: PluginHostMessageKind::Initialize,
        payload_json: plugin_host_payload_with_settings(
            &locale,
            serde_json::json!({
                "pluginId": installed.plugin_id.as_str(),
                "version": installed.active_version,
                "storage": storage,
            }),
            settings_projection.as_ref(),
        ),
    };
    let host_and_contributions_result = tauri::async_runtime::spawn_blocking(move || {
        let mut host = PluginHostProcess::spawn(
            &module,
            instance_generation.get(),
            manifest.protocol_major,
            protocol_minor,
        )
        .map_err(|error| (error, None))?;
        let outputs = match host.execute(initialize) {
            Ok(outputs) => outputs,
            Err(error) => {
                return if host.shutdown().is_ok() {
                    Err((error, None))
                } else {
                    Err((PluginHostProcessError::Cleanup, Some(Box::new(host))))
                };
            }
        };
        let (contributions, ui_templates, navigation, pages) =
            match parse_plugin_ui_outputs(&initialize_request_id, outputs).and_then(|parsed| {
                if parsed.clipboard_text.is_some()
                    || parsed.host_mutation.is_some()
                    || parsed.host_session.is_some()
                    || parsed.host_dom_operations.is_some()
                    || parsed.terminal_input_suggestion.is_some()
                    || parsed.isolated_surface.is_some()
                    || parsed.storage_write.is_some()
                    || parsed.ssh_sync_request.is_some()
                    || parsed.remote_operation.is_some()
                    || parsed.resource_operation.is_some()
                    || parsed.api_call.is_some()
                    || parsed.ui_state.is_some()
                    || (!parsed.navigation.is_empty() && !ui_navigation_granted)
                    || (!parsed.pages.is_empty() && !ui_page_granted)
                    || parsed.pages.iter().any(|page| {
                        !ssh_sync_browser_nodes_supported(&page.document, ssh_sync_declared)
                    })
                {
                    return Err(());
                }
                let contributions = contribution_map(parsed.panels)?;
                let ui_templates = ui_template_map(parsed.templates)?;
                let navigation = navigation_map(parsed.navigation)?;
                let pages = page_map(parsed.pages)?;
                Ok((contributions, ui_templates, navigation, pages))
            }) {
                Ok((contributions, ui_templates, navigation, pages))
                    if contributions.is_empty() || ui_panel_granted =>
                {
                    let targets_supported = ui_templates.values().all(|template| {
                        plugin_extension_registry::find(&template.target_id).is_some_and(
                            |definition| match definition.required_capability {
                                PluginCapability::UiPanel => ui_panel_granted,
                                PluginCapability::TerminalAnnotation => terminal_annotation_granted,
                                _ => false,
                            },
                        )
                    });
                    if !targets_supported {
                        return if host.shutdown().is_ok() {
                            Err((PluginHostProcessError::Rejected, None))
                        } else {
                            Err((PluginHostProcessError::Cleanup, Some(Box::new(host))))
                        };
                    }
                    (contributions, ui_templates, navigation, pages)
                }
                Ok(_) | Err(()) => {
                    return if host.shutdown().is_ok() {
                        Err((PluginHostProcessError::Rejected, None))
                    } else {
                        Err((PluginHostProcessError::Cleanup, Some(Box::new(host))))
                    };
                }
            };
        Ok::<_, (PluginHostProcessError, Option<Box<PluginHostProcess>>)>((
            host,
            contributions,
            ui_templates,
            navigation,
            pages,
        ))
    })
    .await
    .map_err(|_| plugin_runtime_error(request.meta.request_id.clone(), None))?;
    let host_and_contributions = match host_and_contributions_result {
        Ok(value) => value,
        Err((error, cleanup_pending)) => {
            if let Some(host) = cleanup_pending {
                let committed = service.commit_enable_instance(
                    &installed.plugin_id,
                    enable_operation,
                    ActivePluginInstance {
                        signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
                        package_sha256: installed.package_sha256.clone(),
                        instance_generation,
                        state_version: installed.state_version,
                        previously_crashed,
                        protocol_minor,
                        locale: locale.clone(),
                        plugin_name: installed.name.clone(),
                        contributions: BTreeMap::new(),
                        ui_templates: BTreeMap::new(),
                        navigation: BTreeMap::new(),
                        pages: BTreeMap::new(),
                        contribution_revision: WireSequence::new(1),
                        settings_revision,
                        contribution_action_in_flight: false,
                        ui_state_json: "{}".to_owned(),
                        scoped_templates: BTreeMap::new(),
                        scoped_states: BTreeMap::new(),
                        operation_authority_revoked: false,
                        api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                        process: Arc::new(Mutex::new(Some(*host))),
                    },
                );
                debug_assert!(committed, "enable reservation must retain cleanup owner");
            }
            return Err(plugin_runtime_error(request.meta.request_id, Some(error)));
        }
    };
    let (host, contributions, ui_templates, navigation, pages) = host_and_contributions;
    let process = Arc::new(Mutex::new(Some(host)));
    let record_result = if restoring_enabled {
        Ok(installed.clone())
    } else {
        service.set_plugin_state_convergent(
            request.meta.request_id.clone(),
            &installed,
            PluginInstallState::Enabled,
        )
    };
    let record = match record_result {
        Ok(record) => record,
        Err(error) => {
            let cleanup_process = process.clone();
            let cleanup_succeeded = tauri::async_runtime::spawn_blocking(move || {
                let mut process = cleanup_process
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(host) = process.as_mut() else {
                    return true;
                };
                if host.shutdown().is_ok() {
                    process.take();
                    true
                } else {
                    false
                }
            })
            .await
            .unwrap_or(false);
            if !cleanup_succeeded {
                let committed = service.commit_enable_instance(
                    &installed.plugin_id,
                    enable_operation,
                    ActivePluginInstance {
                        signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
                        package_sha256: installed.package_sha256.clone(),
                        instance_generation,
                        state_version: installed.state_version,
                        previously_crashed,
                        protocol_minor,
                        locale: locale.clone(),
                        plugin_name: installed.name.clone(),
                        contributions: BTreeMap::new(),
                        ui_templates: BTreeMap::new(),
                        navigation: BTreeMap::new(),
                        pages: BTreeMap::new(),
                        contribution_revision: WireSequence::new(1),
                        settings_revision,
                        contribution_action_in_flight: false,
                        ui_state_json: "{}".to_owned(),
                        scoped_templates: BTreeMap::new(),
                        scoped_states: BTreeMap::new(),
                        operation_authority_revoked: false,
                        api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                        process,
                    },
                );
                debug_assert!(committed, "enable reservation must retain cleanup owner");
            }
            return Err(error);
        }
    };
    let committed = service.commit_enable_instance(
        &record.plugin_id,
        enable_operation,
        ActivePluginInstance {
            signer_fingerprint_sha256: record.signer_fingerprint_sha256.clone(),
            package_sha256: record.package_sha256.clone(),
            instance_generation,
            state_version: record.state_version,
            previously_crashed,
            protocol_minor,
            locale,
            plugin_name: record.name.clone(),
            contributions,
            ui_templates,
            navigation,
            pages,
            contribution_revision: WireSequence::new(1),
            settings_revision,
            contribution_action_in_flight: false,
            ui_state_json: "{}".to_owned(),
            scoped_templates: BTreeMap::new(),
            scoped_states: BTreeMap::new(),
            operation_authority_revoked: false,
            api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            process,
        },
    );
    if !committed {
        return Err(plugin_runtime_error(request.meta.request_id, None));
    }
    service.installed_summary(request.meta.request_id, record)
}

#[tauri::command]
pub(crate) async fn plugin_disable(
    request: PluginStateChangeRequest,
    service: State<'_, PluginService>,
) -> CoreResult<InstalledPluginSummary> {
    let mutation_operation = Uuid::new_v4();
    let _mutation_reservation = service
        .reserve_plugin_mutation(&request.plugin_id, mutation_operation)
        .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    let installed = service
        .hosts
        .with_plugin_repository(|repository| repository.get_plugin_installation(&request.plugin_id))
        .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
    if installed.state_version != request.expected_state_version
        || installed.state != PluginInstallState::Enabled
    {
        return Err(plugin_conflict_error(
            request.meta.request_id,
            Some(installed.state_version),
        ));
    }
    service.stop_plugin(&request.plugin_id).await?;
    let record = service.set_plugin_state_convergent(
        request.meta.request_id.clone(),
        &installed,
        PluginInstallState::Disabled,
    )?;
    service.installed_summary(request.meta.request_id, record)
}

#[tauri::command]
pub(crate) async fn plugin_uninstall(
    request: norishell_core_api::PluginUninstallRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginOperationSummary> {
    if request.idempotency_key.trim().is_empty() {
        return Err(plugin_validation_error(request.meta.request_id));
    }
    let fingerprint = request_fingerprint(
        &(
            request.plugin_id.as_str(),
            request.expected_state_version,
            request.delete_data,
        ),
        request.meta.request_id.clone(),
    )?;
    let mutation_operation = Uuid::parse_str(request.operation_id.as_str())
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
    let _mutation_reservation = service
        .reserve_plugin_mutation(&request.plugin_id, mutation_operation)
        .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    match service
        .hosts
        .with_plugin_repository(|repository| repository.get_plugin_operation(&request.operation_id))
    {
        Ok(operation) => {
            if operation.plugin_id.as_ref() != Some(&request.plugin_id)
                || operation.idempotency_key != request.idempotency_key
                || operation.request_fingerprint_sha256 != fingerprint
            {
                return Err(plugin_conflict_error(request.meta.request_id, None));
            }
            if matches!(
                operation.state,
                PluginOperationState::Succeeded
                    | PluginOperationState::Failed
                    | PluginOperationState::Cancelled
            ) {
                return Ok(operation_to_wire(operation));
            }
            if request.delete_data {
                service
                    .delete_plugin_credential_data(
                        request.meta.request_id.clone(),
                        &request.plugin_id,
                    )
                    .await?;
                service
                    .delete_plugin_ssh_sync_data(
                        request.meta.request_id.clone(),
                        &request.plugin_id,
                    )
                    .await?;
            }
            service
                .reconcile_uninstall_operation(operation)
                .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
            let reconciled = service
                .hosts
                .with_plugin_repository(|repository| {
                    repository.get_plugin_operation(&request.operation_id)
                })
                .map_err(|error| map_persistence_error(request.meta.request_id, error))?;
            return Ok(operation_to_wire(reconciled));
        }
        Err(AppPersistenceError::NotFound) => {}
        Err(error) => return Err(map_persistence_error(request.meta.request_id, error)),
    }
    let installed = service
        .hosts
        .with_plugin_repository(|repository| repository.get_plugin_installation(&request.plugin_id))
        .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
    if installed.state_version != request.expected_state_version {
        return Err(plugin_conflict_error(
            request.meta.request_id,
            Some(installed.state_version),
        ));
    }
    service.stop_plugin(&request.plugin_id).await?;
    if request.delete_data {
        service
            .delete_plugin_credential_data(request.meta.request_id.clone(), &request.plugin_id)
            .await?;
        service
            .delete_plugin_ssh_sync_data(request.meta.request_id.clone(), &request.plugin_id)
            .await?;
    }
    let operation = service
        .hosts
        .with_plugin_repository(|repository| {
            repository.begin_plugin_operation(
                &request.operation_id,
                Some(&request.plugin_id),
                PluginOperationKind::Uninstall,
                &request.idempotency_key,
                &fingerprint,
                None,
                Some(&installed.active_version),
            )
        })
        .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
    if operation.state == PluginOperationState::Succeeded {
        return Ok(operation_to_wire(operation));
    }
    let uninstall_result = service.installer.prepare_uninstall(
        &request.plugin_id,
        &installed.active_version,
        &request.operation_id,
    );
    if uninstall_result.is_err() {
        let reconciled = service.hosts.with_plugin_repository(|repository| {
            repository.advance_plugin_operation(
                &request.operation_id,
                operation.state_version,
                PluginOperationState::Failed,
                PluginOperationPhase::ReconcileRequired,
                Some("install_conflict"),
            )
        });
        return match reconciled {
            Ok(record) => Ok(operation_to_wire(record)),
            Err(error) => Err(map_persistence_error(request.meta.request_id, error)),
        };
    }
    let database_committed = service.hosts.with_plugin_repository(|repository| {
        repository.uninstall_plugin(
            &request.plugin_id,
            request.expected_state_version,
            &request.operation_id,
            operation.state_version,
            !request.delete_data,
        )
    });
    let database_committed = match database_committed {
        Ok(operation) => operation,
        Err(_) => {
            let database_fact = service.hosts.with_plugin_repository(|repository| {
                let current_installation =
                    match repository.get_plugin_installation(&request.plugin_id) {
                        Ok(current) => Some(current),
                        Err(AppPersistenceError::NotFound) => None,
                        Err(error) => return Err(error),
                    };
                let current_operation = repository.get_plugin_operation(&request.operation_id)?;
                Ok((current_installation, current_operation))
            });
            match database_fact {
                Ok((None, current_operation))
                    if current_operation.phase == PluginOperationPhase::DatabaseCommitted
                        && current_operation.state == PluginOperationState::Running =>
                {
                    current_operation
                }
                Ok((Some(current), current_operation))
                    if current == installed
                        && current_operation.state_version == operation.state_version
                        && current_operation.phase == operation.phase =>
                {
                    if service
                        .installer
                        .restore_uninstall(
                            &request.plugin_id,
                            &installed.active_version,
                            &request.operation_id,
                        )
                        .is_ok()
                    {
                        let failed = service
                            .hosts
                            .with_plugin_repository(|repository| {
                                repository.advance_plugin_operation(
                                    &request.operation_id,
                                    current_operation.state_version,
                                    PluginOperationState::Failed,
                                    PluginOperationPhase::Completed,
                                    Some("install_conflict"),
                                )
                            })
                            .map_err(|error| {
                                map_persistence_error(request.meta.request_id.clone(), error)
                            })?;
                        return Ok(operation_to_wire(failed));
                    }
                    return Err(uninstall_reconciliation_error(
                        request.meta.request_id.clone(),
                    ));
                }
                Ok(_) | Err(_) => {
                    return Err(uninstall_reconciliation_error(
                        request.meta.request_id.clone(),
                    ));
                }
            }
        }
    };
    if service
        .installer
        .finalize_uninstall(
            &request.plugin_id,
            &installed.active_version,
            &request.operation_id,
        )
        .is_err()
    {
        let reconcile = service
            .hosts
            .with_plugin_repository(|repository| {
                repository.advance_plugin_operation(
                    &request.operation_id,
                    database_committed.state_version,
                    PluginOperationState::Running,
                    PluginOperationPhase::ReconcileRequired,
                    Some("install_conflict"),
                )
            })
            .map_err(|error| map_persistence_error(request.meta.request_id.clone(), error))?;
        return Ok(operation_to_wire(reconcile));
    }
    let completed = service
        .hosts
        .with_plugin_repository(|repository| {
            repository.advance_plugin_operation(
                &request.operation_id,
                database_committed.state_version,
                PluginOperationState::Succeeded,
                PluginOperationPhase::Completed,
                None,
            )
        })
        .map_err(|error| map_persistence_error(request.meta.request_id, error))?;
    Ok(operation_to_wire(completed))
}

#[tauri::command]
pub(crate) fn plugin_operation_get(
    request: PluginOperationRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginOperationSummary> {
    service
        .hosts
        .with_plugin_repository(|repository| repository.get_plugin_operation(&request.operation_id))
        .map(operation_to_wire)
        .map_err(|error| map_persistence_error(request.meta.request_id, error))
}

#[tauri::command]
pub(crate) fn plugin_operation_cancel(
    request: PluginOperationRequest,
    service: State<'_, PluginService>,
) -> CoreResult<PluginOperationSummary> {
    let operation = service
        .hosts
        .with_plugin_repository(|repository| {
            let operation = repository.get_plugin_operation(&request.operation_id)?;
            if operation.state != PluginOperationState::AwaitingCapabilities
                || operation.phase != PluginOperationPhase::AwaitingCapabilities
            {
                return Err(AppPersistenceError::Conflict);
            }
            repository.advance_plugin_operation(
                &request.operation_id,
                operation.state_version,
                PluginOperationState::Cancelled,
                PluginOperationPhase::Completed,
                Some("capability_rejected"),
            )
        })
        .map_err(|error| map_persistence_error(request.meta.request_id, error))?;
    Ok(operation_to_wire(operation))
}

#[tauri::command]
pub(crate) fn plugin_operation_permissions_list<R: tauri::Runtime>(
    request: PluginOperationPermissionListRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<PluginOperationPermissionList> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    service.list_operation_permissions(request)
}

#[tauri::command]
pub(crate) fn plugin_operation_permission_revoke<R: tauri::Runtime>(
    request: PluginOperationPermissionRevokeRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    service.revoke_operation_permission(request)
}

#[tauri::command]
pub(crate) fn plugin_operation_permissions_clear<R: tauri::Runtime>(
    request: PluginOperationPermissionsClearRequest,
    window: WebviewWindow<R>,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    require_main_plugin_management_window(&window, request.meta.request_id.clone())?;
    service.clear_operation_permissions(request)
}

#[tauri::command]
pub(crate) fn plugin_terminal_input_pending_list(
    request: PluginTerminalInputPendingListRequest,
    service: State<'_, PluginService>,
) -> CoreResult<Vec<PluginTerminalInputProposal>> {
    let now = unix_time_ms();
    let mut runtime = service
        .runtime
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    runtime
        .pending_inputs
        .retain(|_, pending| pending.proposal.expires_at_unix_ms > now);
    let _ = request.meta.request_id;
    Ok(runtime
        .pending_inputs
        .values()
        .map(|pending| pending.proposal.clone())
        .collect())
}

fn secure_terminal_input_window_label(approval_id: &PluginInputApprovalId) -> String {
    format!("secure-plugin-input-{}", approval_id.as_str())
}

fn require_secure_terminal_input_window(
    window: &WebviewWindow,
    approval_id: &PluginInputApprovalId,
    request_id: RequestId,
) -> CoreResult<()> {
    if window.label() == secure_terminal_input_window_label(approval_id) {
        Ok(())
    } else {
        Err(plugin_permission_error(request_id))
    }
}

#[tauri::command]
// Creating a WebView2 window on Windows must not run on the synchronous IPC main thread, or it deadlocks.
pub(crate) async fn plugin_terminal_input_open(
    request: PluginTerminalInputOpenRequest,
    app: AppHandle,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    service.open_input_proposal(request, &app)
}

#[tauri::command]
pub(crate) fn plugin_terminal_input_get(
    request: PluginTerminalInputGetRequest,
    window: WebviewWindow,
    service: State<'_, PluginService>,
) -> CoreResult<PluginTerminalInputProposal> {
    require_secure_terminal_input_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    service.terminal_input_snapshot(request)
}

#[tauri::command]
pub(crate) async fn plugin_terminal_input_decide(
    request: PluginTerminalInputDecisionRequest,
    window: WebviewWindow,
    app: AppHandle,
    service: State<'_, PluginService>,
) -> CoreResult<PluginTerminalInputDecisionResponse> {
    require_secure_terminal_input_window(
        &window,
        &request.approval_id,
        request.meta.request_id.clone(),
    )?;
    let proposal = service.terminal_input_snapshot(PluginTerminalInputGetRequest {
        meta: RequestMeta {
            request_id: request.meta.request_id.clone(),
        },
        approval_id: request.approval_id.clone(),
    })?;
    let operation_id = Uuid::new_v4();
    let _reservation = service
        .reserve_plugin_mutation(&proposal.plugin_id, operation_id)
        .ok_or_else(|| plugin_conflict_error(request.meta.request_id.clone(), None))?;
    let response = service.decide_terminal_input(request).await?;
    app.emit_to("main", "plugin-terminal-input-decided", &response)
        .map_err(|_| plugin_runtime_error(RequestId::new(), None))?;
    Ok(response)
}

#[tauri::command]
pub(crate) async fn plugin_terminal_observe_attach(
    request: PluginTerminalObserveAttachRequest,
    service: State<'_, PluginService>,
    lifecycle: State<'_, LifecycleState>,
) -> CoreResult<PluginTerminalObserveAttachResponse> {
    service.require_ready(request.meta.request_id.clone())?;
    let _creation_permit = lifecycle.acquire_resource_creation(request.meta.request_id.clone())?;
    if request.idempotency_key.trim().is_empty() {
        return Err(plugin_validation_error(request.meta.request_id));
    }
    let ledger_key = request.operation_id.as_str().to_owned();
    let attach_fingerprint = request_fingerprint(
        &(
            request.idempotency_key.as_str(),
            request.plugin_id.as_str(),
            request.signer_fingerprint_sha256.as_str(),
            request.instance_generation,
            &request.target,
            request.expected_focus_epoch,
        ),
        request.meta.request_id.clone(),
    )?;
    let attach_idempotency_key = request_fingerprint(
        &(
            "attach",
            request.plugin_id.as_str(),
            request.idempotency_key.as_str(),
        ),
        request.meta.request_id.clone(),
    )?;
    let replay = {
        let runtime = service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .observer_attach_idempotency
            .get(&attach_idempotency_key)
            .is_some_and(|existing| existing != &ledger_key)
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        runtime
            .observer_attach_ledger
            .get(&ledger_key)
            .map(|entry| {
                (
                    entry.fingerprint.clone(),
                    entry.binding.clone(),
                    runtime
                        .observers
                        .contains_key(entry.binding.observer_id.as_str()),
                )
            })
    };
    if let Some((fingerprint, binding, active)) = replay {
        return if fingerprint == attach_fingerprint && active {
            Ok(PluginTerminalObserveAttachResponse { binding })
        } else {
            Err(plugin_conflict_error(request.meta.request_id, None))
        };
    }
    service.ensure_instance(
        &request.plugin_id,
        &request.signer_fingerprint_sha256,
        request.instance_generation,
        request.meta.request_id.clone(),
    )?;
    service.has_capability(
        request.meta.request_id.clone(),
        &request.plugin_id,
        &request.signer_fingerprint_sha256,
        PluginCapability::TerminalObserve,
    )?;
    let snapshot = service
        .sessions
        .terminal_focus_snapshot_internal(request.meta.request_id.clone())
        .await?;
    if snapshot.focus_epoch != request.expected_focus_epoch
        || snapshot.target != Some(TerminalInputFocusTarget::Ssh(request.target.clone()))
    {
        return Err(plugin_conflict_error(request.meta.request_id, None));
    }
    {
        let mut runtime = service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .observer_attach_reservations
            .contains_key(&ledger_key)
            || runtime
                .observer_attach_idempotency
                .get(&attach_idempotency_key)
                .is_some_and(|existing| existing != &ledger_key)
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        if !observer_capacity_available(&runtime, &request.plugin_id, &request.target.session_id) {
            return Err(plugin_error(
                request.meta.request_id,
                "plugin.observer_limit_reached",
                ErrorCategory::Unavailable,
                RetryStrategy::AfterMilliseconds(1_000),
                "errors.plugin.observerLimitReached",
                None,
            ));
        }
        runtime.observer_attach_reservations.insert(
            ledger_key.clone(),
            PluginObserverAttachReservation {
                fingerprint: attach_fingerprint.clone(),
                idempotency_key: attach_idempotency_key.clone(),
                plugin_id: request.plugin_id.clone(),
                session_id: request.target.session_id.clone(),
            },
        );
        runtime
            .observer_attach_idempotency
            .insert(attach_idempotency_key.clone(), ledger_key.clone());
    }
    let observer_id = PluginObserverId::new();
    let raw_observer_id = Uuid::parse_str(observer_id.as_str())
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
    let (sink, mut observations) = tokio::sync::mpsc::channel(PLUGIN_OBSERVATION_QUEUE_CAPACITY);
    let attach_result = service
        .sessions
        .attach_approved_plugin_observer(ApprovedPluginObservationAttach {
            request_id: request.meta.request_id.clone(),
            observer_id: raw_observer_id,
            focus_epoch: request.expected_focus_epoch,
            target: request.target.clone(),
            sink,
        })
        .await;
    if let Err(error) = attach_result {
        let mut runtime = service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reservation) = runtime.observer_attach_reservations.remove(&ledger_key)
            && runtime
                .observer_attach_idempotency
                .get(&reservation.idempotency_key)
                == Some(&ledger_key)
        {
            runtime
                .observer_attach_idempotency
                .remove(&reservation.idempotency_key);
        }
        return Err(error);
    }
    let binding = PluginTerminalObserverBinding {
        observer_id: observer_id.clone(),
        plugin_id: request.plugin_id.clone(),
        instance_generation: request.instance_generation,
        session_id: request.target.session_id,
        generation: request.target.expected_generation,
        channel_id: request.target.channel_id,
        attachment_id: request.target.attachment_id,
        view_id: request.target.view_id,
        focus_epoch: request.expected_focus_epoch,
    };
    let reservation_committed = {
        let mut runtime = service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match runtime.observer_attach_reservations.remove(&ledger_key) {
            Some(reservation)
                if reservation.fingerprint == attach_fingerprint
                    && reservation.idempotency_key == attach_idempotency_key =>
            {
                if runtime.observer_attach_ledger.len() >= PLUGIN_OBSERVER_LEDGER_LIMIT
                    && let Some(oldest) = runtime.observer_attach_ledger.keys().next().cloned()
                    && let Some(evicted) = runtime.observer_attach_ledger.remove(&oldest)
                    && runtime
                        .observer_attach_idempotency
                        .get(&evicted.idempotency_key)
                        == Some(&oldest)
                {
                    runtime
                        .observer_attach_idempotency
                        .remove(&evicted.idempotency_key);
                }
                runtime.observer_attach_ledger.insert(
                    ledger_key.clone(),
                    PluginObserverAttachLedgerEntry {
                        fingerprint: attach_fingerprint,
                        idempotency_key: attach_idempotency_key,
                        binding: binding.clone(),
                    },
                );
                runtime
                    .observers
                    .insert(observer_id.as_str().to_owned(), binding.clone());
                true
            }
            Some(reservation) => {
                if runtime
                    .observer_attach_idempotency
                    .get(&reservation.idempotency_key)
                    == Some(&ledger_key)
                {
                    runtime
                        .observer_attach_idempotency
                        .remove(&reservation.idempotency_key);
                }
                false
            }
            None => false,
        }
    };
    if !reservation_committed {
        let _ = service
            .sessions
            .detach_plugin_observer(RequestId::new(), raw_observer_id)
            .await;
        return Err(plugin_conflict_error(request.meta.request_id, None));
    }
    let worker_service = service.inner().clone();
    let worker_plugin_id = request.plugin_id;
    let worker_signer = request.signer_fingerprint_sha256;
    let worker_generation = request.instance_generation;
    let worker_observer_id = observer_id.clone();
    tauri::async_runtime::spawn(async move {
        'observations: while let Some(observation) = observations.recv().await {
            let PluginTerminalObservation::Text { output_seq, text } = observation else {
                break;
            };
            let request_id = RequestId::new();
            if worker_service
                .has_capability(
                    request_id.clone(),
                    &worker_plugin_id,
                    &worker_signer,
                    PluginCapability::TerminalObserve,
                )
                .is_err()
            {
                break;
            }
            let instance = match worker_service.active_instance(
                request_id.clone(),
                &worker_plugin_id,
                &worker_signer,
                worker_generation,
            ) {
                Ok(instance) => instance,
                Err(_) => {
                    worker_service.record_runtime_failure(&worker_plugin_id, worker_generation);
                    break;
                }
            };
            let host_request_id = Uuid::new_v4().to_string();
            let locale = instance.locale.clone();
            let protocol_minor = instance.protocol_minor;
            let host_request = PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor,
                request_id: host_request_id.clone(),
                kind: PluginHostMessageKind::TerminalObservation,
                payload_json: plugin_host_payload(
                    &locale,
                    serde_json::json!({
                        "observerId": worker_observer_id.as_str(),
                        "outputSequence": output_seq.get().to_string(),
                        "text": text,
                    }),
                ),
            };
            let outputs = match worker_service
                .execute_instance(request_id.clone(), instance, host_request)
                .await
            {
                Ok(outputs) => outputs,
                Err(_) => {
                    worker_service.record_runtime_failure(&worker_plugin_id, worker_generation);
                    break;
                }
            };
            for output in outputs {
                if output.request_id != host_request_id {
                    worker_service.record_runtime_failure(&worker_plugin_id, worker_generation);
                    break 'observations;
                }
                if worker_service
                    .queue_runtime_input(
                        request_id.clone(),
                        worker_plugin_id.clone(),
                        worker_signer.clone(),
                        worker_generation,
                        output,
                    )
                    .await
                    .is_err()
                {
                    worker_service.record_runtime_failure(&worker_plugin_id, worker_generation);
                    break 'observations;
                }
            }
        }
        worker_service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observers
            .remove(worker_observer_id.as_str());
        if let Ok(observer_id) = Uuid::parse_str(worker_observer_id.as_str()) {
            let _ = worker_service
                .sessions
                .detach_plugin_observer(RequestId::new(), observer_id)
                .await;
        }
    });
    Ok(PluginTerminalObserveAttachResponse { binding })
}

#[tauri::command]
pub(crate) async fn plugin_terminal_observe_detach(
    request: PluginTerminalObserveDetachRequest,
    service: State<'_, PluginService>,
) -> CoreResult<()> {
    if request.idempotency_key.trim().is_empty() {
        return Err(plugin_validation_error(request.meta.request_id));
    }
    let ledger_key = request.operation_id.as_str().to_owned();
    let detach_fingerprint = request_fingerprint(
        &(
            request.idempotency_key.as_str(),
            request.plugin_id.as_str(),
            request.instance_generation,
            request.observer_id.as_str(),
        ),
        request.meta.request_id.clone(),
    )?;
    let detach_idempotency_key = request_fingerprint(
        &(
            "detach",
            request.plugin_id.as_str(),
            request.idempotency_key.as_str(),
        ),
        request.meta.request_id.clone(),
    )?;
    {
        let mut runtime = service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runtime
            .observer_detach_idempotency
            .get(&detach_idempotency_key)
            .is_some_and(|existing| existing != &ledger_key)
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        if let Some(existing) = runtime.observer_detach_ledger.get(&ledger_key) {
            return if existing == &detach_fingerprint {
                Ok(())
            } else {
                Err(plugin_conflict_error(request.meta.request_id, None))
            };
        }
        if runtime
            .observer_detach_reservations
            .contains_key(&ledger_key)
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        let binding = runtime
            .observers
            .get(request.observer_id.as_str())
            .ok_or_else(|| plugin_not_found_error(request.meta.request_id.clone()))?;
        if binding.plugin_id != request.plugin_id
            || binding.instance_generation != request.instance_generation
        {
            return Err(plugin_conflict_error(request.meta.request_id, None));
        }
        runtime.observer_detach_reservations.insert(
            ledger_key.clone(),
            PluginObserverDetachReservation {
                fingerprint: detach_fingerprint.clone(),
                idempotency_key: detach_idempotency_key.clone(),
                plugin_id: request.plugin_id.clone(),
            },
        );
        runtime
            .observer_detach_idempotency
            .insert(detach_idempotency_key.clone(), ledger_key.clone());
    }
    let raw_observer_id = Uuid::parse_str(request.observer_id.as_str())
        .map_err(|_| plugin_validation_error(request.meta.request_id.clone()))?;
    let detach_result = service
        .sessions
        .detach_plugin_observer(request.meta.request_id.clone(), raw_observer_id)
        .await;
    if let Err(error) = detach_result {
        let mut runtime = service
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reservation) = runtime.observer_detach_reservations.remove(&ledger_key)
            && runtime
                .observer_detach_idempotency
                .get(&reservation.idempotency_key)
                == Some(&ledger_key)
        {
            runtime
                .observer_detach_idempotency
                .remove(&reservation.idempotency_key);
        }
        return Err(error);
    }
    let mut runtime = service
        .runtime
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(reservation) = runtime.observer_detach_reservations.remove(&ledger_key) else {
        return Err(plugin_conflict_error(request.meta.request_id, None));
    };
    if reservation.fingerprint != detach_fingerprint
        || reservation.idempotency_key != detach_idempotency_key
    {
        if runtime
            .observer_detach_idempotency
            .get(&reservation.idempotency_key)
            == Some(&ledger_key)
        {
            runtime
                .observer_detach_idempotency
                .remove(&reservation.idempotency_key);
        }
        return Err(plugin_conflict_error(request.meta.request_id, None));
    }
    runtime.observers.remove(request.observer_id.as_str());
    if runtime.observer_detach_ledger.len() >= PLUGIN_OBSERVER_LEDGER_LIMIT
        && let Some(oldest) = runtime.observer_detach_ledger.keys().next().cloned()
    {
        runtime.observer_detach_ledger.remove(&oldest);
        runtime
            .observer_detach_idempotency
            .retain(|_, operation_id| operation_id != &oldest);
    }
    runtime
        .observer_detach_ledger
        .insert(ledger_key, detach_fingerprint);
    Ok(())
}

fn request_fingerprint(
    request: &impl serde::Serialize,
    request_id: RequestId,
) -> CoreResult<String> {
    serde_json::to_vec(request)
        .map(|bytes| hex::encode(Sha256::digest(bytes)))
        .map_err(|_| plugin_validation_error(request_id))
}

fn local_install_request_fingerprint(
    plugin_id: &PluginId,
    version: &str,
    package_sha256: &str,
    expected_state_version: Option<WireSequence>,
    grants: &[PluginCapabilityGrant],
    request_id: RequestId,
) -> CoreResult<String> {
    let normalized_grants = grants
        .iter()
        .map(|grant| (grant.capability, grant.granted))
        .collect::<BTreeMap<_, _>>();
    if normalized_grants.len() != grants.len() {
        return Err(plugin_validation_error(request_id));
    }
    request_fingerprint(
        &(
            plugin_id.as_str(),
            version,
            package_sha256,
            expected_state_version,
            normalized_grants,
        ),
        request_id,
    )
}

fn open_local_plugin_source(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let source = options.open(path)?;
    let metadata = source.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(std::io::Error::other("reparse points are not accepted"));
        }
    }
    if !metadata.is_file() {
        return Err(std::io::Error::other(
            "plugin package must be a regular file",
        ));
    }
    Ok(source)
}

fn validate_regular_marker(path: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(std::io::Error::other("invalid plugin safe-mode marker"));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove_regular_marker_if_present(path: &Path) -> std::io::Result<()> {
    if validate_regular_marker(path)? {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn activate_safe_mode_marker(next: &Path, active: &Path) -> std::io::Result<bool> {
    if validate_regular_marker(active)? {
        return Ok(true);
    }
    if !validate_regular_marker(next)? {
        return Ok(false);
    }
    fs::rename(next, active)?;
    Ok(true)
}

fn current_app_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("workspace package version must be semver")
}

fn plugin_version_can_replace(
    candidate: &str,
    candidate_hash: &str,
    current: &str,
    current_hash: &str,
) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Ok(candidate), Ok(current)) => {
            candidate > current || (candidate == current && candidate_hash != current_hash)
        }
        _ => false,
    }
}

fn current_plugin_permission_binding(package_sha256: &str) -> PluginPermissionBinding {
    let app_version = current_app_version();
    PluginPermissionBinding {
        artifact_sha256: package_sha256.to_owned(),
        app_version_major: app_version.major,
        app_version_minor: app_version.minor,
        secure_surface_contract_revision: PLUGIN_PERMISSION_SURFACE_CONTRACT_REVISION,
    }
}

fn protocol_is_compatible(protocol_major: u16, protocol_minor: u16) -> bool {
    norishell_core_api::plugin_package_protocol_is_compatible(protocol_major, protocol_minor)
}

fn supported_plugin_capability(capability: &PluginCapability) -> bool {
    matches!(
        capability,
        PluginCapability::UiPanel
            | PluginCapability::UiNavigation
            | PluginCapability::UiPage
            | PluginCapability::ClipboardWrite
            | PluginCapability::TerminalObserve
            | PluginCapability::TerminalRequestInput
            | PluginCapability::TerminalMetadata
            | PluginCapability::TerminalAnnotation
            | PluginCapability::TerminalProposeInput
            | PluginCapability::UiWebviewIsolated
            | PluginCapability::UiHostDomObserve
            | PluginCapability::UiHostDomMutate
            | PluginCapability::UiHostCss
            | PluginCapability::NetworkDomain
            | PluginCapability::LocalFiles
            | PluginCapability::TerminalProvider
            | PluginCapability::DeviceSerial
            | PluginCapability::LocalProcess
            | PluginCapability::CredentialsPlugin
            | PluginCapability::StoragePlugin
            | PluginCapability::HostMetadataRead
            | PluginCapability::HostMutationPropose
            | PluginCapability::HostSessionRequest
            | PluginCapability::RemoteInspect
            | PluginCapability::RemoteExecRequest
            | PluginCapability::SftpRead
            | PluginCapability::SftpWrite
            | PluginCapability::SshSync
            | PluginCapability::AppPreferencesRead
            | PluginCapability::TerminalHistoryRead
    )
}

fn capabilities_match_protocol(capabilities: &[PluginCapability], protocol_minor: u16) -> bool {
    protocol_is_compatible(PLUGIN_PROTOCOL_MAJOR, protocol_minor)
        && (protocol_minor != PLUGIN_THEME_PROTOCOL_MINOR || capabilities.is_empty())
        && capabilities.iter().all(|capability| {
            supported_plugin_capability(capability)
                && norishell_core_api::plugin_capability_min_protocol_minor(*capability)
                    <= protocol_minor
        })
}

fn special_plugin_capability(capability: PluginCapability) -> bool {
    matches!(
        capability,
        PluginCapability::UiWebviewIsolated
            | PluginCapability::NetworkDomain
            | PluginCapability::LocalFiles
            | PluginCapability::LocalProcess
            | PluginCapability::DeviceSerial
            | PluginCapability::TerminalProvider
            | PluginCapability::CredentialsPlugin
            | PluginCapability::UiHostDomObserve
            | PluginCapability::UiHostDomMutate
            | PluginCapability::UiHostCss
            | PluginCapability::HostMetadataRead
            | PluginCapability::HostMutationPropose
            | PluginCapability::HostSessionRequest
            | PluginCapability::RemoteInspect
            | PluginCapability::RemoteExecRequest
            | PluginCapability::SftpRead
            | PluginCapability::SftpWrite
            | PluginCapability::SshSync
            | PluginCapability::AppPreferencesRead
            | PluginCapability::TerminalHistoryRead
    )
}

fn host_scoped_plugin_capability(capability: PluginCapability) -> bool {
    matches!(
        capability,
        PluginCapability::HostMetadataRead
            | PluginCapability::HostMutationPropose
            | PluginCapability::HostSessionRequest
    )
}

fn plugin_ssh_terminal_state(state: norishell_core_api::SshSessionState) -> PluginTerminalState {
    use norishell_core_api::SshSessionState;
    match state {
        SshSessionState::Resolving
        | SshSessionState::Connecting
        | SshSessionState::Authenticating
        | SshSessionState::OpeningChannel
        | SshSessionState::AutomatingLogin => PluginTerminalState::Starting,
        SshSessionState::VerifyingHostKey | SshSessionState::AwaitingHostKeyDecision => {
            PluginTerminalState::AwaitingUser
        }
        SshSessionState::Running => PluginTerminalState::Running,
        SshSessionState::Disconnecting => PluginTerminalState::Closing,
        SshSessionState::Closed => PluginTerminalState::Closed,
        SshSessionState::Failed => PluginTerminalState::Failed,
    }
}

fn plugin_local_terminal_state(
    state: norishell_core_api::LocalSessionState,
) -> PluginTerminalState {
    use norishell_core_api::LocalSessionState;
    match state {
        LocalSessionState::Starting => PluginTerminalState::Starting,
        LocalSessionState::Running => PluginTerminalState::Running,
        LocalSessionState::Stopping => PluginTerminalState::Closing,
        LocalSessionState::Exited | LocalSessionState::Closed => PluginTerminalState::Closed,
        LocalSessionState::Failed => PluginTerminalState::Failed,
    }
}

#[allow(dead_code)]
fn platform_and_architecture_match(platform: &str, architectures: &[String]) -> bool {
    (matches!(platform, "desktop" | "all") || platform == std::env::consts::OS)
        && architectures.iter().any(|architecture| {
            matches!(architecture.as_str(), "universal" | "all")
                || architecture == std::env::consts::ARCH
        })
}

fn explicit_capability_decision(
    required: &[PluginCapability],
    supplied: &[PluginCapabilityGrant],
) -> bool {
    if required.len() != supplied.len() {
        return false;
    }
    let mut decisions = BTreeMap::new();
    for grant in supplied {
        if decisions.insert(grant.capability, grant.granted).is_some() {
            return false;
        }
    }
    required
        .iter()
        .all(|capability| decisions.contains_key(capability))
}

fn resolve_install_grants(
    capabilities: &[PluginCapability],
    supplied: &[PluginCapabilityGrant],
    retained: &[PluginCapabilityGrantRecord],
) -> Option<Vec<(PluginCapability, bool)>> {
    if !explicit_capability_decision(capabilities, supplied) {
        return None;
    }
    let supplied = supplied
        .iter()
        .map(|grant| (grant.capability, grant.granted))
        .collect::<BTreeMap<_, _>>();
    Some(
        capabilities
            .iter()
            .map(|capability| {
                (
                    *capability,
                    retained
                        .iter()
                        .find(|grant| grant.capability == *capability)
                        .map_or_else(
                            || supplied.get(capability).copied().unwrap_or(false),
                            |grant| grant.granted,
                        ),
                )
            })
            .collect(),
    )
}

fn newly_requested_special_capabilities_are_denied(
    supplied: &[PluginCapabilityGrant],
    retained: &[PluginCapabilityGrantRecord],
) -> bool {
    supplied.iter().all(|grant| {
        !special_plugin_capability(grant.capability)
            || retained
                .iter()
                .any(|retained| retained.capability == grant.capability)
            || !grant.granted
    })
}

fn effective_plugin_grant(
    installed: &PluginInstalledRecord,
    grant: &PluginCapabilityGrantRecord,
) -> bool {
    grant.granted
        && grant.binding.as_ref()
            == Some(&current_plugin_permission_binding(
                &installed.package_sha256,
            ))
}

fn effective_plugin_scope(
    installed: &PluginInstalledRecord,
    scope: &PluginHostScopeSetRecord,
) -> bool {
    scope.binding.as_ref()
        == Some(&current_plugin_permission_binding(
            &installed.package_sha256,
        ))
}

fn requested_special_grants_do_not_expand(
    installed: &PluginInstalledRecord,
    current_grants: &[PluginCapabilityGrantRecord],
    requested: &[PluginCapabilityGrant],
) -> bool {
    requested.iter().all(|requested| {
        !special_plugin_capability(requested.capability)
            || !requested.granted
            || current_grants.iter().any(|current| {
                current.capability == requested.capability
                    && effective_plugin_grant(installed, current)
            })
    })
}

fn plugin_grant_request_is_effectively_unchanged(
    installed: &PluginInstalledRecord,
    current_grants: &[PluginCapabilityGrantRecord],
    requested: &[PluginCapabilityGrant],
) -> bool {
    current_grants.len() == requested.len()
        && requested.iter().all(|requested| {
            current_grants.iter().any(|current| {
                current.capability == requested.capability
                    && current.granted == requested.granted
                    && (!requested.granted || effective_plugin_grant(installed, current))
            })
        })
}

fn capability_grant_records_match(
    records: &[PluginCapabilityGrantRecord],
    decisions: &[(PluginCapability, bool)],
    state_version: u64,
) -> bool {
    records.len() == decisions.len()
        && records.iter().all(|record| {
            record.state_version.get() == state_version
                && decisions.iter().any(|(capability, granted)| {
                    record.capability == *capability && record.granted == *granted
                })
        })
}

fn clear_observer_reservations_for_plugin(runtime: &mut PluginRuntimeState, plugin_id: &PluginId) {
    let attach_operations = runtime
        .observer_attach_reservations
        .iter()
        .filter(|(_, reservation)| reservation.plugin_id == *plugin_id)
        .map(|(operation_id, reservation)| {
            (operation_id.clone(), reservation.idempotency_key.clone())
        })
        .collect::<Vec<_>>();
    for (operation_id, idempotency_key) in attach_operations {
        runtime.observer_attach_reservations.remove(&operation_id);
        if runtime.observer_attach_idempotency.get(&idempotency_key) == Some(&operation_id) {
            runtime.observer_attach_idempotency.remove(&idempotency_key);
        }
    }
    let detach_operations = runtime
        .observer_detach_reservations
        .iter()
        .filter(|(_, reservation)| reservation.plugin_id == *plugin_id)
        .map(|(operation_id, reservation)| {
            (operation_id.clone(), reservation.idempotency_key.clone())
        })
        .collect::<Vec<_>>();
    for (operation_id, idempotency_key) in detach_operations {
        runtime.observer_detach_reservations.remove(&operation_id);
        if runtime.observer_detach_idempotency.get(&idempotency_key) == Some(&operation_id) {
            runtime.observer_detach_idempotency.remove(&idempotency_key);
        }
    }
}

fn observer_capacity_available(
    runtime: &PluginRuntimeState,
    plugin_id: &PluginId,
    session_id: &norishell_core_api::SshSessionId,
) -> bool {
    let global_count = runtime.observers.len() + runtime.observer_attach_reservations.len();
    let plugin_count = runtime
        .observers
        .values()
        .filter(|binding| binding.plugin_id == *plugin_id)
        .count()
        + runtime
            .observer_attach_reservations
            .values()
            .filter(|reservation| reservation.plugin_id == *plugin_id)
            .count();
    let session_count = runtime
        .observers
        .values()
        .filter(|binding| binding.session_id == *session_id)
        .count()
        + runtime
            .observer_attach_reservations
            .values()
            .filter(|reservation| reservation.session_id == *session_id)
            .count();
    global_count < PLUGIN_OBSERVER_GLOBAL_LIMIT
        && plugin_count < PLUGIN_OBSERVER_PER_PLUGIN_LIMIT
        && session_count < PLUGIN_OBSERVER_PER_SESSION_LIMIT
}

fn platform_error_code(error: &PluginPlatformError) -> &'static str {
    match error {
        PluginPlatformError::PackageTooLarge => "package_too_large",
        PluginPlatformError::PackageHashMismatch => "package_hash_mismatch",
        PluginPlatformError::InvalidArchive | PluginPlatformError::Zip(_) => {
            "package_archive_invalid"
        }
        PluginPlatformError::RejectedArchivePath => "package_path_rejected",
        PluginPlatformError::ExtractionLimitExceeded => "package_limits_exceeded",
        PluginPlatformError::ManifestMismatch
        | PluginPlatformError::InvalidSettingsSchema
        | PluginPlatformError::InvalidWorkflowCatalog
        | PluginPlatformError::InvalidProtocolCatalog => "manifest_mismatch",
        PluginPlatformError::CoreApiIncompatible => "core_api_incompatible",
        PluginPlatformError::AppVersionIncompatible => "app_version_incompatible",
        PluginPlatformError::InvalidSettingsValues => "runtime_rejected",
        PluginPlatformError::InstallConflict | PluginPlatformError::InstallCommitUncertain => {
            "install_conflict"
        }
        PluginPlatformError::InvalidWasmAbi
        | PluginPlatformError::RuntimeQuotaExceeded
        | PluginPlatformError::RuntimeTimedOut
        | PluginPlatformError::RuntimePoisoned
        | PluginPlatformError::InvalidRuntimeOutput => "runtime_rejected",
        PluginPlatformError::Io(_) | PluginPlatformError::Json(_) => "install_conflict",
    }
}

fn platform_request_error(request_id: RequestId, error: &PluginPlatformError) -> Box<CoreApiError> {
    let suffix = platform_error_code(error);
    plugin_error(
        request_id,
        &format!("plugin.{suffix}"),
        match error {
            PluginPlatformError::InvalidProtocolCatalog
            | PluginPlatformError::CoreApiIncompatible
            | PluginPlatformError::AppVersionIncompatible => ErrorCategory::Incompatible,
            PluginPlatformError::InstallConflict | PluginPlatformError::InstallCommitUncertain => {
                ErrorCategory::Conflict
            }
            _ => ErrorCategory::Validation,
        },
        RetryStrategy::Never,
        match error {
            PluginPlatformError::CoreApiIncompatible => "errors.plugin.coreApiIncompatible",
            PluginPlatformError::AppVersionIncompatible => "errors.plugin.appVersionIncompatible",
            _ => "errors.plugin.invalidPackage",
        },
        None,
    )
}

fn persistence_error_code(error: &AppPersistenceError) -> &'static str {
    match error {
        AppPersistenceError::Conflict
        | AppPersistenceError::IdempotencyConflict
        | AppPersistenceError::DatabaseNotFresh => "install_conflict",
        _ => "install_conflict",
    }
}

fn installed_to_wire(
    record: PluginInstalledRecord,
    grants: Vec<PluginCapabilityGrantRecord>,
    has_settings: bool,
) -> InstalledPluginSummary {
    let permission_binding = current_plugin_permission_binding(&record.package_sha256);
    InstalledPluginSummary {
        plugin_id: record.plugin_id,
        name: record.name,
        publisher: record.publisher,
        signer_fingerprint_sha256: record.signer_fingerprint_sha256,
        active_version: record.active_version,
        package_sha256: record.package_sha256,
        package_kind: PluginPackageKind::Wasm,
        capabilities: record.capabilities,
        grants: grants
            .into_iter()
            .map(|grant| PluginCapabilityGrant {
                capability: grant.capability,
                granted: grant.granted && grant.binding.as_ref() == Some(&permission_binding),
            })
            .collect(),
        state: match record.state {
            PluginInstallState::UpdateAvailable => PluginInstallState::Disabled,
            state => state,
        },
        state_version: record.state_version,
        has_settings,
        installed_at_unix_ms: record.installed_at_unix_ms,
        updated_at_unix_ms: record.updated_at_unix_ms,
    }
}

fn operation_to_wire(record: PluginOperationRecord) -> PluginOperationSummary {
    let progress_percent = match record.phase {
        norishell_app_persistence::PluginOperationPhase::Resolve => 5,
        norishell_app_persistence::PluginOperationPhase::Download => 15,
        norishell_app_persistence::PluginOperationPhase::VerifyCatalog => 30,
        norishell_app_persistence::PluginOperationPhase::VerifyPackage => 45,
        norishell_app_persistence::PluginOperationPhase::AwaitingCapabilities => 55,
        norishell_app_persistence::PluginOperationPhase::Staged => 70,
        norishell_app_persistence::PluginOperationPhase::FilesystemActivated => 82,
        norishell_app_persistence::PluginOperationPhase::DatabaseCommitted => 92,
        norishell_app_persistence::PluginOperationPhase::ReconcileRequired => 92,
        norishell_app_persistence::PluginOperationPhase::Completed => 100,
    };
    PluginOperationSummary {
        operation_id: record.operation_id,
        plugin_id: record.plugin_id,
        // Catalog refresh rows predate the local-only plugin contract. Keep
        // them decodable in persistence without emitting an invalid TS value.
        kind: match record.kind {
            PluginOperationKind::CatalogRefresh => PluginOperationKind::Update,
            kind => kind,
        },
        state: record.state,
        progress_percent,
        error_code: record
            .error_code
            .as_deref()
            .and_then(parse_plugin_error_code),
        started_at_unix_ms: record.created_at_unix_ms,
        updated_at_unix_ms: record.updated_at_unix_ms,
    }
}

fn parse_plugin_error_code(value: &str) -> Option<PluginErrorCode> {
    Some(match value {
        "package_too_large" => PluginErrorCode::PackageTooLarge,
        "package_hash_mismatch" => PluginErrorCode::PackageHashMismatch,
        "package_archive_invalid" => PluginErrorCode::PackageArchiveInvalid,
        "package_path_rejected" => PluginErrorCode::PackagePathRejected,
        "package_limits_exceeded" => PluginErrorCode::PackageLimitsExceeded,
        "manifest_mismatch" => PluginErrorCode::ManifestMismatch,
        "capability_rejected" => PluginErrorCode::CapabilityRejected,
        "protocol_incompatible" => PluginErrorCode::ProtocolIncompatible,
        "app_version_incompatible" => PluginErrorCode::AppVersionIncompatible,
        "core_api_incompatible" => PluginErrorCode::CoreApiIncompatible,
        "install_conflict" => PluginErrorCode::InstallConflict,
        "runtime_rejected" => PluginErrorCode::RuntimeRejected,
        "runtime_quota_exceeded" => PluginErrorCode::RuntimeQuotaExceeded,
        "runtime_timed_out" => PluginErrorCode::RuntimeTimedOut,
        "operation_not_found" => PluginErrorCode::OperationNotFound,
        "invalid_request" => PluginErrorCode::InvalidRequest,
        _ => return None,
    })
}

fn plugin_error(
    request_id: RequestId,
    code: &str,
    category: ErrorCategory,
    retry_strategy: RetryStrategy,
    message_key: &str,
    conflict_version: Option<WireSequence>,
) -> Box<CoreApiError> {
    Box::new(CoreApiError {
        code: code.to_owned(),
        category,
        retry_strategy,
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
        request_id: Some(request_id),
        diagnostic_id: None,
        conflict: conflict_version.map(|entity_version| SafeConflictVersion {
            entity_version: Some(entity_version),
        }),
    })
}

fn plugin_cleanup_reconciliation_error(request_id: RequestId, code: &str) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        code,
        ErrorCategory::NeedsReconciliation,
        RetryStrategy::Reconcile,
        "plugins.remoteApproval.cleanupIncomplete",
        None,
    )
}

fn plugin_cleanup_failure_code(
    operations_clean: bool,
    resources_clean: bool,
    api_resources_clean: bool,
    observers_clean: bool,
    host_clean: bool,
) -> Option<&'static str> {
    (!operations_clean)
        .then_some("plugin.remote_cleanup_incomplete")
        .or_else(|| (!resources_clean).then_some("plugin.resource_cleanup_incomplete"))
        .or_else(|| (!api_resources_clean).then_some("plugin.api_resource_cleanup_incomplete"))
        .or_else(|| (!observers_clean).then_some("plugin.observer_cleanup_incomplete"))
        .or_else(|| (!host_clean).then_some("plugin.host_cleanup_incomplete"))
}

fn plugin_validation_error(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.invalid_request",
        ErrorCategory::Validation,
        RetryStrategy::Never,
        "errors.plugin.invalidRequest",
        None,
    )
}

fn uninstall_reconciliation_error(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.uninstall_requires_reconciliation",
        ErrorCategory::NeedsReconciliation,
        RetryStrategy::Reconcile,
        "errors.plugin.uninstallRequiresReconciliation",
        None,
    )
}

fn plugin_sensitive_data_delete_error(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.sensitive_data_delete_failed",
        ErrorCategory::Internal,
        RetryStrategy::WaitForUser,
        "errors.plugin.sensitiveDataDeleteFailed",
        None,
    )
}

fn plugin_state_reconciliation_error(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.state_requires_reconciliation",
        ErrorCategory::NeedsReconciliation,
        RetryStrategy::Reconcile,
        "errors.plugin.stateRequiresReconciliation",
        None,
    )
}

fn plugin_grants_reconciliation_error(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.grants_requires_reconciliation",
        ErrorCategory::NeedsReconciliation,
        RetryStrategy::Reconcile,
        "errors.plugin.grantsRequiresReconciliation",
        None,
    )
}

fn plugin_not_found_error(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.not_found",
        ErrorCategory::Unavailable,
        RetryStrategy::RefreshSnapshot,
        "errors.plugin.notFound",
        None,
    )
}

fn plugin_conflict_error(
    request_id: RequestId,
    version: Option<WireSequence>,
) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.conflict",
        ErrorCategory::Conflict,
        RetryStrategy::RefreshSnapshot,
        "errors.plugin.conflict",
        version,
    )
}

fn plugin_permission_error(request_id: RequestId) -> Box<CoreApiError> {
    plugin_error(
        request_id,
        "plugin.capability_denied",
        ErrorCategory::Permission,
        RetryStrategy::WaitForUser,
        "errors.plugin.capabilityDenied",
        None,
    )
}

fn plugin_runtime_error(
    request_id: RequestId,
    error: Option<PluginHostProcessError>,
) -> Box<CoreApiError> {
    let (code, category, retry, message) = match error {
        Some(PluginHostProcessError::TimedOut) => (
            "plugin.runtime_timed_out",
            ErrorCategory::Timeout,
            RetryStrategy::Never,
            "errors.plugin.runtimeTimedOut",
        ),
        #[cfg(not(unix))]
        Some(PluginHostProcessError::UnsupportedPlatform) => (
            "plugin.runtime_unsupported",
            ErrorCategory::Unavailable,
            RetryStrategy::Never,
            "errors.plugin.runtimeUnsupported",
        ),
        Some(PluginHostProcessError::Rejected) => (
            "plugin.runtime_rejected",
            ErrorCategory::Permission,
            RetryStrategy::Never,
            "errors.plugin.runtimeRejected",
        ),
        _ => (
            "plugin.runtime_failed",
            ErrorCategory::Unavailable,
            RetryStrategy::Never,
            "errors.plugin.runtimeFailed",
        ),
    };
    plugin_error(request_id, code, category, retry, message, None)
}

fn map_persistence_error(request_id: RequestId, error: AppPersistenceError) -> Box<CoreApiError> {
    match error {
        AppPersistenceError::InvalidInput(_) | AppPersistenceError::Endpoint(_) => {
            plugin_validation_error(request_id)
        }
        AppPersistenceError::NotFound => plugin_not_found_error(request_id),
        AppPersistenceError::Conflict
        | AppPersistenceError::IdempotencyConflict
        | AppPersistenceError::DatabaseNotFresh => plugin_conflict_error(request_id, None),
        AppPersistenceError::UnsupportedSchema(_) => plugin_error(
            request_id,
            "plugin.schema_incompatible",
            ErrorCategory::Incompatible,
            RetryStrategy::Upgrade,
            "errors.plugin.schemaIncompatible",
            None,
        ),
        AppPersistenceError::KnownHostMismatch { .. }
        | AppPersistenceError::KnownHostAlgorithmChanged { .. }
        | AppPersistenceError::RequiresReload
        | AppPersistenceError::RestoreCommitUnknown
        | AppPersistenceError::InvalidStoredData
        | AppPersistenceError::Database(_)
        | AppPersistenceError::Io(_) => Box::new(CoreApiError::safe_internal(
            request_id,
            Uuid::new_v4().to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        transient_credential_service::TransientCredentialService, vault_service::VaultService,
    };
    use norishell_core_api::{
        PluginApiErrorCode, PluginApiResourceState, PluginHostScopeSelection, RequestMeta,
    };
    use serde::de::DeserializeOwned;
    use tauri::{
        WebviewWindow,
        ipc::{CallbackFn, InvokeBody},
        test::{
            INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
        },
        webview::InvokeRequest,
    };
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    #[test]
    fn local_version_policy_accepts_only_newer_or_changed_same_version() {
        assert!(plugin_version_can_replace("1.0.1", "b", "1.0.0", "a"));
        assert!(plugin_version_can_replace("1.0.0", "b", "1.0.0", "a"));
        assert!(!plugin_version_can_replace("1.0.0", "a", "1.0.0", "a"));
        assert!(!plugin_version_can_replace("0.9.9", "b", "1.0.0", "a"));
    }

    #[test]
    fn plugin_stop_reports_reconciliation_after_aggregating_cleanup_failures() {
        assert_eq!(
            plugin_cleanup_failure_code(true, true, true, true, true),
            None
        );
        assert_eq!(
            plugin_cleanup_failure_code(true, true, true, true, false),
            Some("plugin.host_cleanup_incomplete")
        );
        assert_eq!(
            plugin_cleanup_failure_code(true, true, false, false, false),
            Some("plugin.api_resource_cleanup_incomplete")
        );
        assert_eq!(
            plugin_cleanup_failure_code(true, false, false, false, false),
            Some("plugin.resource_cleanup_incomplete")
        );
        assert_eq!(
            plugin_cleanup_failure_code(false, false, false, false, false),
            Some("plugin.remote_cleanup_incomplete")
        );

        let error = plugin_cleanup_reconciliation_error(
            RequestId::new(),
            plugin_cleanup_failure_code(true, true, true, false, false)
                .expect("observer failure must be retained"),
        );
        assert_eq!(error.code, "plugin.observer_cleanup_incomplete");
        assert_eq!(error.category, ErrorCategory::NeedsReconciliation);
        assert_eq!(error.retry_strategy, RetryStrategy::Reconcile);
    }

    #[tokio::test]
    async fn plugin_stop_keeps_failed_api_resources_for_reconciliation() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let plugin_id = PluginId::parse("com.norishell.cleanup-api").expect("plugin id");
        let owner = crate::plugin_api::ResourceOwner {
            plugin_id: plugin_id.clone(),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: WireSequence::new(1),
        };
        service
            .api
            .resources
            .spawn(owner.clone(), "failure-fixture", |_, _| async {
                Err(PluginApiErrorCode::CleanupIncomplete)
            })
            .expect("reserve API resource");

        let error = service
            .stop_plugin(&plugin_id)
            .await
            .expect_err("failed API cleanup must remain reconcilable");
        assert_eq!(error.code, "plugin.api_resource_cleanup_incomplete");
        assert_eq!(error.category, ErrorCategory::NeedsReconciliation);
        assert_eq!(
            service.api.resources.list(&owner)[0].state,
            PluginApiResourceState::CleanupIncomplete
        );

        let retry = service
            .shutdown_all()
            .await
            .expect_err("failed API resource stays in the shutdown owner set");
        assert_eq!(retry.code, "plugin.api_resource_cleanup_incomplete");
    }

    #[test]
    fn auto_refresh_accepts_only_fixed_read_only_no_approval_remote_plans() {
        use norishell_plugin_platform::operations::{
            ProcessIdentity, ProcessOperation, ProcessSignal, RemoteOperation,
        };

        let parsed_for = |operation: RemoteOperation| {
            parse_plugin_ui_outputs(
                "request-1",
                vec![norishell_core_api::PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "remote.operation.request".to_owned(),
                    payload_json: serde_json::json!({
                        "terminalHandle": "a".repeat(64),
                        "operationJson": serde_json::to_string(&operation).expect("operation"),
                        "reason": "Refresh current status"
                    })
                    .to_string(),
                }],
            )
            .expect("parse broker request")
        };
        let read_only = parsed_for(RemoteOperation::Process(ProcessOperation::CpuUsage {}));
        assert!(auto_refresh_initial_output_is_admissible(&read_only));
        assert!(auto_refresh_remote_operation_is_read_only(&read_only));

        let mutation = parsed_for(RemoteOperation::Process(ProcessOperation::Signal {
            identity: ProcessIdentity {
                pid: 123,
                start_time_ticks: 456,
            },
            signal: ProcessSignal::Term,
        }));
        assert!(auto_refresh_initial_output_is_admissible(&mutation));
        assert!(!auto_refresh_remote_operation_is_read_only(&mutation));

        let callback = parse_plugin_ui_outputs(
            "request-1",
            vec![
                norishell_core_api::PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ui.document".to_owned(),
                    payload_json: serde_json::json!({
                        "targetId": "terminal.footer",
                        "document": {
                            "schemaVersion": 1,
                            "rootNodeId": "root",
                            "nodes": [{
                                "kind": "text",
                                "nodeId": "root",
                                "text": "Updated",
                                "style": "body",
                                "tone": "neutral"
                            }]
                        }
                    })
                    .to_string(),
                },
                norishell_core_api::PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "storage.write".to_owned(),
                    payload_json: serde_json::json!({
                        "writeToken": Uuid::new_v4().to_string(),
                        "valueJson": "{}"
                    })
                    .to_string(),
                },
            ],
        )
        .expect("parse callback output");
        assert!(!auto_refresh_callback_output_is_admissible(&callback));
    }

    #[test]
    fn ssh_sync_action_keeps_page_open_until_core_returns_replacement_document() {
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![norishell_core_api::PluginRuntimeOutput {
                request_id: "request-1".to_owned(),
                kind: "ssh.sync.request".to_owned(),
                payload_json: serde_json::json!({
                    "action": "status",
                    "profileId": "primary"
                })
                .to_string(),
            }],
        )
        .expect("parse sync request");
        assert!(parsed.templates.is_empty());
        assert!(ui_action_output_is_admissible(
            &parsed,
            PluginUiActionKind::Standard,
            false
        ));
        assert!(!auto_refresh_initial_output_is_admissible(&parsed));
    }

    #[test]
    fn plugin_ui_parser_rejects_invalid_api_call_ids() {
        let output = |call_id: &str| norishell_core_api::PluginRuntimeOutput {
            request_id: "request-1".to_owned(),
            kind: "api.request".to_owned(),
            payload_json: serde_json::json!({
                "callId": call_id,
                "operation": {"kind": "describe"}
            })
            .to_string(),
        };
        assert!(parse_plugin_ui_outputs("request-1", vec![output("safe.call_id-1")]).is_ok());
        assert!(parse_plugin_ui_outputs("request-1", vec![output("invalid:call")]).is_err());
    }

    #[test]
    fn plugin_host_payload_redacts_page_password_values() {
        let password_id =
            norishell_core_api::PluginUiFieldId::parse("password").expect("password field id");
        let username_id =
            norishell_core_api::PluginUiFieldId::parse("username").expect("username field id");
        let document = PluginUiDocument {
            schema_version: norishell_core_api::PLUGIN_UI_SCHEMA_VERSION,
            root_node_id: norishell_core_api::PluginUiNodeId::parse("root").expect("root node id"),
            nodes: vec![
                norishell_core_api::PluginUiNode::Stack {
                    node_id: norishell_core_api::PluginUiNodeId::parse("root")
                        .expect("root node id"),
                    direction: norishell_core_api::PluginUiDirection::Vertical,
                    align: norishell_core_api::PluginUiAlign::Stretch,
                    gap: 8,
                    children: vec![
                        norishell_core_api::PluginUiNodeId::parse("username")
                            .expect("username node id"),
                        norishell_core_api::PluginUiNodeId::parse("password")
                            .expect("password node id"),
                    ],
                },
                norishell_core_api::PluginUiNode::TextField {
                    node_id: norishell_core_api::PluginUiNodeId::parse("username")
                        .expect("username node id"),
                    field_id: username_id.clone(),
                    label: "Email".to_owned(),
                    value: String::new(),
                    placeholder: None,
                    field_kind: norishell_core_api::PluginUiFieldKind::Text,
                    required: true,
                    disabled: false,
                },
                norishell_core_api::PluginUiNode::TextField {
                    node_id: norishell_core_api::PluginUiNodeId::parse("password")
                        .expect("password node id"),
                    field_id: password_id.clone(),
                    label: "Password".to_owned(),
                    value: String::new(),
                    placeholder: None,
                    field_kind: norishell_core_api::PluginUiFieldKind::Password,
                    required: true,
                    disabled: false,
                },
            ],
        };
        let visible = plugin_visible_ui_fields(
            Some(&document),
            &[
                PluginUiFieldValue {
                    field_id: username_id,
                    value: "alice@example.com".to_owned(),
                },
                PluginUiFieldValue {
                    field_id: password_id,
                    value: "never-reach-wasm".to_owned(),
                },
            ],
        );
        assert_eq!(visible[0].value, "alice@example.com");
        assert!(visible[1].value.is_empty());
    }

    #[test]
    fn standard_ui_actions_never_admit_clipboard_output() {
        let parsed = parse_plugin_ui_outputs(
            "request-1",
            vec![
                norishell_core_api::PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "ui.document".to_owned(),
                    payload_json: serde_json::json!({
                        "targetId": "terminal.tools",
                        "document": {
                            "schemaVersion": 1,
                            "rootNodeId": "root",
                            "nodes": [{
                                "kind": "text",
                                "nodeId": "root",
                                "text": "Ready",
                                "style": "body",
                                "tone": "neutral"
                            }]
                        }
                    })
                    .to_string(),
                },
                norishell_core_api::PluginRuntimeOutput {
                    request_id: "request-1".to_owned(),
                    kind: "clipboard.write".to_owned(),
                    payload_json: serde_json::json!({"text": "clipboard"}).to_string(),
                },
            ],
        )
        .expect("strictly parsed output");
        assert!(!ui_action_output_is_admissible(
            &parsed,
            PluginUiActionKind::Standard,
            false,
        ));
        assert!(ui_action_output_is_admissible(
            &parsed,
            PluginUiActionKind::Copy,
            false,
        ));
    }

    #[test]
    fn retained_update_grants_preserve_true_false_and_require_new_special_approval() {
        let plugin_id = PluginId::parse("com.norishell.retained").expect("plugin id");
        let binding = current_plugin_permission_binding(&"a".repeat(64));
        let retained = vec![
            PluginCapabilityGrantRecord {
                plugin_id: plugin_id.clone(),
                signer_fingerprint_sha256: "1".repeat(64),
                major_version: u64::from(PLUGIN_PROTOCOL_MAJOR),
                capability: PluginCapability::UiPanel,
                granted: true,
                state_version: WireSequence::new(4),
                binding: Some(binding.clone()),
            },
            PluginCapabilityGrantRecord {
                plugin_id,
                signer_fingerprint_sha256: "1".repeat(64),
                major_version: u64::from(PLUGIN_PROTOCOL_MAJOR),
                capability: PluginCapability::SshSync,
                granted: false,
                state_version: WireSequence::new(4),
                binding: Some(binding),
            },
        ];
        let capabilities = [
            PluginCapability::UiPanel,
            PluginCapability::SshSync,
            PluginCapability::UiHostCss,
        ];
        let supplied = [
            PluginCapabilityGrant {
                capability: PluginCapability::UiPanel,
                granted: false,
            },
            PluginCapabilityGrant {
                capability: PluginCapability::SshSync,
                granted: true,
            },
            PluginCapabilityGrant {
                capability: PluginCapability::UiHostCss,
                granted: true,
            },
        ];
        let resolved = resolve_install_grants(&capabilities, &supplied, &retained)
            .expect("complete update decision");
        assert_eq!(
            resolved,
            vec![
                (PluginCapability::UiPanel, true),
                (PluginCapability::SshSync, false),
                (PluginCapability::UiHostCss, true),
            ]
        );
        assert!(!newly_requested_special_capabilities_are_denied(
            &supplied, &retained
        ));
        let denied_new_special = [
            supplied[0].clone(),
            supplied[1].clone(),
            PluginCapabilityGrant {
                capability: PluginCapability::UiHostCss,
                granted: false,
            },
        ];
        assert!(newly_requested_special_capabilities_are_denied(
            &denied_new_special,
            &retained
        ));
    }

    #[test]
    fn stale_binding_can_reapprove_ordinary_but_not_special_permission() {
        let installed = PluginInstalledRecord {
            plugin_id: PluginId::parse("com.norishell.binding").expect("plugin id"),
            name: "Binding".to_owned(),
            publisher: "Publisher".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::UiPanel, PluginCapability::SshSync],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let stale_binding = PluginPermissionBinding {
            artifact_sha256: installed.package_sha256.clone(),
            app_version_major: current_app_version().major,
            app_version_minor: current_app_version().minor.saturating_add(1),
            secure_surface_contract_revision: PLUGIN_PERMISSION_SURFACE_CONTRACT_REVISION,
        };
        let current = vec![
            PluginCapabilityGrantRecord {
                plugin_id: installed.plugin_id.clone(),
                signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
                major_version: u64::from(PLUGIN_PROTOCOL_MAJOR),
                capability: PluginCapability::UiPanel,
                granted: true,
                state_version: WireSequence::new(1),
                binding: Some(stale_binding.clone()),
            },
            PluginCapabilityGrantRecord {
                plugin_id: installed.plugin_id.clone(),
                signer_fingerprint_sha256: installed.signer_fingerprint_sha256.clone(),
                major_version: u64::from(PLUGIN_PROTOCOL_MAJOR),
                capability: PluginCapability::SshSync,
                granted: true,
                state_version: WireSequence::new(1),
                binding: Some(stale_binding),
            },
        ];
        let requested = vec![
            PluginCapabilityGrant {
                capability: PluginCapability::UiPanel,
                granted: true,
            },
            PluginCapabilityGrant {
                capability: PluginCapability::SshSync,
                granted: true,
            },
        ];
        assert!(!plugin_grant_request_is_effectively_unchanged(
            &installed, &current, &requested
        ));
        assert!(!requested_special_grants_do_not_expand(
            &installed, &current, &requested
        ));
    }

    #[test]
    fn plugin_locale_is_persisted_for_the_next_runtime_restore() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        {
            let hosts = HostService::start(directory.path()).expect("host repository");
            let sessions = SshSessionService::start(
                hosts.clone(),
                VaultService::start(directory.path()),
                TransientCredentialService::default(),
            );
            let service =
                PluginService::start(directory.path(), hosts, sessions).expect("first service");
            assert_eq!(service.locale().as_str(), "zh-CN");
            service
                .set_locale(
                    PluginLocale::parse("en").expect("English locale"),
                    RequestId::new(),
                )
                .expect("persist locale");
            assert_eq!(service.locale().as_str(), "en");
        }
        assert_eq!(
            fs::read_to_string(directory.path().join("plugin-locale"))
                .expect("persisted plugin locale"),
            "en",
        );

        let hosts = HostService::start(directory.path()).expect("restarted host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let restarted =
            PluginService::start(directory.path(), hosts, sessions).expect("restarted service");
        assert_eq!(restarted.locale().as_str(), "en");
        restarted
            .runtime
            .lock()
            .expect("runtime")
            .active_instances
            .insert(
                "org.example.locale".to_owned(),
                ActivePluginInstance {
                    plugin_name: "Locale Fixture".to_owned(),
                    signer_fingerprint_sha256: "a".repeat(64),
                    package_sha256: "b".repeat(64),
                    instance_generation: WireSequence::new(1),
                    state_version: WireSequence::new(1),
                    previously_crashed: false,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    locale: PluginLocale::parse("en").expect("active locale"),
                    contributions: BTreeMap::new(),
                    ui_templates: BTreeMap::new(),
                    navigation: BTreeMap::new(),
                    pages: BTreeMap::new(),
                    contribution_revision: WireSequence::new(1),
                    settings_revision: None,
                    contribution_action_in_flight: false,
                    ui_state_json: "{}".to_owned(),
                    scoped_templates: BTreeMap::new(),
                    scoped_states: BTreeMap::new(),
                    operation_authority_revoked: false,
                    api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    process: Arc::new(Mutex::new(None)),
                },
            );
        restarted
            .set_locale(
                PluginLocale::parse("zh-CN").expect("Chinese locale"),
                RequestId::new(),
            )
            .expect("update active locale");
        assert_eq!(restarted.locale().as_str(), "zh-CN");
        assert_eq!(
            restarted
                .runtime
                .lock()
                .expect("runtime")
                .active_instances
                .get("org.example.locale")
                .expect("active instance")
                .locale
                .as_str(),
            "zh-CN",
        );
    }

    #[test]
    #[ignore = "requires a local signed plugin ZIP and the current real desktop executable"]
    fn local_signed_norixor_real_host_initialization() {
        use std::io::Read as _;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let package_path = std::env::var_os("NORISHELL_INITIALIZE_TEST_ZIP")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                root.join("output/official-plugins/protocol13-20260913/signed/Norixor-1.0.10.zip")
            });
        let executable = std::env::var_os("NORISHELL_INITIALIZE_TEST_HOST")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target/debug/norishell"));
        let package = std::fs::read(&package_path).expect("stage ZIP read");
        eprintln!(
            "package={} sha256={} executable={}",
            package_path.display(),
            hex::encode(Sha256::digest(&package)),
            executable.display()
        );
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(package)).expect("stage ZIP parse");
        let manifest: serde_json::Value = serde_json::from_reader(
            archive
                .by_name("manifest.json")
                .expect("stage manifest entry"),
        )
        .expect("stage manifest serde");
        let ssh_sync_declared = manifest["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "sshSync");
        let mut module = Vec::new();
        archive
            .by_name("plugin.wasm")
            .expect("stage module entry")
            .read_to_end(&mut module)
            .expect("stage module read");
        if let Some(path) = std::env::var_os("NORISHELL_INITIALIZE_TEST_WASM") {
            module = std::fs::read(&path).expect("stage unsigned candidate Wasm read");
            eprintln!(
                "candidate_wasm={} sha256={}",
                PathBuf::from(path).display(),
                hex::encode(Sha256::digest(&module))
            );
        }
        let mut host = PluginHostProcess::spawn_with_executable_for_tests(&executable, &module, 1)
            .expect("stage spawn");
        let request_id = Uuid::new_v4().to_string();
        let outputs = host
            .execute(PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: request_id.clone(),
                kind: PluginHostMessageKind::Initialize,
                payload_json: plugin_host_payload(
                    &PluginLocale::parse("zh-CN").unwrap(),
                    serde_json::json!({
                        "pluginId": manifest["pluginId"], "version": manifest["version"], "storage": null
                    }),
                ),
            })
            .expect("stage host.execute Initialize");
        if manifest["pluginId"] == "org.norixor"
            && std::env::var_os("NORISHELL_INITIALIZE_TEST_STATUS_CHAIN").is_some()
        {
            let original_page: PluginPageContribution = serde_json::from_str(
                &outputs
                    .iter()
                    .find(|output| output.kind == "ui.page")
                    .unwrap()
                    .payload_json,
            )
            .unwrap();
            for error_code in [
                None,
                Some("vaultLocked"),
                Some("vaultMissing"),
                Some("interactionRequired"),
            ] {
                for (kind, result) in [
                    (PluginHostMessageKind::UiAction, serde_json::Value::Null),
                    (
                        PluginHostMessageKind::SshSyncResult,
                        serde_json::json!({
                            "profileId":"primary", "accountState":"connected", "operationState":if error_code.is_some() {"failed"} else {"succeeded"},
                            "stableErrorCode":error_code, "httpStatus":200, "localHostCount":12,"localCredentialCount":7,
                            "remoteHostCount":10,"remoteCredentialCount":6,"differenceState":"different","scopeMode":"allEligible"
                        }),
                    ),
                ] {
                    let callback_id = Uuid::new_v4().to_string();
                    let callback = host.execute(PluginHostRequest {
                        protocol_major: PLUGIN_PROTOCOL_MAJOR, protocol_minor: PLUGIN_PROTOCOL_MINOR,
                        request_id: callback_id.clone(), kind,
                        payload_json: plugin_host_payload(&PluginLocale::parse("zh-CN").unwrap(), serde_json::json!({
                            "targetId":"app.page","contextHandle":"context-1","targetRevision":"1","actionId":"status",
                            "fields":[],"storage":null,"result":result
                        })),
                    }).expect("stage status/callback execute");
                    for output in &callback {
                        if output.kind == "ui.document" {
                            let template: PluginUiTemplate = serde_json::from_str(
                                &output.payload_json,
                            )
                            .unwrap_or_else(|error| {
                                panic!("stage {kind:?}/{error_code:?} template serde: {error}")
                            });
                            norishell_plugin_platform::validate_plugin_page_document(
                                &template.document,
                            )
                            .unwrap_or_else(|error| {
                                panic!("stage {kind:?}/{error_code:?} page document: {error:?}")
                            });
                            let mut replacement = original_page.clone();
                            replacement.document = template.document.clone();
                            norishell_plugin_platform::validate_plugin_page_lifecycle(&replacement)
                                .expect("stage retained Page lifecycle after callback replacement");
                            validate_template_lifecycle(&template)
                                .expect("stage callback template lifecycle");
                        }
                    }
                    if kind == PluginHostMessageKind::SshSyncResult {
                        assert_eq!(
                            callback.len(),
                            1,
                            "status callback must only replace the document"
                        );
                        assert_eq!(callback[0].kind, "ui.document");
                    }
                    let parsed =
                        parse_plugin_ui_outputs(&callback_id, callback).unwrap_or_else(|()| {
                            panic!("stage {kind:?}/{error_code:?} parse_plugin_ui_outputs")
                        });
                    assert!(
                        ui_action_output_is_admissible(
                            &parsed,
                            PluginUiActionKind::Standard,
                            false
                        ),
                        "stage status action output admission"
                    );
                    eprintln!("stage {kind:?}/{error_code:?} callback parse passed");
                }
            }
        }
        host.shutdown().expect("stage host cleanup");
        for (index, output) in outputs.iter().enumerate() {
            eprintln!(
                "output[{index}] kind={} bytes={} request_matches={}",
                output.kind,
                output.payload_json.len(),
                output.request_id == request_id
            );
            if output.kind == "ui.navigation" {
                serde_json::from_str::<PluginNavigationContribution>(&output.payload_json)
                    .unwrap_or_else(|error| {
                        panic!("stage navigation serde output[{index}]: {error}")
                    });
            }
            if output.kind == "ui.page" {
                let page = serde_json::from_str::<PluginPageContribution>(&output.payload_json)
                    .unwrap_or_else(|error| panic!("stage page serde output[{index}]: {error}"));
                norishell_plugin_platform::validate_plugin_page_document(&page.document)
                    .unwrap_or_else(|error| {
                        panic!("stage page document {}: {error:?}", page.page_id.as_str())
                    });
                assert!(
                    ssh_sync_browser_nodes_supported(&page.document, ssh_sync_declared),
                    "stage ssh sync nodes"
                );
            }
        }
        let raw_navigation = outputs
            .iter()
            .filter(|output| output.kind == "ui.navigation")
            .map(|output| {
                serde_json::from_str::<PluginNavigationContribution>(&output.payload_json).unwrap()
            })
            .collect::<Vec<_>>();
        let raw_pages = outputs
            .iter()
            .filter(|output| output.kind == "ui.page")
            .map(|output| {
                serde_json::from_str::<PluginPageContribution>(&output.payload_json).unwrap()
            })
            .collect::<Vec<_>>();
        for page in &raw_pages {
            eprintln!(
                "page={} icon={:?} on_open={:?}",
                page.page_id.as_str(),
                page.icon,
                page.on_open_action_id
            );
            if let Some(action_id) = &page.on_open_action_id {
                for node in &page.document.nodes {
                    let value = serde_json::to_value(node).unwrap();
                    if value.get("actionId").is_some() || value.get("fieldId").is_some() {
                        eprintln!(
                            "node id={} kind={} action={} field={} disabled={}",
                            value["nodeId"],
                            value["kind"],
                            value["actionId"],
                            value["fieldId"],
                            value["disabled"]
                        );
                    }
                }
                let result = norishell_plugin_platform::plugin_page_lifecycle_action_admission(
                    page,
                    action_id,
                    &[],
                );
                assert_eq!(result, Ok(true), "stage Page lifecycle admission");
                eprintln!("stage on_open action {}: {result:?}", action_id.as_str());
            }
        }
        eprintln!("navigation={raw_navigation:?}");
        norishell_plugin_platform::validate_plugin_navigation(&raw_navigation, &raw_pages)
            .unwrap_or_else(|error| panic!("stage validate_plugin_navigation: {error:?}"));
        let parsed =
            parse_plugin_ui_outputs(&request_id, outputs).expect("stage parse_plugin_ui_outputs");
        assert!(
            parsed.clipboard_text.is_none()
                && parsed.host_mutation.is_none()
                && parsed.host_session.is_none()
                && parsed.host_dom_operations.is_none()
                && parsed.terminal_input_suggestion.is_none()
                && parsed.isolated_surface.is_none()
                && parsed.storage_write.is_none()
                && parsed.ssh_sync_request.is_none()
                && parsed.remote_operation.is_none()
                && parsed.resource_operation.is_none()
                && parsed.api_call.is_none()
                && parsed.ui_state.is_none(),
            "stage initialize side effect admission"
        );
        contribution_map(parsed.panels).expect("stage contribution_map");
        ui_template_map(parsed.templates).expect("stage ui_template_map");
        let navigation = navigation_map(parsed.navigation).expect("stage navigation_map");
        let pages = page_map(parsed.pages).expect("stage page_map");
        assert!(
            navigation
                .values()
                .all(|item| pages.contains_key(item.page_id.as_str())),
            "stage navigation references"
        );
        eprintln!(
            "all initialization stages passed navigation={} pages={}",
            navigation.len(),
            pages.len()
        );
    }

    fn request_meta() -> RequestMeta {
        RequestMeta {
            request_id: RequestId::new(),
        }
    }

    fn write_local_plugin_fixture(
        directory: &tempfile::TempDir,
        file_name: &str,
        plugin_id: &str,
        version: &str,
        capabilities: &[PluginCapability],
    ) -> PathBuf {
        let path = directory.path().join(file_name);
        let file = File::create(&path).expect("create local plugin fixture");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let manifest = serde_json::json!({
            "pluginId": plugin_id,
            "name": "Permission fixture",
            "publisher": "NoriShell tests",
            "version": version,
            "protocolMajor": PLUGIN_PROTOCOL_MAJOR,
            "protocolMinor": PLUGIN_PROTOCOL_MINOR,
            "platform": "desktop",
            "architectures": ["universal"],
            "capabilities": capabilities,
            "minimumAppVersion": "0.1.0",
            "minimumCoreApiVersion": capabilities
                .contains(&PluginCapability::SshSync)
                .then(norishell_core_api::CoreApiVersion::current),
        });
        archive
            .start_file("manifest.json", options)
            .expect("start fixture manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest).expect("serialize fixture manifest"))
            .expect("write fixture manifest");
        archive
            .start_file("plugin.wasm", options)
            .expect("start fixture wasm");
        archive
            .write_all(b"\0asm\x01\0\0\0")
            .expect("write fixture wasm");
        archive.finish().expect("finish fixture package");
        path
    }

    fn clear_theme_package_fixture(directory: &tempfile::TempDir) -> PathBuf {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf();
        let generated = workspace_root.join("output/theme-plugins/NoriShell-Theme-Clear-1.0.0.zip");
        if generated.is_file() {
            return generated;
        }
        let source = workspace_root.join("examples/theme-plugins/clear");
        let package = directory.path().join("clear-theme.zip");
        let file = File::create(&package).expect("create theme fixture");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for relative in ["manifest.json", "assets/theme.json"] {
            archive.start_file(relative, options).expect("theme entry");
            archive
                .write_all(&fs::read(source.join(relative)).expect("theme source"))
                .expect("theme bytes");
        }
        archive.finish().expect("finish theme fixture");
        package
    }

    fn invoke_plugin_command<T: DeserializeOwned>(
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
                .expect("mock Tauri URL"),
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

    fn prepared_special_permission_request(
        preview: &PluginLocalPackagePreview,
        requested_capability: Option<PluginCapability>,
    ) -> PluginSpecialPermissionOpenRequest {
        PluginSpecialPermissionOpenRequest {
            meta: request_meta(),
            target: PluginSpecialPermissionTarget::PreparedPackage {
                preparation_id: preview.preparation_id.clone(),
                expected_package_sha256: preview.package_sha256.clone(),
                expected_state_version: preview.current_state_version,
            },
            requested_capability,
        }
    }

    fn local_install_request_from_preview(
        preview: &PluginLocalPackagePreview,
        grants: Vec<PluginCapabilityGrant>,
    ) -> PluginLocalInstallRequest {
        PluginLocalInstallRequest {
            meta: request_meta(),
            operation_id: norishell_core_api::PluginOperationId::new(),
            idempotency_key: Uuid::new_v4().to_string(),
            preparation_id: preview.preparation_id.clone(),
            expected_package_sha256: preview.package_sha256.clone(),
            expected_state_version: preview.current_state_version,
            capability_grants: grants,
        }
    }

    #[test]
    fn local_zip_prepare_and_commit_bind_artifact_hash_and_explicit_grants() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let source = write_local_plugin_fixture(
            &directory,
            "local-fixture.zip",
            "com.norishell.fixture",
            "1.0.0",
            &[PluginCapability::UiPanel],
        );
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare local package");
        let grants = preview
            .capabilities
            .iter()
            .copied()
            .map(|capability| PluginCapabilityGrant {
                capability,
                granted: false,
            })
            .collect::<Vec<_>>();
        let operation = service
            .install_local_plugin(PluginLocalInstallRequest {
                meta: request_meta(),
                operation_id: norishell_core_api::PluginOperationId::new(),
                idempotency_key: Uuid::new_v4().to_string(),
                preparation_id: preview.preparation_id,
                expected_package_sha256: preview.package_sha256.clone(),
                expected_state_version: preview.current_state_version,
                capability_grants: grants,
            })
            .expect("install local package");

        assert_eq!(
            operation.state,
            PluginOperationState::Succeeded,
            "{operation:?}"
        );
        let installed = service
            .list_installed(RequestId::new())
            .expect("installed projection");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].package_sha256, preview.package_sha256);
        assert_eq!(
            installed[0].signer_fingerprint_sha256,
            preview.package_sha256
        );
        assert!(installed[0].grants.iter().all(|grant| !grant.granted));
        assert!(
            service
                .runtime
                .lock()
                .unwrap()
                .prepared_local_packages
                .is_empty()
        );
    }

    #[test]
    fn stale_local_preparation_cannot_downgrade_a_newer_installed_version() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let plugin_id = "com.norishell.rollback-fixture";

        let v1 = write_local_plugin_fixture(
            &directory,
            "rollback-v1.zip",
            plugin_id,
            "1.0.0",
            &[PluginCapability::UiPanel],
        );
        let v1 = service
            .prepare_local_package(&v1, RequestId::new())
            .expect("prepare v1");
        service
            .install_local_plugin(local_install_request_from_preview(
                &v1,
                vec![PluginCapabilityGrant {
                    capability: PluginCapability::UiPanel,
                    granted: false,
                }],
            ))
            .expect("install v1");

        let v2 = write_local_plugin_fixture(
            &directory,
            "rollback-v2.zip",
            plugin_id,
            "2.0.0",
            &[PluginCapability::UiPanel],
        );
        let v2 = service
            .prepare_local_package(&v2, RequestId::new())
            .expect("prepare v2");
        let v3 = write_local_plugin_fixture(
            &directory,
            "rollback-v3.zip",
            plugin_id,
            "3.0.0",
            &[PluginCapability::UiPanel],
        );
        let v3 = service
            .prepare_local_package(&v3, RequestId::new())
            .expect("prepare v3");
        service
            .install_local_plugin(local_install_request_from_preview(
                &v3,
                vec![PluginCapabilityGrant {
                    capability: PluginCapability::UiPanel,
                    granted: false,
                }],
            ))
            .expect("install v3");

        let installed_v3 = service
            .list_installed(RequestId::new())
            .expect("list v3")
            .pop()
            .expect("installed v3");
        let mut stale_request = local_install_request_from_preview(
            &v2,
            vec![PluginCapabilityGrant {
                capability: PluginCapability::UiPanel,
                granted: false,
            }],
        );
        stale_request.expected_state_version = Some(installed_v3.state_version);
        assert!(
            service.install_local_plugin(stale_request).is_err(),
            "a stale prepared package must not downgrade a newer installation"
        );

        let installed = service
            .list_installed(RequestId::new())
            .expect("list after rejected downgrade")
            .pop()
            .expect("installed plugin");
        assert_eq!(installed.active_version, "3.0.0");
    }

    #[tokio::test]
    async fn pure_data_themes_use_install_state_without_a_plugin_host_lifecycle() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let source = clear_theme_package_fixture(&directory);
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare clear theme");
        assert!(preview.capabilities.is_empty());
        service
            .install_local_plugin(local_install_request_from_preview(&preview, Vec::new()))
            .expect("install clear theme");

        let disabled_themes = service.list_themes(RequestId::new()).expect("theme list");
        assert_eq!(disabled_themes.themes.len(), 1);
        assert_eq!(disabled_themes.themes[0].definition.id, "clear");
        assert!(!disabled_themes.themes[0].enabled);
        let initial = service
            .list_installed(RequestId::new())
            .expect("installed theme")
            .pop()
            .expect("theme installation");
        assert_eq!(initial.package_kind, PluginPackageKind::Theme);
        assert!(service.exit_blockers().is_empty());

        let app = crate::with_production_invoke_handler(
            mock_builder()
                .manage(LifecycleState::default())
                .manage(service.clone()),
        )
        .build(mock_context(noop_assets()))
        .expect("mock Tauri application");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("mock main window");

        let listed: PluginThemeListResponse = invoke_plugin_command(
            &webview,
            "plugin_theme_list",
            serde_json::json!({ "request": PluginThemeListRequest { meta: request_meta() } }),
        );
        assert_eq!(listed.themes, disabled_themes.themes);

        let enabled: InstalledPluginSummary = invoke_plugin_command(
            &webview,
            "plugin_enable",
            serde_json::json!({
                "request": PluginStateChangeRequest {
                    meta: request_meta(),
                    plugin_id: initial.plugin_id.clone(),
                    expected_state_version: initial.state_version,
                }
            }),
        );
        assert_eq!(enabled.package_kind, PluginPackageKind::Theme);
        assert_eq!(enabled.state, PluginInstallState::Enabled);
        assert!(
            service
                .runtime
                .lock()
                .expect("runtime")
                .active_instances
                .is_empty()
        );
        assert!(service.exit_blockers().is_empty());
        assert!(
            service
                .enabled_plugins_for_startup()
                .expect("startup candidates")
                .is_empty()
        );
        assert!(
            service
                .list_themes(RequestId::new())
                .expect("enabled theme list")
                .themes[0]
                .enabled
        );

        let disabled: InstalledPluginSummary = invoke_plugin_command(
            &webview,
            "plugin_disable",
            serde_json::json!({
                "request": PluginStateChangeRequest {
                    meta: request_meta(),
                    plugin_id: enabled.plugin_id.clone(),
                    expected_state_version: enabled.state_version,
                }
            }),
        );
        assert_eq!(disabled.state, PluginInstallState::Disabled);
        assert!(
            !service
                .list_themes(RequestId::new())
                .expect("disabled theme list")
                .themes[0]
                .enabled
        );

        let reenabled: InstalledPluginSummary = invoke_plugin_command(
            &webview,
            "plugin_enable",
            serde_json::json!({
                "request": PluginStateChangeRequest {
                    meta: request_meta(),
                    plugin_id: disabled.plugin_id.clone(),
                    expected_state_version: disabled.state_version,
                }
            }),
        );
        let uninstalled: PluginOperationSummary = invoke_plugin_command(
            &webview,
            "plugin_uninstall",
            serde_json::json!({
                "request": norishell_core_api::PluginUninstallRequest {
                    meta: request_meta(),
                    operation_id: norishell_core_api::PluginOperationId::new(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    plugin_id: reenabled.plugin_id,
                    expected_state_version: reenabled.state_version,
                    delete_data: false,
                }
            }),
        );
        assert_eq!(uninstalled.state, PluginOperationState::Succeeded);
        assert!(
            service
                .list_themes(RequestId::new())
                .expect("empty theme list")
                .themes
                .is_empty()
        );
        assert!(
            service
                .list_installed(RequestId::new())
                .expect("empty installed list")
                .is_empty()
        );
        assert!(service.exit_blockers().is_empty());
    }

    #[test]
    fn safe_mode_lists_installed_themes_as_disabled_and_never_restores_a_host() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let plugin_id;
        {
            let hosts = HostService::start(directory.path()).expect("host repository");
            let sessions = SshSessionService::start(
                hosts.clone(),
                VaultService::start(directory.path()),
                TransientCredentialService::default(),
            );
            let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
            let preview = service
                .prepare_local_package(&clear_theme_package_fixture(&directory), RequestId::new())
                .expect("prepare clear theme");
            plugin_id = preview.plugin_id.clone();
            service
                .install_local_plugin(local_install_request_from_preview(&preview, Vec::new()))
                .expect("install clear theme");
            let installed = service
                .hosts
                .with_plugin_repository(|repository| repository.get_plugin_installation(&plugin_id))
                .expect("installed theme");
            service
                .set_plugin_state_convergent(
                    RequestId::new(),
                    &installed,
                    PluginInstallState::Enabled,
                )
                .expect("enable persisted theme");
        }
        fs::write(
            directory.path().join("plugin-safe-mode-next-start"),
            b"safe-mode\n",
        )
        .expect("schedule safe mode");
        let hosts = HostService::start(directory.path()).expect("reopen host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service =
            PluginService::start(directory.path(), hosts, sessions).expect("safe service");
        assert!(service.readiness().safe_mode_active);
        let themes = service.list_themes(RequestId::new()).expect("theme list");
        assert_eq!(themes.themes.len(), 1);
        assert_eq!(themes.themes[0].plugin_id, plugin_id);
        assert!(!themes.themes[0].enabled);
        assert!(
            service
                .enabled_plugins_for_startup()
                .expect("safe startup candidates")
                .is_empty()
        );
        assert!(service.exit_blockers().is_empty());
    }

    #[test]
    fn prepared_special_grant_cannot_install_without_secure_approval_and_is_consumed() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let source = write_local_plugin_fixture(
            &directory,
            "unapproved-special.zip",
            "com.norishell.unapproved-special",
            "1.0.0",
            &[PluginCapability::UiPanel, PluginCapability::SshSync],
        );
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare special package");
        let private_path = service
            .runtime
            .lock()
            .expect("runtime")
            .prepared_local_packages
            .get(&preview.preparation_id)
            .expect("prepared package")
            .path
            .clone();
        let mut stale_target =
            prepared_special_permission_request(&preview, Some(PluginCapability::SshSync));
        if let PluginSpecialPermissionTarget::PreparedPackage {
            expected_package_sha256,
            ..
        } = &mut stale_target.target
        {
            *expected_package_sha256 = "f".repeat(64);
        }
        let stale_error = service
            .prepare_special_permission(stale_target)
            .expect_err("a changed package cannot reuse this preparation");
        assert_eq!(stale_error.code, "plugin.conflict");
        let request = local_install_request_from_preview(
            &preview,
            vec![
                PluginCapabilityGrant {
                    capability: PluginCapability::UiPanel,
                    granted: true,
                },
                PluginCapabilityGrant {
                    capability: PluginCapability::SshSync,
                    granted: true,
                },
            ],
        );

        let error = service
            .install_local_plugin(request.clone())
            .expect_err("main cannot approve a special capability itself");
        assert_eq!(error.code, "plugin.conflict");
        assert!(!private_path.exists());
        let runtime = service.runtime.lock().expect("runtime");
        assert!(
            !runtime
                .prepared_local_packages
                .contains_key(&preview.preparation_id)
        );
        assert!(runtime.pending_special_permissions.is_empty());
        drop(runtime);
        assert!(service.install_local_plugin(request).is_err());
    }

    #[tokio::test]
    async fn protected_prepared_approval_commits_special_grants_and_host_scope_atomically() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let host = hosts
            .with_plugin_repository(|repository| {
                repository.create_host("Scoped", "scoped.example", 22, None, None, false)
            })
            .expect("fixture Host");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let source = write_local_plugin_fixture(
            &directory,
            "approved-special.zip",
            "com.norishell.approved-special",
            "1.0.0",
            &[
                PluginCapability::UiPanel,
                PluginCapability::HostMetadataRead,
                PluginCapability::HostMutationPropose,
            ],
        );
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare special package");
        let snapshot = service
            .prepare_special_permission(prepared_special_permission_request(
                &preview,
                Some(PluginCapability::HostMutationPropose),
            ))
            .expect("open protected approval");
        let special_grants = snapshot
            .special_grants
            .iter()
            .map(|grant| PluginCapabilityGrant {
                capability: grant.capability,
                granted: true,
            })
            .collect::<Vec<_>>();
        let decision = service
            .decide_special_permission(PluginSpecialPermissionDecisionRequest {
                meta: request_meta(),
                approval_id: snapshot.approval_id.clone(),
                decision: PluginApprovalDecision::Approve,
                expected_approval_state_version: snapshot.approval_state_version,
                special_grants: special_grants.clone(),
                host_selections: vec![PluginHostScopeSelection {
                    host_id: host.host_id.clone(),
                    capabilities: vec![
                        PluginCapability::HostMetadataRead,
                        PluginCapability::HostMutationPropose,
                    ],
                }],
            })
            .await
            .expect("protected approval");
        let PluginSpecialPermissionOutcome::PreparedPackage {
            preview: approved_preview,
        } = decision.target
        else {
            panic!("prepared approval must return its preview");
        };
        assert_eq!(approved_preview.preparation_id, preview.preparation_id);
        assert!(
            approved_preview
                .special_permission_expires_at_unix_ms
                .is_some()
        );
        assert!(
            approved_preview
                .approved_special_grants
                .iter()
                .all(|grant| grant.granted)
        );

        service
            .install_local_plugin(local_install_request_from_preview(
                &preview,
                vec![
                    PluginCapabilityGrant {
                        capability: PluginCapability::UiPanel,
                        granted: true,
                    },
                    PluginCapabilityGrant {
                        capability: PluginCapability::HostMetadataRead,
                        granted: true,
                    },
                    PluginCapabilityGrant {
                        capability: PluginCapability::HostMutationPropose,
                        granted: true,
                    },
                ],
            ))
            .expect("install approved package");

        service
            .hosts
            .with_plugin_repository(|repository| {
                let installed = repository
                    .get_plugin_installation(&preview.plugin_id)
                    .expect("installed plugin");
                let grants = repository
                    .list_plugin_capability_grants(
                        &preview.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        u64::from(PLUGIN_PROTOCOL_MAJOR),
                    )
                    .expect("persisted grants");
                assert!(grants.iter().all(|grant| grant.granted));
                let scopes = repository
                    .list_plugin_host_scope_grants(
                        &preview.plugin_id,
                        &installed.signer_fingerprint_sha256,
                        u64::from(PLUGIN_PROTOCOL_MAJOR),
                    )
                    .expect("persisted scopes");
                assert_eq!(scopes.len(), 2);
                assert!(scopes.iter().all(|scope| scope.host_id == host.host_id));
                assert!(
                    scopes
                        .iter()
                        .any(|scope| { scope.capability == PluginCapability::HostMetadataRead })
                );
                assert!(
                    scopes
                        .iter()
                        .any(|scope| { scope.capability == PluginCapability::HostMutationPropose })
                );
                Ok(())
            })
            .expect("read atomic permission state");
    }

    #[test]
    fn cancelling_special_permission_returns_a_projection_for_installed_and_prepared_targets() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let plugin_id = PluginId::parse("com.norishell.cancelled-special").expect("plugin id");
        let installed = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Cancellation fixture".to_owned(),
            publisher: "NoriShell tests".to_owned(),
            signer_fingerprint_sha256: "1".repeat(64),
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::UiPanel, PluginCapability::SshSync],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let binding = current_plugin_permission_binding(&installed.package_sha256);
        let initial_grants = [
            (PluginCapability::UiPanel, true),
            (PluginCapability::SshSync, false),
        ];
        service
            .hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    None,
                    &installed,
                    8,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    Some(PluginActivationPermissions {
                        protocol_major: u64::from(PLUGIN_PROTOCOL_MAJOR),
                        binding: &binding,
                        previous_binding: None,
                        expected_previous_grant_state_version: None,
                        expected_previous_scope_state_version: None,
                        grants: &initial_grants,
                        carried_host_scope_capabilities: &[],
                        approved_host_scopes: None,
                    }),
                )
            })
            .expect("install fixture record");
        let installed_snapshot = service
            .prepare_special_permission(PluginSpecialPermissionOpenRequest {
                meta: request_meta(),
                target: PluginSpecialPermissionTarget::Installed {
                    plugin_id: plugin_id.clone(),
                    expected_plugin_state_version: installed.state_version,
                },
                requested_capability: Some(PluginCapability::SshSync),
            })
            .expect("open installed approval");
        let installed_outcome = service
            .cancel_special_permission(&installed_snapshot.approval_id)
            .expect("installed cancellation outcome");
        assert!(matches!(
            installed_outcome,
            PluginSpecialPermissionOutcome::Installed { ref plugin }
                if plugin.plugin_id == plugin_id && plugin.state == PluginInstallState::Disabled
        ));

        let source = write_local_plugin_fixture(
            &directory,
            "cancelled-prepared.zip",
            "com.norishell.cancelled-prepared",
            "1.0.0",
            &[PluginCapability::UiPanel, PluginCapability::SshSync],
        );
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare cancellation package");
        let prepared_snapshot = service
            .prepare_special_permission(prepared_special_permission_request(
                &preview,
                Some(PluginCapability::SshSync),
            ))
            .expect("open prepared approval");
        let prepared_outcome = service
            .cancel_special_permission(&prepared_snapshot.approval_id)
            .expect("prepared cancellation outcome");
        assert!(matches!(
            prepared_outcome,
            PluginSpecialPermissionOutcome::PreparedPackage { preview: ref outcome_preview }
                if outcome_preview.preparation_id == preview.preparation_id
                    && outcome_preview.approved_special_grants.is_empty()
        ));
    }

    #[tokio::test]
    async fn prepared_approval_survives_enabled_to_disabled_update_transition() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let plugin_id = PluginId::parse("com.norishell.enabled-update").expect("plugin id");
        let capabilities = vec![PluginCapability::UiPanel, PluginCapability::SshSync];
        let current_source = write_local_plugin_fixture(
            &directory,
            "enabled-update-current.zip",
            plugin_id.as_str(),
            "1.0.0",
            &capabilities,
        );
        let current_package = inspect_local_package(
            &current_source,
            &current_app_version(),
            std::env::consts::ARCH,
            service.package_limits,
        )
        .expect("inspect current package");
        let current_operation = norishell_core_api::PluginOperationId::new();
        let staged = service
            .installer
            .stage(&current_source, &current_package, &current_operation)
            .expect("stage current package");
        service
            .installer
            .activate(staged, None, &current_operation)
            .expect("activate current package");
        let source = write_local_plugin_fixture(
            &directory,
            "enabled-update.zip",
            plugin_id.as_str(),
            "1.0.1",
            &capabilities,
        );
        let candidate_signer = hex::encode(Sha256::digest(
            fs::read(&source).expect("read candidate package"),
        ));
        let current = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Enabled update fixture".to_owned(),
            publisher: "NoriShell tests".to_owned(),
            signer_fingerprint_sha256: candidate_signer,
            active_version: "1.0.0".to_owned(),
            package_sha256: hex::encode(current_package.package_sha256),
            capabilities: capabilities.clone(),
            state: PluginInstallState::Enabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let current_binding = current_plugin_permission_binding(&current.package_sha256);
        let initial_grants = [
            (PluginCapability::UiPanel, true),
            (PluginCapability::SshSync, false),
        ];
        service
            .hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    None,
                    &current,
                    8,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    Some(PluginActivationPermissions {
                        protocol_major: u64::from(PLUGIN_PROTOCOL_MAJOR),
                        binding: &current_binding,
                        previous_binding: None,
                        expected_previous_grant_state_version: None,
                        expected_previous_scope_state_version: None,
                        grants: &initial_grants,
                        carried_host_scope_capabilities: &[],
                        approved_host_scopes: None,
                    }),
                )
            })
            .expect("install current fixture");
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare update package");
        let snapshot = service
            .prepare_special_permission(prepared_special_permission_request(
                &preview,
                Some(PluginCapability::SshSync),
            ))
            .expect("open protected approval");
        let disabled = service
            .hosts
            .with_plugin_repository(|repository| {
                repository.set_plugin_install_state(
                    &plugin_id,
                    current.state_version,
                    PluginInstallState::Disabled,
                )
            })
            .expect("disable current plugin before commit");
        assert_eq!(disabled.state_version, WireSequence::new(2));
        service
            .decide_special_permission(PluginSpecialPermissionDecisionRequest {
                meta: request_meta(),
                approval_id: snapshot.approval_id,
                decision: PluginApprovalDecision::Approve,
                expected_approval_state_version: snapshot.approval_state_version,
                special_grants: snapshot
                    .special_grants
                    .iter()
                    .map(|grant| PluginCapabilityGrant {
                        capability: grant.capability,
                        granted: true,
                    })
                    .collect(),
                host_selections: Vec::new(),
            })
            .await
            .expect("approval stays valid after only the disable state transition");
        let mut request = local_install_request_from_preview(
            &preview,
            vec![
                PluginCapabilityGrant {
                    capability: PluginCapability::UiPanel,
                    granted: true,
                },
                PluginCapabilityGrant {
                    capability: PluginCapability::SshSync,
                    granted: true,
                },
            ],
        );
        request.expected_state_version = Some(disabled.state_version);
        service
            .install_local_plugin(request)
            .expect("disabled update can consume the still-current approval");
        service
            .hosts
            .with_plugin_repository(|repository| {
                let installed = repository
                    .get_plugin_installation(&plugin_id)
                    .expect("updated installation");
                assert_eq!(installed.state, PluginInstallState::Disabled);
                assert_eq!(installed.state_version, WireSequence::new(3));
                assert_eq!(installed.active_version, "1.0.1");
                let grants = repository
                    .list_plugin_capability_grants(
                        &plugin_id,
                        &installed.signer_fingerprint_sha256,
                        u64::from(PLUGIN_PROTOCOL_MAJOR),
                    )
                    .expect("updated grants");
                assert!(grants.iter().any(|grant| {
                    grant.capability == PluginCapability::SshSync && grant.granted
                }));
                Ok(())
            })
            .expect("verify update facts");
    }

    #[tokio::test]
    async fn prepared_approval_rejects_grant_and_scope_baseline_drift() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let plugin_id = PluginId::parse("com.norishell.drifted-approval").expect("plugin id");
        let source = write_local_plugin_fixture(
            &directory,
            "drifted-approval.zip",
            plugin_id.as_str(),
            "1.0.1",
            &[PluginCapability::UiPanel, PluginCapability::SshSync],
        );
        let candidate_signer = hex::encode(Sha256::digest(
            fs::read(&source).expect("read candidate package"),
        ));
        let current = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Drift fixture".to_owned(),
            publisher: "NoriShell tests".to_owned(),
            signer_fingerprint_sha256: candidate_signer,
            active_version: "1.0.0".to_owned(),
            package_sha256: "a".repeat(64),
            capabilities: vec![PluginCapability::UiPanel, PluginCapability::SshSync],
            state: PluginInstallState::Disabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let binding = current_plugin_permission_binding(&current.package_sha256);
        let initial_grants = [
            (PluginCapability::UiPanel, true),
            (PluginCapability::SshSync, false),
        ];
        service
            .hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    None,
                    &current,
                    8,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    Some(PluginActivationPermissions {
                        protocol_major: u64::from(PLUGIN_PROTOCOL_MAJOR),
                        binding: &binding,
                        previous_binding: None,
                        expected_previous_grant_state_version: None,
                        expected_previous_scope_state_version: None,
                        grants: &initial_grants,
                        carried_host_scope_capabilities: &[],
                        approved_host_scopes: None,
                    }),
                )
            })
            .expect("install current fixture");
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare update package");
        let snapshot = service
            .prepare_special_permission(prepared_special_permission_request(
                &preview,
                Some(PluginCapability::SshSync),
            ))
            .expect("open protected approval");
        service
            .hosts
            .with_plugin_repository(|repository| {
                repository.replace_plugin_special_permissions(
                    &plugin_id,
                    &current.signer_fingerprint_sha256,
                    u64::from(PLUGIN_PROTOCOL_MAJOR),
                    current.state_version,
                    Some(WireSequence::new(1)),
                    None,
                    &binding,
                    &[
                        (PluginCapability::UiPanel, true),
                        (PluginCapability::SshSync, true),
                    ],
                    &[],
                )
            })
            .expect("simulate a competing permission and scope update");
        let error = service
            .decide_special_permission(PluginSpecialPermissionDecisionRequest {
                meta: request_meta(),
                approval_id: snapshot.approval_id,
                decision: PluginApprovalDecision::Approve,
                expected_approval_state_version: snapshot.approval_state_version,
                special_grants: snapshot
                    .special_grants
                    .iter()
                    .map(|grant| PluginCapabilityGrant {
                        capability: grant.capability,
                        granted: true,
                    })
                    .collect(),
                host_selections: Vec::new(),
            })
            .await
            .expect_err("stale permission baseline must fail closed");
        assert_eq!(error.code, "plugin.conflict");
    }

    #[tokio::test]
    async fn expired_prepared_approval_rejects_even_a_fully_denied_install_and_cleans_up() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let source = write_local_plugin_fixture(
            &directory,
            "expired-approved.zip",
            "com.norishell.expired-approved",
            "1.0.0",
            &[PluginCapability::UiPanel, PluginCapability::SshSync],
        );
        let preview = service
            .prepare_local_package(&source, RequestId::new())
            .expect("prepare special package");
        let private_path = service
            .runtime
            .lock()
            .expect("runtime")
            .prepared_local_packages
            .get(&preview.preparation_id)
            .expect("prepared package")
            .path
            .clone();
        let snapshot = service
            .prepare_special_permission(prepared_special_permission_request(
                &preview,
                Some(PluginCapability::SshSync),
            ))
            .expect("open protected approval");
        service
            .decide_special_permission(PluginSpecialPermissionDecisionRequest {
                meta: request_meta(),
                approval_id: snapshot.approval_id,
                decision: PluginApprovalDecision::Approve,
                expected_approval_state_version: snapshot.approval_state_version,
                special_grants: snapshot
                    .special_grants
                    .iter()
                    .map(|grant| PluginCapabilityGrant {
                        capability: grant.capability,
                        granted: true,
                    })
                    .collect(),
                host_selections: Vec::new(),
            })
            .await
            .expect("protected approval");
        service
            .runtime
            .lock()
            .expect("runtime")
            .prepared_local_packages
            .get_mut(&preview.preparation_id)
            .expect("prepared package")
            .approved_special_permissions
            .as_mut()
            .expect("approved permission")
            .expire_for_test();
        let expired_projection = service
            .runtime
            .lock()
            .expect("runtime")
            .prepared_local_packages
            .get(&preview.preparation_id)
            .expect("prepared package")
            .permission_preview();
        assert!(expired_projection.approved_special_grants.is_empty());
        assert_eq!(
            expired_projection.special_permission_expires_at_unix_ms,
            Some(0)
        );

        let error = service
            .install_local_plugin(local_install_request_from_preview(
                &preview,
                vec![
                    PluginCapabilityGrant {
                        capability: PluginCapability::UiPanel,
                        granted: true,
                    },
                    PluginCapabilityGrant {
                        capability: PluginCapability::SshSync,
                        granted: false,
                    },
                ],
            ))
            .expect_err("expired approval must reject a fresh install attempt");
        assert_eq!(error.code, "plugin.conflict");
        assert!(!private_path.exists());
        let runtime = service.runtime.lock().expect("runtime");
        assert!(
            !runtime
                .prepared_local_packages
                .contains_key(&preview.preparation_id)
        );
        assert!(runtime.pending_special_permissions.is_empty());
    }

    #[test]
    fn local_install_fingerprint_binds_grants_and_normalizes_their_order() {
        let plugin_id = PluginId::parse("com.norishell.fixture").expect("plugin id");
        let denied = vec![
            PluginCapabilityGrant {
                capability: PluginCapability::TerminalObserve,
                granted: false,
            },
            PluginCapabilityGrant {
                capability: PluginCapability::UiPanel,
                granted: false,
            },
        ];
        let mut reversed = denied.clone();
        reversed.reverse();
        let granted = vec![
            PluginCapabilityGrant {
                capability: PluginCapability::TerminalObserve,
                granted: true,
            },
            PluginCapabilityGrant {
                capability: PluginCapability::UiPanel,
                granted: false,
            },
        ];
        let fingerprint = |grants: &[PluginCapabilityGrant]| {
            local_install_request_fingerprint(
                &plugin_id,
                "1.0.0",
                &"a".repeat(64),
                None,
                grants,
                RequestId::new(),
            )
            .expect("fingerprint")
        };

        assert_eq!(fingerprint(&denied), fingerprint(&reversed));
        assert_ne!(fingerprint(&denied), fingerprint(&granted));
    }

    #[tokio::test]
    async fn local_plugin_surface_does_not_require_catalog_roots() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service =
            PluginService::start(directory.path(), hosts, sessions).expect("plugin service");
        let readiness = service.readiness();
        assert!(readiness.ready);
        service
            .require_ready(RequestId::new())
            .expect("local plugin operations are available without catalog roots");
        assert!(service.exit_blockers().is_empty());
        service.shutdown_all().await.expect("empty cleanup");
    }

    #[tokio::test]
    async fn plugin_mutation_reservation_is_atomic_per_plugin_and_operation_owned() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service =
            PluginService::start(directory.path(), hosts, sessions).expect("plugin service");
        let plugin_id = PluginId::parse("com.norishell.fixture").expect("plugin id");
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();

        let reservation = service
            .reserve_plugin_mutation(&plugin_id, first)
            .expect("first mutation reservation");
        assert!(
            service
                .reserve_plugin_mutation(&plugin_id, second)
                .is_none()
        );
        drop(reservation);
        assert!(
            service
                .reserve_plugin_mutation(&plugin_id, second)
                .is_some()
        );
    }

    #[test]
    fn contextual_documents_and_state_do_not_leak_into_another_terminal_pane() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).unwrap();
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).unwrap();
        let plugin_id = PluginId::parse("test.ops.context").unwrap();
        let record = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Context fixture".to_owned(),
            publisher: "Tests".to_owned(),
            signer_fingerprint_sha256: "a".repeat(64),
            package_sha256: "b".repeat(64),
            active_version: "1.0.0".to_owned(),
            capabilities: vec![PluginCapability::UiPanel, PluginCapability::SshSync],
            state: PluginInstallState::Enabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let binding = current_plugin_permission_binding(&record.package_sha256);
        service
            .hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    None,
                    &record,
                    1,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    Some(PluginActivationPermissions {
                        protocol_major: u64::from(PLUGIN_PROTOCOL_MAJOR),
                        binding: &binding,
                        previous_binding: None,
                        expected_previous_grant_state_version: None,
                        expected_previous_scope_state_version: None,
                        grants: &[
                            (PluginCapability::UiPanel, true),
                            (PluginCapability::SshSync, true),
                        ],
                        carried_host_scope_capabilities: &[],
                        approved_host_scopes: None,
                    }),
                )
            })
            .unwrap();
        let target_id =
            norishell_core_api::PluginExtensionTargetId::parse("terminal.tools").unwrap();
        let open = |pane: &str| {
            service
                .open_target_context(PluginTargetContextOpenRequest {
                    meta: request_meta(),
                    target_id: target_id.clone(),
                    target_instance_key: pane.to_owned(),
                    display_label: None,
                })
                .unwrap()
        };
        let first = open("pane-a");
        let second = open("pane-b");
        let template = |label: &str| {
            serde_json::from_value::<PluginUiTemplate>(serde_json::json!({
            "targetId":"terminal.tools", "onOpenActionId":"open", "document": { "schemaVersion":1,"rootNodeId":"root", "nodes": [
                {"kind":"stack","nodeId":"root","direction":"vertical","align":"stretch","gap":8,"children":["refresh"]},
                {"kind":"button","nodeId":"refresh","actionId":"refresh","label":label,"icon":null,"variant":"primary","disabled":false}
            ] }
        })).unwrap()
        };
        let baseline = template("No session results yet");
        let first_result = template("Result from connection A");
        service.runtime.lock().unwrap().active_instances.insert(
            plugin_id.to_string(),
            ActivePluginInstance {
                plugin_name: record.name,
                signer_fingerprint_sha256: record.signer_fingerprint_sha256,
                package_sha256: record.package_sha256,
                instance_generation: WireSequence::new(1),
                state_version: record.state_version,
                previously_crashed: false,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                locale: PluginLocale::parse("en").unwrap(),
                contributions: BTreeMap::new(),
                ui_templates: BTreeMap::from([(target_id.to_string(), baseline.clone())]),
                scoped_templates: BTreeMap::from([(
                    first.context_handle.to_string(),
                    first_result.clone(),
                )]),
                scoped_states: BTreeMap::from([(
                    first.context_handle.to_string(),
                    r#"{"result":"connection A"}"#.to_owned(),
                )]),
                operation_authority_revoked: false,
                api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                navigation: BTreeMap::new(),
                pages: BTreeMap::new(),
                contribution_revision: WireSequence::new(2),
                settings_revision: None,
                contribution_action_in_flight: false,
                ui_state_json: "{}".to_owned(),
                process: Arc::new(Mutex::new(None)),
            },
        );
        let list = |target| {
            service
                .list_ui_contributions(PluginUiContributionListRequest {
                    meta: request_meta(),
                    target,
                })
                .unwrap()
        };
        let first_contribution = list(first.clone()).remove(0);
        assert_eq!(first_contribution.document, first_result.document);
        assert_eq!(
            first_contribution
                .on_open_action_id
                .as_ref()
                .map(|action| action.as_str()),
            Some("open")
        );
        let second_contribution = list(second.clone()).remove(0);
        assert_eq!(second_contribution.document, baseline.document);
        assert_eq!(
            second_contribution
                .on_open_action_id
                .as_ref()
                .map(|action| action.as_str()),
            Some("open")
        );
        let action = PluginUiActionRequest {
            background: Some(false),
            meta: request_meta(),
            plugin_id: plugin_id.clone(),
            signer_fingerprint_sha256: "a".repeat(64),
            expected_package_sha256: "b".repeat(64),
            instance_generation: WireSequence::new(1),
            expected_state_version: WireSequence::new(1),
            expected_contribution_revision: WireSequence::new(2),
            target_id: first.target_id.clone(),
            context_handle: first.context_handle.clone(),
            expected_target_revision: first.target_revision,
            action_id: norishell_core_api::PluginUiActionId::parse("open").unwrap(),
            fields: Vec::new(),
            host_dom_snapshot: None,
        };
        service
            .runtime
            .lock()
            .unwrap()
            .active_instances
            .get_mut(plugin_id.as_str())
            .unwrap()
            .contribution_action_in_flight = true;
        assert!(service.ssh_sync_action_fence_current(&action));
        service
            .close_target_context(PluginTargetContextCloseRequest {
                meta: request_meta(),
                context_handle: first.context_handle.clone(),
                expected_target_revision: first.target_revision,
            })
            .unwrap();
        assert!(
            !service.ssh_sync_action_fence_current(&action),
            "closed context revokes in-flight synchronization authority"
        );
        service
            .runtime
            .lock()
            .unwrap()
            .active_instances
            .get_mut(plugin_id.as_str())
            .unwrap()
            .contribution_action_in_flight = false;
        let second_after_close = list(second).remove(0);
        assert_eq!(second_after_close.document, baseline.document);
        assert_eq!(
            second_after_close
                .on_open_action_id
                .as_ref()
                .map(|action| action.as_str()),
            Some("open")
        );
        let runtime = service.runtime.lock().unwrap();
        let active = runtime.active_instances.get(plugin_id.as_str()).unwrap();
        assert!(active.scoped_states.is_empty());
        assert!(active.scoped_templates.is_empty());
        assert_eq!(
            active.ui_templates["terminal.tools"].document,
            baseline.document
        );
    }

    #[test]
    fn failed_plugin_ui_contribution_does_not_block_healthy_plugin_listing() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let target_id =
            norishell_core_api::PluginExtensionTargetId::parse("terminal.tools").expect("target");
        let target = service
            .open_target_context(PluginTargetContextOpenRequest {
                meta: request_meta(),
                target_id: target_id.clone(),
                target_instance_key: "pane-a".to_owned(),
                display_label: None,
            })
            .expect("target context");
        let install = |record: &PluginInstalledRecord| {
            let binding = current_plugin_permission_binding(&record.package_sha256);
            service
                .hosts
                .with_plugin_repository(|repository| {
                    repository.activate_plugin_installation(
                        None,
                        record,
                        1,
                        &"A".repeat(44),
                        &"B".repeat(88),
                        true,
                        Some(PluginActivationPermissions {
                            protocol_major: u64::from(PLUGIN_PROTOCOL_MAJOR),
                            binding: &binding,
                            previous_binding: None,
                            expected_previous_grant_state_version: None,
                            expected_previous_scope_state_version: None,
                            grants: &[(PluginCapability::UiPanel, true)],
                            carried_host_scope_capabilities: &[],
                            approved_host_scopes: None,
                        }),
                    )
                })
                .expect("persist plugin fixture");
        };
        let record = |plugin_id: &str, name: &str, hash: char| PluginInstalledRecord {
            plugin_id: PluginId::parse(plugin_id).expect("plugin id"),
            name: name.to_owned(),
            publisher: "Tests".to_owned(),
            signer_fingerprint_sha256: hash.to_string().repeat(64),
            package_sha256: hash.to_string().repeat(64),
            active_version: "1.0.0".to_owned(),
            capabilities: vec![PluginCapability::UiPanel],
            state: PluginInstallState::Enabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let healthy = record("com.norishell.healthy", "Healthy", 'a');
        let failed = record("com.norishell.failed", "Failed", 'b');
        install(&healthy);
        install(&failed);
        let template = |label: &str| {
            serde_json::from_value::<PluginUiTemplate>(serde_json::json!({
                "targetId": "terminal.tools",
                "document": {
                    "schemaVersion": 1,
                    "rootNodeId": "root",
                    "nodes": [{
                        "kind": "text",
                        "nodeId": "root",
                        "text": label,
                        "style": "body",
                        "tone": "neutral"
                    }]
                }
            }))
            .expect("template")
        };
        let instance = |record: &PluginInstalledRecord, label: &str| ActivePluginInstance {
            plugin_name: record.name.clone(),
            signer_fingerprint_sha256: record.signer_fingerprint_sha256.clone(),
            package_sha256: record.package_sha256.clone(),
            instance_generation: WireSequence::new(1),
            state_version: record.state_version,
            previously_crashed: false,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            locale: PluginLocale::parse("en").expect("locale"),
            contributions: BTreeMap::new(),
            ui_templates: BTreeMap::from([(target_id.to_string(), template(label))]),
            scoped_templates: BTreeMap::new(),
            scoped_states: BTreeMap::new(),
            operation_authority_revoked: false,
            api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            navigation: BTreeMap::new(),
            pages: BTreeMap::new(),
            contribution_revision: WireSequence::new(1),
            settings_revision: None,
            contribution_action_in_flight: false,
            ui_state_json: "{}".to_owned(),
            process: Arc::new(Mutex::new(None)),
        };
        let healthy_instance = instance(&healthy, "Healthy contribution");
        let failed_instance = instance(&failed, "Failed contribution");
        service
            .runtime
            .lock()
            .expect("runtime")
            .active_instances
            .insert(healthy.plugin_id.to_string(), healthy_instance);
        service
            .runtime
            .lock()
            .expect("runtime")
            .active_instances
            .insert(failed.plugin_id.to_string(), failed_instance.clone());

        service.record_runtime_failure(&failed.plugin_id, WireSequence::new(1));
        let crashed = service
            .hosts
            .with_plugin_repository(|repository| {
                repository.get_plugin_installation(&failed.plugin_id)
            })
            .expect("crashed plugin record");
        assert_eq!(crashed.state, PluginInstallState::Crashed);

        // A live Host process retains this revoked instance while cleanup runs.
        let mut retained_failure = failed_instance;
        retained_failure.operation_authority_revoked = true;
        service
            .runtime
            .lock()
            .expect("runtime")
            .active_instances
            .insert(failed.plugin_id.to_string(), retained_failure);

        let listed = service
            .list_ui_contributions(PluginUiContributionListRequest {
                meta: request_meta(),
                target,
            })
            .expect("a failed plugin must not block healthy contributions");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].plugin_id, healthy.plugin_id);
        assert_eq!(listed[0].plugin_name, healthy.name);
    }

    #[test]
    fn template_action_updates_retain_the_declared_on_open_action() {
        let template = |on_open_action_id: Option<&str>| {
            serde_json::from_value::<PluginUiTemplate>(serde_json::json!({
                "targetId": "terminal.tools",
                "onOpenActionId": on_open_action_id,
                "routePaths": ["/terminal"],
                "icon": "storage",
                "document": {
                    "schemaVersion": 1,
                    "rootNodeId": "root",
                    "nodes": [
                        {
                            "kind": "stack",
                            "nodeId": "root",
                            "direction": "vertical",
                            "align": "stretch",
                            "gap": 8,
                            "children": ["refresh"]
                        },
                        {
                            "kind": "button",
                            "nodeId": "refresh",
                            "actionId": "refresh",
                            "label": "Refresh",
                            "icon": null,
                            "variant": "primary",
                            "disabled": false
                        }
                    ]
                }
            }))
            .expect("template")
        };
        let previous = template(Some("open"));
        let mut replacement = template(None);
        replacement.route_paths = None;
        replacement.icon = None;

        retain_template_presentation(&previous, &mut replacement);

        assert_eq!(replacement.on_open_action_id, previous.on_open_action_id);
        assert_eq!(replacement.route_paths, previous.route_paths);
        assert_eq!(replacement.icon, previous.icon);
        assert_eq!(
            template_on_open_action_id(&replacement)
                .as_ref()
                .map(|action| action.as_str()),
            Some("open")
        );

        let mut explicit = template(Some("open"));
        explicit.on_open_action_id = norishell_core_api::PluginUiActionId::parse("openB").ok();
        retain_template_presentation(&previous, &mut explicit);
        assert_eq!(
            explicit
                .on_open_action_id
                .as_ref()
                .map(|action| action.as_str()),
            Some("openB")
        );
    }

    #[test]
    fn action_response_prefers_scoped_lifecycle_and_rejects_ambiguous_inheritance() {
        let template = |on_open_action_id: Option<&str>, action_node: serde_json::Value| {
            let node_id = action_node["nodeId"]
                .as_str()
                .expect("action node id")
                .to_owned();
            serde_json::from_value::<PluginUiTemplate>(serde_json::json!({
                "targetId": "terminal.tools",
                "onOpenActionId": on_open_action_id,
                "document": {
                    "schemaVersion": 1,
                    "rootNodeId": "root",
                    "nodes": [
                        {
                            "kind": "stack",
                            "nodeId": "root",
                            "direction": "vertical",
                            "align": "stretch",
                            "gap": 8,
                            "children": [node_id]
                        },
                        action_node
                    ]
                }
            }))
            .expect("template")
        };
        let button = |node_id: &str, action_id: &str, disabled: bool| {
            serde_json::json!({
                "kind": "button",
                "nodeId": node_id,
                "actionId": action_id,
                "label": "Open",
                "icon": null,
                "variant": "primary",
                "disabled": disabled
            })
        };
        let baseline = template(Some("openA"), button("refresh", "refresh", false));
        let scoped = template(Some("openB"), button("refresh", "refresh", false));
        let ui_templates = BTreeMap::from([("terminal.tools".to_owned(), baseline.clone())]);
        let scoped_templates = BTreeMap::from([("context-b".to_owned(), scoped.clone())]);

        let selected = template_presentation_for_action(
            &ui_templates,
            &scoped_templates,
            true,
            "terminal.tools",
            "context-b",
        )
        .expect("scoped presentation");
        assert_eq!(selected.on_open_action_id, scoped.on_open_action_id);
        let no_scoped_templates = BTreeMap::new();
        let fallback = template_presentation_for_action(
            &ui_templates,
            &no_scoped_templates,
            true,
            "terminal.tools",
            "context-b",
        )
        .expect("baseline presentation");
        assert_eq!(fallback.on_open_action_id, baseline.on_open_action_id);

        let mut response = template(None, button("refresh", "refresh", false));
        retain_template_presentation(selected, &mut response);
        assert_eq!(
            response
                .on_open_action_id
                .as_ref()
                .map(|action| action.as_str()),
            Some("openB")
        );
        assert!(validate_template_lifecycle(&response).is_ok());

        let ambiguous_nodes = [
            serde_json::json!({
                "kind": "copyButton",
                "nodeId": "openB",
                "actionId": "openB",
                "label": "Open",
                "disabled": false
            }),
            button("openB", "openB", true),
            serde_json::json!({
                "kind": "table",
                "nodeId": "table",
                "label": "Rows",
                "columns": [{"columnId": "name", "label": "Name", "width": null}],
                "rows": [{"rowId": "row", "cells": ["one"], "actionId": "openB"}],
                "emptyText": null
            }),
        ];
        for action_node in ambiguous_nodes {
            let mut candidate = template(None, action_node);
            retain_template_presentation(selected, &mut candidate);
            assert!(
                validate_template_lifecycle(&candidate).is_err(),
                "lifecycle action must remain unclaimed by any user action"
            );
        }
    }

    #[tokio::test]
    async fn revoked_ui_panel_grant_rejects_a_stale_template_lifecycle_fence() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let plugin_id = PluginId::parse("test.ops.revoked-lifecycle").expect("plugin id");
        let record = PluginInstalledRecord {
            plugin_id: plugin_id.clone(),
            name: "Revoked lifecycle fixture".to_owned(),
            publisher: "Tests".to_owned(),
            signer_fingerprint_sha256: "a".repeat(64),
            package_sha256: "b".repeat(64),
            active_version: "1.0.0".to_owned(),
            capabilities: vec![PluginCapability::UiPanel],
            state: PluginInstallState::Enabled,
            state_version: WireSequence::new(1),
            installed_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        };
        let binding = current_plugin_permission_binding(&record.package_sha256);
        service
            .hosts
            .with_plugin_repository(|repository| {
                repository.activate_plugin_installation(
                    None,
                    &record,
                    1,
                    &"A".repeat(44),
                    &"B".repeat(88),
                    true,
                    Some(PluginActivationPermissions {
                        protocol_major: u64::from(PLUGIN_PROTOCOL_MAJOR),
                        binding: &binding,
                        previous_binding: None,
                        expected_previous_grant_state_version: None,
                        expected_previous_scope_state_version: None,
                        grants: &[(PluginCapability::UiPanel, false)],
                        carried_host_scope_capabilities: &[],
                        approved_host_scopes: None,
                    }),
                )
            })
            .expect("persist revoked grant");
        let target = service
            .open_target_context(PluginTargetContextOpenRequest {
                meta: request_meta(),
                target_id: norishell_core_api::PluginExtensionTargetId::parse("terminal.tools")
                    .expect("target id"),
                target_instance_key: "pane-a".to_owned(),
                display_label: None,
            })
            .expect("target context");
        let template = serde_json::from_value::<PluginUiTemplate>(serde_json::json!({
            "targetId": "terminal.tools",
            "onOpenActionId": "open",
            "document": {
                "schemaVersion": 1,
                "rootNodeId": "root",
                "nodes": [{
                    "kind": "text",
                    "nodeId": "root",
                    "text": "Ready",
                    "style": "body",
                    "tone": "neutral"
                }]
            }
        }))
        .expect("stale template");
        service
            .runtime
            .lock()
            .expect("runtime")
            .active_instances
            .insert(
                plugin_id.to_string(),
                ActivePluginInstance {
                    plugin_name: record.name.clone(),
                    signer_fingerprint_sha256: record.signer_fingerprint_sha256.clone(),
                    package_sha256: record.package_sha256.clone(),
                    instance_generation: WireSequence::new(1),
                    state_version: record.state_version,
                    previously_crashed: false,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    locale: PluginLocale::parse("en").expect("locale"),
                    contributions: BTreeMap::new(),
                    ui_templates: BTreeMap::from([("terminal.tools".to_owned(), template)]),
                    scoped_templates: BTreeMap::new(),
                    scoped_states: BTreeMap::new(),
                    operation_authority_revoked: false,
                    api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    navigation: BTreeMap::new(),
                    pages: BTreeMap::new(),
                    contribution_revision: WireSequence::new(1),
                    settings_revision: None,
                    contribution_action_in_flight: false,
                    ui_state_json: "{}".to_owned(),
                    process: Arc::new(Mutex::new(None)),
                },
            );

        let error = service
            .invoke_ui_action(PluginUiActionRequest {
                background: Some(false),
                meta: request_meta(),
                plugin_id: plugin_id.clone(),
                signer_fingerprint_sha256: record.signer_fingerprint_sha256.clone(),
                expected_package_sha256: record.package_sha256.clone(),
                instance_generation: WireSequence::new(1),
                expected_state_version: record.state_version,
                expected_contribution_revision: WireSequence::new(1),
                target_id: target.target_id,
                context_handle: target.context_handle,
                expected_target_revision: target.target_revision,
                action_id: norishell_core_api::PluginUiActionId::parse("open")
                    .expect("lifecycle action"),
                fields: Vec::new(),
                host_dom_snapshot: None,
            })
            .await
            .expect_err("revoked UiPanel grant must fence a stale lifecycle contribution");
        assert_eq!(error.code, "plugin.capability_denied");
        assert!(
            !service
                .runtime
                .lock()
                .expect("runtime")
                .active_instances
                .get(plugin_id.as_str())
                .expect("active instance")
                .contribution_action_in_flight
        );
    }

    #[test]
    fn contribution_commit_rejects_a_runtime_failure_that_cleared_the_source_action() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        let plugin_id = PluginId::parse("com.norishell.utility-demo").expect("plugin id");
        let request = PluginContributionInvokeRequest {
            meta: request_meta(),
            plugin_id: plugin_id.clone(),
            signer_fingerprint_sha256: "a".repeat(64),
            expected_package_sha256: "b".repeat(64),
            instance_generation: WireSequence::new(4),
            expected_state_version: WireSequence::new(7),
            expected_contribution_revision: WireSequence::new(2),
            slot: PluginContributionSlot::TerminalToolbar,
            action_id: "generateUuid".to_owned(),
        };
        service
            .runtime
            .lock()
            .expect("runtime")
            .active_instances
            .insert(
                plugin_id.as_str().to_owned(),
                ActivePluginInstance {
                    plugin_name: "Utility Demo".to_owned(),
                    signer_fingerprint_sha256: request.signer_fingerprint_sha256.clone(),
                    package_sha256: request.expected_package_sha256.clone(),
                    instance_generation: request.instance_generation,
                    state_version: request.expected_state_version,
                    previously_crashed: false,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    locale: norishell_core_api::PluginLocale::parse("zh-CN")
                        .expect("fixture locale"),
                    contributions: BTreeMap::new(),
                    ui_templates: BTreeMap::new(),
                    navigation: BTreeMap::new(),
                    pages: BTreeMap::new(),
                    contribution_revision: request.expected_contribution_revision,
                    settings_revision: None,
                    contribution_action_in_flight: true,
                    ui_state_json: "{}".to_owned(),
                    scoped_templates: BTreeMap::new(),
                    scoped_states: BTreeMap::new(),
                    operation_authority_revoked: false,
                    api_authority: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    process: Arc::new(Mutex::new(None)),
                },
            );

        let error = service
            .commit_contribution_action(&request, Vec::new())
            .expect_err("cleared source action must reject a late commit");
        assert_eq!(error.code, "plugin.conflict");
    }

    #[test]
    fn observer_capacity_counts_inflight_reservations() {
        let plugin_id = PluginId::parse("com.norishell.observer").expect("plugin id");
        let session_id = norishell_core_api::SshSessionId::new();
        let mut runtime = PluginRuntimeState::default();
        for index in 0..PLUGIN_OBSERVER_PER_SESSION_LIMIT {
            runtime.observer_attach_reservations.insert(
                format!("reservation-{index}"),
                PluginObserverAttachReservation {
                    fingerprint: format!("fingerprint-{index}"),
                    idempotency_key: format!("idempotency-{index}"),
                    plugin_id: plugin_id.clone(),
                    session_id: session_id.clone(),
                },
            );
        }
        assert!(!observer_capacity_available(
            &runtime,
            &plugin_id,
            &session_id
        ));
        assert!(observer_capacity_available(
            &runtime,
            &PluginId::parse("com.norishell.other").expect("other plugin"),
            &norishell_core_api::SshSessionId::new(),
        ));
    }

    #[test]
    fn safe_mode_marker_survives_until_startup_completes_and_can_be_scheduled_again() {
        let directory = tempfile::tempdir().expect("plugin service directory");
        let marker = directory.path().join("plugin-safe-mode-next-start");
        fs::write(&marker, b"safe-mode\n").expect("safe-mode marker");
        let hosts = HostService::start(directory.path()).expect("host repository");
        let sessions = SshSessionService::start(
            hosts.clone(),
            VaultService::start(directory.path()),
            TransientCredentialService::default(),
        );
        let service = PluginService::start(directory.path(), hosts, sessions).expect("service");
        assert!(service.readiness().safe_mode_active);
        assert!(!marker.exists());
        let active_marker = directory.path().join("plugin-safe-mode-active-startup");
        assert!(active_marker.is_file());
        assert!(
            service
                .enabled_plugins_for_startup()
                .expect("safe mode candidates")
                .is_empty()
        );
        service
            .complete_safe_mode_startup()
            .expect("complete safe-mode startup");
        assert!(!active_marker.exists());
        let readiness = service
            .set_safe_mode_next_start(true, RequestId::new())
            .expect("schedule safe mode");
        assert!(readiness.safe_mode_next_start);
        assert!(marker.is_file());
        let readiness = service
            .set_safe_mode_next_start(false, RequestId::new())
            .expect("cancel safe mode");
        assert!(!readiness.safe_mode_next_start);
        assert!(!marker.exists());
    }
}
