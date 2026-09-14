//! Protocol-13 isolated-surface example.
//!
//! The Wasm guest contributes only the header action and asks Core to open the owned HTML asset.
//! The HTML receives a Core-owned MessagePort at runtime; neither surface has direct network,
//! Tauri, filesystem, Vault, terminal, or secret access.

use norishell_plugin_sdk::{
    Plugin, PluginError, PluginHostMessageKind, PluginHostRequest, PluginRuntimeOutput,
    export_plugin, output, payload,
};
use serde_json::{Value, json};

const TARGET: &str = "app.header.actions";
const OPEN_ACTION: &str = "isolated-demo:open";
const SURFACE_ID: &str = "isolated-demo";

#[derive(Default)]
struct IsolatedDemo;

impl Plugin for IsolatedDemo {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        let body: Value = payload(&request)?;
        match request.kind {
            PluginHostMessageKind::Initialize => Ok(vec![document(&request.request_id, &body)?]),
            PluginHostMessageKind::UiAction
                if body.get("actionId").and_then(Value::as_str) == Some(OPEN_ACTION) =>
            {
                // A UI action must refresh its single document for the same target before
                // asking Core to open the package-owned isolated surface.
                Ok(vec![
                    document(&request.request_id, &body)?,
                    output(
                        &request.request_id,
                        // This is the existing contribution-parser kind for a Core-owned
                        // isolated WebView surface. The asset itself is resolved from the
                        // verified package.
                        "ui.webview.open",
                        &json!({
                            "surfaceId": SURFACE_ID,
                            "title": "NoriShell Isolated API Demo",
                            "width": 760,
                            "height": 620,
                        }),
                    )?,
                ])
            }
            _ => Err(PluginError::InvalidRequest),
        }
    }
}

fn document(request_id: &str, payload: &Value) -> Result<PluginRuntimeOutput, PluginError> {
    let zh = payload.get("locale").and_then(Value::as_str) == Some("zh-CN");
    let (title, description, open) = if zh {
        (
            "隔离 API 示例",
            "在独立 WebView 中演示 Core 管理的描述、私有存储和受保护网络请求。",
            "打开隔离示例",
        )
    } else {
        (
            "Isolated API demo",
            "Open a separate WebView that demonstrates Core-managed description, private storage, and protected network requests.",
            "Open isolated demo",
        )
    };
    output(
        request_id,
        "ui.document",
        &json!({
            "targetId": TARGET,
            "document": {
                "schemaVersion": 1,
                "rootNodeId": "isolatedDemoDialog",
                "nodes": [
                    {
                        "kind": "dialog",
                        "nodeId": "isolatedDemoDialog",
                        "title": title,
                        "description": description,
                        "triggerLabel": open,
                        "closeLabel": zh.then_some("关闭示例").unwrap_or("Close demo"),
                        "children": ["isolatedDemoIntro", "isolatedDemoOpen"]
                    },
                    {
                        "kind": "text",
                        "nodeId": "isolatedDemoIntro",
                        "text": zh.then_some("页面不直接访问网络或 Tauri；所有请求通过 Core 提供的受限端口。")
                            .unwrap_or("The page has no direct network or Tauri access; every request uses the constrained Core port."),
                        "style": "caption",
                        "tone": "neutral"
                    },
                    {
                        "kind": "button",
                        "nodeId": "isolatedDemoOpen",
                        "actionId": OPEN_ACTION,
                        "label": open,
                        "icon": "info",
                        "variant": "primary",
                        "disabled": false
                    }
                ]
            }
        }),
    )
}

export_plugin!(IsolatedDemo);
