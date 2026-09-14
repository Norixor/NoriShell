//! Native acceptance harness for the SDK-built guest.
//!
//! This program plays the narrowly scoped Core broker role for a local fixture:
//! it executes the actual Wasm through `WasmRuntime`, honors only the guest's
//! typed `NetworkStart`/`NetworkSend` calls, and owns the test TCP sockets.
//! The guest itself has no socket import or native networking dependency.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginApiCall, PluginApiOperation,
    PluginApiOutcome, PluginApiReply, PluginApiResourceEvent, PluginApiResourceEventKind,
    PluginApiValue, PluginHostMessageKind, PluginHostRequest, PluginNetworkEvent,
    PluginProtocolCloseReason, PluginProtocolEvent, PluginProtocolEventKind, PluginProtocolOutput,
    PluginProtocolResponse, PluginSettingValue, PluginUiActionId, PluginUiTemplate, WireSequence,
};
use norishell_plugin_platform::{
    RuntimeLimits, WasmRuntime, validate_plugin_dialog_document, validate_plugin_dialog_ui_action,
};
use serde_json::json;

const INPUT_TYPE: u8 = 0x01;
const RESIZE_TYPE: u8 = 0x02;

fn main() {
    let (wasm, port) = arguments();
    let module = std::fs::read(wasm).expect("read protocol-demo Wasm");
    let mut runtime = WasmRuntime::new(&module, RuntimeLimits::default())
        .expect("protocol-13 ABI and the sole host emit import");

    let initial = verify_initial_contribution(&mut runtime);
    verify_explicit_open_action(&mut runtime, &initial);
    let endpoint = format!("tcp://127.0.0.1:{port}");

    let mut first = open_connection(&mut runtime, "connection-a", "resource-a", &endpoint, port);
    send_input(
        &mut runtime,
        "connection-a",
        "resource-a",
        &mut first,
        b"first input\n",
    );
    send_resize(
        &mut runtime,
        "connection-a",
        "resource-a",
        &mut first,
        33,
        101,
    );
    let first_output = read_framed(&mut first);
    let first_terminal =
        deliver_split_data(&mut runtime, "connection-a", "resource-a", &first_output);
    assert_eq!(
        first_terminal,
        b"\x1b[32mfirst: \xe4\xb8\x96\xe7\x95\x8c\x1b[0m\r\n"
    );

    let mut second = open_connection(&mut runtime, "connection-b", "resource-b", &endpoint, port);
    let closed = execute_protocol(
        &mut runtime,
        "close-old",
        PluginProtocolEvent {
            connection_handle: "connection-a".into(),
            event_id: "close-old".into(),
            event: PluginProtocolEventKind::Close {
                reason: PluginProtocolCloseReason::User,
            },
        },
    );
    assert!(closed.complete && closed.call.is_none());

    // The old connection's Close must remove only connection-a. A subsequent
    // input on connection-b still uses resource-b and reaches the fixture.
    send_input(
        &mut runtime,
        "connection-b",
        "resource-b",
        &mut second,
        b"second input\n",
    );
    let second_output = read_framed(&mut second);
    let second_terminal =
        deliver_split_data(&mut runtime, "connection-b", "resource-b", &second_output);
    assert_eq!(
        second_terminal,
        b"\x1b[35msecond: \xe6\xad\xa3\xe5\xb8\xb8\x1b[0m\r\n"
    );
    println!(
        "Protocol demo passed production Wasm runtime and local Core-broker fixture validation."
    );
}

fn verify_initial_contribution(runtime: &mut WasmRuntime) -> PluginUiTemplate {
    let outputs = execute_raw(
        runtime,
        "protocol-demo-initialize",
        PluginHostMessageKind::Initialize,
        json!({"pluginId":"com.norishell.protocol-demo", "locale":"en", "storage":null}),
    );
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].kind, "ui.document");
    let template: PluginUiTemplate =
        serde_json::from_str(&outputs[0].payload_json).expect("typed initial contribution");
    assert_eq!(template.target_id.as_str(), "app.header.actions");
    validate_plugin_dialog_document(&template.document)
        .expect("initial dialog must pass the production Core UI validator");
    template
}

fn arguments() -> (std::path::PathBuf, u16) {
    let mut wasm = None;
    let mut port = None;
    let mut values = std::env::args_os().skip(1);
    while let Some(flag) = values.next() {
        match flag.to_string_lossy().as_ref() {
            "--wasm" => wasm = values.next().map(std::path::PathBuf::from),
            "--port" => {
                port = values
                    .next()
                    .and_then(|value| value.to_string_lossy().parse::<u16>().ok())
            }
            _ => panic!("usage: protocol-demo-harness --wasm <guest.wasm> --port <fixture-port>"),
        }
    }
    (wasm.expect("--wasm"), port.expect("--port"))
}

