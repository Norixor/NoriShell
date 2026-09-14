//! Executes the SDK-built API demo through the production persistent runtime.
//! It is an ABI/protocol gate, not a desktop UI or broker acceptance test.

use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginApiAvailability, PluginApiDescription,
    PluginApiLimits, PluginApiMethod, PluginApiOperation, PluginApiOutcome, PluginApiReply,
    PluginApiValue, PluginCapability, PluginHostMessageKind, PluginHostRequest, PluginUiTemplate,
};
use norishell_plugin_platform::{RuntimeLimits, WasmRuntime, validate_plugin_dialog_document};
use serde_json::json;

fn main() {
    let path = std::env::args_os()
        .nth(1)
        .expect("usage: verify_api_demo <api-demo.wasm>");
    let module = std::fs::read(path).expect("read SDK-built Wasm");
    let mut runtime = WasmRuntime::new(&module, RuntimeLimits::default())
        .expect("protocol-13 SDK ABI and sole host import");

    let initial = execute(
        &mut runtime,
        "00000000-0000-4000-8000-000000000101",
        PluginHostMessageKind::Initialize,
        json!({
            "pluginId":"com.norishell.api-demo", "locale":"en", "storage":null
        }),
    );
    assert_eq!(initial.len(), 1);
    let initial: PluginUiTemplate =
        serde_json::from_str(&initial[0].payload_json).expect("initial template");
    assert_eq!(initial.target_id.as_str(), "app.header.actions");
    validate_plugin_dialog_document(&initial.document).expect("header dialog");
    assert!(initial.document.nodes.iter().any(|node| matches!(
        node,
        norishell_core_api::PluginUiNode::Button { action_id, .. } if action_id.as_str() == "api-demo:describe"
    )));

    let request = execute(
        &mut runtime,
        "00000000-0000-4000-8000-000000000102",
        PluginHostMessageKind::UiAction,
        json!({
            "actionId":"api-demo:describe", "fields":[]
        }),
    );
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].kind, "api.request");
    let call: norishell_core_api::PluginApiCall =
        serde_json::from_str(&request[0].payload_json).expect("typed API call");
    assert_eq!(call.call_id, "api-demo.describe");
    assert!(matches!(call.operation, PluginApiOperation::Describe {}));

    // Keep this fixture in the same order as PluginApi::description(). In
    // particular, the local-files, credentials, provider, serial and local
    // process variants caught the outdated 1.0.0 demo package at the Wasm ABI
    // boundary. A native broker sends this reply as JSON, so serialize it
    // before providing it to the production persistent Wasm runtime.
    let reply = description_reply(call.call_id.clone(), current_core_methods());
    let reply_json = serde_json::to_value(&reply).expect("serialize current Core description");
    for capability in [
        "localFiles",
        "credentialsPlugin",
        "terminalProvider",
        "deviceSerial",
        "localProcess",
    ] {
        assert!(
            reply_json.to_string().contains(capability),
            "current Core description includes {capability}"
        );
    }
    assert_eq!(
        serde_json::from_value::<PluginApiReply>(reply_json.clone())
            .expect("round-trip current Core description"),
        reply
    );
    let callback_outputs = broker_callback(
        &mut runtime,
        "00000000-0000-4000-8000-000000000103",
        reply_json,
    );
    let callback = validate_callback_document(&callback_outputs);
    let callback_json = serde_json::to_string(&callback).expect("document JSON");
    assert!(callback_json.contains("describe"));
    assert!(callback_json.contains("filePick"));
    assert!(callback_json.contains("credential"));
    assert!(callback_json.contains("protocolOpen"));
    assert!(callback_json.contains("serialDevices"));
    assert!(callback_json.contains("processStart"));
    assert!(callback_json.contains("maxCallBytes: 65536"));

    // The production-shaped callback above is the acceptance fixture. Also
    // make a separate full-enum callback so additions cannot become a hidden
    // guest serde failure before the demo package is rebuilt.
    let all_capabilities =
        description_reply("api-demo.all-capabilities".into(), all_capability_methods());
    let all_capabilities_json =
        serde_json::to_value(&all_capabilities).expect("serialize every current capability");
    assert_eq!(
        serde_json::from_value::<PluginApiReply>(all_capabilities_json.clone())
            .expect("round-trip every current capability"),
        all_capabilities
    );
    let all_callback_outputs = broker_callback(
        &mut runtime,
        "00000000-0000-4000-8000-000000000104",
        all_capabilities_json,
    );
    let all_callback = validate_callback_document(&all_callback_outputs);
    let all_callback_json = serde_json::to_string(&all_callback).expect("document JSON");
    assert!(all_callback_json.contains("Core API"));

    assert!(callback.document.nodes.iter().any(|node| matches!(
        node,
        norishell_core_api::PluginUiNode::CopyButton { action_id, .. } if action_id.as_str() == "api-demo:copy"
    )));
    let copied = execute(
        &mut runtime,
        "00000000-0000-4000-8000-000000000105",
        PluginHostMessageKind::UiAction,
        json!({
            "actionId":"api-demo:copy", "fields":[]
        }),
    );
    let copied = copied
        .iter()
        .find(|output| output.kind == "clipboard.write")
        .expect("copy output after API callback");
    assert!(copied.payload_json.contains("maxCallBytes: 65536"));
    println!("SDK api-demo passed persistent Wasm ABI and protocol-13 broker callback validation.");
}

