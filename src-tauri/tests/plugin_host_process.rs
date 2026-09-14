#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use norishell_core_api::{PLUGIN_API_PROTOCOL_MINOR, PluginPageContribution, PluginSshSyncRequest};
use norishell_plugin_platform::validate_plugin_page_document;

const MAX_FRAME_BYTES: usize = 512 * 1024;

fn fixture_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (import "norishell.host" "emit" (func $emit (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "nvx_alloc") (param i32) (result i32) i32.const 1024)
            (func (export "nvx_handle") (param i32 i32) (result i32) i32.const 0)
        )"#,
    )
    .expect("compile fixture Wasm")
}

fn persistent_fixture_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (import "norishell.host" "emit" (func $emit (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "nvx_alloc") (param i32) (result i32) i32.const 1024)
            (func (export "nvx_dealloc") (param i32 i32))
            (func (export "nvx_handle") (param i32 i32) (result i32) i32.const 0)
        )"#,
    )
    .expect("compile persistent fixture Wasm")
}

fn write_frame(writer: &mut impl Write, value: &Value) {
    let bytes = serde_json::to_vec(value).expect("serialize frame");
    assert!(!bytes.is_empty() && bytes.len() <= MAX_FRAME_BYTES);
    writer
        .write_all(
            &u32::try_from(bytes.len())
                .expect("frame length")
                .to_be_bytes(),
        )
        .expect("write frame length");
    writer.write_all(&bytes).expect("write frame body");
    writer.flush().expect("flush frame");
}

fn try_read_frame(reader: &mut impl Read) -> std::io::Result<Value> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = usize::try_from(u32::from_be_bytes(length)).expect("bounded frame length");
    assert!((1..=MAX_FRAME_BYTES).contains(&length));
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

fn read_frame(reader: &mut impl Read) -> Value {
    try_read_frame(reader).expect("read plugin host frame")
}

#[test]
fn exact_desktop_binary_enters_plugin_host_before_tauri_and_cleans_up() {
    let directory = tempfile::tempdir().expect("plugin host cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_norishell"))
        .arg("--plugin-host")
        .env_clear()
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plugin host");
    let mut input = child.stdin.take().expect("plugin host stdin");
    let mut output = child.stdout.take().expect("plugin host stdout");
    let module = fixture_module();
    let module_sha256 = hex::encode(Sha256::digest(&module));
    let nonce = "019d-plugin-host-test-nonce";
    write_frame(
        &mut input,
        &json!({
            "kind": "initialize",
            "protocolMajor": 1,
            "protocolMinor": 0,
            "instanceGeneration": 7,
            "nonce": nonce,
            "moduleSha256": module_sha256,
            "moduleBase64": STANDARD.encode(&module),
        }),
    );
    let ready = read_frame(&mut output);
    assert_eq!(ready["kind"], "ready");
    assert_eq!(ready["instanceGeneration"], 7);
    assert_eq!(ready["nonce"], nonce);

    write_frame(
        &mut input,
        &json!({
            "kind": "execute",
            "instanceGeneration": 7,
            "nonce": nonce,
            "sequence": 1,
            "request": {
                "protocolMajor": 1,
                "protocolMinor": 0,
                "requestId": "fixture-request",
                "kind": "initialize",
                "payloadJson": "{}"
            }
        }),
    );
    let result = read_frame(&mut output);
    assert_eq!(result["kind"], "result");
    assert_eq!(result["sequence"], 1);
    assert_eq!(result["outputs"], json!([]));

    write_frame(
        &mut input,
        &json!({
            "kind": "shutdown",
            "instanceGeneration": 7,
            "nonce": nonce,
            "sequence": 2,
        }),
    );
    let stopped = read_frame(&mut output);
    assert_eq!(stopped["kind"], "stopped");
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().expect("poll plugin host") {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "plugin host did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn plugin_host_keeps_protocol_13_runtime_strategy_for_its_instance() {
    let directory = tempfile::tempdir().expect("plugin host cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_norishell"))
        .arg("--plugin-host")
        .env_clear()
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plugin host");
    let mut input = child.stdin.take().expect("plugin host stdin");
    let mut output = child.stdout.take().expect("plugin host stdout");
    let module = persistent_fixture_module();
    let nonce = "019d-plugin-host-persistent";
    write_frame(
        &mut input,
        &json!({
            "kind": "initialize",
            "protocolMajor": 1,
            "protocolMinor": 0,
            "instanceGeneration": 8,
            "nonce": nonce,
            "moduleSha256": hex::encode(Sha256::digest(&module)),
            "moduleBase64": STANDARD.encode(&module),
        }),
    );
    assert_eq!(read_frame(&mut output)["kind"], "ready");

    write_frame(
        &mut input,
        &json!({
            "kind": "execute",
            "instanceGeneration": 8,
            "nonce": nonce,
            "sequence": 1,
            "request": {
                "protocolMajor": 1,
                "protocolMinor": PLUGIN_API_PROTOCOL_MINOR,
                "requestId": "persistent-request",
                "kind": "initialize",
                "payloadJson": "{}"
            }
        }),
    );
    assert_eq!(read_frame(&mut output)["kind"], "result");

    write_frame(
        &mut input,
        &json!({
            "kind": "execute",
            "instanceGeneration": 8,
            "nonce": nonce,
            "sequence": 2,
            "request": {
                "protocolMajor": 1,
                "protocolMinor": PLUGIN_API_PROTOCOL_MINOR - 1,
                "requestId": "legacy-request",
                "kind": "initialize",
                "payloadJson": "{}"
            }
        }),
    );
    let rejection = read_frame(&mut output);
    assert_eq!(rejection["kind"], "rejected");
    assert_eq!(rejection["code"], "incompatibleProtocol");

    write_frame(
        &mut input,
        &json!({
            "kind": "shutdown",
            "instanceGeneration": 8,
            "nonce": nonce,
            "sequence": 3,
        }),
    );
    assert_eq!(read_frame(&mut output)["kind"], "stopped");
    drop(input);
    assert!(child.wait().expect("wait plugin host").success());
}

