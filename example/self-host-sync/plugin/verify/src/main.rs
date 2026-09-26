//! Exercises the real Wasm runtime and the production page/request validators.

use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
    PluginPageContribution, PluginSshSyncRequest, PluginUiDocument,
};
use norishell_plugin_platform::{
    RuntimeLimits, WasmRuntime, validate_plugin_navigation, validate_plugin_page_document,
};
use serde_json::{Value, json};

fn main() {
    let wasm = std::fs::read(
        std::env::args_os()
            .nth(1)
            .expect("verify_sync <plugin.wasm>"),
    )
    .expect("read Wasm");
    let mut runtime = WasmRuntime::new(&wasm, RuntimeLimits::default()).expect("production ABI");
    let settings = json!({"revision":"1","values":{"serverUrl":"https://127.0.0.1:8443"}});
    let initial = execute(
        &mut runtime,
        PluginHostMessageKind::Initialize,
        json!({
            "pluginId":"com.norishell.self-host-sync", "locale":"en", "storage":null,
            "settings":settings
        }),
    );
    assert_eq!(initial.len(), 2);
    let navigation: norishell_core_api::PluginNavigationContribution =
        serde_json::from_str(&initial[0].payload_json).expect("navigation");
    assert_eq!(navigation.label, "Sync");
    let page: PluginPageContribution =
        serde_json::from_str(&initial[1].payload_json).expect("page");
    validate_plugin_page_document(&page.document).expect("valid page");
    let layout: Value = serde_json::to_value(&page.document).expect("document JSON");
    assert!(
        layout["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| {
                node["nodeId"] == "statusBar"
                    && node["kind"] == "stack"
                    && node["direction"] == "horizontal"
            })
    );
    assert!(
        !layout["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| { node["nodeId"] == "setupNotice" })
    );
    validate_plugin_navigation(&[navigation], &[page.clone()]).expect("valid navigation");
    assert_eq!(
        page.on_open_action_id.as_ref().expect("lifecycle").as_str(),
        "sync.pageOpened"
    );
    verify_page_open_status(&wasm, &settings);

    for (action, expected) in [
        ("sync.status", "status"),
        ("sync.login", "login"),
        ("sync.logout", "logout"),
    ] {
        let result = execute(
            &mut runtime,
            PluginHostMessageKind::UiAction,
            json!({
                "locale":"en", "actionId":action, "fields":[], "settings":settings
            }),
        );
        assert_eq!(result.len(), 1, "{action}");
        assert_eq!(result[0].kind, "ssh.sync.request", "{action}");
        let request: Value = serde_json::from_str(&result[0].payload_json).expect("request JSON");
        assert_eq!(request["action"], expected);
        serde_json::from_value::<PluginSshSyncRequest>(request.clone()).expect("typed request");
        if expected == "login" {
            assert_eq!(request["auth"]["clientId"], "norishell-self-host");
            assert_eq!(request["auth"]["scopes"], json!(["ssh.sync"]));
            assert_eq!(
                request["auth"]["resourceOrigins"],
                json!(["https://127.0.0.1:8443"])
            );
            assert_eq!(
                request["auth"]["loginUrl"],
                "https://127.0.0.1:8443/auth/login"
            );
            assert!(request.get("password").is_none());
        }
        execute(
            &mut runtime,
            PluginHostMessageKind::SshSyncResult,
            json!({
                "locale":"en", "actionId":action, "result":{"accountState":"connected","operationState":"succeeded"}
            }),
        );
    }

    let callback = execute(
        &mut runtime,
        PluginHostMessageKind::SshSyncResult,
        json!({
            "locale":"en", "actionId":"sync.refresh", "result":{
                "accountState":"connected", "operationState":"succeeded", "differenceState":"equal",
                "localHostCount":2,"remoteHostCount":2,"localCredentialCount":1,"remoteCredentialCount":1,
                "etag":"\"never-store-this\"", "remoteRevision":4
            }
        }),
    );
    assert_eq!(callback.len(), 1);
    let document: Value =
        serde_json::from_str(&callback[0].payload_json).expect("document payload");
    let page_document: PluginUiDocument =
        serde_json::from_value(document["document"].clone()).expect("document");
    validate_plugin_page_document(&page_document).expect("valid refreshed page");
    assert!(!callback[0].payload_json.contains("never-store-this"));
    assert!(callback[0].payload_json.contains("Account: Connected"));
    assert!(callback[0].payload_json.contains("Operation: Completed"));
    assert!(callback[0].payload_json.contains("Difference: Equal"));
    let refreshed: Value = serde_json::to_value(&page_document).expect("refreshed document JSON");
    let nodes = refreshed["nodes"].as_array().expect("refreshed nodes");
    let find = |id| {
        nodes
            .iter()
            .find(|node| node["nodeId"] == id)
            .expect("page node")
    };
    assert_eq!(find("connectionBar")["children"], json!(["statusBar"]));
    assert_eq!(
        find("statusBar")["children"],
        json!(["accountGroup", "syncState", "serverInfo", "actionButtons"])
    );
    assert_eq!(
        find("headingRow")["children"],
        json!(["heading", "settingsDialog"])
    );
    assert_eq!(
        find("settingsDialog")["children"],
        json!(["serverSettings", "scopeRow", "policyRow"])
    );
    assert_eq!(find("syncState")["label"], "Up to date");
    assert_eq!(find("syncState")["tone"], "success");
    assert_eq!(find("hostCount")["text"], "2");
    assert_eq!(find("accountPasswordField")["fieldKind"], "password");
    let offline = execute(
        &mut runtime,
        PluginHostMessageKind::SshSyncResult,
        json!({
            "locale":"en", "actionId":"sync.refresh", "result":{
                "accountState":"connected", "operationState":"failed",
                "differenceState":"unavailable", "stableErrorCode":"networkUnavailable",
                "localHostCount":2, "remoteHostCount":null,
                "localCredentialCount":1, "remoteCredentialCount":null,
                "remoteDesktopProfileCount":null,
                "diagnosticCode":null, "httpStatus":null
            }
        }),
    );
    assert_eq!(offline.len(), 1);
    let offline_payload: Value =
        serde_json::from_str(&offline[0].payload_json).expect("offline output");
    let offline_document: PluginUiDocument =
        serde_json::from_value(offline_payload["document"].clone()).expect("offline document");
    validate_plugin_page_document(&offline_document).expect("valid offline page");
    assert!(offline[0].payload_json.contains("Cannot reach the server"));
    assert!(
        offline[0]
            .payload_json
            .contains("Cloud counts are from the last successful read")
    );
    let not_signed_in = execute(
        &mut runtime,
        PluginHostMessageKind::SshSyncResult,
        json!({
            "locale":"en", "actionId":"sync.refresh", "result":{
                "accountState":"disconnected", "operationState":"failed",
                "differenceState":"unavailable", "stableErrorCode":"accountNotConnected"
            }
        }),
    );
    let sign_in_payload: Value =
        serde_json::from_str(&not_signed_in[0].payload_json).expect("sign-in output");
    let sign_in_document: PluginUiDocument =
        serde_json::from_value(sign_in_payload["document"].clone()).expect("sign-in document");
    validate_plugin_page_document(&sign_in_document).expect("valid sign-in page");
    assert!(
        not_signed_in[0]
            .payload_json
            .contains("Connect your account first")
    );
    for (row, children) in [
        ("scopeRow", json!(["scopeText"])),
        ("policyRow", json!(["policyText", "policyButton"])),
    ] {
        assert_eq!(find(row)["children"], children);
    }
    assert!(
        find("root")["children"]
            .as_array()
            .expect("root children")
            .contains(&json!("browser"))
    );
    for (account_state, error_code) in [
        ("disconnected", Value::Null),
        ("disconnected", json!("vaultLocked")),
        ("connected", json!("interactionRequired")),
    ] {
        let callback = execute(
            &mut runtime,
            PluginHostMessageKind::SshSyncResult,
            json!({
                "locale":"en", "actionId":"sync.refresh", "result":{
                    "accountState":account_state, "operationState":"needsReview",
                    "differenceState":"unavailable", "stableErrorCode":error_code,
                    "localHostCount":0,"remoteHostCount":null,
                    "localCredentialCount":0,"remoteCredentialCount":null
                }
            }),
        );
        let document: Value =
            serde_json::from_str(&callback[0].payload_json).expect("status document");
        let page_document: PluginUiDocument =
            serde_json::from_value(document["document"].clone()).expect("status document payload");
        validate_plugin_page_document(&page_document).expect("valid status page");
        let page: Value = serde_json::to_value(&page_document).expect("status page JSON");
        let run = page["nodes"]
            .as_array()
            .expect("status nodes")
            .iter()
            .find(|node| node["nodeId"] == "run")
            .expect("sync button");
        assert_eq!(run["label"], "Review and sync");
    }
    let rejected = execute(
        &mut runtime,
        PluginHostMessageKind::SshSyncResult,
        json!({
            "locale":"en", "actionId":"sync.refresh", "result":{
                "accountState":"connected", "operationState":"failed",
                "stableErrorCode":"remoteRequestRejected", "httpStatus":404
            }
        }),
    );
    let document: Value = serde_json::from_str(&rejected[0].payload_json).expect("failed status");
    let rejected_page: PluginUiDocument =
        serde_json::from_value(document["document"].clone()).expect("failed status document");
    validate_plugin_page_document(&rejected_page).expect("valid failed status page");
    let error = document["document"]["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["nodeId"] == "errorNotice")
        .expect("error notice");
    assert_eq!(error["tone"], "danger");
    assert!(
        error["label"]
            .as_str()
            .expect("error label")
            .contains("HTTP 404")
    );
    let chinese_callback = execute(
        &mut runtime,
        PluginHostMessageKind::SshSyncResult,
        json!({
            "locale":"zh-CN", "actionId":"sync.refresh", "result":{
                "accountState":"expired", "operationState":"failed",
                "differenceState":"conflict", "stableErrorCode":"authorizationExpired",
                "localHostCount":2, "remoteHostCount":3
            }
        }),
    );
    assert!(
        chinese_callback[0]
            .payload_json
            .contains("账户：登录已过期")
    );
    assert!(chinese_callback[0].payload_json.contains("操作：失败"));
    assert!(chinese_callback[0].payload_json.contains("差异：冲突"));
    assert!(
        chinese_callback[0]
            .payload_json
            .contains("错误：登录已过期，请重新登录。")
    );
    assert!(
        !chinese_callback[0]
            .payload_json
            .contains("authorizationExpired")
    );
    let diagnostic = execute(
        &mut runtime,
        PluginHostMessageKind::SshSyncResult,
        json!({
            "locale":"zh-CN", "result":{
                "accountState":"connected", "operationState":"failed",
                "stableErrorCode":"operationRejected",
                "diagnosticCode":"local.snapshot_current.02"
            }
        }),
    );
    assert!(
        diagnostic[0]
            .payload_json
            .contains("local.snapshot_current.02")
    );
    assert!(diagnostic[0].payload_json.contains("同步检查未通过"));
    let chinese = execute(
        &mut runtime,
        PluginHostMessageKind::Initialize,
        json!({
            "pluginId":"com.norishell.self-host-sync", "locale":"zh-CN", "storage":null,
            "settings":settings
        }),
    );
    let chinese_navigation: norishell_core_api::PluginNavigationContribution =
        serde_json::from_str(&chinese[0].payload_json).expect("Chinese navigation");
    assert_eq!(chinese_navigation.label, "同步");
    let unconfigured = execute(
        &mut runtime,
        PluginHostMessageKind::Initialize,
        json!({
            "pluginId":"com.norishell.self-host-sync", "locale":"zh-CN", "storage":null,
            "settings":{"revision":"2","values":{"serverUrl":""}}
        }),
    );
    let setup_page: PluginPageContribution =
        serde_json::from_str(&unconfigured[1].payload_json).expect("unconfigured page");
    validate_plugin_page_document(&setup_page.document).expect("valid setup page");
    let setup: Value = serde_json::to_value(&setup_page.document).expect("setup JSON");
    let http_settings = json!({"revision":"3","values":{"serverUrl":"127.0.0.1:8787"}});
    let http_page = execute(
        &mut runtime,
        PluginHostMessageKind::Initialize,
        json!({
            "pluginId":"com.norishell.self-host-sync", "locale":"zh-CN", "storage":null,
            "settings":http_settings
        }),
    );
    let http_document: PluginPageContribution =
        serde_json::from_str(&http_page[1].payload_json).expect("HTTP page");
    validate_plugin_page_document(&http_document.document).expect("valid HTTP page");
    let http_nodes: Value = serde_json::to_value(&http_document.document).expect("HTTP document");
    assert!(
        http_nodes["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| { node["nodeId"] == "httpWarning" && node["tone"] == "warning" })
    );
    let http_action = execute(
        &mut runtime,
        PluginHostMessageKind::UiAction,
        json!({"locale":"zh-CN", "actionId":"sync.login", "fields":[], "settings":http_settings}),
    );
    assert_eq!(http_action.len(), 1);
    assert_eq!(http_action[0].kind, "ssh.sync.request");
    let http_request: Value =
        serde_json::from_str(&http_action[0].payload_json).expect("HTTP request");
    assert_eq!(
        http_request["auth"]["resourceOrigins"],
        json!(["http://127.0.0.1:8787"])
    );
    assert_eq!(
        http_request["auth"]["loginUrl"],
        "http://127.0.0.1:8787/auth/login"
    );
    assert_eq!(http_request["action"], "login");
    let invalid_settings =
        json!({"revision":"4","values":{"serverUrl":"ftp://sync.example.invalid"}});
    let invalid = execute(
        &mut runtime,
        PluginHostMessageKind::Initialize,
        json!({
            "pluginId":"com.norishell.self-host-sync", "locale":"zh-CN", "storage":null,
            "settings":invalid_settings
        }),
    );
    let invalid_page: PluginPageContribution =
        serde_json::from_str(&invalid[1].payload_json).expect("invalid HTTP page");
    validate_plugin_page_document(&invalid_page.document).expect("valid invalid-URL page");
    let invalid_action = execute(
        &mut runtime,
        PluginHostMessageKind::UiAction,
        json!({"locale":"zh-CN", "actionId":"sync.pageOpened", "fields":[], "settings":invalid_settings}),
    );
    assert_eq!(invalid_action.len(), 1);
    assert_eq!(invalid_action[0].kind, "ui.document");
    assert!(
        setup["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| {
                node["nodeId"] == "setupSettings"
                    && node["kind"] == "button"
                    && node["actionId"] == "norishell.openSettings:serverUrl"
                    && node["disabled"] == false
            })
    );
    assert!(
        setup["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .any(|node| { node["nodeId"] == "root" && node["children"][1] == "setupNotice" })
    );
    if let Some(path) = std::env::args_os().nth(2) {
        execute(
            &mut runtime,
            PluginHostMessageKind::Initialize,
            json!({
                "pluginId":"com.norishell.self-host-sync", "locale":"zh-CN", "storage":null,
                "settings":{"revision":"5","values":{"serverUrl":"http://sync.example.test"}}
            }),
        );
        let fixture = execute(
            &mut runtime,
            PluginHostMessageKind::SshSyncResult,
            json!({
                "locale":"zh-CN", "actionId":"sync.refresh", "result":{
                    "accountState":"connected", "operationState":"succeeded", "differenceState":"equal",
                    "localHostCount":7,"remoteHostCount":7,"localCredentialCount":1,"remoteCredentialCount":1,
                    "localDesktopProfileCount":0,"remoteDesktopProfileCount":0
                }
            }),
        );
        let payload: Value =
            serde_json::from_str(&fixture[0].payload_json).expect("visual fixture");
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&payload["document"]).expect("fixture JSON"),
        )
        .expect("write visual fixture");
    }
    verify_provider_flow(&mut runtime, &settings);
    println!(
        "self-host sync Wasm page, provider data/network orchestration, and callback validated"
    );
}

