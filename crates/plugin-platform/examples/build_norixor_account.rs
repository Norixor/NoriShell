use std::{
    env,
    fs::File,
    io::Write as _,
    path::{Path, PathBuf},
};

use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
};
use serde_json::{Value, json};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const REQUEST_ID_BYTES: usize = 36;
const NAVIGATION_ADDRESS: usize = 0;
const LOGIN_PAGE_ADDRESS: usize = 4_096;
const LOGGED_PAGE_ADDRESS: usize = 16_384;
const ACTION_PAGE_ADDRESS: usize = 24_576;
const STORAGE_WRITE_ADDRESS: usize = 32_768;
const INPUT_ADDRESS: usize = 65_536;

struct OutputTemplate {
    bytes: Vec<u8>,
    request_id_offsets: Vec<usize>,
}

fn output_template(kind: &str, payload: Value) -> OutputTemplate {
    let placeholder = "0".repeat(REQUEST_ID_BYTES);
    let bytes = json!({
        "requestId": placeholder,
        "kind": kind,
        "payloadJson": payload.to_string(),
    })
    .to_string()
    .into_bytes();
    let request_id_offsets = bytes
        .windows(REQUEST_ID_BYTES)
        .enumerate()
        .filter_map(|(index, window)| (window == placeholder.as_bytes()).then_some(index))
        .collect::<Vec<_>>();
    assert!(!request_id_offsets.is_empty(), "template request id slot");
    OutputTemplate {
        bytes,
        request_id_offsets,
    }
}

fn navigation_payload() -> Value {
    json!({
        "navigationId": "norixorAccount",
        "label": "Norixor",
        "icon": "shield",
        "pageId": "account",
        "order": 100,
    })
}

fn login_document() -> Value {
    json!({
        "schemaVersion": 1,
        "rootNodeId": "accountRoot",
        "nodes": [
            {
                "kind": "stack", "nodeId": "accountRoot", "direction": "vertical",
                "align": "stretch", "gap": 20,
                "children": ["hero", "accountGrid", "security"]
            },
            {
                "kind": "stack", "nodeId": "hero", "direction": "horizontal",
                "align": "center", "gap": 12, "children": ["heroIcon", "heroCopy"]
            },
            {
                "kind": "icon", "nodeId": "heroIcon", "icon": "shield",
                "accessibleLabel": "Norixor 账户", "tone": "info"
            },
            {
                "kind": "stack", "nodeId": "heroCopy", "direction": "vertical",
                "align": "start", "gap": 4, "children": ["heading", "intro"]
            },
            {
                "kind": "text", "nodeId": "heading", "text": "登录 Norixor",
                "style": "heading", "tone": "neutral"
            },
            {
                "kind": "text", "nodeId": "intro",
                "text": "使用你的账户继续，或创建一个新账户。",
                "style": "secondary", "tone": "neutral"
            },
            {
                "kind": "grid", "nodeId": "accountGrid", "columns": 2,
                "gap": 16, "children": ["login", "register"]
            },
            {
                "kind": "section", "nodeId": "login", "title": "账户登录",
                "children": ["loginEmail", "loginPassword", "loginButton"]
            },
            {
                "kind": "textField", "nodeId": "loginEmail", "fieldId": "loginEmail",
                "label": "邮箱", "value": "", "placeholder": "name@example.com",
                "fieldKind": "text", "required": true, "disabled": false
            },
            {
                "kind": "textField", "nodeId": "loginPassword", "fieldId": "loginPassword",
                "label": "密码", "value": "", "placeholder": "输入 Norixor 密码",
                "fieldKind": "password", "required": true, "disabled": false
            },
            {
                "kind": "button", "nodeId": "loginButton", "actionId": "login",
                "label": "登录", "icon": "shield", "variant": "primary", "disabled": false
            },
            {
                "kind": "section", "nodeId": "register", "title": "还没有账户？",
                "children": ["registerText", "registerStatus", "registerDialog"]
            },
            {
                "kind": "text", "nodeId": "registerText",
                "text": "创建 Norixor 账户，注册资料会在独立对话框中填写。",
                "style": "secondary", "tone": "neutral"
            },
            {
                "kind": "status", "nodeId": "registerStatus",
                "label": "注册页面已准备", "tone": "info"
            },
            {
                "kind": "dialog", "nodeId": "registerDialog", "title": "注册 Norixor",
                "description": "填写注册资料。当前示例只实现页面，不会向 Norixor 服务发送注册请求。",
                "triggerLabel": "创建账户", "closeLabel": "关闭注册窗口",
                "children": ["registerName", "registerEmail", "registerPassword", "registerConfirm", "registerButton"]
            },
            {
                "kind": "textField", "nodeId": "registerName", "fieldId": "registerName",
                "label": "昵称", "value": "", "placeholder": "你的显示名称",
                "fieldKind": "text", "required": false, "disabled": false
            },
            {
                "kind": "textField", "nodeId": "registerEmail", "fieldId": "registerEmail",
                "label": "邮箱", "value": "", "placeholder": "name@example.com",
                "fieldKind": "text", "required": false, "disabled": false
            },
            {
                "kind": "textField", "nodeId": "registerPassword", "fieldId": "registerPassword",
                "label": "密码", "value": "", "placeholder": "设置密码",
                "fieldKind": "password", "required": false, "disabled": false
            },
            {
                "kind": "textField", "nodeId": "registerConfirm", "fieldId": "registerConfirm",
                "label": "确认密码", "value": "", "placeholder": "再次输入密码",
                "fieldKind": "password", "required": false, "disabled": false
            },
            {
                "kind": "button", "nodeId": "registerButton", "actionId": "registerUnavailable",
                "label": "注册服务尚未连接", "icon": "shield", "variant": "primary", "disabled": true
            },
            {
                "kind": "text", "nodeId": "security",
                "text": "当前示例尚未连接 Norixor 网络服务。登录按钮只演示插件页面状态保存；密码不会写入 NoriShell Vault。",
                "style": "caption", "tone": "neutral"
            }
        ]
    })
}

