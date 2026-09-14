//! A public HTTPS service example whose only I/O path is the Core broker.
//!
//! The workflow is needed because Core, not the guest, owns asynchronous network resources.
//! A foreground task resume may obtain a `networkDomain` approval; automatic workflow steps and
//! resource events never create that prompt.

use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use norishell_plugin_sdk::{
    Plugin, PluginApiErrorCode, PluginApiOperation, PluginApiOutcome, PluginApiReply,
    PluginApiResourceEvent, PluginApiResourceEventKind, PluginApiValue, PluginError,
    PluginHostMessageKind, PluginHostRequest, PluginHttpMethod, PluginNetworkEndpointRequest,
    PluginNetworkEvent, PluginNetworkHeader, PluginNetworkOperation, PluginNetworkStartRequest,
    PluginWorkflowTaskSnapshot, PluginWorkflowTaskState, WorkflowEvent, WorkflowResponse,
    api_request, output, payload, workflow_event, workflow_response,
};
use serde_json::{Value, json};

const TARGET: &str = "app.header.actions";
const WORKFLOW_ID: &str = "github.repository.read";
const REQUEST_STEP: &str = "github.repository.request";
const QUERY_ACTION: &str = "service.query";
const CONTINUE_ACTION: &str = "service.continue";
const REFRESH_ACTION: &str = "service.refresh";
const CANCEL_ACTION: &str = "service.cancel";
const GITHUB_REPOSITORY_URL: &str = "https://api.github.com/repos/rust-lang/rust";
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct ServiceDemo {
    latest: Option<PluginWorkflowTaskSnapshot>,
    reports: BTreeMap<String, QueryReport>,
}

#[derive(Default)]
struct QueryReport {
    handle: Option<String>,
    status: Option<u16>,
    is_json: bool,
    body: Vec<u8>,
    result: Option<Repository>,
    error: Option<String>,
}

struct Repository {
    full_name: String,
    name: String,
    description: String,
    stars: u64,
}

impl Plugin for ServiceDemo {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<norishell_plugin_sdk::PluginRuntimeOutput>, PluginError> {
        if request.kind == PluginHostMessageKind::WorkflowEvent {
            let event = workflow_event(&request)?;
            let response = self.advance_workflow(&event.task_id.to_string(), event.event)?;
            return Ok(vec![workflow_response(&request, &response)?]);
        }

        let body: Value = payload(&request)?;
        match request.kind {
            PluginHostMessageKind::Initialize => self.document(&request.request_id),
            PluginHostMessageKind::UiAction => self.handle_ui_action(&request, &body),
            PluginHostMessageKind::BrokerResult => self.handle_broker_result(&request, &body),
            _ => Err(PluginError::InvalidRequest),
        }
    }
}

impl ServiceDemo {
    fn handle_ui_action(
        &mut self,
        request: &PluginHostRequest,
        body: &Value,
    ) -> Result<Vec<norishell_plugin_sdk::PluginRuntimeOutput>, PluginError> {
        let action = body.get("actionId").and_then(Value::as_str);
        let operation = match action {
            Some(QUERY_ACTION) => PluginApiOperation::TaskStart {
                workflow_id: WORKFLOW_ID.into(),
                input_json: None,
                file_scope_handles: vec![],
            },
            Some(CONTINUE_ACTION) => {
                let task = self.needs_user_action()?;
                PluginApiOperation::TaskResume {
                    task_id: task.task_id.clone(),
                    expected_revision: task.revision,
                }
            }
            Some(REFRESH_ACTION) => match self.latest.as_ref() {
                Some(snapshot) => PluginApiOperation::TaskGet {
                    task_id: snapshot.task.task_id.clone(),
                },
                None => PluginApiOperation::TaskList {},
            },
            Some(CANCEL_ACTION) => {
                let task = self.active_task()?;
                PluginApiOperation::TaskCancel {
                    task_id: task.task_id.clone(),
                    expected_revision: task.revision,
                }
            }
            _ => return Err(PluginError::InvalidRequest),
        };
        Ok(vec![api_request(
            &request.request_id,
            action.unwrap_or_default(),
            operation,
        )?])
    }

