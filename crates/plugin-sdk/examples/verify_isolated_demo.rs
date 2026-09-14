//! ABI and installed-package gate for the isolated WebView demo guest; it never opens a native window.

use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use norishell_core_api::{
    PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR, PluginHostMessageKind, PluginHostRequest,
    PluginIsolatedSurfaceOpenRequest, PluginOperationId, PluginUiTemplate,
};
use norishell_plugin_platform::{
    PackageLimits, PluginInstaller, RuntimeLimits, WasmRuntime, inspect_local_package,
    validate_plugin_dialog_document,
};
use semver::Version;
use serde_json::json;
use zip::ZipArchive;

fn main() {
    let path = std::env::args_os()
        .nth(1)
        .expect("usage: verify_isolated_demo <isolated-demo.wasm> [isolated-demo.zip]");
    let package_path = std::env::args_os().nth(2).map(PathBuf::from);
    let module = std::fs::read(path).expect("read isolated-demo Wasm");
    let mut runtime = WasmRuntime::new(&module, RuntimeLimits::default())
        .expect("SDK ABI, required exports and sole host import");

    let initial = execute(
        &mut runtime,
        "00000000-0000-4000-8000-000000000201",
        PluginHostMessageKind::Initialize,
        json!({"pluginId":"com.norishell.isolated-demo","locale":"en"}),
    );
    assert_eq!(initial.len(), 1);
    assert_eq!(
        initial[0].request_id,
        "00000000-0000-4000-8000-000000000201"
    );
    assert_eq!(initial[0].kind, "ui.document");
    let template: PluginUiTemplate =
        serde_json::from_str(&initial[0].payload_json).expect("header template");
    assert_eq!(template.target_id.as_str(), "app.header.actions");
    validate_plugin_dialog_document(&template.document).expect("valid header dialog");

    let opened = execute(
        &mut runtime,
        "00000000-0000-4000-8000-000000000202",
        PluginHostMessageKind::UiAction,
        json!({"actionId":"isolated-demo:open","fields":[]}),
    );
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[0].request_id, "00000000-0000-4000-8000-000000000202");
    assert_eq!(opened[0].kind, "ui.document");
    let refreshed: PluginUiTemplate =
        serde_json::from_str(&opened[0].payload_json).expect("refreshed header template");
    assert_eq!(refreshed.target_id.as_str(), "app.header.actions");
    validate_plugin_dialog_document(&refreshed.document).expect("valid refreshed header dialog");

    assert_eq!(opened[1].request_id, "00000000-0000-4000-8000-000000000202");
    assert_eq!(opened[1].kind, "ui.webview.open");
    let surface: PluginIsolatedSurfaceOpenRequest =
        serde_json::from_str(&opened[1].payload_json).expect("isolated surface request");
    assert_eq!(surface.surface_id, "isolated-demo");
    assert!((480..=1600).contains(&surface.width));
    assert!((360..=1200).contains(&surface.height));
    if let Some(package_path) = package_path.as_deref() {
        verify_active_surface(package_path, &surface.surface_id);
    }
    println!(
        "isolated-demo passed protocol-13 Wasm ABI, output, and installed isolated-surface validation."
    );
}

fn verify_active_surface(package_path: &Path, surface_id: &str) {
    let expected_path = format!("assets/isolated/{surface_id}.html");
    let expected_html = zip_entry(package_path, &expected_path);
    let verification_root = std::env::temp_dir().join(format!(
        "norishell-isolated-demo-verify-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    fs::create_dir(&verification_root).expect("create isolated package verification directory");

    {
        let inspected = inspect_local_package(
            package_path,
            &Version::parse("0.1.0").expect("current app version"),
            std::env::consts::ARCH,
            PackageLimits::default(),
        )
        .expect("inspect completed package");
        assert_eq!(inspected.manifest.version, "1.0.2");
        let expected_package_sha256 = inspected
            .package_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let installer =
            PluginInstaller::new(verification_root.join("plugins"), PackageLimits::default())
                .expect("create package installer");
        let operation_id = PluginOperationId::new();
        let staged = installer
            .stage(package_path, &inspected, &operation_id)
            .expect("stage inspected package");
        installer
            .activate(staged, None, &operation_id)
            .expect("activate staged package");
        let active_html = installer
            .read_active_isolated_surface(
                &inspected.manifest.plugin_id,
                &inspected.manifest.version,
                &expected_package_sha256,
                surface_id,
                PackageLimits::default().max_single_file_bytes,
            )
            .expect("read active surface named by guest output");
        assert_eq!(active_html, expected_html);
        let html = std::str::from_utf8(&active_html).expect("UTF-8 isolated surface");
        assert!(html.contains("const BRIDGE_PROTOCOL = 13;"));
        assert!(html.contains("data.protocol !== BRIDGE_PROTOCOL"));
        assert!(!html.contains("protocolMajor"));
        assert!(!html.contains("protocolMinor"));
    }

    fs::remove_dir_all(&verification_root).expect("remove isolated package verification directory");
}

fn zip_entry(package_path: &Path, entry_name: &str) -> Vec<u8> {
    let file = File::open(package_path).expect("open completed package");
    let mut archive = ZipArchive::new(file).expect("open completed package ZIP");
    let mut entry = archive
        .by_name(entry_name)
        .expect("named isolated surface in ZIP");
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .expect("read isolated surface ZIP entry");
    bytes
}

fn execute(
    runtime: &mut WasmRuntime,
    request_id: &str,
    kind: PluginHostMessageKind,
    payload: serde_json::Value,
) -> Vec<norishell_core_api::PluginRuntimeOutput> {
    let request = PluginHostRequest {
        protocol_major: PLUGIN_PROTOCOL_MAJOR,
        protocol_minor: PLUGIN_PROTOCOL_MINOR,
        request_id: request_id.into(),
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
