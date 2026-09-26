//! Compile-only boundary check for the guest-facing protocol-13 SDK DTOs.
//!
//! This intentionally uses only opaque handles and explicit request/response
//! types. It must not need a Core plan, permission lease, scope, or secret.

use std::collections::BTreeMap;

use norishell_plugin_sdk::{
    PluginApiAvailability, PluginApiCall, PluginApiDescription, PluginApiErrorCode,
    PluginApiLimits, PluginApiMethod, PluginApiOperation, PluginApiOutcome, PluginApiReply,
    PluginApiResourceEvent, PluginApiResourceEventKind, PluginApiResourceState,
    PluginApiResourceSummary, PluginApiValue, PluginCapability, PluginCapabilityGrant,
    PluginCredentialInjection, PluginCredentialOperation, PluginCredentialResult,
    PluginCredentialState, PluginCredentialSummary, PluginCredentialTarget,
    PluginFileAccessRequest, PluginFileEntry, PluginFileEntryKind, PluginFileOperation,
    PluginFilePickerKind, PluginFileResult, PluginFileWatchChange, PluginHttpMethod,
    PluginNetworkCredentialRef, PluginNetworkEndpointRequest, PluginNetworkErrorCode,
    PluginNetworkEvent, PluginNetworkHeader, PluginNetworkOperation, PluginNetworkProtocol,
    PluginNetworkSendRequest, PluginNetworkStartRequest, PluginProcessOutputStream,
    PluginProcessSendRequest, PluginProtocolCatalog, PluginProtocolCloseReason,
    PluginProtocolEvent, PluginProtocolEventKind, PluginProtocolFeatures, PluginProtocolOpen,
    PluginProtocolOutput, PluginProtocolProvider, PluginProtocolResizeSupport,
    PluginProtocolResource, PluginProtocolResponse, PluginRemoteExecEvent,
    PluginRemoteExecSendRequest, PluginRemoteExecStartRequest, PluginSerialDataBits,
    PluginSerialDeviceCandidate, PluginSerialEvent, PluginSerialFlowControl, PluginSerialParity,
    PluginSerialPortKind, PluginSerialPortMetadata, PluginSerialSendRequest, PluginSerialSettings,
    PluginSerialStopBits, PluginSettingField, PluginSettingLabel, PluginSettingSelectOption,
    PluginSettingValue, PluginSettingsSchema, PluginSettingsTargetVisibility, PluginSettingsValues,
    PluginSftpDirectoryEntry, PluginSftpEntryKind, PluginSftpEvent, PluginSftpObjectPrecondition,
    PluginSftpOperation, PluginSftpResult, PluginStorageBlobRead, PluginStorageEntry,
    PluginStorageMutation, PluginStorageOperation, PluginStorageResult, PluginStorageState,
    PluginSubscriptionEvent, PluginSubscriptionTopic, WireSequence, api_request,
};

