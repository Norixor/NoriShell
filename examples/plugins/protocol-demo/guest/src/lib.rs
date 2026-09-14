//! A protocol-provider example whose protocol parser remains entirely in Wasm.
//!
//! Wire format: a 4-byte big-endian length (the following type byte plus
//! payload), one type byte, then opaque payload bytes. Core owns the TCP
//! resource; this guest only emits typed broker calls and consumes resource
//! events. Terminal output stays raw bytes, so ANSI and UTF-8 are preserved.

use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_plugin_sdk::{
    Plugin, PluginApiCall, PluginApiErrorCode, PluginApiOperation, PluginApiOutcome,
    PluginApiReply, PluginApiResourceEventKind, PluginApiValue, PluginError, PluginHostMessageKind,
    PluginHostRequest, PluginNetworkEndpointRequest, PluginNetworkEvent, PluginNetworkOperation,
    PluginNetworkSendRequest, PluginNetworkStartRequest, PluginProtocolEvent,
    PluginProtocolEventKind, PluginProtocolOpen, PluginProtocolOutput, PluginRuntimeOutput,
    PluginSettingValue, api_request, export_plugin, output, payload, protocol_event,
    protocol_response,
};
use serde_json::{Value, json};

const PROVIDER_ID: &str = "framedTcp";
const OPEN_ACTION: &str = "protocol-demo:open";
const OPEN_CALL_ID: &str = "protocol-demo.open";
const DEFAULT_ENDPOINT: &str = "tcp://127.0.0.1:19071";
const MAX_FRAME_BODY: usize = 32 * 1024;
const INPUT_TYPE: u8 = 0x01;
const RESIZE_TYPE: u8 = 0x02;
const OUTPUT_TYPE: u8 = 0x81;

#[derive(Default)]
struct ProtocolDemo {
    connections: BTreeMap<String, Connection>,
}

#[derive(Default)]
struct Connection {
    resource_handle: Option<String>,
    opened: bool,
    inbound: Vec<u8>,
}

impl Plugin for ProtocolDemo {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        match request.kind {
            PluginHostMessageKind::Initialize => Ok(vec![document(&request.request_id, None)?]),
            PluginHostMessageKind::UiAction => self.handle_ui_action(&request),
            PluginHostMessageKind::BrokerResult => self.handle_broker_result(&request),
            PluginHostMessageKind::ProtocolEvent => self.handle_protocol_event(&request),
            _ => Err(PluginError::InvalidRequest),
        }
    }
}

impl ProtocolDemo {
    fn handle_ui_action(
        &mut self,
        request: &PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let body: Value = payload(request)?;
        if body.get("actionId").and_then(Value::as_str) != Some(OPEN_ACTION) {
            return Err(PluginError::InvalidRequest);
        }
        let mut configuration = BTreeMap::new();
        configuration.insert(
            "endpoint".to_owned(),
            Value::String(DEFAULT_ENDPOINT.to_owned()),
        );
        Ok(vec![api_request(
            &request.request_id,
            OPEN_CALL_ID,
            PluginApiOperation::ProtocolOpen {
                request: PluginProtocolOpen {
                    provider_id: PROVIDER_ID.to_owned(),
                    configuration,
                    label: Some("Framed TCP demo".to_owned()),
                },
            },
        )?])
    }

    fn handle_broker_result(
        &mut self,
        request: &PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let body: Value = payload(request)?;
        let reply = body
            .get("result")
            .filter(|value| value.get("kind").and_then(Value::as_str) == Some("api"))
            .and_then(|value| value.get("reply"))
            .ok_or(PluginError::InvalidRequest)?;
        let reply: PluginApiReply =
            serde_json::from_value(reply.clone()).map_err(|_| PluginError::InvalidRequest)?;
        let status = match reply.outcome {
            PluginApiOutcome::Completed {
                value: PluginApiValue::ProtocolLaunched { launch_id },
            } => format!("Protocol launch is ready to claim: {launch_id}"),
            PluginApiOutcome::Failed { code } => {
                format!("Core rejected the protocol launch: {code:?}")
            }
            _ => return Err(PluginError::InvalidRequest),
        };
        Ok(vec![document(&request.request_id, Some(&status))?])
    }

    fn handle_protocol_event(
        &mut self,
        request: &PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let event = protocol_event(request)?;
        let response = match &event.event {
            PluginProtocolEventKind::Connect {
                provider_id,
                configuration,
                ..
            } => self.connect(request, &event, provider_id, configuration),
            PluginProtocolEventKind::Input { data_base64 } => {
                self.send_frame(request, &event, INPUT_TYPE, decode(data_base64)?)
            }
            PluginProtocolEventKind::Resize { rows, cols } => {
                self.send_frame(request, &event, RESIZE_TYPE, resize_payload(*rows, *cols))
            }
            PluginProtocolEventKind::Resource {
                resource_handle,
                events,
            } => self.resource(request, &event, resource_handle, events),
            PluginProtocolEventKind::ApiResult { call_id, reply } => {
                self.api_result(request, &event, call_id, reply)
            }
            PluginProtocolEventKind::Close { .. } => {
                self.connections.remove(&event.connection_handle);
                protocol_response(request, &event, Vec::new(), None)
            }
        }?;
        Ok(vec![response])
    }

