//! Minimal Rust guest support for NoriShell protocol-13 WebAssembly plugins.
//!
//! The SDK deliberately owns only ABI framing and typed protocol helpers. It
//! has no executor, transport, filesystem access, or host capability logic.
//! The Core remains the authority for those operations.

use std::{error::Error, fmt};

pub use norishell_core_api::{
    PluginAppCommand, PluginAppNavigation, PluginAppNotification, PluginAppRegistration,
    PluginAppStatus, PluginExtensionTargetId, PluginUiActionId, PluginWorkflowCatalog,
    PluginWorkflowDefinition, PluginWorkflowEvent, PluginWorkflowStepDefinition,
    PluginWorkflowStepState, PluginWorkflowTaskId, PluginWorkflowTaskResult,
    PluginWorkflowTaskSnapshot, PluginWorkflowTaskState, PluginWorkflowTaskStepSummary,
    PluginWorkflowTaskSummary, WorkflowEvent, WorkflowResponse,
};
use serde::{Serialize, de::DeserializeOwned};

pub use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginApiAvailability, PluginApiCall,
    PluginApiDescription, PluginApiErrorCode, PluginApiLimits, PluginApiMethod, PluginApiOperation,
    PluginApiOutcome, PluginApiReply, PluginApiResourceEvent, PluginApiResourceEventKind,
    PluginApiResourceState, PluginApiResourceSummary, PluginApiValue, PluginCapability,
    PluginCapabilityGrant, PluginHostMessageKind, PluginHostRequest, PluginOperationPermission,
    PluginRuntimeOutput, WireSequence,
};

/// Non-authorizing guest DTOs, grouped by the Core broker capability that owns
/// their execution. These types deliberately stop at the guest wire: Core-only
/// frozen plans, scopes, leases, approval decisions and secret inputs remain
/// unavailable from this SDK.
pub use norishell_core_api::{
    // Plugin credentials
    PluginCredentialInjection,
    PluginCredentialOperation,
    PluginCredentialResult,
    PluginCredentialState,
    PluginCredentialSummary,
    PluginCredentialTarget,
    // Local files
    PluginFileAccessRequest,
    PluginFileEntry,
    PluginFileEntryKind,
    PluginFileOperation,
    PluginFilePickerKind,
    PluginFileResult,
    PluginFileWatchChange,
    // Network
    PluginHttpMethod,
    PluginNetworkCredentialRef,
    PluginNetworkEndpointRequest,
    PluginNetworkErrorCode,
    PluginNetworkEvent,
    PluginNetworkHeader,
    PluginNetworkOperation,
    PluginNetworkProtocol,
    PluginNetworkSendRequest,
    PluginNetworkStartRequest,
    // Independently connected remote exec and local process resources
    PluginProcessOutputStream,
    PluginProcessSendRequest,
    // Protocol-provider package declarations and event/response framing
    PluginProtocolCatalog,
    PluginProtocolCloseReason,
    PluginProtocolEvent,
    PluginProtocolEventKind,
    PluginProtocolFeatures,
    PluginProtocolOpen,
    PluginProtocolOutput,
    PluginProtocolProvider,
    PluginProtocolResizeSupport,
    PluginProtocolResource,
    PluginProtocolResponse,
    PluginRemoteExecEvent,
    PluginRemoteExecSendRequest,
    PluginRemoteExecStartRequest,
    // Serial-device resources
    PluginSerialDataBits,
    PluginSerialDeviceCandidate,
    PluginSerialEvent,
    PluginSerialFlowControl,
    PluginSerialParity,
    PluginSerialPortKind,
    PluginSerialPortMetadata,
    PluginSerialSendRequest,
    PluginSerialSettings,
    PluginSerialStopBits,
    PluginSettingField,
    PluginSettingLabel,
    PluginSettingSelectOption,
    PluginSettingValue,
    PluginSettingsSchema,
    PluginSettingsTargetVisibility,
    PluginSettingsValues,
    // SFTP
    PluginSftpDirectoryEntry,
    PluginSftpEntryKind,
    PluginSftpEvent,
    PluginSftpObjectPrecondition,
    PluginSftpOperation,
    PluginSftpResult,
    // Plugin-private storage
    PluginStorageBlobRead,
    PluginStorageEntry,
    PluginStorageMutation,
    PluginStorageOperation,
    PluginStorageResult,
    PluginStorageState,
    // Timers and metadata subscriptions
    PluginSubscriptionEvent,
    PluginSubscriptionTopic,
};

