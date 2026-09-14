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
const DOCUMENT_ADDRESS: usize = 0;
const CLIPBOARD_ADDRESS: usize = 8_192;
const INPUT_ADDRESS: usize = 16_384;

struct OutputTemplate {
    bytes: Vec<u8>,
    request_id_offsets: Vec<usize>,
}

fn uuid_document(uuid: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "rootNodeId": "uuidDialog",
        "nodes": [
            {
                "kind": "dialog", "nodeId": "uuidDialog", "title": "UUID 生成器",
                "description": "生成 RFC 9562 UUIDv4。UUID 只在本机的隔离插件调用中创建。",
                "triggerLabel": "UUID", "closeLabel": "关闭 UUID 生成器",
                "children": ["uuidValue", "uuidHint", "uuidActions"]
            },
            {
                "kind": "code", "nodeId": "uuidValue", "text": uuid,
                "language": null, "wrap": true
            },
            {
                "kind": "text", "nodeId": "uuidHint",
                "text": "“生成并复制”会创建一个新的 UUID，并在同一次明确点击后写入剪贴板。",
                "style": "caption", "tone": "neutral"
            },
            {
                "kind": "stack", "nodeId": "uuidActions", "direction": "horizontal",
                "align": "center", "gap": 8, "children": ["generateUuid", "copyUuid"]
            },
            {
                "kind": "button", "nodeId": "generateUuid", "actionId": "generateUuid",
                "label": "生成 UUID", "icon": "refresh", "variant": "primary", "disabled": false
            },
            {
                "kind": "copyButton", "nodeId": "copyUuid", "actionId": "copyUuid",
                "label": "生成并复制", "disabled": false
            }
        ]
    })
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
    assert!(
        !request_id_offsets.is_empty(),
        "output must contain request id slots"
    );
    OutputTemplate {
        bytes,
        request_id_offsets,
    }
}

fn document_output() -> OutputTemplate {
    let placeholder = "0".repeat(REQUEST_ID_BYTES);
    output_template(
        "ui.document",
        json!({
            "targetId": "app.header.actions",
            "document": uuid_document(&placeholder),
        }),
    )
}

fn clipboard_output() -> OutputTemplate {
    let placeholder = "0".repeat(REQUEST_ID_BYTES);
    output_template("clipboard.write", json!({ "text": placeholder }))
}

fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