    fn handle_broker_result(
        &mut self,
        request: &PluginHostRequest,
        body: &Value,
    ) -> Result<Vec<norishell_plugin_sdk::PluginRuntimeOutput>, PluginError> {
        let reply = body
            .get("result")
            .filter(|result| result.get("kind").and_then(Value::as_str) == Some("api"))
            .and_then(|result| result.get("reply"))
            .ok_or(PluginError::InvalidRequest)?;
        let reply: PluginApiReply =
            serde_json::from_value(reply.clone()).map_err(|_| PluginError::InvalidRequest)?;
        match reply.outcome {
            PluginApiOutcome::Completed { value } => match value {
                PluginApiValue::Task { snapshot } => self.latest = Some(snapshot),
                PluginApiValue::Tasks { mut snapshots } => {
                    snapshots.sort_by_key(|snapshot| snapshot.task.updated_at_unix_ms);
                    self.latest = snapshots.pop();
                }
                _ => return Err(PluginError::InvalidRequest),
            },
            PluginApiOutcome::Failed { code } => self.record_broker_error(code),
        }
        self.document(&request.request_id)
    }

    fn advance_workflow(
        &mut self,
        task_id: &str,
        event: WorkflowEvent,
    ) -> Result<WorkflowResponse, PluginError> {
        match event {
            WorkflowEvent::Start { .. } => {
                self.reports.insert(task_id.into(), QueryReport::default());
                Ok(step(
                    REQUEST_STEP,
                    PluginApiOperation::NetworkStart {
                        endpoint: PluginNetworkEndpointRequest {
                            endpoint: GITHUB_REPOSITORY_URL.into(),
                        },
                        request: PluginNetworkStartRequest {
                            timeout_ms: 10_000,
                            credential: None,
                            operation: PluginNetworkOperation::Http {
                                method: PluginHttpMethod::Get,
                                headers: vec![
                                    PluginNetworkHeader {
                                        name: "User-Agent".into(),
                                        value: "NoriShell-Service-Demo/1.0".into(),
                                    },
                                    PluginNetworkHeader {
                                        name: "Accept".into(),
                                        value: "application/vnd.github+json".into(),
                                    },
                                    PluginNetworkHeader {
                                        name: "X-GitHub-Api-Version".into(),
                                        value: "2022-11-28".into(),
                                    },
                                ],
                                body_base64: String::new(),
                            },
                        },
                    },
                ))
            }
            WorkflowEvent::StepResult { step_id, reply } if step_id == REQUEST_STEP => {
                let report = self.report_mut(task_id);
                match reply.outcome {
                    PluginApiOutcome::Completed {
                        value: PluginApiValue::NetworkStarted { handle },
                    } => {
                        report.handle = Some(handle);
                        Ok(wait())
                    }
                    PluginApiOutcome::Failed { code } => {
                        report.error = Some(format!("Core rejected the GitHub request: {code:?}"));
                        Ok(complete())
                    }
                    _ => Err(PluginError::InvalidRequest),
                }
            }
            WorkflowEvent::ResourceEvents {
                handle,
                events,
                backpressured,
            } => {
                let report = self.report_mut(task_id);
                if report.handle.as_deref() != Some(handle.as_str()) {
                    return Ok(wait());
                }
                if backpressured {
                    report.error = Some(
                        "Core resource event queue was backpressured; response is incomplete."
                            .into(),
                    );
                    return Ok(complete());
                }
                for event in events {
                    if report.accept(event)? {
                        return Ok(complete());
                    }
                }
                Ok(wait())
            }
            WorkflowEvent::StepResult { .. } => Err(PluginError::InvalidRequest),
        }
    }

    fn report_mut(&mut self, task_id: &str) -> &mut QueryReport {
        self.reports.entry(task_id.into()).or_default()
    }