fn logged_document() -> Value {
    json!({
        "schemaVersion": 1,
        "rootNodeId": "loggedRoot",
        "nodes": [
            {
                "kind": "stack", "nodeId": "loggedRoot", "direction": "vertical",
                "align": "stretch", "gap": 20,
                "children": ["loggedHero", "loggedPanel", "loggedFoot"]
            },
            {
                "kind": "stack", "nodeId": "loggedHero", "direction": "horizontal",
                "align": "center", "gap": 12, "children": ["loggedIcon", "loggedCopy"]
            },
            {
                "kind": "icon", "nodeId": "loggedIcon", "icon": "check",
                "accessibleLabel": "账户状态正常", "tone": "success"
            },
            {
                "kind": "stack", "nodeId": "loggedCopy", "direction": "vertical",
                "align": "start", "gap": 4, "children": ["loggedHeading", "loggedIntro"]
            },
            {
                "kind": "text", "nodeId": "loggedHeading", "text": "账户已就绪",
                "style": "heading", "tone": "neutral"
            },
            {
                "kind": "text", "nodeId": "loggedIntro", "text": "欢迎回来，NoriShell 已恢复此插件的页面状态。",
                "style": "secondary", "tone": "neutral"
            },
            {
                "kind": "section", "nodeId": "loggedPanel", "title": "登录状态",
                "children": ["loggedStatus", "loggedText"]
            },
            {
                "kind": "status", "nodeId": "loggedStatus",
                "label": "插件登录态已恢复", "tone": "success"
            },
            {
                "kind": "text", "nodeId": "loggedText",
                "text": "这是 storage.plugin 恢复的本地页面演示状态；尚未连接 Norixor 网络服务，不能作为真实账户认证结果。",
                "style": "secondary", "tone": "neutral"
            },
            {
                "kind": "text", "nodeId": "loggedFoot",
                "text": "该状态仅属于 Norixor Account 插件，不会写入 NoriShell Vault 或 Host 配置。",
                "style": "caption", "tone": "neutral"
            }
        ]
    })
}

fn page_output(document: Value) -> OutputTemplate {
    output_template(
        "ui.page",
        json!({
            "pageId": "account", "title": "Norixor 账户", "icon": "shield", "document": document,
        }),
    )
}

fn action_page_output() -> OutputTemplate {
    output_template(
        "ui.document",
        json!({ "targetId": "app.page", "document": logged_document() }),
    )
}

fn storage_write_output() -> OutputTemplate {
    let placeholder = "0".repeat(REQUEST_ID_BYTES);
    output_template(
        "storage.write",
        json!({
            "writeToken": placeholder,
            "valueJson": "{\"session\":\"active\"}",
        }),
    )
}

fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

fn request_layout() -> (usize, usize) {
    let request_id = "0".repeat(REQUEST_ID_BYTES);
    let encoded = serde_json::to_vec(&PluginHostRequest {
        protocol_major: PLUGIN_PROTOCOL_MAJOR,
        protocol_minor: PLUGIN_PROTOCOL_MINOR,
        request_id: request_id.clone(),
        kind: PluginHostMessageKind::Initialize,
        payload_json: "{}".to_owned(),
    })
    .expect("serialize ABI request");
    let request_offset = encoded
        .windows(REQUEST_ID_BYTES)
        .position(|window| window == request_id.as_bytes())
        .expect("request id offset");
    let kind_prefix = b"\"kind\":\"";
    let kind_offset = encoded
        .windows(kind_prefix.len())
        .position(|window| window == kind_prefix)
        .expect("message kind offset")
        + kind_prefix.len();
    (request_offset, kind_offset)
}