fn request_id_input_offset() -> usize {
    let request_id = "0".repeat(REQUEST_ID_BYTES);
    let encoded = serde_json::to_vec(&PluginHostRequest {
        protocol_major: PLUGIN_PROTOCOL_MAJOR,
        protocol_minor: PLUGIN_PROTOCOL_MINOR,
        request_id: request_id.clone(),
        kind: PluginHostMessageKind::Initialize,
        payload_json: "{}".to_owned(),
    })
    .expect("serialize ABI request");
    encoded
        .windows(REQUEST_ID_BYTES)
        .position(|window| window == request_id.as_bytes())
        .expect("request id offset")
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
    let document = document_output();
    let clipboard = clipboard_output();
    assert!(CLIPBOARD_ADDRESS + clipboard.bytes.len() < INPUT_ADDRESS);
    let copy_marker = u64::from_le_bytes(*b"copyUuid");
    let wat = format!(
        r#"(module
  (import "norishell.host" "emit" (func $emit (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const {DOCUMENT_ADDRESS}) "{document_bytes}")
  (data (i32.const {CLIPBOARD_ADDRESS}) "{clipboard_bytes}")

  (func $copy_request_id (param $output i32) (param $input i32) (param $offset i32)
    local.get $output
    local.get $offset
    i32.add
    local.get $input
    i32.const {request_id_input_offset}
    i32.add
    i32.const {REQUEST_ID_BYTES}
    memory.copy)

  (func $contains_copy_action (param $pointer i32) (param $length i32) (result i32)
    (local $index i32)
    (block $missing
      (loop $scan
        local.get $index
        local.get $length
        i32.const 8
        i32.sub
        i32.gt_u
        br_if $missing
        local.get $pointer
        local.get $index
        i32.add
        i64.load align=1
        i64.const {copy_marker}
        i64.eq
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
{document_copy}{document_emit}
    local.get $pointer
    local.get $length
    call $contains_copy_action
    if
{clipboard_copy}{clipboard_emit}
    end
    i32.const 0))"#,
        document_bytes = wat_bytes(&document.bytes),
        clipboard_bytes = wat_bytes(&clipboard.bytes),
        request_id_input_offset = request_id_input_offset(),
        document_copy = copy_instructions(DOCUMENT_ADDRESS, &document),
        document_emit = emit_instructions(DOCUMENT_ADDRESS, &document),
        clipboard_copy = copy_instructions(CLIPBOARD_ADDRESS, &clipboard),
        clipboard_emit = emit_instructions(CLIPBOARD_ADDRESS, &clipboard),
    );
    wat::parse_str(wat).expect("compile UUID generator Wasm")
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/utility-demo/manifest.json")
}

fn main() {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: build_utility_demo <output.zip>");
    let manifest = std::fs::read(manifest_path()).expect("read UUID generator manifest");
    let file = File::create(&output).expect("create output ZIP");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive
        .start_file("manifest.json", options)
        .expect("start manifest entry");
    archive.write_all(&manifest).expect("write manifest entry");
    archive
        .start_file("plugin.wasm", options)
        .expect("start Wasm entry");
    archive
        .write_all(&build_module())
        .expect("write Wasm entry");
    archive.finish().expect("finish output ZIP");
    let inspected = norishell_plugin_platform::inspect_local_package(
        &output,
        &semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("current app version"),
        std::env::consts::ARCH,
        norishell_plugin_platform::PackageLimits::default(),
    )
    .expect("self-check generated plugin package");
    assert_eq!(
        inspected.manifest.plugin_id.as_str(),
        "com.norishell.utility-demo"
    );
    println!("{}", output.display());
}

#[cfg(test)]
mod tests {
    use norishell_core_api::{
        PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
        PluginRuntimeOutput, PluginUiTemplate,
    };
    use norishell_plugin_platform::{RuntimeLimits, execute_wasm, validate_plugin_dialog_document};

    use super::{build_module, json};

    fn execute(action_id: Option<&str>, request_id: &str) -> Vec<PluginRuntimeOutput> {
        let request = PluginHostRequest {
            protocol_major: PLUGIN_PROTOCOL_MAJOR,
            protocol_minor: PLUGIN_PROTOCOL_MINOR,
            request_id: request_id.to_owned(),
            kind: if action_id.is_some() {
                PluginHostMessageKind::UiAction
            } else {
                PluginHostMessageKind::Initialize
            },
            payload_json: action_id
                .map(|action_id| json!({ "actionId": action_id, "fields": [] }).to_string())
                .unwrap_or_else(|| "{}".to_owned()),
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
        .expect("execute UUID generator");
        outputs
    }

    fn document(outputs: &[PluginRuntimeOutput]) -> PluginUiTemplate {
        let output = outputs
            .iter()
            .find(|output| output.kind == "ui.document")
            .expect("document output");
        serde_json::from_str(&output.payload_json).expect("UI template")
    }

    #[test]
    fn initialize_contributes_a_valid_header_uuid_dialog() {
        let request_id = "019d0000-0000-4000-8000-000000000901";
        let outputs = execute(None, request_id);
        assert_eq!(outputs.len(), 1);
        let template = document(&outputs);
        assert_eq!(template.target_id.as_str(), "app.header.actions");
        assert_eq!(validate_plugin_dialog_document(&template.document), Ok(()));
        assert!(template.document.nodes.iter().any(|node| matches!(
            node,
            norishell_core_api::PluginUiNode::Dialog { trigger_label, .. }
                if trigger_label == "UUID"
        )));
        assert!(template.document.nodes.iter().any(|node| matches!(
            node,
            norishell_core_api::PluginUiNode::Code { text, .. } if text == request_id
        )));
    }

    #[test]
    fn generate_refreshes_the_document_without_writing_the_clipboard() {
        let request_id = "019d0000-0000-4000-8000-000000000902";
        let outputs = execute(Some("generateUuid"), request_id);
        assert_eq!(outputs.len(), 1);
        assert!(
            document(&outputs)
                .document
                .nodes
                .iter()
                .any(|node| matches!(
                    node,
                    norishell_core_api::PluginUiNode::Code { text, .. } if text == request_id
                ))
        );
    }

    #[test]
    fn copy_generates_a_new_uuid_and_returns_it_only_for_the_copy_action() {
        let request_id = "019d0000-0000-4000-8000-000000000903";
        let outputs = execute(Some("copyUuid"), request_id);
        assert_eq!(outputs.len(), 2);
        let clipboard = outputs
            .iter()
            .find(|output| output.kind == "clipboard.write")
            .expect("clipboard output");
        let payload: serde_json::Value =
            serde_json::from_str(&clipboard.payload_json).expect("clipboard payload");
        assert_eq!(payload["text"], request_id);
        assert!(
            document(&outputs)
                .document
                .nodes
                .iter()
                .any(|node| matches!(
                    node,
                    norishell_core_api::PluginUiNode::Code { text, .. } if text == request_id
                ))
        );
    }
}
