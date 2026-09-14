//! Protocol-13 SDK example. It uses only the typed `describe` broker call and
//! renders the Core reply; it does not read a Host, terminal, file, network or
//! any other system data.

use norishell_plugin_sdk::{
    Plugin, PluginApiOperation, PluginApiOutcome, PluginApiReply, PluginApiValue, PluginError,
    PluginHostMessageKind, PluginHostRequest, PluginRuntimeOutput, api_request, export_plugin,
    output, payload,
};
use serde_json::{Value, json};

const TARGET: &str = "app.header.actions";
const DESCRIBE_ACTION: &str = "api-demo:describe";
const COPY_ACTION: &str = "api-demo:copy";

#[derive(Default)]
struct ApiDemo {
    latest_description: Option<String>,
}

impl Plugin for ApiDemo {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let request_id = request.request_id.clone();
        let body: Value = payload(&request)?;
        match request.kind {
            PluginHostMessageKind::Initialize => Ok(vec![document_output(
                &request_id,
                self.latest_description
                    .as_deref()
                    .unwrap_or("Select Query API to ask Core for its typed API description."),
            )?]),
            PluginHostMessageKind::UiAction => match body["actionId"].as_str() {
                Some(DESCRIBE_ACTION) => Ok(vec![api_request(
                    &request_id,
                    "api-demo.describe",
                    PluginApiOperation::Describe {},
                )?]),
                Some(COPY_ACTION) => {
                    let text = self
                        .latest_description
                        .as_deref()
                        .unwrap_or("No API description is available yet. Select Query API first.");
                    Ok(vec![
                        output(&request_id, "clipboard.write", &json!({"text": text}))?,
                        document_output(&request_id, text)?,
                    ])
                }
                _ => Err(PluginError::InvalidRequest),
            },
            PluginHostMessageKind::BrokerResult => {
                let result = body
                    .get("result")
                    .filter(|result| result.get("kind").and_then(Value::as_str) == Some("api"))
                    .and_then(|result| result.get("reply"))
                    .ok_or(PluginError::InvalidRequest)?;
                let reply: PluginApiReply = serde_json::from_value(result.clone())
                    .map_err(|_| PluginError::InvalidRequest)?;
                let text = render_description(&reply)?;
                self.latest_description = Some(text.clone());
                Ok(vec![document_output(&request_id, &text)?])
            }
            _ => Err(PluginError::InvalidRequest),
        }
    }
}

fn render_description(reply: &PluginApiReply) -> Result<String, PluginError> {
    match &reply.outcome {
        PluginApiOutcome::Completed {
            value: PluginApiValue::Description { api },
        } => {
            let methods = api
                .methods
                .iter()
                .map(|method| format!("- {} ({:?})", method.name, method.availability))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!(
                "Core API {}.{} on {}\n\nMethods:\n{}\n\nLimits:\nmaxCallBytes: {}\nmaxChunkBytes: {}\nmaxResources: {}\nmaxPendingEvents: {}",
                api.protocol_major,
                api.protocol_minor,
                api.platform,
                methods,
                api.limits.max_call_bytes,
                api.limits.max_chunk_bytes,
                api.limits.max_resources,
                api.limits.max_pending_events,
            ))
        }
        PluginApiOutcome::Completed { value } => {
            serde_json::to_string_pretty(value).map_err(|_| PluginError::HandlerFailed)
        }
        PluginApiOutcome::Failed { code } => Ok(format!("Core rejected describe: {code:?}")),
    }
}

fn document_output(request_id: &str, details: &str) -> Result<PluginRuntimeOutput, PluginError> {
    output(
        request_id,
        "ui.document",
        &json!({
            "targetId": TARGET,
            "document": {
                "schemaVersion": 1,
                "rootNodeId": "apiDemoDialog",
                "nodes": [
                    {
                        "kind": "dialog",
                        "nodeId": "apiDemoDialog",
                        "title": "NoriShell API demo",
                        "description": "A protocol-13 SDK example that asks Core to describe only its plugin API.",
                        "triggerLabel": "API demo",
                        "closeLabel": "Close API demo",
                        "children": ["intro", "describe", "details", "copy"]
                    },
                    {
                        "kind": "text",
                        "nodeId": "intro",
                        "text": "The plugin has no Host, terminal, network, file, or Vault access.",
                        "style": "caption",
                        "tone": "neutral"
                    },
                    {
                        "kind": "button",
                        "nodeId": "describe",
                        "actionId": DESCRIBE_ACTION,
                        "label": "Query API",
                        "icon": "info",
                        "variant": "primary",
                        "disabled": false
                    },
                    {
                        "kind": "code",
                        "nodeId": "details",
                        "text": details,
                        "language": "text",
                        "wrap": true
                    },
                    {
                        "kind": "copyButton",
                        "nodeId": "copy",
                        "actionId": COPY_ACTION,
                        "label": "Copy API description",
                        "disabled": false
                    }
                ]
            }
        }),
    )
}

export_plugin!(ApiDemo);