fn verify_explicit_open_action(runtime: &mut WasmRuntime, initial: &PluginUiTemplate) {
    let action_id = PluginUiActionId::parse("protocol-demo:open").expect("declared action id");
    validate_plugin_dialog_ui_action(&initial.document, &action_id, &[])
        .expect("Open action must pass the production Core UI action validator");
    let outputs = execute_raw(
        runtime,
        "ui-open",
        PluginHostMessageKind::UiAction,
        json!({"actionId":"protocol-demo:open", "fields":[]}),
    );
    assert_eq!(outputs.len(), 1);
    let call: PluginApiCall =
        serde_json::from_str(&outputs[0].payload_json).expect("typed UI call");
    assert_eq!(call.call_id, "protocol-demo.open");
    assert!(matches!(
        call.operation,
        PluginApiOperation::ProtocolOpen { ref request }
            if request.provider_id == "framedTcp"
    ));

    let reply = PluginApiReply {
        call_id: call.call_id,
        outcome: PluginApiOutcome::Completed {
            value: PluginApiValue::ProtocolLaunched {
                launch_id: "fixture-protocol-launch".into(),
            },
        },
    };
    let callback = execute_raw(
        runtime,
        "ui-open-result",
        PluginHostMessageKind::BrokerResult,
        json!({"result":{"kind":"api","reply":reply}}),
    );
    assert_eq!(callback.len(), 1);
    assert_eq!(callback[0].kind, "ui.document");
    let callback: PluginUiTemplate =
        serde_json::from_str(&callback[0].payload_json).expect("typed broker callback document");
    validate_plugin_dialog_document(&callback.document)
        .expect("broker callback must pass the production Core UI validator");
    assert!(
        serde_json::to_string(&callback)
            .expect("callback document JSON")
            .contains("fixture-protocol-launch")
    );
}