    fn connect(
        &mut self,
        request: &PluginHostRequest,
        event: &PluginProtocolEvent,
        provider_id: &str,
        configuration: &BTreeMap<String, PluginSettingValue>,
    ) -> Result<PluginRuntimeOutput, PluginError> {
        if provider_id != PROVIDER_ID || self.connections.contains_key(&event.connection_handle) {
            return fail(request, event, PluginApiErrorCode::InvalidRequest);
        }
        let Some(PluginSettingValue::String(endpoint)) = configuration.get("endpoint") else {
            return fail(request, event, PluginApiErrorCode::InvalidRequest);
        };
        self.connections
            .insert(event.connection_handle.clone(), Connection::default());
        let call = PluginApiCall {
            call_id: start_call_id(&event.event_id),
            operation: PluginApiOperation::NetworkStart {
                endpoint: PluginNetworkEndpointRequest {
                    endpoint: endpoint.clone(),
                },
                request: PluginNetworkStartRequest {
                    timeout_ms: 5_000,
                    credential: None,
                    operation: PluginNetworkOperation::Tcp {},
                },
            },
        };
        protocol_response(request, event, Vec::new(), Some(call))
    }

    fn send_frame(
        &mut self,
        request: &PluginHostRequest,
        event: &PluginProtocolEvent,
        frame_type: u8,
        payload: Vec<u8>,
    ) -> Result<PluginRuntimeOutput, PluginError> {
        let Some(connection) = self.connections.get(&event.connection_handle) else {
            return fail(request, event, PluginApiErrorCode::NotFound);
        };
        let Some(handle) = connection.resource_handle.as_ref() else {
            return fail(request, event, PluginApiErrorCode::Unavailable);
        };
        if !connection.opened {
            return fail(request, event, PluginApiErrorCode::Unavailable);
        }
        let call = PluginApiCall {
            call_id: send_call_id(&event.event_id),
            operation: PluginApiOperation::NetworkSend {
                request: PluginNetworkSendRequest {
                    handle: handle.clone(),
                    data_base64: BASE64.encode(frame(frame_type, &payload)?),
                },
            },
        };
        protocol_response(request, event, Vec::new(), Some(call))
    }

    fn api_result(
        &mut self,
        request: &PluginHostRequest,
        event: &PluginProtocolEvent,
        call_id: &str,
        reply: &PluginApiReply,
    ) -> Result<PluginRuntimeOutput, PluginError> {
        let Some(connection) = self.connections.get_mut(&event.connection_handle) else {
            return protocol_response(request, event, Vec::new(), None);
        };
        match &reply.outcome {
            PluginApiOutcome::Failed { code } => fail(request, event, *code),
            PluginApiOutcome::Completed {
                value: PluginApiValue::NetworkStarted { handle },
            } if call_id.starts_with("protocol-demo.start.") && reply.call_id == call_id => {
                connection.resource_handle = Some(handle.clone());
                protocol_response(request, event, Vec::new(), None)
            }
            PluginApiOutcome::Completed {
                value: PluginApiValue::NetworkSent { handle },
            } if call_id.starts_with("protocol-demo.send.")
                && reply.call_id == call_id
                && connection.resource_handle.as_deref() == Some(handle) =>
            {
                protocol_response(request, event, Vec::new(), None)
            }
            _ => fail(request, event, PluginApiErrorCode::InvalidRequest),
        }
    }

