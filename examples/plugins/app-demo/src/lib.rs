//! Explicit app integration and file commands use the same typed, fenced broker as UI actions.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use norishell_plugin_sdk::*;
use serde_json::{Value, json};

const TARGET: &str = "app.header.actions";
#[derive(Default)]
struct AppDemo {
    root: Option<String>,
    text: String,
}
impl Plugin for AppDemo {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let body: Value = payload(&request)?;
        match request.kind {
            PluginHostMessageKind::Initialize => self.document(&request.request_id),
            PluginHostMessageKind::UiAction => {
                let operation = match body["actionId"].as_str() {
                    Some("app-demo.register") => PluginApiOperation::AppRegister {
                        registration: PluginAppRegistration {
                            commands: vec![PluginAppCommand {
                                id: "open-text".into(),
                                label: "Open text file".into(),
                                target_id: TARGET
                                    .parse()
                                    .map_err(|_| PluginError::InvalidRequest)?,
                                action_id: "app-demo.pick"
                                    .parse()
                                    .map_err(|_| PluginError::InvalidRequest)?,
                                page_id: None,
                                shortcut: Some("Alt+Shift+O".into()),
                                file_extensions: vec!["txt".into(), "md".into(), "log".into()],
                            }],
                            statuses: vec![PluginAppStatus {
                                id: "ready".into(),
                                text: "Text file command ready".into(),
                            }],
                        },
                    },
                    Some("app-demo.pick") if self.root.is_none() => PluginApiOperation::FilePick {
                        picker_kind: PluginFilePickerKind::File,
                        access: PluginFileAccessRequest {
                            read: true,
                            write: false,
                            list: false,
                            rename: false,
                            remove: false,
                            recursive_remove: false,
                            watch: false,
                        },
                    },
                    Some("app-demo.read") => PluginApiOperation::File {
                        operation: PluginFileOperation::Read {
                            root_handle: self.root.clone().ok_or(PluginError::InvalidRequest)?,
                            relative_path: String::new(),
                            offset: 0,
                        },
                    },
                    Some("app-demo.close") => PluginApiOperation::ResourceClose {
                        handle: self.root.clone().ok_or(PluginError::InvalidRequest)?,
                    },
                    Some("app-demo.notify") => PluginApiOperation::AppNotify {
                        notification: PluginAppNotification {
                            id: "demo-notice".into(),
                            text: "App integration demo notification".into(),
                        },
                    },
                    Some("app-demo.navigate") => PluginApiOperation::AppNavigate {
                        destination: PluginAppNavigation::App {
                            path: "/plugins".into(),
                        },
                    },
                    _ => return Err(PluginError::InvalidRequest),
                };
                Ok(vec![api_request(
                    &request.request_id,
                    "app-demo.control",
                    operation,
                )?])
            }
            PluginHostMessageKind::BrokerResult => {
                let reply: PluginApiReply = serde_json::from_value(body["result"]["reply"].clone())
                    .map_err(|_| PluginError::InvalidRequest)?;
                match reply.outcome {
                    PluginApiOutcome::Completed {
                        value: PluginApiValue::FilePicked { root_handle, label },
                    } => {
                        self.root = Some(root_handle);
                        self.text = format!(
                            "Selected: {label}. Select Read preview to read up to one bounded chunk."
                        );
                    }
                    PluginApiOutcome::Completed {
                        value:
                            PluginApiValue::File {
                                result:
                                    PluginFileResult::Read {
                                        data_base64, eof, ..
                                    },
                            },
                    } => {
                        let data = STANDARD
                            .decode(data_base64)
                            .map_err(|_| PluginError::InvalidRequest)?;
                        match std::str::from_utf8(&data) {
                            Ok(text) => {
                                self.text = text.chars().take(2048).collect();
                                if !eof || text.chars().count() > 2048 {
                                    self.text.push_str("\n[Preview truncated]");
                                }
                            }
                            Err(_) => self.text = "The selected file is not UTF-8 text.".into(),
                        }
                    }
                    PluginApiOutcome::Completed {
                        value: PluginApiValue::Closed { .. },
                    } => {
                        self.root = None;
                        self.text = "File handle closed.".into();
                    }
                    PluginApiOutcome::Completed { .. } => {
                        self.text = "Core accepted the explicit action.".into()
                    }
                    PluginApiOutcome::Failed { code } => {
                        if matches!(
                            code,
                            PluginApiErrorCode::Revoked | PluginApiErrorCode::NotFound
                        ) {
                            self.root = None;
                        }
                        self.text = format!("Core rejected the action: {code:?}")
                    }
                }
                self.document(&request.request_id)
            }
            _ => Err(PluginError::InvalidRequest),
        }
    }
}
impl AppDemo {
    fn document(&self, request_id: &str) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let mut nodes = vec![json!({
            "kind":"dialog", "nodeId":"appDemo", "title":"App integration demo", "description":"Register a file command, select a file through Core, and preview its text.",
            "triggerLabel":"App demo", "closeLabel":"Close app demo", "children":["register","pick","read","close","notify","navigate","result"]
        })];
        for (id, label, disabled) in [
            ("register", "Register app command", false),
            ("pick", "Select text file", self.root.is_some()),
            ("read", "Read preview", self.root.is_none()),
            ("close", "Close file handle", self.root.is_none()),
            ("notify", "Show notification", false),
            ("navigate", "Go to plugins", false),
        ] {
            nodes.push(json!({"kind":"button", "nodeId":id, "actionId":format!("app-demo.{id}"), "label":label, "variant":"secondary", "disabled":disabled}));
        }
        nodes.push(json!({"kind":"code", "nodeId":"result", "text":if self.text.is_empty() { "No file has been selected." } else { &self.text }, "language":"text", "wrap":true}));
        Ok(vec![output(
            request_id,
            "ui.document",
            &json!({"targetId":TARGET,"document":{"schemaVersion":1,"rootNodeId":"appDemo","nodes":nodes}}),
        )?])
    }
}
export_plugin!(AppDemo);