fn execute(
    runtime: &mut WasmRuntime,
    kind: PluginHostMessageKind,
    payload: Value,
) -> Vec<norishell_core_api::PluginRuntimeOutput> {
    let request = PluginHostRequest {
        protocol_major: PLUGIN_PROTOCOL_MAJOR,
        protocol_minor: PLUGIN_PROTOCOL_MINOR,
        request_id: "00000000-0000-4000-8000-000000000101".into(),
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
    outputs
}

fn api_call(outputs: &[norishell_core_api::PluginRuntimeOutput], kind: &str) -> Value {
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].kind, "api.request");
    let value: Value = serde_json::from_str(&outputs[0].payload_json).expect("API call");
    assert_eq!(value["operation"]["kind"], kind);
    assert!(
        value["callId"]
            .as_str()
            .unwrap()
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    );
    serde_json::from_value::<norishell_core_api::PluginApiCall>(value.clone())
        .expect("typed data/network request");
    value
}

fn verify_page_open_status(wasm: &[u8], settings: &Value) {
    let mut runtime = WasmRuntime::new(wasm, RuntimeLimits::default()).expect("production ABI");
    execute(
        &mut runtime,
        PluginHostMessageKind::Initialize,
        json!({"pluginId":"com.norishell.self-host-sync","locale":"en","storage":null,"settings":settings}),
    );
    let opened = execute(
        &mut runtime,
        PluginHostMessageKind::UiAction,
        json!({"locale":"en","actionId":"sync.pageOpened","settings":settings}),
    );
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].kind, "ssh.sync.request");
    let request: Value = serde_json::from_str(&opened[0].payload_json).expect("status request");
    assert_eq!(request["action"], "status");
    let status = execute(
        &mut runtime,
        PluginHostMessageKind::SshSyncResult,
        json!({"locale":"en","actionId":"sync.pageOpened","result":{
            "accountState":"connected","operationState":"idle","stableErrorCode":null
        }}),
    );
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].kind, "ui.document");
    let document: Value = serde_json::from_str(&status[0].payload_json).expect("document");
    assert!(document.to_string().contains("Connected"));
}