/// The runtime's per-request ABI bound. A guest checks this before parsing an
/// untrusted request buffer; Core separately applies its full quota policy.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// Errors cross the guest ABI only as a non-zero `nvx_handle` status. Details
/// stay inside the guest and must not be formatted into a host output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginError {
    InvalidRequest,
    InvalidOutput,
    HandlerFailed,
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "invalid plugin host request",
            Self::InvalidOutput => "invalid plugin runtime output",
            Self::HandlerFailed => "plugin handler failed",
        })
    }
}

impl Error for PluginError {}

/// Stateful guest implementation retained for the lifetime of one Wasm
/// instance. Core serializes calls to that instance, so the SDK needs no
/// asynchronous runtime or concurrent guest dispatcher.
pub trait Plugin: Default + 'static {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError>;
}

/// Parses the string-encoded payload of a trusted host request.
pub fn payload<T: DeserializeOwned>(request: &PluginHostRequest) -> Result<T, PluginError> {
    serde_json::from_str(&request.payload_json).map_err(|_| PluginError::InvalidRequest)
}

pub fn workflow_event(request: &PluginHostRequest) -> Result<PluginWorkflowEvent, PluginError> {
    if request.kind != PluginHostMessageKind::WorkflowEvent {
        return Err(PluginError::InvalidRequest);
    }
    payload(request)
}

pub fn workflow_response(
    request: &PluginHostRequest,
    response: &WorkflowResponse,
) -> Result<PluginRuntimeOutput, PluginError> {
    if request.kind != PluginHostMessageKind::WorkflowEvent
        || (response.complete && response.call.is_some())
        || response.call.is_some() != response.step_id.is_some()
    {
        return Err(PluginError::InvalidOutput);
    }
    output(&request.request_id, "workflow.response", response)
}

/// Encodes a runtime output without hand-built JSON escaping.
pub fn output<T: Serialize>(
    request_id: &str,
    kind: impl Into<String>,
    value: &T,
) -> Result<PluginRuntimeOutput, PluginError> {
    let kind = kind.into();
    if request_id.is_empty() || kind.is_empty() {
        return Err(PluginError::InvalidOutput);
    }
    let payload_json = serde_json::to_string(value).map_err(|_| PluginError::InvalidOutput)?;
    Ok(PluginRuntimeOutput {
        request_id: request_id.to_owned(),
        kind,
        payload_json,
    })
}

/// Constructs the protocol-13 broker request for one typed API operation.
pub fn api_request(
    request_id: &str,
    call_id: impl Into<String>,
    operation: PluginApiOperation,
) -> Result<PluginRuntimeOutput, PluginError> {
    output(
        request_id,
        "api.request",
        &PluginApiCall {
            call_id: call_id.into(),
            operation,
        },
    )
}

/// Parses a provider event without accepting a UI action as protocol authority.
pub fn protocol_event(request: &PluginHostRequest) -> Result<PluginProtocolEvent, PluginError> {
    if request.kind != PluginHostMessageKind::ProtocolEvent {
        return Err(PluginError::InvalidRequest);
    }
    norishell_core_api::parse_plugin_protocol_event(request.payload_json.as_bytes())
        .map_err(|_| PluginError::InvalidRequest)
}

/// Completes one provider event or asks for one broker continuation. Correlation comes
/// from the received event, so protocol code never invents a Core session identity.
pub fn protocol_response(
    request: &PluginHostRequest,
    event: &PluginProtocolEvent,
    outputs: Vec<PluginProtocolOutput>,
    call: Option<PluginApiCall>,
) -> Result<PluginRuntimeOutput, PluginError> {
    if request.kind != PluginHostMessageKind::ProtocolEvent {
        return Err(PluginError::InvalidRequest);
    }
    let response = PluginProtocolResponse {
        connection_handle: event.connection_handle.clone(),
        event_id: event.event_id.clone(),
        outputs,
        complete: call.is_none(),
        call,
    };
    norishell_core_api::validate_plugin_protocol_response(&response)
        .map_err(|_| PluginError::InvalidOutput)?;
    output(&request.request_id, "protocol.response", &response)
}