fn copy_instructions(address: usize, template: &OutputTemplate) -> String {
    template
        .request_id_offsets
        .iter()
        .map(|offset| {
            format!(
                "    i32.const {address}\n    local.get $pointer\n    i32.const {offset}\n    call $copy_request_id\n"
            )
        })
        .collect()
}

fn emit_instructions(address: usize, template: &OutputTemplate) -> String {
    format!(
        "    i32.const {address}\n    i32.const {}\n    call $emit\n    drop\n",
        template.bytes.len()
    )
}

fn build_module() -> Vec<u8> {
    let navigation = output_template("ui.navigation", navigation_payload());
    let login_page = page_output(login_document());
    let logged_page = page_output(logged_document());
    let action_page = action_page_output();
    let storage_write = storage_write_output();
    assert!(STORAGE_WRITE_ADDRESS + storage_write.bytes.len() < INPUT_ADDRESS);
    let (request_id_input_offset, message_kind_input_offset) = request_layout();
    let active_prefix = u32::from_le_bytes(*b"acti");
    let wat = format!(
        r#"(module
  (import "norishell.host" "emit" (func $emit (param i32 i32) (result i32)))
  (memory (export "memory") 2)
  (data (i32.const {NAVIGATION_ADDRESS}) "{navigation_bytes}")
  (data (i32.const {LOGIN_PAGE_ADDRESS}) "{login_page_bytes}")
  (data (i32.const {LOGGED_PAGE_ADDRESS}) "{logged_page_bytes}")
  (data (i32.const {ACTION_PAGE_ADDRESS}) "{action_page_bytes}")
  (data (i32.const {STORAGE_WRITE_ADDRESS}) "{storage_write_bytes}")

  (func $copy_request_id (param $output i32) (param $input i32) (param $offset i32)
    local.get $output
    local.get $offset
    i32.add
    local.get $input
    i32.const {request_id_input_offset}
    i32.add
    i32.const {REQUEST_ID_BYTES}
    memory.copy)

  (func $contains_active (param $pointer i32) (param $length i32) (result i32)
    (local $index i32)
    (block $missing
      (loop $scan
        local.get $index
        local.get $length
        i32.const 4
        i32.sub
        i32.ge_u
        br_if $missing
        local.get $pointer
        local.get $index
        i32.add
        i32.load align=1
        i32.const {active_prefix}
        i32.eq
        if
          i32.const 1
          return
        end
        local.get $index
        i32.const 1
        i32.add
        local.set $index
        br $scan))
    i32.const 0)

  (func (export "nvx_alloc") (param i32) (result i32)
    i32.const {INPUT_ADDRESS})

  (func (export "nvx_handle") (param $pointer i32) (param $length i32) (result i32)
    local.get $pointer
    i32.const {message_kind_input_offset}
    i32.add
    i32.load8_u
    i32.const 105
    i32.eq
    if
{navigation_copy}{navigation_emit}
      local.get $pointer
      local.get $length
      call $contains_active
      if
{logged_page_copy}{logged_page_emit}
      else
{login_page_copy}{login_page_emit}
      end
    else
{action_page_copy}{action_page_emit}{storage_write_copy}{storage_write_emit}
    end
    i32.const 0))"#,
        navigation_bytes = wat_bytes(&navigation.bytes),
        login_page_bytes = wat_bytes(&login_page.bytes),
        logged_page_bytes = wat_bytes(&logged_page.bytes),
        action_page_bytes = wat_bytes(&action_page.bytes),
        storage_write_bytes = wat_bytes(&storage_write.bytes),
        navigation_copy = copy_instructions(NAVIGATION_ADDRESS, &navigation),
        navigation_emit = emit_instructions(NAVIGATION_ADDRESS, &navigation),
        login_page_copy = copy_instructions(LOGIN_PAGE_ADDRESS, &login_page),
        login_page_emit = emit_instructions(LOGIN_PAGE_ADDRESS, &login_page),
        logged_page_copy = copy_instructions(LOGGED_PAGE_ADDRESS, &logged_page),
        logged_page_emit = emit_instructions(LOGGED_PAGE_ADDRESS, &logged_page),
        action_page_copy = copy_instructions(ACTION_PAGE_ADDRESS, &action_page),
        action_page_emit = emit_instructions(ACTION_PAGE_ADDRESS, &action_page),
        storage_write_copy = copy_instructions(STORAGE_WRITE_ADDRESS, &storage_write),
        storage_write_emit = emit_instructions(STORAGE_WRITE_ADDRESS, &storage_write),
    );
    wat::parse_str(wat).expect("compile Norixor account Wasm")
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins/norixor-account/manifest.json")
}