fn broker_callback(
    runtime: &mut WasmRuntime,
    request_id: &str,
    reply: serde_json::Value,
) -> Vec<norishell_core_api::PluginRuntimeOutput> {
    execute(
        runtime,
        request_id,
        PluginHostMessageKind::BrokerResult,
        json!({
            "actionId":"api-demo:describe", "fields":[], "result":{"kind":"api","reply":reply}
        }),
    )
}

fn validate_callback_document(
    outputs: &[norishell_core_api::PluginRuntimeOutput],
) -> PluginUiTemplate {
    assert_eq!(outputs.len(), 1);
    let callback: PluginUiTemplate =
        serde_json::from_str(&outputs[0].payload_json).expect("callback template");
    validate_plugin_dialog_document(&callback.document).expect("callback dialog");
    callback
}

fn description_reply(call_id: String, methods: Vec<PluginApiMethod>) -> PluginApiReply {
    PluginApiReply {
        call_id,
        outcome: PluginApiOutcome::Completed {
            value: PluginApiValue::Description {
                api: PluginApiDescription {
                    protocol_major: PLUGIN_PROTOCOL_MAJOR,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    platform: "fixture".into(),
                    methods,
                    limits: PluginApiLimits {
                        max_call_bytes: 65536,
                        max_chunk_bytes: 16384,
                        max_resources: 32,
                        max_pending_events: 32,
                    },
                },
            },
        },
    }
}

fn current_core_methods() -> Vec<PluginApiMethod> {
    let methods: &[(&str, Option<PluginCapability>)] = &[
        ("describe", None),
        ("permissions", None),
        ("permissionRequest", None),
        ("permissionRevoke", None),
        ("permissionsForget", None),
        ("filePick", Some(PluginCapability::LocalFiles)),
        ("file", Some(PluginCapability::LocalFiles)),
        ("sftpOpen", Some(PluginCapability::SftpRead)),
        ("sftp", Some(PluginCapability::SftpRead)),
        ("remoteExecStart", Some(PluginCapability::RemoteExecRequest)),
        ("remoteExecSend", Some(PluginCapability::RemoteExecRequest)),
        ("credential", Some(PluginCapability::CredentialsPlugin)),
        ("protocolOpen", Some(PluginCapability::TerminalProvider)),
        ("serialDevices", Some(PluginCapability::DeviceSerial)),
        ("serialOpen", Some(PluginCapability::DeviceSerial)),
        ("serialSend", Some(PluginCapability::DeviceSerial)),
        ("subscriptionStart", None),
        ("taskStart", None),
        ("taskGet", None),
        ("taskList", None),
        ("taskCancel", None),
        ("taskResume", None),
        ("appRegister", None),
        ("appNotify", None),
        ("appNavigate", None),
        ("resourcesList", None),
        ("resourceClose", None),
        ("timerStart", None),
        ("resourceEvents", None),
        ("storage", Some(PluginCapability::StoragePlugin)),
        ("networkStart", Some(PluginCapability::NetworkDomain)),
        ("networkSend", Some(PluginCapability::NetworkDomain)),
        ("processStart", Some(PluginCapability::LocalProcess)),
        ("processSend", Some(PluginCapability::LocalProcess)),
        (
            "terminalRequestInput",
            Some(PluginCapability::TerminalRequestInput),
        ),
    ];
    methods
        .iter()
        .map(|(name, capability)| PluginApiMethod {
            name: (*name).into(),
            capability: *capability,
            availability: PluginApiAvailability::Available,
        })
        .collect()
}

fn all_capability_methods() -> Vec<PluginApiMethod> {
    use PluginCapability::*;

    [
        UiPanel,
        UiNavigation,
        UiPage,
        UiWebviewIsolated,
        UiHostDomObserve,
        UiHostDomMutate,
        UiHostCss,
        ClipboardWrite,
        TerminalProvider,
        DeviceSerial,
        TerminalMetadata,
        TerminalObserve,
        TerminalAnnotation,
        TerminalProposeInput,
        TerminalRequestInput,
        HostMetadataRead,
        HostMutationPropose,
        HostSessionRequest,
        RemoteInspect,
        RemoteExecRequest,
        NetworkDomain,
        LocalFiles,
        LocalProcess,
        StoragePlugin,
        CredentialsPlugin,
        SftpRead,
        SftpWrite,
        MetricsRead,
        SshSync,
    ]
    .into_iter()
    .map(|capability| {
        let capability_name = serde_json::to_value(capability)
            .expect("capability JSON")
            .as_str()
            .expect("capability string")
            .to_owned();
        PluginApiMethod {
            name: format!("capability-{capability_name}"),
            capability: Some(capability),
            availability: PluginApiAvailability::Available,
        }
    })
    .collect()
}

fn execute(
    runtime: &mut WasmRuntime,
    request_id: &str,
    kind: PluginHostMessageKind,
    payload: serde_json::Value,
) -> Vec<norishell_core_api::PluginRuntimeOutput> {
    let request = PluginHostRequest {
        protocol_major: PLUGIN_PROTOCOL_MAJOR,
        protocol_minor: PLUGIN_PROTOCOL_MINOR,
        request_id: request_id.into(),
        kind,
        payload_json: payload.to_string(),
    };
    let mut outputs = Vec::new();
    runtime
        .execute(&request, |output| {
            outputs.push(output);
            Ok(())
        })
        .expect("guest execution");
    assert!(outputs.iter().all(|output| output.request_id == request_id));
    outputs
}