    fn needs_user_action(
        &self,
    ) -> Result<&norishell_plugin_sdk::PluginWorkflowTaskSummary, PluginError> {
        self.latest
            .as_ref()
            .filter(|snapshot| snapshot.task.state == PluginWorkflowTaskState::NeedsUserAction)
            .map(|snapshot| &snapshot.task)
            .ok_or(PluginError::InvalidRequest)
    }

    fn active_task(&self) -> Result<&norishell_plugin_sdk::PluginWorkflowTaskSummary, PluginError> {
        self.latest
            .as_ref()
            .filter(|snapshot| !snapshot.task.state.is_terminal())
            .map(|snapshot| &snapshot.task)
            .ok_or(PluginError::InvalidRequest)
    }

    fn record_broker_error(&mut self, code: PluginApiErrorCode) {
        if let Some(snapshot) = &self.latest {
            self.report_mut(&snapshot.task.task_id.to_string()).error = Some(format!(
                "Core rejected the requested task control: {code:?}"
            ));
        }
    }

    fn document(
        &self,
        request_id: &str,
    ) -> Result<Vec<norishell_plugin_sdk::PluginRuntimeOutput>, PluginError> {
        let needs_continue = self.latest.as_ref().is_some_and(|snapshot| {
            snapshot.task.state == PluginWorkflowTaskState::NeedsUserAction
        });
        let active = self
            .latest
            .as_ref()
            .is_some_and(|snapshot| !snapshot.task.state.is_terminal());
        let details = self.render_details();
        Ok(vec![output(
            request_id,
            "ui.document",
            &json!({"targetId": TARGET, "document": {
                "schemaVersion": 1,
                "rootNodeId": "serviceDemoDialog",
                "nodes": [
                    {"kind": "dialog", "nodeId": "serviceDemoDialog", "title": "Public service demo", "description": "Read non-secret rust-lang/rust metadata through the Core-approved GitHub HTTPS broker.", "triggerLabel": "Service demo", "closeLabel": "Close service demo", "children": ["intro", "query", "continue", "refresh", "cancel", "details"]},
                    {"kind": "text", "nodeId": "intro", "text": "No request runs until you explicitly continue the Core workflow approval. This Wasm guest has no direct network access.", "style": "caption", "tone": "neutral"},
                    {"kind": "button", "nodeId": "query", "actionId": QUERY_ACTION, "label": "Query GitHub", "icon": "link", "variant": "primary", "disabled": active},
                    {"kind": "button", "nodeId": "continue", "actionId": CONTINUE_ACTION, "label": "Continue query", "icon": "play", "variant": "secondary", "disabled": !needs_continue},
                    {"kind": "button", "nodeId": "refresh", "actionId": REFRESH_ACTION, "label": "Refresh", "icon": "refresh", "variant": "secondary", "disabled": false},
                    {"kind": "button", "nodeId": "cancel", "actionId": CANCEL_ACTION, "label": "Cancel query", "icon": "stop", "variant": "secondary", "disabled": !active},
                    {"kind": "code", "nodeId": "details", "text": details, "language": "text", "wrap": true}
                ]
            }}),
        )?])
    }

    fn render_details(&self) -> String {
        let Some(snapshot) = &self.latest else {
            return "No query task yet. Query GitHub starts a workflow; it does not grant network access.".into();
        };
        let task_id = snapshot.task.task_id.to_string();
        let state = format!("Task: {task_id}\nState: {:?}", snapshot.task.state);
        let Some(report) = self.reports.get(&task_id) else {
            return format!(
                "{state}\nNo in-memory response report is available. Refresh after the Core workflow advances."
            );
        };
        if let Some(error) = &report.error {
            let status = report
                .status
                .map(|value| format!("\nHTTP status: {value}"))
                .unwrap_or_default();
            return format!("{state}{status}\nError: {error}");
        }
        if let Some(repository) = &report.result {
            return format!(
                "{state}\nHTTP status: {}\nRepository: {} ({})\nDescription: {}\nStars: {}\nValidated response bytes: {}",
                report.status.unwrap_or_default(),
                repository.full_name,
                repository.name,
                repository.description,
                repository.stars,
                report.body.len(),
            );
        }
        let status = report
            .status
            .map(|value| format!("HTTP status: {value}\n"))
            .unwrap_or_default();
        format!(
            "{state}\n{status}Waiting for Core-owned HTTP resource events. Refresh displays the validated result after the task reaches a terminal state."
        )
    }
}