fn main() {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: build_norixor_account <output.zip>");
    let manifest = std::fs::read(manifest_path()).expect("read Norixor account manifest");
    let file = File::create(&output).expect("create output ZIP");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive
        .start_file("manifest.json", options)
        .expect("manifest entry");
    archive.write_all(&manifest).expect("write manifest");
    archive
        .start_file("plugin.wasm", options)
        .expect("Wasm entry");
    archive.write_all(&build_module()).expect("write Wasm");
    archive.finish().expect("finish ZIP");
    let inspected = norishell_plugin_platform::inspect_local_package(
        &output,
        &semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("current app version"),
        std::env::consts::ARCH,
        norishell_plugin_platform::PackageLimits::default(),
    )
    .expect("self-check generated plugin package");
    assert_eq!(inspected.manifest.plugin_id.as_str(), "org.norixor.account");
    println!("{}", output.display());
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
        PluginNavigationContribution, PluginPageContribution, PluginUiTemplate,
    };
    use norishell_plugin_platform::{
        RuntimeLimits, execute_wasm, validate_plugin_navigation, validate_plugin_page_document,
    };

    use super::build_module;

    fn execute(
        kind: PluginHostMessageKind,
        payload_json: String,
        request_id: &str,
    ) -> Vec<norishell_core_api::PluginRuntimeOutput> {
        let request = PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            request_id: request_id.to_owned(),
            kind,
            payload_json,
        };
        let mut outputs = Vec::new();
        execute_wasm(
            &build_module(),
            &request,
            |output| {
                outputs.push(output);
                Ok(())
            },
            RuntimeLimits::default(),
        )
        .expect("execute account plugin");
        outputs
    }

    #[test]
    fn initialize_contributes_page_passwords_and_a_host_owned_registration_dialog() {
        let outputs = execute(
            PluginHostMessageKind::Initialize,
            serde_json::json!({ "storage": { "revision": 0, "valueJson": "{}" } }).to_string(),
            "019d0000-0000-4000-8000-000000000902",
        );
        let navigation = outputs
            .iter()
            .find(|output| output.kind == "ui.navigation")
            .map(|output| {
                serde_json::from_str::<PluginNavigationContribution>(&output.payload_json)
                    .expect("navigation")
            })
            .expect("navigation output");
        let page = outputs
            .iter()
            .find(|output| output.kind == "ui.page")
            .map(|output| {
                serde_json::from_str::<PluginPageContribution>(&output.payload_json).expect("page")
            })
            .expect("page output");
        validate_plugin_page_document(&page.document).expect("page document");
        validate_plugin_navigation(
            std::slice::from_ref(&navigation),
            std::slice::from_ref(&page),
        )
        .expect("navigation binding");
        let page_json = serde_json::to_value(page).expect("page JSON");
        let nodes = page_json["document"]["nodes"].as_array().expect("nodes");
        assert!(nodes.iter().any(|node| node["kind"] == "dialog"));
        assert!(
            nodes
                .iter()
                .any(|node| node["fieldKind"] == "password" && node["disabled"] == false)
        );
    }

    #[test]
    fn login_action_returns_a_logged_page_and_request_bound_storage_write() {
        let request_id = "019d0000-0000-4000-8000-000000000904";
        let outputs = execute(
            PluginHostMessageKind::UiAction,
            serde_json::json!({
                "actionId": "login",
                "fields": [
                    { "fieldId": "loginEmail", "value": "vincent@example.com" },
                    { "fieldId": "loginPassword", "value": "secret" }
                ],
                "storage": { "revision": 0, "valueJson": "{}" }
            })
            .to_string(),
            request_id,
        );
        assert_eq!(outputs.len(), 2);
        let template = outputs
            .iter()
            .find(|output| output.kind == "ui.document")
            .map(|output| {
                serde_json::from_str::<PluginUiTemplate>(&output.payload_json).expect("template")
            })
            .expect("document output");
        validate_plugin_page_document(&template.document).expect("logged page");
        let storage = outputs
            .iter()
            .find(|output| output.kind == "storage.write")
            .expect("storage output");
        let payload: serde_json::Value =
            serde_json::from_str(&storage.payload_json).expect("storage payload");
        assert_eq!(payload["writeToken"], request_id);
        assert_eq!(payload["valueJson"], "{\"session\":\"active\"}");
    }

    #[test]
    fn initialize_restores_the_persisted_page_state() {
        let outputs = execute(
            PluginHostMessageKind::Initialize,
            serde_json::json!({
                "storage": { "revision": 1, "valueJson": "{\"session\":\"active\"}" }
            })
            .to_string(),
            "019d0000-0000-4000-8000-000000000905",
        );
        let page = outputs
            .iter()
            .find(|output| output.kind == "ui.page")
            .expect("page output");
        assert!(page.payload_json.contains("插件登录态已恢复"));
    }
}