fn open_connection(
    runtime: &mut WasmRuntime,
    connection: &str,
    resource: &str,
    endpoint: &str,
    port: u16,
) -> TcpStream {
    let mut configuration = BTreeMap::new();
    configuration.insert(
        "endpoint".into(),
        PluginSettingValue::String(endpoint.into()),
    );
    let connect = execute_protocol(
        runtime,
        &format!("connect-{connection}"),
        PluginProtocolEvent {
            connection_handle: connection.into(),
            event_id: format!("connect-{connection}"),
            event: PluginProtocolEventKind::Connect {
                provider_id: "framedTcp".into(),
                configuration,
                rows: 24,
                cols: 80,
            },
        },
    );
    let call = only_call(connect);
    assert!(matches!(
        call.operation,
        PluginApiOperation::NetworkStart { .. }
    ));

    // The harness is the test Core broker: it dials only after receiving the
    // typed NetworkStart request, mirroring Core's approved resource boundary.
    let stream = TcpStream::connect(("127.0.0.1", port)).expect("fixture TCP connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("fixture read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("fixture write timeout");
    let api_result = PluginApiReply {
        call_id: call.call_id,
        outcome: PluginApiOutcome::Completed {
            value: PluginApiValue::NetworkStarted {
                handle: resource.into(),
            },
        },
    };
    complete_api(
        runtime,
        connection,
        &format!("started-{connection}"),
        api_result,
    );

    let opened = execute_protocol(
        runtime,
        &format!("opened-{connection}"),
        resource_event(
            connection,
            resource,
            "opened",
            vec![PluginNetworkEvent::Opened {
                protocol: norishell_core_api::PluginNetworkProtocol::Tcp,
                peer_address: None,
            }],
        ),
    );
    assert!(opened.complete && opened.call.is_none());
    assert!(
        opened
            .outputs
            .iter()
            .any(|output| matches!(output, PluginProtocolOutput::Ready {}))
    );
    stream
}

fn send_input(
    runtime: &mut WasmRuntime,
    connection: &str,
    resource: &str,
    stream: &mut TcpStream,
    bytes: &[u8],
) {
    let response = execute_protocol(
        runtime,
        &format!("input-{connection}"),
        PluginProtocolEvent {
            connection_handle: connection.into(),
            event_id: format!("input-{connection}"),
            event: PluginProtocolEventKind::Input {
                data_base64: BASE64.encode(bytes),
            },
        },
    );
    let call = only_call(response);
    let PluginApiOperation::NetworkSend { request } = call.operation else {
        panic!("input must emit NetworkSend");
    };
    assert_eq!(request.handle, resource);
    let frame = BASE64
        .decode(request.data_base64)
        .expect("input frame base64");
    assert_eq!(frame_type(&frame), INPUT_TYPE);
    assert_eq!(&frame[5..], bytes);
    stream.write_all(&frame).expect("Core writes input frame");
    complete_api(
        runtime,
        connection,
        &format!("input-sent-{connection}"),
        sent_reply(call.call_id, resource),
    );
}

fn send_resize(
    runtime: &mut WasmRuntime,
    connection: &str,
    resource: &str,
    stream: &mut TcpStream,
    rows: u16,
    cols: u16,
) {
    let response = execute_protocol(
        runtime,
        &format!("resize-{connection}"),
        PluginProtocolEvent {
            connection_handle: connection.into(),
            event_id: format!("resize-{connection}"),
            event: PluginProtocolEventKind::Resize { rows, cols },
        },
    );
    let call = only_call(response);
    let PluginApiOperation::NetworkSend { request } = call.operation else {
        panic!("resize must emit NetworkSend");
    };
    assert_eq!(request.handle, resource);
    let frame = BASE64
        .decode(request.data_base64)
        .expect("resize frame base64");
    assert_eq!(frame_type(&frame), RESIZE_TYPE);
    assert_eq!(&frame[5..], &[0, rows as u8, 0, cols as u8]);
    stream.write_all(&frame).expect("Core writes resize frame");
    complete_api(
        runtime,
        connection,
        &format!("resize-sent-{connection}"),
        sent_reply(call.call_id, resource),
    );
}

fn sent_reply(call_id: String, resource: &str) -> PluginApiReply {
    PluginApiReply {
        call_id,
        outcome: PluginApiOutcome::Completed {
            value: PluginApiValue::NetworkSent {
                handle: resource.into(),
            },
        },
    }
}

fn complete_api(
    runtime: &mut WasmRuntime,
    connection: &str,
    event_id: &str,
    reply: PluginApiReply,
) {
    let response = execute_protocol(
        runtime,
        event_id,
        PluginProtocolEvent {
            connection_handle: connection.into(),
            event_id: event_id.into(),
            event: PluginProtocolEventKind::ApiResult {
                call_id: reply.call_id.clone(),
                reply,
            },
        },
    );
    assert!(response.complete && response.call.is_none());
}

fn read_framed(stream: &mut TcpStream) -> Vec<u8> {
    let mut header = [0_u8; 4];
    stream
        .read_exact(&mut header)
        .expect("fixture frame length");
    let body = u32::from_be_bytes(header) as usize;
    assert!((1..=32 * 1024).contains(&body));
    let mut full = Vec::with_capacity(4 + body);
    full.extend_from_slice(&header);
    full.resize(4 + body, 0);
    stream
        .read_exact(&mut full[4..])
        .expect("fixture frame body");
    full
}

fn deliver_split_data(
    runtime: &mut WasmRuntime,
    connection: &str,
    resource: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let split = [2_usize, 5, bytes.len()];
    let mut previous = 0;
    let mut output = Vec::new();
    for (index, next) in split.into_iter().enumerate() {
        let response = execute_protocol(
            runtime,
            &format!("data-{connection}-{index}"),
            resource_event(
                connection,
                resource,
                &format!("data-{connection}-{index}"),
                vec![PluginNetworkEvent::Data {
                    data_base64: BASE64.encode(&bytes[previous..next]),
                }],
            ),
        );
        for item in response.outputs {
            if let PluginProtocolOutput::Output { data_base64 } = item {
                output.extend(BASE64.decode(data_base64).expect("terminal output base64"));
            }
        }
        previous = next;
    }
    output
}

fn resource_event(
    connection: &str,
    resource: &str,
    event_id: &str,
    network_events: Vec<PluginNetworkEvent>,
) -> PluginProtocolEvent {
    PluginProtocolEvent {
        connection_handle: connection.into(),
        event_id: event_id.into(),
        event: PluginProtocolEventKind::Resource {
            resource_handle: resource.into(),
            events: network_events
                .into_iter()
                .enumerate()
                .map(|(index, event)| PluginApiResourceEvent {
                    sequence: WireSequence::new((index + 1) as u64),
                    kind: PluginApiResourceEventKind::Network { event },
                })
                .collect(),
        },
    }
}

fn only_call(response: PluginProtocolResponse) -> PluginApiCall {
    assert!(!response.complete);
    assert!(response.outputs.is_empty());
    response.call.expect("one typed broker call")
}

fn frame_type(frame: &[u8]) -> u8 {
    assert!(frame.len() >= 5);
    assert_eq!(
        u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
        frame.len() - 4
    );
    frame[4]
}

fn execute_protocol(
    runtime: &mut WasmRuntime,
    request_id: &str,
    event: PluginProtocolEvent,
) -> PluginProtocolResponse {
    let output = execute_raw(
        runtime,
        request_id,
        PluginHostMessageKind::ProtocolEvent,
        serde_json::to_value(event).expect("protocol event JSON"),
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].kind, "protocol.response");
    serde_json::from_str(&output[0].payload_json).expect("protocol response")
}

fn execute_raw(
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
        .expect("Wasm guest execution");
    outputs
}