impl QueryReport {
    /// Returns true after a terminal resource fact; Core then cancels/closes the task-owned
    /// resource before marking the workflow terminal.
    fn accept(&mut self, event: PluginApiResourceEvent) -> Result<bool, PluginError> {
        match event.kind {
            PluginApiResourceEventKind::Network { event } => match event {
                PluginNetworkEvent::Opened { .. } => Ok(false),
                PluginNetworkEvent::HttpResponse { status, headers } => {
                    self.status = Some(status);
                    self.is_json = headers.iter().any(|header| {
                        header.name.eq_ignore_ascii_case("content-type")
                            && header
                                .value
                                .to_ascii_lowercase()
                                .contains("application/json")
                    });
                    Ok(false)
                }
                PluginNetworkEvent::Data { data_base64 } => {
                    if self.status.is_none() {
                        self.error = Some("HTTP data arrived before an HTTP status.".into());
                        return Ok(true);
                    }
                    let chunk = BASE64
                        .decode(data_base64)
                        .map_err(|_| PluginError::InvalidRequest)?;
                    if self.body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                        self.error = Some(format!(
                            "Response exceeded the {} byte validation limit.",
                            MAX_RESPONSE_BYTES
                        ));
                        return Ok(true);
                    }
                    self.body.extend_from_slice(&chunk);
                    Ok(false)
                }
                PluginNetworkEvent::Closed {} => {
                    self.finish_response();
                    Ok(true)
                }
                PluginNetworkEvent::Error { code } => {
                    self.error = Some(format!("Core network resource failed: {code:?}"));
                    Ok(true)
                }
                PluginNetworkEvent::Datagram { .. } => Err(PluginError::InvalidRequest),
            },
            PluginApiResourceEventKind::Cancelled {} => {
                self.error =
                    Some("Core cancelled the network resource before a complete response.".into());
                Ok(true)
            }
            _ => Err(PluginError::InvalidRequest),
        }
    }

    fn finish_response(&mut self) {
        let Some(status) = self.status else {
            self.error = Some("HTTP resource closed without an HTTP status.".into());
            return;
        };
        if status != 200 {
            self.error = Some(format!(
                "GitHub returned HTTP {status}; the payload was not treated as repository metadata."
            ));
            return;
        }
        if !self.is_json {
            self.error =
                Some("HTTP 200 response did not declare an application/json content type.".into());
            return;
        }
        let parsed = parse_repository(&self.body);
        match parsed {
            Ok(repository) => self.result = Some(repository),
            Err(message) => self.error = Some(message),
        }
    }
}

fn parse_repository(bytes: &[u8]) -> Result<Repository, String> {
    let document: Value =
        serde_json::from_slice(bytes).map_err(|_| "Response body is not valid JSON.".to_owned())?;
    let object = document
        .as_object()
        .ok_or_else(|| "Response JSON must be an object.".to_owned())?;
    let bounded_string = |key: &str, maximum: usize| -> Result<String, String> {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= maximum)
            .map(str::to_owned)
            .ok_or_else(|| format!("Response JSON field {key:?} is missing or invalid."))
    };
    let description = match object.get("description") {
        Some(Value::String(value)) if value.len() <= 4 * 1024 => value.clone(),
        Some(Value::Null) | None => "(no description)".into(),
        _ => return Err("Response JSON field \"description\" is invalid.".into()),
    };
    let stars = object
        .get("stargazers_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            "Response JSON field \"stargazers_count\" is missing or invalid.".to_owned()
        })?;
    Ok(Repository {
        full_name: bounded_string("full_name", 256)?,
        name: bounded_string("name", 256)?,
        description,
        stars,
    })
}