fn main() {
    let sequence = WireSequence::new(1);
    let _network = api_request(
        "network-request",
        "network-call",
        PluginApiOperation::NetworkStart {
            endpoint: PluginNetworkEndpointRequest {
                endpoint: "https://example.test/status".into(),
            },
            request: PluginNetworkStartRequest {
                timeout_ms: 1_000,
                oauth_profile_id: None,
                credential: Some(PluginNetworkCredentialRef {
                    handle: "credential".into(),
                    expected_revision: sequence,
                }),
                operation: PluginNetworkOperation::Http {
                    method: PluginHttpMethod::Get,
                    headers: vec![PluginNetworkHeader {
                        name: "accept".into(),
                        value: "application/json".into(),
                    }],
                    body_base64: String::new(),
                },
            },
        },
    )
    .expect("typed network request");
    let _file = PluginApiOperation::File {
        operation: PluginFileOperation::Write {
            root_handle: "root".into(),
            relative_path: "notes.txt".into(),
            expected_fingerprint: None,
            offset: 0,
            data_base64: "bm90ZQ==".into(),
            final_size: Some(4),
        },
    };
    let _sftp = PluginApiOperation::Sftp {
        operation: PluginSftpOperation::List {
            root_handle: "root".into(),
            directory_handle: None,
            child_entry_handle: None,
            cursor: None,
            limit: 20,
        },
    };
    let _credential = PluginApiOperation::Credential {
        operation: PluginCredentialOperation::Create {
            operation_id: "credential-create".into(),
            idempotency_key: "credential-create-once".into(),
            label: "Example API".into(),
            target: PluginCredentialTarget {
                origin: "https://example.test".into(),
                injection: PluginCredentialInjection::Bearer {},
            },
        },
    };
    let _storage = PluginApiOperation::Storage {
        operation: PluginStorageOperation::KvSet {
            key: "theme".into(),
            value_base64: "ZGFyaw==".into(),
            expected_revision: 0,
        },
    };
    let _timer = PluginApiOperation::TimerStart {
        delay_ms: 1_000,
        interval_ms: Some(5_000),
    };
    let _subscription = PluginApiOperation::SubscriptionStart {
        topics: vec![PluginSubscriptionTopic::PluginSettings {}],
    };
    let _remote_exec = PluginApiOperation::RemoteExecStart {
        request: PluginRemoteExecStartRequest {
            host_handle: "host".into(),
            command: "uname".into(),
            timeout_ms: 1_000,
        },
    };
    let _process = PluginApiOperation::ProcessSend {
        request: PluginProcessSendRequest {
            handle: "process".into(),
            data_base64: String::new(),
            close_stdin: true,
        },
    };
    let _serial = PluginApiOperation::SerialOpen {
        candidate_id: "candidate".into(),
        settings: PluginSerialSettings {
            baud_rate: 115_200,
            data_bits: PluginSerialDataBits::Eight,
            parity: PluginSerialParity::None,
            stop_bits: PluginSerialStopBits::One,
            flow_control: PluginSerialFlowControl::None,
        },
    };
    let _protocol = PluginApiOperation::ProtocolOpen {
        request: PluginProtocolOpen {
            provider_id: "example".into(),
            configuration: BTreeMap::new(),
            label: Some("Example".into()),
        },
    };
    let _reply = PluginApiReply {
        call_id: "resource-events".into(),
        outcome: PluginApiOutcome::Completed {
            value: PluginApiValue::ResourceEvents {
                handle: "timer".into(),
                events: vec![PluginApiResourceEvent {
                    sequence,
                    kind: PluginApiResourceEventKind::TimerFired {},
                }],
                backpressured: false,
            },
        },
    };

    // Every guest-safe public type must remain importable without Core internals.
    exposes::<PluginApiAvailability>();
    exposes::<PluginApiCall>();
    exposes::<PluginApiDescription>();
    exposes::<PluginApiErrorCode>();
    exposes::<PluginApiLimits>();
    exposes::<PluginApiMethod>();
    exposes::<PluginApiResourceState>();
    exposes::<PluginApiResourceSummary>();
    exposes::<PluginCapability>();
    exposes::<PluginCapabilityGrant>();
    exposes::<PluginCredentialResult>();
    exposes::<PluginCredentialState>();
    exposes::<PluginCredentialSummary>();
    exposes::<PluginFileAccessRequest>();
    exposes::<PluginFileEntry>();
    exposes::<PluginFileEntryKind>();
    exposes::<PluginFilePickerKind>();
    exposes::<PluginFileResult>();
    exposes::<PluginFileWatchChange>();
    exposes::<PluginNetworkErrorCode>();
    exposes::<PluginNetworkEvent>();
    exposes::<PluginNetworkProtocol>();
    exposes::<PluginNetworkSendRequest>();
    exposes::<PluginProcessOutputStream>();
    exposes::<PluginProtocolCatalog>();
    exposes::<PluginProtocolCloseReason>();
    exposes::<PluginProtocolEvent>();
    exposes::<PluginProtocolEventKind>();
    exposes::<PluginProtocolFeatures>();
    exposes::<PluginProtocolOutput>();
    exposes::<PluginProtocolProvider>();
    exposes::<PluginProtocolResizeSupport>();
    exposes::<PluginProtocolResource>();
    exposes::<PluginProtocolResponse>();
    exposes::<PluginRemoteExecEvent>();
    exposes::<PluginRemoteExecSendRequest>();
    exposes::<PluginSerialDeviceCandidate>();
    exposes::<PluginSerialEvent>();
    exposes::<PluginSerialPortKind>();
    exposes::<PluginSerialPortMetadata>();
    exposes::<PluginSerialSendRequest>();
    exposes::<PluginSftpDirectoryEntry>();
    exposes::<PluginSftpEntryKind>();
    exposes::<PluginSftpEvent>();
    exposes::<PluginSftpObjectPrecondition>();
    exposes::<PluginSftpResult>();
    exposes::<PluginSettingField>();
    exposes::<PluginSettingLabel>();
    exposes::<PluginSettingSelectOption>();
    exposes::<PluginSettingValue>();
    exposes::<PluginSettingsSchema>();
    exposes::<PluginSettingsTargetVisibility>();
    exposes::<PluginSettingsValues>();
    exposes::<PluginStorageBlobRead>();
    exposes::<PluginStorageEntry>();
    exposes::<PluginStorageMutation>();
    exposes::<PluginStorageResult>();
    exposes::<PluginStorageState>();
    exposes::<PluginSubscriptionEvent>();
}

fn exposes<T>() {}
