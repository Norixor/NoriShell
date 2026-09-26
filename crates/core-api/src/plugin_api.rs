//! Plugin-facing broker messages. Identity and authority never come from this wire payload.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    PluginCapability, PluginFileWatchChange, PluginNetworkEndpointRequest, PluginNetworkEvent,
    PluginNetworkSendRequest, PluginNetworkStartRequest, PluginProcessOutputStream,
    PluginProcessSendRequest, PluginRemoteExecSendRequest, PluginRemoteExecStartRequest,
    WireSequence,
};

/// A guest correlates its calls with this ID; it is not an authorization token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApiCall {
    pub call_id: String,
    pub operation: PluginApiOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginApiOperation {
    DataCatalog {},
    DataRead {
        request: crate::PluginDataReadRequest,
    },
    DataSnapshot {
        request: crate::PluginDataSnapshotRequest,
    },
    DataInspect {
        request: crate::PluginDataInspectRequest,
    },
    DataCompose {
        request: crate::PluginDataComposeRequest,
    },
    DataReview {
        request: crate::PluginDataReviewRequest,
    },
    DataApply {
        request: crate::PluginDataApplyRequest,
    },
    DataExport {
        request: crate::PluginDataExportRequest,
    },
    DataCheckpoint {
        request: crate::PluginDataCheckpointRequest,
    },
    DataRelease {
        request: crate::PluginDataReleaseRequest,
    },
    AppRegister {
        registration: crate::PluginAppRegistration,
    },
    AppNotify {
        notification: crate::PluginAppNotification,
    },
    AppNavigate {
        destination: crate::PluginAppNavigation,
    },
    TaskStart {
        workflow_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        input_json: Option<String>,
        #[serde(default)]
        file_scope_handles: Vec<String>,
    },
    TaskGet {
        task_id: crate::PluginWorkflowTaskId,
    },
    TaskList {},
    TaskCancel {
        task_id: crate::PluginWorkflowTaskId,
        expected_revision: WireSequence,
    },
    TaskResume {
        task_id: crate::PluginWorkflowTaskId,
        expected_revision: WireSequence,
    },
    SerialDevices {},
    SerialOpen {
        candidate_id: String,
        settings: crate::PluginSerialSettings,
    },
    SerialSend {
        request: crate::PluginSerialSendRequest,
    },
    ProtocolOpen {
        request: crate::PluginProtocolOpen,
    },
    Describe {},
    Permissions {},
    PermissionRequest {
        capability: PluginCapability,
    },
    /// Removes one remembered exact-operation approval belonging to this
    /// running plugin. Core supplies the plugin identity and rejects stale
    /// policy revisions.
    PermissionRevoke {
        permission_id: String,
        expected_policy_revision: WireSequence,
    },
    PermissionsForget {},
    ResourcesList {},
    ResourceClose {
        handle: String,
    },
    SubscriptionStart {
        topics: Vec<crate::PluginSubscriptionTopic>,
    },
    TimerStart {
        delay_ms: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        interval_ms: Option<u32>,
    },
    ResourceEvents {
        handle: String,
        limit: u16,
        /// Wait for a newly queued event, bounded by Core. Zero polls immediately.
        #[serde(default)]
        wait_ms: u32,
    },
    /// Core turns this untrusted request into one frozen endpoint and a protected approval before
    /// starting any socket. It never accepts guest-supplied resolved addresses or authority.
    NetworkStart {
        endpoint: PluginNetworkEndpointRequest,
        request: PluginNetworkStartRequest,
    },
    NetworkSend {
        request: PluginNetworkSendRequest,
    },
    /// Core canonicalizes and rechecks this absolute executable before creating a frozen plan.
    /// The plugin never receives authority to invoke a shell or a process by path prefix.
    RemoteExecStart {
        request: PluginRemoteExecStartRequest,
    },
    RemoteExecSend {
        request: PluginRemoteExecSendRequest,
    },
    ProcessStart {
        program: String,
        arguments: Vec<String>,
        timeout_ms: u32,
    },
    ProcessSend {
        request: PluginProcessSendRequest,
    },
    SftpOpen {
        host_handle: String,
        root_path: String,
        write: bool,
    },
    Sftp {
        operation: crate::PluginSftpOperation,
    },
    FilePick {
        picker_kind: crate::PluginFilePickerKind,
        access: crate::PluginFileAccessRequest,
    },
    File {
        operation: crate::PluginFileOperation,
    },
    Credential {
        operation: crate::PluginCredentialOperation,
    },
    Storage {
        operation: crate::PluginStorageOperation,
    },
    TerminalRequestInput {
        terminal_handle: String,
        payload: String,
        append_enter: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApiReply {
    pub call_id: String,
    pub outcome: PluginApiOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginApiOutcome {
    Completed { value: PluginApiValue },
    Failed { code: PluginApiErrorCode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginApiValue {
    DataCatalog {
        catalog: crate::PluginDataCatalog,
    },
    DataRead {
        result: crate::PluginDataReadResult,
    },
    DataSnapshot {
        snapshot_handle: String,
        key_pending: bool,
        local_counts: crate::PluginDataLocalCounts,
        objects: Vec<crate::PluginDataObjectDescriptor>,
    },
    DataInspect {
        inspection_handle: String,
        objects: Vec<crate::PluginDataObjectDescriptor>,
        remote_revision: u64,
        etag: String,
        migration_required: bool,
        excluded_categories: Vec<crate::PluginDataCategory>,
    },
    DataCompose {
        composed_handle: String,
        objects: Vec<crate::PluginDataObjectDescriptor>,
    },
    DataReview {
        composed_handle: String,
        source: crate::PluginDataObjectSource,
        objects: Vec<crate::PluginDataObjectDescriptor>,
    },
    DataApply {
        apply_receipt_handle: String,
    },
    DataExport {
        export_handle: String,
        blob_handle: String,
        revision: u64,
        idempotency_key: String,
        content_type: String,
        objects: Vec<crate::PluginDataObjectDescriptor>,
    },
    DataCheckpoint {
        #[ts(type = "number")]
        synced_at_unix_ms: i64,
    },
    DataRelease {},
    AppAccepted {},
    Task {
        snapshot: crate::PluginWorkflowTaskSnapshot,
    },
    Tasks {
        snapshots: Vec<crate::PluginWorkflowTaskSnapshot>,
    },
    SerialDevices {
        devices: Vec<crate::PluginSerialDeviceCandidate>,
    },
    SerialStarted {
        handle: String,
    },
    SerialSent {
        handle: String,
    },
    ProtocolLaunched {
        launch_id: String,
    },
    Description {
        api: PluginApiDescription,
    },
    Permissions {
        grants: Vec<crate::PluginCapabilityGrant>,
        policy_revision: Option<WireSequence>,
        /// Only this plugin's non-secret operation summaries are returned.
        operation_permissions: Vec<crate::PluginOperationPermission>,
    },
    PermissionRequested {
        approval_id: String,
    },
    PermissionRevoked {
        policy_revision: WireSequence,
    },
    PermissionsForgotten {},
    Resources {
        resources: Vec<PluginApiResourceSummary>,
    },
    Closed {
        handle: String,
    },
    TimerStarted {
        handle: String,
    },
    ResourceEvents {
        handle: String,
        events: Vec<PluginApiResourceEvent>,
        backpressured: bool,
    },
    Sftp {
        result: crate::PluginSftpResult,
    },
    SubscriptionStarted {
        handle: String,
    },
    Credential {
        result: crate::PluginCredentialResult,
    },
    NetworkStarted {
        handle: String,
    },
    /// The owner-scoped driver accepted bytes into its local socket writer. This does not prove
    /// that the remote peer received them.
    NetworkSent {
        handle: String,
    },
    RemoteExecStarted {
        handle: String,
    },
    RemoteExecSent {
        handle: String,
    },
    ProcessStarted {
        handle: String,
    },
    /// Bytes reached the Core-owned process stdin writer; this does not prove program handling.
    ProcessSent {
        handle: String,
    },
    FilePicked {
        root_handle: String,
        label: String,
    },
    File {
        result: crate::PluginFileResult,
    },
    Storage {
        result: crate::PluginStorageResult,
    },
    InputApprovalRequested {
        approval_id: String,
    },
    InputSent {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApiResourceEvent {
    pub sequence: WireSequence,
    pub kind: PluginApiResourceEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PluginApiResourceEventKind {
    Serial {
        event: crate::PluginSerialEvent,
    },
    TimerFired {},
    Cancelled {},
    FileChanged {
        change: PluginFileWatchChange,
    },
    Sftp {
        event: crate::PluginSftpEvent,
    },
    Subscription {
        event: crate::PluginSubscriptionEvent,
    },
    Network {
        event: PluginNetworkEvent,
    },
    ProcessOutput {
        stream: PluginProcessOutputStream,
        data_base64: String,
    },
    ProcessExited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        exit_code: Option<i64>,
    },
    RemoteExec {
        event: crate::PluginRemoteExecEvent,
    },
}

/// Stable errors contain neither upstream text nor request data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginApiErrorCode {
    InvalidRequest,
    PermissionDenied,
    InteractionRequired,
    VaultMissing,
    VaultLocked,
    VaultRequiresReload,
    AuthorizationExpired,
    AccountNotConnected,
    NetworkUnavailable,
    LocalStateChanged,
    OwnerConflict,
    KeyBindingConflict,
    RevisionExhausted,
    RestoreConflict,
    MergeInvalid,
    RemoteRequestRejected,
    RemoteDataInvalid,
    RemoteFormatUnsupported,
    RecoveryAuthenticationFailed,
    RecoveryActionExpired,
    LocalDataInvalid,
    LocalKeyUnavailable,
    Unsupported,
    Revoked,
    NotFound,
    Conflict,
    Busy,
    QuotaExceeded,
    TimedOut,
    Cancelled,
    OutcomeUnknown,
    CleanupIncomplete,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApiDescription {
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub platform: String,
    pub methods: Vec<PluginApiMethod>,
    pub limits: PluginApiLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApiMethod {
    pub name: String,
    pub capability: Option<PluginCapability>,
    pub availability: PluginApiAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginApiAvailability {
    Available,
    NotImplemented,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApiLimits {
    pub max_call_bytes: u32,
    pub max_chunk_bytes: u32,
    pub max_resources: u16,
    pub max_pending_events: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginApiResourceSummary {
    pub handle: String,
    pub generation: WireSequence,
    pub resource_kind: String,
    pub state: PluginApiResourceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PluginApiResourceState {
    Opening,
    Open,
    Closing,
    CleanupIncomplete,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_cannot_supply_an_identity_or_authorization_flag() {
        for input in [
            r#"{"callId":"1","pluginId":"other","operation":{"kind":"describe"}}"#,
            r#"{"callId":"1","operation":{"kind":"describe","approved":true}}"#,
            r#"{"callId":"1","operation":{"kind":"unknown"}}"#,
        ] {
            assert!(serde_json::from_str::<PluginApiCall>(input).is_err());
        }
    }

    #[test]
    fn resource_events_and_timer_requests_are_strictly_typed() {
        let call = serde_json::from_str::<PluginApiCall>(
            r#"{"callId":"timer","operation":{"kind":"timerStart","delayMs":100,"intervalMs":null}}"#,
        )
        .expect("timer call");
        assert!(matches!(
            call.operation,
            PluginApiOperation::TimerStart { .. }
        ));
        assert!(serde_json::from_str::<PluginApiCall>(
            r#"{"callId":"timer","operation":{"kind":"timerStart","delayMs":100,"extra":true}}"#,
        )
        .is_err());
        let value = PluginApiValue::ResourceEvents {
            handle: "019d0000-0000-7000-8000-000000000001".to_owned(),
            events: vec![PluginApiResourceEvent {
                sequence: WireSequence::new(1),
                kind: PluginApiResourceEventKind::TimerFired {},
            }],
            backpressured: false,
        };
        let encoded = serde_json::to_string(&value).expect("event value");
        assert!(encoded.contains("timerFired"));
        assert_eq!(
            serde_json::from_str::<PluginApiValue>(&encoded).expect("round trip"),
            value
        );

        let network = serde_json::json!({
            "callId": "network",
            "operation": {
                "kind": "networkStart",
                "endpoint": {"endpoint": "https://example.test/path"},
                "request": {"timeoutMs": 1000, "operation": {"kind": "http", "method": "get", "headers": [], "bodyBase64": ""}}
            }
        });
        assert!(serde_json::from_value::<PluginApiCall>(network).is_ok());
    }
}