/// Copies an exact host-sized request buffer into guest-owned boxed memory.
/// The corresponding [`deallocate`] call consumes that allocation exactly
/// once after `nvx_handle` returns.
pub fn allocate(length: i32) -> i32 {
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    if length > MAX_REQUEST_BYTES {
        return 0;
    }
    let allocation = vec![0_u8; length].into_boxed_slice();
    let pointer = Box::into_raw(allocation).cast::<u8>() as usize;
    i32::try_from(pointer).unwrap_or(0)
}

/// Releases a buffer allocated by [`allocate`]. Invalid host ABI input is
/// rejected by the runtime before this export is called.
///
/// # Safety
///
/// `pointer` and `length` must be an allocation returned by [`allocate`] that
/// has not already been released.
pub unsafe fn deallocate(pointer: i32, length: i32) {
    let (Ok(pointer), Ok(length)) = (usize::try_from(pointer), usize::try_from(length)) else {
        return;
    };
    if length > MAX_REQUEST_BYTES {
        return;
    }
    let pointer = pointer as *mut u8;
    if pointer.is_null() {
        return;
    }
    // SAFETY: the ABI export only calls this for a Box<[u8]> returned by
    // allocate with the same length.
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            pointer, length,
        )));
    }
}

/// Parses, handles, validates and emits one request. `export_plugin!` keeps
/// its mutable plugin value local to one Wasm instance; Core serializes calls
/// through `WasmRuntime`.
pub fn dispatch<P: Plugin>(
    plugin: &mut P,
    pointer: i32,
    length: i32,
    mut emit: impl FnMut(*const u8, i32) -> i32,
) -> i32 {
    let (Ok(pointer), Ok(length)) = (usize::try_from(pointer), usize::try_from(length)) else {
        return 1;
    };
    if pointer == 0 || length == 0 || length > MAX_REQUEST_BYTES {
        return 1;
    }
    // SAFETY: Core wrote exactly `length` bytes into the allocation returned
    // by nvx_alloc and calls nvx_dealloc after this function returns.
    let request = unsafe { std::slice::from_raw_parts(pointer as *const u8, length) };
    let request = match serde_json::from_slice::<PluginHostRequest>(request) {
        Ok(request) if valid_request(&request) => request,
        _ => return 1,
    };

    let outputs = match plugin.handle(request.clone()) {
        Ok(outputs) => outputs,
        Err(_) => return 1,
    };
    if outputs.iter().any(|output| {
        output.request_id != request.request_id
            || output.kind.is_empty()
            || output.payload_json.is_empty()
    }) {
        return 1;
    }
    for output in outputs {
        let Ok(output) = serde_json::to_vec(&output) else {
            return 1;
        };
        let Ok(length) = i32::try_from(output.len()) else {
            return 1;
        };
        if emit(output.as_ptr(), length) != 0 {
            return 1;
        }
    }
    0
}

fn valid_request(request: &PluginHostRequest) -> bool {
    norishell_core_api::plugin_protocol_is_compatible(
        request.protocol_major,
        request.protocol_minor,
    ) && request.protocol_minor == PLUGIN_PROTOCOL_MINOR
        && !request.request_id.is_empty()
        && request.request_id.len() <= 120
        && !request.payload_json.is_empty()
}

