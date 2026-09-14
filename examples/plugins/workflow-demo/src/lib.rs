//! A real asynchronous Core task. No polling loop or work scheduler runs in Wasm.
use norishell_plugin_sdk::*;
use serde_json::{Value, json};

#[derive(Default)]
struct WorkflowDemo {
    latest: Option<PluginWorkflowTaskSnapshot>,
    last_error: Option<PluginApiErrorCode>,
}

impl Plugin for WorkflowDemo {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        if request.kind == PluginHostMessageKind::WorkflowEvent {
            let event = workflow_event(&request)?;
            let response = advance(event.event)?;
            return Ok(vec![workflow_response(&request, &response)?]);
        }
        let body: Value = payload(&request)?;
        match request.kind {
            PluginHostMessageKind::Initialize => self.document(&request.request_id),
            PluginHostMessageKind::UiAction => {
                let operation = match body["actionId"].as_str() {
                    Some("workflow.start") => PluginApiOperation::TaskStart {
                        workflow_id: "inspect-delay-inspect".into(),
                        input_json: None,
                        file_scope_handles: vec![],
                    },
                    Some("workflow.refresh") => PluginApiOperation::TaskList {},
                    Some("workflow.cancel") => {
                        let task = &self
                            .latest
                            .as_ref()
                            .ok_or(PluginError::InvalidRequest)?
                            .task;
                        PluginApiOperation::TaskCancel {
                            task_id: task.task_id.clone(),
                            expected_revision: task.revision,
                        }
                    }
                    Some("workflow.resume") => {
                        let task = &self
                            .latest
                            .as_ref()
                            .ok_or(PluginError::InvalidRequest)?
                            .task;
                        PluginApiOperation::TaskResume {
                            task_id: task.task_id.clone(),
                            expected_revision: task.revision,
                        }
                    }
                    _ => return Err(PluginError::InvalidRequest),
                };
                Ok(vec![api_request(
                    &request.request_id,
                    "workflow.control",
                    operation,
                )?])
            }
            PluginHostMessageKind::BrokerResult => {
                let reply: PluginApiReply = serde_json::from_value(body["result"]["reply"].clone())
                    .map_err(|_| PluginError::InvalidRequest)?;
                match reply.outcome {
                    PluginApiOutcome::Completed { value } => {
                        self.last_error = None;
                        match value {
                            PluginApiValue::Task { snapshot } => self.latest = Some(snapshot),
                            PluginApiValue::Tasks { mut snapshots } => {
                                snapshots.sort_by_key(|snapshot| snapshot.task.created_at_unix_ms);
                                self.latest = snapshots.pop();
                            }
                            _ => return Err(PluginError::InvalidRequest),
                        }
                    }
                    PluginApiOutcome::Failed { code } => self.last_error = Some(code),
                }
                self.document(&request.request_id)
            }
            _ => Err(PluginError::InvalidRequest),
        }
    }
}

fn step(id: &str, operation: PluginApiOperation) -> WorkflowResponse {
    WorkflowResponse {
        step_id: Some(id.into()),
        call: Some(PluginApiCall {
            call_id: id.into(),
            operation,
        }),
        complete: false,
    }
}
fn advance(event: WorkflowEvent) -> Result<WorkflowResponse, PluginError> {
    match event {
        WorkflowEvent::Start { .. } => Ok(step("inspect-first", PluginApiOperation::Describe {})),
        WorkflowEvent::StepResult { step_id, reply } => {
            if !matches!(reply.outcome, PluginApiOutcome::Completed { .. }) {
                return Err(PluginError::HandlerFailed);
            }
            match step_id.as_str() {
                "inspect-first" => Ok(step(
                    "start-timer",
                    PluginApiOperation::TimerStart {
                        delay_ms: 3000,
                        interval_ms: None,
                    },
                )),
                "start-timer" => Ok(WorkflowResponse {
                    step_id: None,
                    call: None,
                    complete: false,
                }),
                "inspect-last" => Ok(WorkflowResponse {
                    step_id: None,
                    call: None,
                    complete: true,
                }),
                _ => Err(PluginError::InvalidRequest),
            }
        }
        WorkflowEvent::ResourceEvents { events, .. } => {
            if events
                .iter()
                .any(|event| matches!(event.kind, PluginApiResourceEventKind::TimerFired {}))
            {
                Ok(step("inspect-last", PluginApiOperation::Describe {}))
            } else {
                Ok(WorkflowResponse {
                    step_id: None,
                    call: None,
                    complete: false,
                })
            }
        }
    }
}

impl WorkflowDemo {
    fn document(&self, request_id: &str) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let pending = self.latest.as_ref().is_some_and(|snapshot| {
            snapshot.task.state == PluginWorkflowTaskState::NeedsUserAction
        });
        let active = self
            .latest
            .as_ref()
            .is_some_and(|snapshot| !snapshot.task.state.is_terminal());
        let mut details = self.latest.as_ref().map(|snapshot| json!({
            "taskId": snapshot.task.task_id, "state": snapshot.task.state, "steps": snapshot.steps,
            "outcomeUnknown": snapshot.task.outcome_unknown, "cleanupIncomplete": snapshot.task.cleanup_incomplete,
        }).to_string()).unwrap_or_else(|| "Start creates a Core task; refresh reads its real status. No task resumes after app restart.".into());
        if let Some(code) = &self.last_error {
            details = format!("Core rejected the last action: {code:?}\n{details}");
        }
        Ok(vec![output(
            request_id,
            "ui.document",
            &json!({"targetId":"app.header.actions", "document": {
                "schemaVersion":1,"rootNodeId":"workflowDemo","nodes":[
                    {"kind":"dialog","nodeId":"workflowDemo","title":"Workflow demo","description":"Three steps with a real asynchronous timer between them.","triggerLabel":"Workflow demo","closeLabel":"Close","children":["start","refresh","resume","cancel","status"]},
                    {"kind":"button","nodeId":"start","actionId":"workflow.start","label":"Start task","icon":"play","variant":"primary","disabled":false},
                    {"kind":"button","nodeId":"refresh","actionId":"workflow.refresh","label":"Refresh tasks","icon":"refresh","variant":"secondary","disabled":false},
                    {"kind":"button","nodeId":"resume","actionId":"workflow.resume","label":"Continue pending approval","icon":"play","variant":"secondary","disabled":!pending},
                    {"kind":"button","nodeId":"cancel","actionId":"workflow.cancel","label":"Cancel task","icon":"stop","variant":"secondary","disabled":!active},
                    {"kind":"code","nodeId":"status","text":details,"language":"json","wrap":true}
                ]
            }}),
        )?])
    }
}
#[cfg(target_arch = "wasm32")]
export_plugin!(WorkflowDemo);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timer_event_is_required_before_the_final_step() {
        let waiting = advance(WorkflowEvent::StepResult {
            step_id: "start-timer".into(),
            reply: PluginApiReply {
                call_id: "start-timer".into(),
                outcome: PluginApiOutcome::Completed {
                    value: PluginApiValue::TimerStarted {
                        handle: "timer".into(),
                    },
                },
            },
        })
        .unwrap();
        assert!(!waiting.complete && waiting.call.is_none());
        let next = advance(WorkflowEvent::ResourceEvents {
            handle: "timer".into(),
            events: vec![PluginApiResourceEvent {
                sequence: WireSequence::new(1),
                kind: PluginApiResourceEventKind::TimerFired {},
            }],
            backpressured: false,
        })
        .unwrap();
        assert_eq!(next.step_id.as_deref(), Some("inspect-last"));
    }
}