fn step(id: &str, operation: PluginApiOperation) -> WorkflowResponse {
    WorkflowResponse {
        step_id: Some(id.into()),
        call: Some(norishell_plugin_sdk::PluginApiCall {
            call_id: id.into(),
            operation,
        }),
        complete: false,
    }
}

fn wait() -> WorkflowResponse {
    WorkflowResponse {
        step_id: None,
        call: None,
        complete: false,
    }
}

fn complete() -> WorkflowResponse {
    WorkflowResponse {
        step_id: None,
        call: None,
        complete: true,
    }
}

#[cfg(target_arch = "wasm32")]
norishell_plugin_sdk::export_plugin!(ServiceDemo);

#[cfg(test)]
mod tests {
    use super::*;
    use norishell_plugin_sdk::{PluginNetworkProtocol, WireSequence};

    #[test]
    fn initial_document_passes_the_host_dialog_contract() {
        let mut plugin = ServiceDemo::default();
        let outputs = plugin.handle(PluginHostRequest {
            protocol_major: norishell_plugin_sdk::PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: norishell_plugin_sdk::PLUGIN_PROTOCOL_MINOR,
            request_id: "00000000-0000-4000-8000-000000000101".into(),
            kind: PluginHostMessageKind::Initialize,
            payload_json: json!({"pluginId": "com.norishell.service-demo", "locale": "en", "storage": null}).to_string(),
        }).expect("initialize service demo");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].kind, "ui.document");
        let template: norishell_core_api::PluginUiTemplate =
            serde_json::from_str(&outputs[0].payload_json).expect("typed initial document");
        assert_eq!(template.target_id.as_str(), TARGET);
        norishell_plugin_platform::validate_plugin_dialog_document(&template.document)
            .expect("initial header dialog must pass production validation");
    }

    fn event(kind: PluginNetworkEvent) -> PluginApiResourceEvent {
        PluginApiResourceEvent {
            sequence: WireSequence::new(1),
            kind: PluginApiResourceEventKind::Network { event: kind },
        }
    }

    #[test]
    fn parses_a_bounded_json_repository_response_only_after_close() {
        let mut report = QueryReport::default();
        assert!(
            !report
                .accept(event(PluginNetworkEvent::Opened {
                    protocol: PluginNetworkProtocol::Http,
                    peer_address: None,
                }))
                .unwrap()
        );
        assert!(
            !report
                .accept(event(PluginNetworkEvent::HttpResponse {
                    status: 200,
                    headers: vec![PluginNetworkHeader {
                        name: "Content-Type".into(),
                        value: "application/json; charset=utf-8".into()
                    }],
                }))
                .unwrap()
        );
        assert!(!report.accept(event(PluginNetworkEvent::Data {
            data_base64: BASE64.encode(br#"{"name":"rust","full_name":"rust-lang/rust","description":"A language","stargazers_count":123}"#),
        })).unwrap());
        assert!(report.accept(event(PluginNetworkEvent::Closed {})).unwrap());
        assert_eq!(report.result.as_ref().unwrap().stars, 123);
    }

    #[test]
    fn rejects_an_oversized_or_non_json_response() {
        let mut report = QueryReport::default();
        assert!(
            !report
                .accept(event(PluginNetworkEvent::HttpResponse {
                    status: 200,
                    headers: vec![PluginNetworkHeader {
                        name: "content-type".into(),
                        value: "application/json".into(),
                    }],
                }))
                .unwrap()
        );
        assert!(
            report
                .accept(event(PluginNetworkEvent::Data {
                    data_base64: BASE64.encode(vec![b'x'; MAX_RESPONSE_BYTES + 1]),
                }))
                .unwrap()
        );
        assert!(report.error.as_deref().unwrap().contains("exceeded"));

        let mut report = QueryReport {
            status: Some(200),
            is_json: false,
            ..Default::default()
        };
        report.finish_response();
        assert!(report.error.as_deref().unwrap().contains("content type"));
    }
}