    fn resource(
        &mut self,
        request: &PluginHostRequest,
        event: &PluginProtocolEvent,
        resource_handle: &str,
        events: &[norishell_plugin_sdk::PluginApiResourceEvent],
    ) -> Result<PluginRuntimeOutput, PluginError> {
        let Some(connection) = self.connections.get_mut(&event.connection_handle) else {
            return fail(request, event, PluginApiErrorCode::NotFound);
        };
        if connection.resource_handle.as_deref() != Some(resource_handle) {
            return fail(request, event, PluginApiErrorCode::InvalidRequest);
        }
        let mut outputs = Vec::new();
        for resource_event in events {
            let PluginApiResourceEventKind::Network { event: network } = &resource_event.kind
            else {
                return fail(request, event, PluginApiErrorCode::InvalidRequest);
            };
            match network {
                PluginNetworkEvent::Opened { .. } if !connection.opened => {
                    connection.opened = true;
                    outputs.push(PluginProtocolOutput::Ready {});
                }
                PluginNetworkEvent::Opened { .. } => {}
                PluginNetworkEvent::Data { data_base64 } => {
                    connection.inbound.extend(decode(data_base64)?);
                    parse_frames(&mut connection.inbound, &mut outputs)?;
                }
                PluginNetworkEvent::Closed {} => {
                    self.connections.remove(&event.connection_handle);
                    outputs.push(PluginProtocolOutput::Exit { code: None });
                    break;
                }
                PluginNetworkEvent::Error { .. } => {
                    self.connections.remove(&event.connection_handle);
                    outputs.push(PluginProtocolOutput::Fail {
                        stable_code: PluginApiErrorCode::Unavailable,
                    });
                    break;
                }
                _ => return fail(request, event, PluginApiErrorCode::InvalidRequest),
            }
        }
        protocol_response(request, event, outputs, None)
    }
}

fn decode(value: &str) -> Result<Vec<u8>, PluginError> {
    BASE64
        .decode(value)
        .map_err(|_| PluginError::InvalidRequest)
}

fn frame(frame_type: u8, payload: &[u8]) -> Result<Vec<u8>, PluginError> {
    let body_len = payload
        .len()
        .checked_add(1)
        .filter(|size| *size <= MAX_FRAME_BODY)
        .ok_or(PluginError::InvalidRequest)?;
    let mut bytes = Vec::with_capacity(4 + body_len);
    bytes.extend_from_slice(&(body_len as u32).to_be_bytes());
    bytes.push(frame_type);
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

fn resize_payload(rows: u16, cols: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4);
    payload.extend_from_slice(&rows.to_be_bytes());
    payload.extend_from_slice(&cols.to_be_bytes());
    payload
}

fn parse_frames(
    inbound: &mut Vec<u8>,
    outputs: &mut Vec<PluginProtocolOutput>,
) -> Result<(), PluginError> {
    while inbound.len() >= 4 {
        let body_len = u32::from_be_bytes(inbound[..4].try_into().unwrap()) as usize;
        if body_len == 0 || body_len > MAX_FRAME_BODY {
            return Err(PluginError::InvalidRequest);
        }
        let total = 4 + body_len;
        if inbound.len() < total {
            break;
        }
        let frame_type = inbound[4];
        if frame_type != OUTPUT_TYPE {
            return Err(PluginError::InvalidRequest);
        }
        outputs.push(PluginProtocolOutput::Output {
            data_base64: BASE64.encode(&inbound[5..total]),
        });
        inbound.drain(..total);
    }
    if inbound.len() > MAX_FRAME_BODY + 4 {
        return Err(PluginError::InvalidRequest);
    }
    Ok(())
}

fn start_call_id(event_id: &str) -> String {
    format!("protocol-demo.start.{event_id}")
}

fn send_call_id(event_id: &str) -> String {
    format!("protocol-demo.send.{event_id}")
}

fn fail(
    request: &PluginHostRequest,
    event: &PluginProtocolEvent,
    code: PluginApiErrorCode,
) -> Result<PluginRuntimeOutput, PluginError> {
    protocol_response(
        request,
        event,
        vec![PluginProtocolOutput::Fail { stable_code: code }],
        None,
    )
}

fn document(request_id: &str, status: Option<&str>) -> Result<PluginRuntimeOutput, PluginError> {
    output(
        request_id,
        "ui.document",
        &json!({
            "targetId": "app.header.actions",
            "document": {
                "schemaVersion": 1,
                "rootNodeId": "protocolDemoDialog",
                "nodes": [
                    {
                        "kind": "dialog",
                        "nodeId": "protocolDemoDialog",
                        "title": "Framed TCP protocol demo",
                        "description": "Open an explicitly approved terminal-provider launch. TCP is brokered by Core; framing stays in Wasm.",
                        "triggerLabel": "Protocol demo",
                        "closeLabel": "Close protocol demo",
                        "children": ["intro", "open", "status"]
                    },
                    {
                        "kind": "text",
                        "nodeId": "intro",
                        "text": "The package requests a Core-approved TCP resource only after this button is clicked.",
                        "style": "caption",
                        "tone": "neutral"
                    },
                    {
                        "kind": "button",
                        "nodeId": "open",
                        "actionId": OPEN_ACTION,
                        "label": "Open framed TCP terminal",
                        "icon": "link",
                        "variant": "primary",
                        "disabled": false
                    },
                    {
                        "kind": "code",
                        "nodeId": "status",
                        "text": status.unwrap_or("No protocol terminal launch has been requested."),
                        "language": "text",
                        "wrap": true
                    }
                ]
            }
        }),
    )
}

export_plugin!(ProtocolDemo);
