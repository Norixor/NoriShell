//! Runs official guest Wasm through the production runtime and UI validator.
//! This is a protocol gate, not SSH or native-page acceptance.

use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
    PluginUiTemplate,
};
use norishell_plugin_platform::{RuntimeLimits, execute_wasm, validate_plugin_ui_document};
use serde_json::json;

#[path = "../../../examples/plugins/official-operations/src/catalog.rs"]
#[allow(dead_code, unexpected_cfgs)]
mod catalog;

fn main() {
    let (path, fuel, selected) = arguments();
    let module = std::fs::read(path).expect("read guest Wasm");
    let apps = selected.map_or_else(|| catalog::App::ALL.to_vec(), |app| vec![app]);
    let mut checked = 0;
    for app in &apps {
        for locale in ["en", "zh-CN"] {
            let request = PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: "00000000-0000-4000-8000-000000000001".to_owned(),
                kind: PluginHostMessageKind::Initialize,
                payload_json: json!({"pluginId":format!("org.norishell.{}",app.key()),"locale":locale,"storage":null}).to_string(),
            };
            let mut outputs = Vec::new();
            let report = execute_wasm(
                &module,
                &request,
                |output| {
                    outputs.push(output);
                    Ok(())
                },
                RuntimeLimits::default(),
            )
            .unwrap_or_else(|error| panic!("{} {locale}: {error}", app.key()));
            assert_eq!(outputs.len(), if app.key() == "processes" { 2 } else { 1 });
            for output in &outputs {
                assert_eq!(output.kind, "ui.document");
                let template: PluginUiTemplate =
                    serde_json::from_str(&output.payload_json).expect("strict template schema");
                validate_plugin_ui_document(&template.document).expect("template document bounds");
                validate_default_action_fields(&template);
                if template.target_id.as_str() == "terminal.footer" {
                    let refresh = template.auto_refresh.as_ref().expect("CPU periodic hook");
                    assert_eq!(refresh.action_id.as_str(), "processes:cpuUsage");
                    assert_eq!(refresh.interval_ms, 5000);
                }
            }
            let page = outputs
                .iter()
                .find(|output| output.kind == "ui.document")
                .expect("page output");
            let page: PluginUiTemplate =
                serde_json::from_str(&page.payload_json).expect("strict public page schema");
            validate_plugin_ui_document(&page.document).expect("bounded page document");
            assert_eq!(
                page.on_open_action_id.as_ref().map(|id| id.as_str()),
                Some(format!("{}:open", app.key()).as_str())
            );
            let open_request = PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: "00000000-0000-4000-8000-000000000099".to_owned(),
                kind: PluginHostMessageKind::UiAction,
                payload_json: json!({"actionId":format!("{}:open",app.key()),"targetId":app.target(),"locale":locale,"fields":[],
                    "terminalMetadata":{"terminalHandle":"current-session","kind":"ssh","state":"running"}}).to_string(),
            };
            let mut opened = Vec::new();
            execute_wasm(
                &module,
                &open_request,
                |output| {
                    opened.push(output);
                    Ok(())
                },
                RuntimeLimits::default(),
            )
            .expect("visible template initialization");
            assert!(
                opened
                    .iter()
                    .all(|output| matches!(output.kind.as_str(), "ui.document" | "ui.state"))
            );
            let opened: PluginUiTemplate = serde_json::from_str(
                &opened
                    .iter()
                    .find(|output| output.kind == "ui.document")
                    .expect("opened template")
                    .payload_json,
            )
            .expect("strict opened template schema");
            validate_plugin_ui_document(&opened.document).expect("opened template bounds");
            assert_eq!(opened.on_open_action_id, page.on_open_action_id);
            assert!(
                !serde_json::to_value(&opened.document).unwrap()["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|node| node["nodeId"] == "sessionHint"),
                "{} did not consume current session",
                app.key()
            );

            assert_eq!(
                page.target_id.as_str(),
                if app.key() == "tasks" {
                    "terminal.floating"
                } else {
                    "terminal.tools"
                }
            );
            println!(
                "{} {locale}: {} nodes; {} fuel",
                app.key(),
                page.document.nodes.len(),
                report.fuel_consumed
            );
            checked += 1;
        }
    }
    if supports(&apps, "processes") {
        let metadata = json!({"terminalHandle":"current-session","kind":"ssh","state":"running"});
        let request = PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            request_id: "00000000-0000-4000-8000-000000000072".into(),
            kind: PluginHostMessageKind::UiAction,
            payload_json: json!({"locale":"en","actionId":"processes:cpuUsage","targetId":"terminal.footer","terminalMetadata":metadata,"fields":[]}).to_string(),
        };
        let mut outputs = Vec::new();
        execute_wasm(
            &module,
            &request,
            |output| {
                outputs.push(output);
                Ok(())
            },
            RuntimeLimits::default(),
        )
        .expect("CPU sample request");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].kind, "remote.operation.request");
        let operation: norishell_core_api::PluginRemoteOperationRequest =
            serde_json::from_str(&outputs[0].payload_json).expect("CPU Core DTO");
        let operation: norishell_plugin_platform::operations::RemoteOperation =
            serde_json::from_str(&operation.operation_json).expect("CPU fixed operation");
        let plan = norishell_plugin_platform::operations::plan(&operation).expect("CPU fixed plan");
        assert_eq!(
            plan.class,
            norishell_plugin_platform::operations::OperationClass::ReadOnly
        );
        assert_eq!(
            plan.approval,
            norishell_plugin_platform::operations::ApprovalRequirement::None
        );
        for basis_points in [0, 1234, 10000] {
            let request = PluginHostRequest {
                protocol_major: PLUGIN_PROTOCOL_MAJOR,
                protocol_minor: PLUGIN_PROTOCOL_MINOR,
                request_id: "00000000-0000-4000-8000-000000000073".into(),
                kind: PluginHostMessageKind::BrokerResult,
                payload_json: json!({"locale":"zh-CN","actionId":"processes:cpuUsage","targetId":"terminal.footer","terminalMetadata":metadata,"fields":[],"state":{"valueJson":"{}"},"result":{"kind":"operation","state":"succeeded","stdout":"","stderr":"","data":{"kind":"cpuUsage","basisPoints":basis_points,"sampleDurationMs":200}}}).to_string(),
            };
            let mut outputs = Vec::new();
            let report = execute_wasm(
                &module,
                &request,
                |output| {
                    outputs.push(output);
                    Ok(())
                },
                RuntimeLimits::default(),
            )
            .expect("CPU sample result");
            let output = outputs
                .iter()
                .find(|output| output.kind == "ui.document")
                .expect("CPU footer");
            let template: PluginUiTemplate =
                serde_json::from_str(&output.payload_json).expect("CPU footer schema");
            validate_plugin_ui_document(&template.document).expect("CPU footer document");
            assert_eq!(template.target_id.as_str(), "terminal.footer");
            println!(
                "CPU footer {basis_points} basis points: {} fuel",
                report.fuel_consumed
            );
        }
    }
    if supports(&apps, "network") {
        for locale in ["en", "zh-CN"] {
            for status in [200, 404] {
                let stdout = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: text/plain\r\n\r\nfixture\r\n\nNORISHELL_HTTP_METRICS status={status} remote_ip=127.0.0.1 dns=0.001 connect=0.002 tls=0.000 first_byte=0.003 total=0.004\n"
                );
                let request = PluginHostRequest {
                    protocol_major: PLUGIN_PROTOCOL_MAJOR,
                    protocol_minor: PLUGIN_PROTOCOL_MINOR,
                    request_id: "00000000-0000-4000-8000-000000000071".into(),
                    kind: PluginHostMessageKind::BrokerResult,
                    payload_json: json!({
                        "locale": locale, "actionId": "network:http", "fields": [],
                        "terminalMetadata": {"terminalHandle":"deadbeef","kind":"ssh","state":"running","label":"Fixture"},
                        "state": {"valueJson":"{}"},
                        "result": {"kind":"operation","state":"succeeded","exitStatus":0,"durationMs":4,"stdout":stdout,"stderr":"","truncated":false}
                    }).to_string(),
                };
                let mut outputs = Vec::new();
                let report = execute_wasm(
                    &module,
                    &request,
                    |output| {
                        outputs.push(output);
                        Ok(())
                    },
                    RuntimeLimits::default(),
                )
                .expect("HTTP callback runtime");
                let output = outputs
                    .iter()
                    .find(|output| output.kind == "ui.document")
                    .expect("HTTP callback document");
                let template: PluginUiTemplate =
                    serde_json::from_str(&output.payload_json).expect("HTTP callback schema");
                validate_plugin_ui_document(&template.document)
                    .expect("HTTP CRLF document admission");
                assert!(output.payload_json.contains("httpTimings"));
                assert!(output.payload_json.contains(&format!("HTTP {status}")));
                println!(
                    "HTTP {status} {locale} CRLF callback: {} fuel",
                    report.fuel_consumed
                );
            }
        }
    }
    if supports(&apps, "docker") {
        for length in [1024, 16 * 1024, 32 * 1024] {
            let request=PluginHostRequest {
            protocol_major:PLUGIN_PROTOCOL_MAJOR,protocol_minor:PLUGIN_PROTOCOL_MINOR,
            request_id:"00000000-0000-4000-8000-000000000002".to_owned(),kind:PluginHostMessageKind::BrokerResult,
            payload_json:json!({"locale":"zh-CN","actionId":"docker:logs","fields":[],"state":{"valueJson":"{}"},
                "authorizedHosts":(0..100).map(|i|json!({"hostHandle":format!("00000000-0000-4000-8000-{i:012}"),"label":format!("Server {i}"),"address":"test.invalid","port":22,"username":"test"})).collect::<Vec<_>>(),"result":{"kind":"operation","state":"succeeded","exitStatus":0,"durationMs":50,
                    "stdout":"x".repeat(length),"stderr":"","truncated":false}}).to_string(),
        };
            let mut outputs = Vec::new();
            let mut limits = RuntimeLimits::default();
            if let Some(fuel) = fuel {
                limits.fuel = fuel;
            }
            let report = execute_wasm(
                &module,
                &request,
                |output| {
                    outputs.push(output);
                    Ok(())
                },
                limits,
            )
            .unwrap_or_else(|error| panic!("callback {length} bytes: {error}"));
            let output = outputs
                .iter()
                .find(|output| output.kind == "ui.document")
                .expect("callback document");
            let template: PluginUiTemplate =
                serde_json::from_str(&output.payload_json).expect("callback schema");
            validate_plugin_ui_document(&template.document).expect("callback document bounds");
            println!(
                "callback {length} bytes, 100 hosts: {} fuel, {} ms",
                report.fuel_consumed, report.elapsed_milliseconds
            );
        }
    }
    println!("Validated {checked} localized guest pages through the production Wasm runtime.");
    for (action, fields, state, expected) in [
        (
            "logs:refresh",
            json!([{"fieldId":"path","value":"/var/log/a\n/var/log/b"}]),
            json!({}),
            "resource.operation.request",
        ),
        (
            "tunnels:start",
            json!([{"fieldId":"direction","value":"remote"}]),
            json!({}),
            "resource.operation.request",
        ),
        (
            "tunnels:refresh",
            json!([]),
            json!({}),
            "resource.operation.request",
        ),
        (
            "disk:sftp",
            json!([{"fieldId":"path","value":"/tmp"}]),
            json!({}),
            "resource.operation.request",
        ),
        (
            "nginx:edit",
            json!([{"fieldId":"path","value":"/etc/nginx/nginx.conf"}]),
            json!({}),
            "resource.operation.request",
        ),
        (
            "docker:terminal",
            json!([{"fieldId":"resource","value":"a".repeat(64)}]),
            json!({}),
            "resource.operation.request",
        ),
    ] {
        if !supports(&apps, action.split(':').next().expect("action app")) {
            continue;
        }
        let mut fields = fields.as_array().unwrap().clone();
        fields.push(json!({"fieldId":"host","value":"deadbeef"}));
        let request=PluginHostRequest { protocol_major:PLUGIN_PROTOCOL_MAJOR,protocol_minor:PLUGIN_PROTOCOL_MINOR,
            request_id:"00000000-0000-4000-8000-000000000003".into(),kind:PluginHostMessageKind::UiAction,
            payload_json:json!({"locale":"en","actionId":action,"terminalMetadata":{"terminalHandle":"deadbeef","kind":"ssh","state":"running","label":"Fixture","generation":"1","attachmentCount":1},"fields":fields,"state":{"valueJson":state.to_string()}}).to_string() };
        let mut outputs = Vec::new();
        execute_wasm(
            &module,
            &request,
            |output| {
                outputs.push(output);
                Ok(())
            },
            RuntimeLimits::default(),
        )
        .expect(action);
        let output = outputs
            .iter()
            .find(|output| output.kind == expected)
            .expect(action);
        if expected == "resource.operation.request" {
            serde_json::from_str::<norishell_core_api::PluginResourceOperationRequest>(
                &output.payload_json,
            )
            .expect("strict resource DTO");
        }

        println!("{action}: strict Core request passed");
    }
    if supports(&apps, "logs") {
        let files=(0..8).map(|i|json!({"path":format!("/log/{i}"),"text":"line\n".repeat(205),"nextOffset":"1025","totalSize":"9000","reset":false})).collect::<Vec<_>>();
        let request=PluginHostRequest {protocol_major:PLUGIN_PROTOCOL_MAJOR,protocol_minor:PLUGIN_PROTOCOL_MINOR,
        request_id:"00000000-0000-4000-8000-000000000004".into(),kind:PluginHostMessageKind::BrokerResult,
        payload_json:json!({"locale":"zh-CN","actionId":"logs:refresh","fields":[{"fieldId":"host","value":"deadbeef"}],"state":{"valueJson":"{}"},"result":{"kind":"resource","value":{"kind":"logsRead","files":files,"totalBytes":8200}}}).to_string()};
        let mut outputs = Vec::new();
        let report = execute_wasm(
            &module,
            &request,
            |output| {
                outputs.push(output);
                Ok(())
            },
            RuntimeLimits::default(),
        )
        .expect("eight logs callback");
        for output in outputs {
            if output.kind == "ui.document" {
                let template: PluginUiTemplate =
                    serde_json::from_str(&output.payload_json).unwrap();
                validate_plugin_ui_document(&template.document).expect("eight logs UI bounds");
            }
        }
        println!("eight logs callback: {} fuel", report.fuel_consumed);
    }
    if supports(&apps, "tasks") {
        let storage = json!({"records":(0..100).map(|i|json!({"id":format!("record-{i}"),"name":format!("Task {i}"),"group":"QA","hostMatch":"fixture-key","values":{"command":"echo fixture; ".repeat(22)}})).collect::<Vec<_>>()});
        assert!(storage.to_string().len() < 48 * 1024);
        let request=PluginHostRequest {protocol_major:PLUGIN_PROTOCOL_MAJOR,protocol_minor:PLUGIN_PROTOCOL_MINOR,
        request_id:"00000000-0000-4000-8000-000000000005".into(),kind:PluginHostMessageKind::BrokerResult,
        payload_json:json!({"locale":"en","targetId":"terminal.floating","actionId":"tasks:run","terminalMetadata":{"terminalHandle":"deadbeef","kind":"ssh","state":"running","label":"Fixture","connectionKey":"fixture-key"},
            "storage":{"valueJson":storage.to_string()},"state":{"valueJson":json!({"host":"deadbeef","result":{"kind":"operation","stdout":"x".repeat(32*1024)}}).to_string()},
            "result":{"kind":"operation","state":"succeeded","exitStatus":0,"stdout":"x".repeat(32*1024),"stderr":""}}).to_string()};
        let mut outputs = Vec::new();
        let report = execute_wasm(
            &module,
            &request,
            |output| {
                outputs.push(output);
                Ok(())
            },
            RuntimeLimits::default(),
        )
        .expect("100 presets plus maximum output");
        for output in outputs {
            if output.kind == "ui.document" {
                let template: PluginUiTemplate =
                    serde_json::from_str(&output.payload_json).unwrap();
                validate_plugin_ui_document(&template.document).expect("presets UI bounds");
            }
        }
        println!(
            "100 presets plus maximum output: {} fuel",
            report.fuel_consumed
        );
    }
    if let Some(app) = selected {
        verify_rejected_foreign_identity(&module, app);
    }
}