/// Exports the complete protocol-13 guest ABI for a [`Plugin`] implementation.
/// It intentionally declares the only permitted host import here, so a plugin
/// using this macro has one `norishell.host.emit` import and no WASI surface.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        std::thread_local! {
            static NVX_PLUGIN: std::cell::RefCell<Option<$plugin>> = const {
                std::cell::RefCell::new(None)
            };
        }

        #[link(wasm_import_module = "norishell.host")]
        unsafe extern "C" {
            fn emit(pointer: *const u8, length: i32) -> i32;
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn nvx_alloc(length: i32) -> i32 {
            $crate::allocate(length)
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn nvx_dealloc(pointer: i32, length: i32) {
            // SAFETY: Core calls this only for a preceding nvx_alloc result.
            unsafe { $crate::deallocate(pointer, length) };
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn nvx_handle(pointer: i32, length: i32) -> i32 {
            NVX_PLUGIN.with(|state| {
                let mut state = state.borrow_mut();
                let plugin = state.get_or_insert_with(<$plugin>::default);
                $crate::dispatch(plugin, pointer, length, |pointer, length| {
                    // SAFETY: the temporary serialized output remains valid for
                    // this synchronous imported host call.
                    unsafe { emit(pointer, length) }
                })
            })
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_core_api::PluginHostMessageKind;

    #[derive(Default)]
    struct Counter(u8);

    impl Plugin for Counter {
        fn handle(
            &mut self,
            request: PluginHostRequest,
        ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
            self.0 = self.0.saturating_add(1);
            Ok(vec![output(
                &request.request_id,
                "state",
                &serde_json::json!({"count": self.0}),
            )?])
        }
    }

    fn request(id: &str) -> Vec<u8> {
        serde_json::to_vec(&PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            request_id: id.into(),
            kind: PluginHostMessageKind::Initialize,
            payload_json: "{}".into(),
        })
        .expect("request JSON")
    }

    #[test]
    fn workflow_wait_preserves_task_identity_and_rejects_non_workflow_requests() {
        let event = PluginWorkflowEvent {
            task_id: PluginWorkflowTaskId::new(),
            workflow_id: "timer-demo".into(),
            event: WorkflowEvent::Start {
                input_json: None,
                file_scopes: std::collections::BTreeMap::new(),
            },
        };
        let request = PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            request_id: "workflow-event".into(),
            kind: PluginHostMessageKind::WorkflowEvent,
            payload_json: serde_json::to_string(&event).unwrap(),
        };
        assert_eq!(workflow_event(&request).unwrap().task_id, event.task_id);
        let wait = WorkflowResponse {
            step_id: None,
            call: None,
            complete: false,
        };
        let output = workflow_response(&request, &wait).unwrap();
        assert_eq!(output.request_id, request.request_id);
        assert_eq!(output.kind, "workflow.response");
        let decoded: WorkflowResponse = serde_json::from_str(&output.payload_json).unwrap();
        assert!(!decoded.complete && decoded.call.is_none() && decoded.step_id.is_none());
        let other = PluginHostRequest {
            kind: PluginHostMessageKind::UiAction,
            ..request
        };
        assert!(workflow_event(&other).is_err());
        assert!(workflow_response(&other, &wait).is_err());
    }

    #[test]
    fn api_request_uses_the_core_api_call_type() {
        let output = api_request("request-1", "describe", PluginApiOperation::Describe {})
            .expect("typed API request");
        assert_eq!(output.kind, "api.request");
        let call: PluginApiCall = serde_json::from_str(&output.payload_json).expect("core call");
        assert_eq!(call.call_id, "describe");
        assert!(matches!(call.operation, PluginApiOperation::Describe {}));
    }

    #[test]
    fn host_unit_validation_rejects_old_protocol_and_malformed_payload() {
        let valid: PluginHostRequest = serde_json::from_slice(&request("valid")).expect("request");
        assert!(valid_request(&valid));
        let old = PluginHostRequest {
            protocol_minor: PLUGIN_PROTOCOL_MINOR - 1,
            ..valid.clone()
        };
        assert!(!valid_request(&old));
        assert!(serde_json::from_slice::<PluginHostRequest>(br#"{not-json}"#).is_err());
    }

    #[test]
    fn plugin_state_is_owned_by_the_guest_instance() {
        let mut plugin = Counter::default();
        let first_request: PluginHostRequest =
            serde_json::from_slice(&request("first")).expect("request");
        let first = plugin.handle(first_request).expect("first action");
        let second_request: PluginHostRequest =
            serde_json::from_slice(&request("second")).expect("request");
        let second = plugin.handle(second_request).expect("second action");
        assert_eq!(first[0].payload_json, r#"{"count":1}"#);
        assert_eq!(second[0].payload_json, r#"{"count":2}"#);
    }
}
