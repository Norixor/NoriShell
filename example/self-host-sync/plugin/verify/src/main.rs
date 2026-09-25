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

    for (action, expected) in [
        ("sync.pageOpened", "refresh"),
        ("sync.status", "status"),
        ("sync.login", "login"),
        ("sync.refresh", "refresh"),
        ("sync.run", "sync"),
        ("sync.scope", "configureScope"),
        ("sync.reset", "resetRemote"),
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
        if expected == "sync" {
            assert_eq!(request["source"]["url"], request["destination"]["url"]);
            assert_eq!(request["destination"]["method"], "put");
            assert_eq!(request["destination"]["useOauth"], true);
            assert_eq!(request["conflictPolicy"], "newest");
            assert_eq!(request["deletionPolicy"], "newest");
        }
        if action == "sync.pageOpened" {
            assert_eq!(request["source"]["url"], "https://127.0.0.1:8443/exchange");
            assert_eq!(request["source"]["useOauth"], true);
            assert!(request.get("destination").is_none());
        }
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
        json!([
            "serverSettings",
            "scopeRow",
            "automationRow",
            "policyRow",
            "remoteRow"
        ])
    );
    assert_eq!(find("syncState")["label"], "Up to date");
    assert_eq!(find("syncState")["tone"], "success");
    assert_eq!(find("hostCount")["text"], "2");
    assert_eq!(find("accountPasswordField")["fieldKind"], "password");
    for (row, children) in [
        ("scopeRow", json!(["scopeText", "scopeButton"])),
        (
            "automationRow",
            json!(["automationText", "automationButton"]),
        ),
        ("policyRow", json!(["policyText", "policyButton"])),
        ("remoteRow", json!(["remoteText", "resetDialog"])),
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
                "locale":"en", "actionId":"sync.pageOpened", "result":{
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
    }
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
        json!({"locale":"zh-CN", "actionId":"sync.pageOpened", "fields":[], "settings":http_settings}),
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
    assert_eq!(http_request["action"], "refresh");
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
    println!("self-host sync Wasm page, requests, and callback validated");
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