#[test]
fn plugin_host_rejects_tampered_module_before_runtime_ready() {
    let directory = tempfile::tempdir().expect("plugin host cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_norishell"))
        .arg("--plugin-host")
        .env_clear()
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plugin host");
    let module = fixture_module();
    write_frame(
        child.stdin.as_mut().expect("plugin host stdin"),
        &json!({
            "kind": "initialize",
            "protocolMajor": 1,
            "protocolMinor": 0,
            "instanceGeneration": 1,
            "nonce": "019d-plugin-host-tamper",
            "moduleSha256": "00".repeat(32),
            "moduleBase64": STANDARD.encode(module),
        }),
    );
    drop(child.stdin.take());
    let status = child.wait().expect("wait for rejected plugin host");
    assert!(!status.success());
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("plugin host stdout")
        .read_to_end(&mut stdout)
        .expect("read plugin host stdout");
    assert!(stdout.is_empty(), "tampered module must not reach ready");
}

#[test]
#[ignore = "requires NORISHELL_OPERATIONS_PLUGIN_WASM built from the current candidate"]
fn official_operations_use_terminal_surfaces_in_the_exact_plugin_host() {
    let module =
        fs::read(std::env::var("NORISHELL_OPERATIONS_PLUGIN_WASM").expect("candidate Wasm path"))
            .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_norishell"))
        .arg("--plugin-host")
        .env_clear()
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let nonce = "operations-terminal-candidate";
    write_frame(
        &mut input,
        &json!({"kind":"initialize","protocolMajor":1,"protocolMinor":0,"instanceGeneration":1,"nonce":nonce,
        "moduleSha256":hex::encode(Sha256::digest(&module)),"moduleBase64":STANDARD.encode(&module)}),
    );
    assert_eq!(read_frame(&mut output)["kind"], "ready");
    let mut sequence = 0;
    for app in [
        "docker",
        "systemd",
        "disk",
        "network",
        "processes",
        "logs",
        "cron",
        "tunnels",
        "nginx",
        "tasks",
    ] {
        if std::env::var("NORISHELL_OPERATIONS_PLUGIN_KEY").is_ok_and(|key| key != app) {
            continue;
        }
        for locale in ["en", "zh-CN"] {
            sequence += 1;
            write_frame(
                &mut input,
                &json!({"kind":"execute","instanceGeneration":1,"nonce":nonce,"sequence":sequence,
                "request":{"protocolMajor":1,"protocolMinor":norishell_core_api::PLUGIN_PROTOCOL_MINOR,"requestId":format!("fixture-{sequence}"),"kind":"initialize",
                "payloadJson":json!({"pluginId":format!("org.norishell.{app}"),"locale":locale}).to_string()}}),
            );
            let result = read_frame(&mut output);
            assert_eq!(result["kind"], "result", "{app} {locale}: {result}");
            let outputs = result["outputs"].as_array().unwrap();
            assert!(outputs.len() == 1 || (app == "processes" && outputs.len() == 2));
            for item in outputs {
                assert_eq!(item["kind"], "ui.document");
                let declared: norishell_core_api::PluginUiTemplate =
                    serde_json::from_str(item["payloadJson"].as_str().unwrap()).unwrap();
                norishell_plugin_platform::validate_plugin_ui_document(&declared.document).unwrap();
                if declared.target_id.as_str() == "terminal.footer" {
                    assert_eq!(app, "processes");
                    let refresh = declared.auto_refresh.as_ref().unwrap();
                    assert_eq!(refresh.action_id.as_str(), "processes:cpuUsage");
                    assert_eq!(refresh.interval_ms, 5000);
                }
            }
            assert_eq!(outputs[0]["kind"], "ui.document");
            let template: norishell_core_api::PluginUiTemplate =
                serde_json::from_str(outputs[0]["payloadJson"].as_str().unwrap()).unwrap();
            assert_eq!(
                template.target_id.as_str(),
                if app == "tasks" {
                    "terminal.floating"
                } else {
                    "terminal.tools"
                }
            );
            norishell_plugin_platform::validate_plugin_ui_document(&template.document).unwrap();
            assert_eq!(
                template.on_open_action_id.as_ref().map(|id| id.as_str()),
                Some(format!("{app}:open").as_str())
            );
            sequence += 1;
            write_frame(
                &mut input,
                &json!({"kind":"execute","instanceGeneration":1,"nonce":nonce,"sequence":sequence,
                "request":{"protocolMajor":1,"protocolMinor":norishell_core_api::PLUGIN_PROTOCOL_MINOR,"requestId":format!("fixture-{sequence}"),"kind":"uiAction",
                "payloadJson":json!({"actionId":format!("{app}:open"),"targetId":template.target_id,"locale":locale,"fields":[],
                    "terminalMetadata":{"terminalHandle":"current-session","kind":"ssh","state":"running"}}).to_string()}}),
            );
            let opened = read_frame(&mut output);
            assert_eq!(opened["kind"], "result", "{app} {locale}: {opened}");
            let outputs = opened["outputs"].as_array().unwrap();
            assert!(
                outputs
                    .iter()
                    .all(|item| item["kind"] == "ui.document" || item["kind"] == "ui.state")
            );
            let document = outputs
                .iter()
                .find(|item| item["kind"] == "ui.document")
                .unwrap();
            let opened: norishell_core_api::PluginUiTemplate =
                serde_json::from_str(document["payloadJson"].as_str().unwrap()).unwrap();
            assert_eq!(opened.on_open_action_id, template.on_open_action_id);
            assert!(
                !serde_json::to_value(&opened.document).unwrap()["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|node| node["nodeId"] == "sessionHint")
            );
            norishell_plugin_platform::validate_plugin_ui_document(&opened.document).unwrap();
        }
    }
    sequence += 1;
    write_frame(
        &mut input,
        &json!({"kind":"shutdown","instanceGeneration":1,"nonce":nonce,"sequence":sequence}),
    );
    assert_eq!(read_frame(&mut output)["kind"], "stopped");
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
#[ignore = "requires NORISHELL_NORIXOR_PLUGIN_WASM from the signed official package candidate"]
fn official_norixor_protocol_seven_page_and_core_owned_sync_validate_in_real_plugin_host() {
    let module_path =
        std::env::var("NORISHELL_NORIXOR_PLUGIN_WASM").expect("NORISHELL_NORIXOR_PLUGIN_WASM");
    let module = fs::read(module_path).expect("read official Norixor Wasm");
    let protocol_minor = std::env::var("NORISHELL_NORIXOR_PROTOCOL_MINOR")
        .map(|value| value.parse::<u16>().expect("protocol minor"))
        .unwrap_or(9);
    let plugin_version =
        std::env::var("NORISHELL_NORIXOR_PLUGIN_VERSION").unwrap_or_else(|_| "1.0.9".to_owned());
    let directory = tempfile::tempdir().expect("plugin host cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_norishell"))
        .arg("--plugin-host")
        .env_clear()
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plugin host");
    let mut input = child.stdin.take().expect("plugin host stdin");
    let mut output = child.stdout.take().expect("plugin host stdout");
    let nonce = "019d-norixor-official-candidate";
    write_frame(
        &mut input,
        &json!({
            "kind": "initialize",
            "protocolMajor": 1,
            "protocolMinor": 0,
            "instanceGeneration": 1,
            "nonce": nonce,
            "moduleSha256": hex::encode(Sha256::digest(&module)),
            "moduleBase64": STANDARD.encode(&module),
        }),
    );
    let ready = try_read_frame(&mut output).unwrap_or_else(|error| {
        let status = child.wait().expect("wait failed plugin host");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("plugin host stderr")
            .read_to_string(&mut stderr)
            .expect("read plugin host stderr");
        panic!("official plugin host failed before ready: {error}; {status}; {stderr}");
    });
    assert_eq!(ready["kind"], "ready");

    let execute = |input: &mut std::process::ChildStdin,
                   output: &mut std::process::ChildStdout,
                   sequence: u64,
                   request_id: &str,
                   kind: &str,
                   payload: Value| {
        write_frame(
            input,
            &json!({
                "kind": "execute",
                "instanceGeneration": 1,
                "nonce": nonce,
                "sequence": sequence,
                "request": {
                    "protocolMajor": 1,
                    "protocolMinor": protocol_minor,
                    "requestId": request_id,
                    "kind": kind,
                    "payloadJson": payload.to_string(),
                }
            }),
        );
        let result = read_frame(output);
        assert_eq!(result["kind"], "result");
        result["outputs"]
            .as_array()
            .expect("plugin outputs")
            .clone()
    };

    let initialized = execute(
        &mut input,
        &mut output,
        1,
        "00000000-0000-4000-8000-000000000001",
        "initialize",
        json!({
            "pluginId":"org.norixor","version":plugin_version,"locale":"zh-CN",
            "storage":{"revision":0,"valueJson":"{}"}
        }),
    );
    let page = initialized
        .iter()
        .find(|item| item["kind"] == "ui.page")
        .expect("official page contribution");
    let page: PluginPageContribution =
        serde_json::from_str(page["payloadJson"].as_str().expect("page payload"))
            .expect("strict official page");
    assert_eq!(page.title, "Norixor 同步");
    assert_eq!(
        page.on_open_action_id.as_ref().map(|value| value.as_str()),
        Some("status")
    );
    validate_plugin_page_document(&page.document).expect("validated official page document");
    let page_json = serde_json::to_string(&page).expect("serialize page");
    assert!(page_json.contains("立即同步"));
    assert!(page_json.contains("刷新"));
    assert!(!page_json.contains("云端预览"));

    let mut actions = vec![
        (2, "status"),
        (3, "refresh"),
        (4, "sync"),
        (5, "configureScope"),
    ];
    if protocol_minor >= 8 {
        actions.push((6, "resetRemote"));
    }
    for (sequence, action) in actions {
        let outputs = execute(
            &mut input,
            &mut output,
            sequence,
            &format!("00000000-0000-4000-8000-{sequence:012}"),
            "uiAction",
            json!({
                "locale":"zh-CN","targetId":"app.page","contextHandle":"candidate",
                "targetRevision":"1","actionId":action,"fields":[],
                "hostMetadata":null,"hostDomSnapshot":null,"terminalMetadata":null,
                "storage":{"revision":0,"valueJson":"{}"}
            }),
        );
        let document = outputs
            .iter()
            .find(|item| item["kind"] == "ui.document")
            .expect("updated page document");
        let document_payload: Value =
            serde_json::from_str(document["payloadJson"].as_str().expect("document payload"))
                .expect("document wrapper");
        let document = serde_json::from_value(document_payload["document"].clone())
            .expect("strict UI document");
        validate_plugin_page_document(&document).expect("validated action document");

        let request = outputs
            .iter()
            .find(|item| item["kind"] == "ssh.sync.request")
            .expect("sync request");
        let request: PluginSshSyncRequest = serde_json::from_str(
            request["payloadJson"]
                .as_str()
                .expect("sync request payload"),
        )
        .expect("strict sync request");
        match request {
            PluginSshSyncRequest::Status { auth, .. } => {
                assert_eq!(action, "status");
                assert_eq!(
                    auth.expect("status account configuration").client_id,
                    "norishell-native"
                );
            }
            PluginSshSyncRequest::Refresh { auth, .. }
            | PluginSshSyncRequest::Sync { auth, .. } => {
                assert_eq!(auth.client_id, "norishell-native");
            }
            PluginSshSyncRequest::ConfigureScope { .. } => {}
            PluginSshSyncRequest::ResetRemote { auth, .. } => {
                assert_eq!(action, "resetRemote");
                assert_eq!(auth.client_id, "norishell-native");
            }
            _ => panic!("unexpected official action"),
        }
    }

    let review_sequence = if protocol_minor >= 8 { 7 } else { 6 };
    let review = execute(
        &mut input,
        &mut output,
        review_sequence,
        &format!("00000000-0000-4000-8000-{review_sequence:012}"),
        "sshSyncResult",
        json!({
            "locale":"zh-CN","targetId":"app.page","contextHandle":"candidate",
            "targetRevision":"1","actionId":"sync",
            "result":{
                "profileId":"primary","accountState":"connected",
                "operationState":"succeeded","lastSyncAtUnixMs":1,
                "stableErrorCode":null,"httpStatus":200,
                "localHostCount":128,"localCredentialCount":43,
                "remoteHostCount":128,"remoteCredentialCount":43,
                "differenceState":"equal","scopeMode":"allEligible"
            },
            "storage":{"revision":0,"valueJson":"{}"}
        }),
    );
    let review_document = review
        .iter()
        .find(|item| item["kind"] == "ui.document")
        .expect("review document");
    let review_payload: Value = serde_json::from_str(
        review_document["payloadJson"]
            .as_str()
            .expect("review payload"),
    )
    .expect("review wrapper");
    let review_document = serde_json::from_value(review_payload["document"].clone())
        .expect("strict review UI document");
    validate_plugin_page_document(&review_document).expect("validated review document");
    if protocol_minor >= 8 {
        let connected = serde_json::to_string(&review_document).expect("connected document");
        assert!(
            connected.contains("resetRemote"),
            "connected account retains reset action"
        );
        assert!(!connected.contains("这里只显示 Core"));
        let unsupported = execute(
            &mut input,
            &mut output,
            review_sequence + 1,
            "00000000-0000-4000-8000-000000000099",
            "sshSyncResult",
            json!({
                "locale":"zh-CN","targetId":"app.page","contextHandle":"candidate",
                "targetRevision":"1","actionId":"refresh",
                "result":{
                    "profileId":"primary","accountState":"connected","operationState":"failed",
                    "stableErrorCode":"remoteFormatUnsupported", "localHostCount":0,
                    "localCredentialCount":0,"remoteHostCount":null,"remoteCredentialCount":null,
                    "differenceState":"unavailable","scopeMode":null
                }, "storage":{"revision":0,"valueJson":"{}"}
            }),
        );
        let unsupported = unsupported
            .iter()
            .find(|item| item["kind"] == "ui.document")
            .expect("legacy error document");
        let payload: Value =
            serde_json::from_str(unsupported["payloadJson"].as_str().expect("payload"))
                .expect("json");
        let document =
            serde_json::from_value(payload["document"].clone()).expect("strict legacy error UI");
        validate_plugin_page_document(&document).expect("valid legacy error UI");
        let document = serde_json::to_string(&document).expect("legacy UI");
        assert!(document.contains("resetRemote"));
        assert!(document.contains("旧版格式"));
        assert!(!document.contains("这里只显示 Core"));
    }
    if let Ok(path) = std::env::var("NORISHELL_NORIXOR_QA_DOCUMENT") {
        fs::write(
            path,
            serde_json::to_vec_pretty(&review_document).expect("serialize QA document"),
        )
        .expect("write QA document");
    }

    write_frame(
        &mut input,
        &json!({
            "kind":"shutdown","instanceGeneration":1,"nonce":nonce,"sequence":review_sequence + if protocol_minor >= 8 {2} else {1}
        }),
    );
    assert_eq!(read_frame(&mut output)["kind"], "stopped");
    drop(input);
    assert!(child.wait().expect("wait plugin host").success());
}