fn arguments() -> (std::ffi::OsString, Option<u64>, Option<catalog::App>) {
    let mut values = std::env::args_os();
    let _program = values.next();
    let path = values
        .next()
        .expect("usage: verify_official_operations <guest.wasm> [fuel] [--plugin <suffix>]");
    let mut fuel = None;
    let mut selected = None;
    while let Some(value) = values.next() {
        if value == "--plugin" {
            let key = values
                .next()
                .expect("--plugin requires an official plugin suffix");
            let key = key.to_string_lossy();
            assert!(selected.is_none(), "--plugin may appear only once");
            selected = Some(
                catalog::App::parse(&key)
                    .unwrap_or_else(|| panic!("unknown official plugin suffix: {key}")),
            );
        } else if fuel.is_none() {
            fuel = Some(
                value
                    .to_string_lossy()
                    .parse::<u64>()
                    .expect("optional fuel budget"),
            );
        } else {
            panic!("unexpected argument: {}", value.to_string_lossy());
        }
    }
    (path, fuel, selected)
}

fn validate_default_action_fields(template: &PluginUiTemplate) {
    use norishell_core_api::{PluginUiFieldValue, PluginUiNode};
    let fields = template
        .document
        .nodes
        .iter()
        .filter_map(|node| {
            let (field_id, value) = match node {
                PluginUiNode::TextField {
                    field_id,
                    value,
                    disabled: false,
                    ..
                } => (field_id, value.clone()),
                PluginUiNode::Select {
                    field_id,
                    value,
                    disabled: false,
                    ..
                } => (field_id, value.clone().unwrap_or_default()),
                PluginUiNode::Checkbox {
                    field_id,
                    checked,
                    disabled: false,
                    ..
                }
                | PluginUiNode::Switch {
                    field_id,
                    checked,
                    disabled: false,
                    ..
                } => (field_id, checked.to_string()),
                _ => return None,
            };
            Some(PluginUiFieldValue {
                field_id: field_id.clone(),
                value,
            })
        })
        .collect::<Vec<_>>();
    for node in &template.document.nodes {
        if let PluginUiNode::Button {
            action_id,
            disabled: false,
            ..
        } = node
        {
            norishell_plugin_platform::validate_plugin_ui_action(
                &template.document,
                action_id,
                &fields,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "{} default fields block {}: {error:?}",
                    template.target_id.as_str(),
                    action_id.as_str()
                )
            });
        }
    }
}

fn supports(apps: &[catalog::App], key: &str) -> bool {
    apps.iter().any(|app| app.key() == key)
}

fn verify_rejected_foreign_identity(module: &[u8], selected: catalog::App) {
    let foreign = catalog::App::ALL
        .iter()
        .copied()
        .find(|app| *app != selected)
        .expect("a foreign official plugin");
    for (kind, payload) in [
        (
            PluginHostMessageKind::Initialize,
            json!({"pluginId": format!("org.norishell.{}", foreign.key()), "locale": "en"}),
        ),
        (
            PluginHostMessageKind::UiAction,
            json!({"actionId": format!("{}:refresh", foreign.key()), "locale": "en", "fields": [], "state": {"valueJson": "{}"}}),
        ),
    ] {
        let request = PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            request_id: "00000000-0000-4000-8000-000000000006".to_owned(),
            kind,
            payload_json: payload.to_string(),
        };
        let error = execute_wasm(module, &request, |_| Ok(()), RuntimeLimits::default())
            .expect_err("single-plugin guest must reject another plugin identity");
        println!("rejected {} identity: {error}", foreign.key());
    }
}