fn reply(
    runtime: &mut WasmRuntime,
    call: &Value,
    value: Value,
) -> Vec<norishell_core_api::PluginRuntimeOutput> {
    serde_json::from_value::<norishell_core_api::PluginApiValue>(value.clone())
        .expect("typed API response");
    execute(
        runtime,
        PluginHostMessageKind::BrokerResult,
        json!({"locale":"en","actionId":"sync.run",
        "result":{"kind":"api","reply":{"callId":call["callId"],"outcome":{"kind":"completed","value":value}}}}),
    )
}

fn network_receipt(status: u16, receipt: &str, blob: Option<&str>, etag: Option<&str>) -> Value {
    json!({"kind":"resourceEvents","handle":"network","backpressured":false,"events":[{
        "sequence":"1","kind":{"kind":"network","event":{"kind":"httpExchangeCompleted",
            "status":status,"receiptHandle":receipt,"bodyBlobHandle":blob,"etag":etag,"byteLength":0}}
    }]})
}

fn verify_provider_flow(runtime: &mut WasmRuntime, settings: &Value) {
    let output = execute(
        runtime,
        PluginHostMessageKind::UiAction,
        json!({"locale":"en","actionId":"sync.run","settings":settings}),
    );
    let snapshot = api_call(&output, "dataSnapshot");
    assert_eq!(
        snapshot["operation"]["request"]["categories"],
        json!(["hosts", "credentials", "desktopProfiles"])
    );
    let object = json!({"category":"hosts","kind":"host","stableId":"host-one","objectHandle":"object",
        "equalityTag":"keyed","updateTimeUnixMs":10,"tombstone":false,"dependency":false,
        "display":{"kind":"host","label":"Production","address":"host.example","port":22}});
    let output = reply(
        runtime,
        &snapshot,
        json!({"kind":"dataSnapshot","snapshotHandle":"local","keyPending":false,"localCounts":{"hostCount":1,"credentialCount":0,"desktopProfileCount":0,"tombstoneCount":0},"objects":[object]}),
    );
    let get = api_call(&output, "networkStart");
    assert_eq!(get["operation"]["request"]["operation"]["method"], "get");
    let output = reply(
        runtime,
        &get,
        json!({"kind":"networkStarted","handle":"network"}),
    );
    let poll = api_call(&output, "resourceEvents");
    assert_eq!(poll["operation"]["waitMs"], 30000);
    let output = reply(runtime, &poll, network_receipt(404, "empty", None, None));
    let close = api_call(&output, "resourceClose");
    let output = reply(runtime, &close, json!({"kind":"closed","handle":"network"}));
    let export = api_call(&output, "dataExport");
    assert_eq!(export["operation"]["request"]["sourceHandle"], "local");
    let output = reply(
        runtime,
        &export,
        json!({"kind":"dataExport","blobHandle":"encrypted","exportHandle":"exported","objects":[object],"revision":7,
        "idempotencyKey":"00000000-0000-4000-8000-000000000777","contentType":"application/vnd.norishell.ssh-sync-exchange+json;version=1"}),
    );
    let put = api_call(&output, "networkStart");
    let http = &put["operation"]["request"]["operation"];
    assert_eq!(http["method"], "put");
    assert_eq!(http["bodyBlobHandle"], "encrypted");
    let headers = http["headers"].as_array().unwrap();
    assert!(headers.iter().any(
        |header| header["name"] == "X-NoriShell-Expected-Next-Revision" && header["value"] == "7"
    ));
    assert!(!headers.iter().any(|header| header["name"] == "If-Match"));
    let output = reply(
        runtime,
        &put,
        json!({"kind":"networkStarted","handle":"network"}),
    );
    let poll = api_call(&output, "resourceEvents");
    let output = reply(
        runtime,
        &poll,
        network_receipt(200, "uploaded", None, Some("\"v7\"")),
    );
    let close = api_call(&output, "resourceClose");
    let output = reply(runtime, &close, json!({"kind":"closed","handle":"network"}));
    let checkpoint = api_call(&output, "dataCheckpoint");
    assert_eq!(
        checkpoint["operation"]["request"]["authoritativeReceiptHandle"],
        "uploaded"
    );
    let output = reply(
        runtime,
        &checkpoint,
        json!({"kind":"dataCheckpoint","syncedAtUnixMs":100}),
    );
    let release = api_call(&output, "dataRelease");
    assert_eq!(
        release["operation"]["request"]["stateHandles"],
        json!(["local", "exported"])
    );
    assert_eq!(
        release["operation"]["request"]["blobHandles"],
        json!(["encrypted"])
    );
    assert_eq!(
        release["operation"]["request"]["receiptHandles"],
        json!(["empty", "uploaded"])
    );
    let output = reply(runtime, &release, json!({"kind":"dataRelease"}));
    let document = output
        .iter()
        .find(|output| output.kind == "ui.document")
        .expect("finished document");
    assert!(document.payload_json.contains("Production"));
    let storage = output
        .iter()
        .find(|output| output.kind == "storage.write")
        .expect("offline cache");
    for forbidden in ["keyed", "objectHandle", "encrypted", "uploaded"] {
        assert!(!storage.payload_json.contains(forbidden));
    }
}
