//! Small local development harness for current NoriShell Wasm plugins.
//!
//! This binary deliberately runs only the production package validator and
//! Wasm runtime. It does not emulate Core's capability broker: `api.request`
//! output is left for an explicit test caller to answer with `BrokerResult`.

use std::{
    env,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, BufRead, Cursor, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime},
};

use norishell_core_api::{
    CORE_API_MAJOR, CORE_API_MINOR, PLUGIN_PROTOCOL_MAJOR, PLUGIN_PROTOCOL_MINOR,
    PluginHostMessageKind, PluginHostRequest,
};
use norishell_plugin_platform::{PackageLimits, RuntimeLimits, WasmRuntime, inspect_local_package};
use semver::Version;
use serde_json::json;
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter, write::SimpleFileOptions};

const TOOL_NAME: &str = "norishell-plugin-dev";
const DEFAULT_APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_ARCHITECTURE: &str = "universal";
const ZIP_TIME: (u16, u8, u8, u8, u8, u8) = (2026, 9, 10, 0, 0, 0);

#[derive(Debug)]
struct CliError(String);

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

type Result<T> = std::result::Result<T, CliError>;

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{TOOL_NAME}: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<()> {
    let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
        print_help();
        return Ok(());
    };
    if matches!(command, "--help" | "-h" | "help") {
        print_help();
        return Ok(());
    }
    let arguments = &arguments[1..];
    match command {
        "scaffold" => scaffold(arguments),
        "check" => check(arguments),
        "pack" => pack(arguments),
        "run" => run_wasm(arguments, false),
        "watch" => run_wasm(arguments, true),
        _ => Err(CliError(format!(
            "unknown command {command:?}; run `{TOOL_NAME} --help`"
        ))),
    }
}

fn print_help() {
    println!(
        "{TOOL_NAME} — local SDK tooling for NoriShell plugin protocol {PLUGIN_PROTOCOL_MAJOR}.{PLUGIN_PROTOCOL_MINOR}\n\
\nUSAGE:\n\
  {TOOL_NAME} scaffold <directory> [--id <plugin-id>] [--name <name>]\n\
  {TOOL_NAME} check <package.zip> [--app-version <version>] [--architecture <name>]\n\
  {TOOL_NAME} pack <directory> --wasm <plugin.wasm> --output <package.zip> [--app-version <version>] [--architecture <name>]\n\
  {TOOL_NAME} run <plugin.wasm> [--debug]\n\
  {TOOL_NAME} watch <plugin.wasm> [--debug]\n\
\n`run` and `watch` read one PluginHostRequest JSON object per stdin line and write\n\
actual PluginRuntimeOutput JSON objects to stdout. They never create a Core broker\n\
or approve capabilities. Feed a typed BrokerResult yourself when the plugin emits\n\
an api.request. `watch` observes only the supplied Wasm file and replays the last\n\
explicit Initialize request after a successful reload."
    );
}

fn scaffold(arguments: &[OsString]) -> Result<()> {
    let mut parser = Arguments::new(arguments);
    let destination = parser.required_path("directory")?;
    let plugin_id = parser.optional_string("--id")?.unwrap_or_else(|| {
        let suffix = destination
            .file_name()
            .and_then(|value| value.to_str())
            .map(sanitize_plugin_id_segment)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "plugin".into());
        format!("com.example.{suffix}")
    });
    let name = parser
        .optional_string("--name")?
        .unwrap_or_else(|| "NoriShell Plugin".into());
    parser.finish()?;

    if destination.exists() {
        return Err(CliError(format!(
            "scaffold target already exists: {}",
            display_path(&destination)
        )));
    }
    norishell_core_api::PluginId::parse(&plugin_id)
        .map_err(|message| CliError(format!("invalid --id: {message}")))?;
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(CliError("--name must be non-empty plain text".into()));
    }

    fs::create_dir_all(destination.join("src")).map_err(io_error)?;
    let package_name = format!(
        "norishell-{}",
        plugin_id.replace('.', "-").replace('-', "_")
    );
    let sdk_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .map_err(io_error)?;
    write_new(
        &destination.join("Cargo.toml"),
        format!(
            "[package]\nname = {package_name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nnorishell-plugin-sdk = {{ path = {} }}\nserde_json = \"1\"\n\n[profile.release]\nopt-level = \"s\"\nlto = true\npanic = \"abort\"\nstrip = true\n",
            toml_string(&display_path(&sdk_path))
        )
        .as_bytes(),
    )?;
    write_new(
        &destination.join("manifest.json"),
        format!(
            "{{\n  \"pluginId\": {},\n  \"name\": {},\n  \"publisher\": \"Example publisher (self-reported)\",\n  \"version\": \"0.1.0\",\n  \"protocolMajor\": {PLUGIN_PROTOCOL_MAJOR},\n  \"protocolMinor\": {PLUGIN_PROTOCOL_MINOR},\n  \"platform\": \"desktop\",\n  \"architectures\": [\"universal\"],\n  \"capabilities\": [],\n  \"minimumAppVersion\": \"{DEFAULT_APP_VERSION}\",\n  \"minimumCoreApiVersion\": {{\"major\": {CORE_API_MAJOR}, \"minor\": {CORE_API_MINOR}}}\n}}\n",
            serde_json::to_string(&plugin_id).expect("string JSON"),
            serde_json::to_string(&name).expect("string JSON"),
        )
        .as_bytes(),
    )?;
    write_new(
        &destination.join("src/lib.rs"),
        br#"use norishell_plugin_sdk::{
    Plugin, PluginError, PluginHostRequest, PluginRuntimeOutput, export_plugin, output,
};

#[derive(Default)]
struct ExamplePlugin {
    requests_seen: u64,
}

impl Plugin for ExamplePlugin {
    fn handle(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        self.requests_seen = self.requests_seen.saturating_add(1);
        Ok(vec![output(
            &request.request_id,
            "example.ready",
            &serde_json::json!({"requestsSeen": self.requests_seen}),
        )?])
    }
}

export_plugin!(ExamplePlugin);
"#,
    )?;
    write_new(
        &destination.join("README.md"),
        format!(
            "# {name}\n\nThis scaffold targets the only supported NoriShell plugin ABI: protocol `{PLUGIN_PROTOCOL_MAJOR}.{PLUGIN_PROTOCOL_MINOR}`.\n\nBuild its Wasm with the repository Rust toolchain and an isolated target, then create a deterministic local ZIP:\n\n```sh\nRUSTC=\"$(rustup which --toolchain 1.97.1 rustc)\" CARGO_INCREMENTAL=0 cargo build --release --target wasm32-unknown-unknown --target-dir /tmp/{plugin_id}-target\nnorishell-plugin-dev pack . --wasm /tmp/{plugin_id}-target/wasm32-unknown-unknown/release/{package_name}.wasm --output /tmp/{plugin_id}-0.1.0.zip\nnorishell-plugin-dev check /tmp/{plugin_id}-0.1.0.zip\n```\n\n`run` and `watch` are an ABI harness only. They do not provide Core capabilities or auto-answer `api.request`; a test caller must send the typed `BrokerResult` request itself. Native installation remains the NoriShell local-ZIP permission flow.\n"
        )
        .as_bytes(),
    )?;
    println!(
        "{}",
        json!({"scaffold": display_path(&destination), "pluginId": plugin_id, "protocol": format!("{PLUGIN_PROTOCOL_MAJOR}.{PLUGIN_PROTOCOL_MINOR}")})
    );
    Ok(())
}

fn check(arguments: &[OsString]) -> Result<()> {
    let mut parser = Arguments::new(arguments);
    let package = parser.required_path("package.zip")?;
    let context = package_context(&mut parser)?;
    parser.finish()?;
    let inspected = inspect_checked_package(&package, &context)?;
    println!(
        "{}",
        json!({
            "package": display_path(&package),
            "pluginId": inspected.manifest.plugin_id.to_string(),
            "version": inspected.manifest.version,
            "protocol": format!("{}.{}", inspected.manifest.protocol_major, inspected.manifest.protocol_minor),
            "sha256": lower_hex(&inspected.package_sha256),
            "bytes": inspected.package_size,
            "settings": inspected.settings.is_some(),
            "protocolCatalog": inspected.protocols.is_some(),
            "wasmAbi": "valid"
        })
    );
    Ok(())
}

fn pack(arguments: &[OsString]) -> Result<()> {
    let mut parser = Arguments::new(arguments);
    let source = parser.required_path("directory")?;
    let wasm = parser.required_option_path("--wasm")?;
    let output = parser.required_option_path("--output")?;
    let context = package_context(&mut parser)?;
    parser.finish()?;
    if !source.is_dir() {
        return Err(CliError(format!(
            "plugin directory is not a directory: {}",
            display_path(&source)
        )));
    }
    if output.exists() {
        return Err(CliError(format!(
            "refusing to overwrite package output: {}",
            display_path(&output)
        )));
    }
    ensure_regular_file(&wasm, "Wasm input")?;
    let manifest = source.join("manifest.json");
    ensure_regular_file(&manifest, "manifest")?;
    let mut entries = vec![
        ("manifest.json".to_owned(), manifest),
        ("plugin.wasm".to_owned(), wasm),
    ];
    let assets = source.join("assets");
    if assets.exists() {
        collect_assets(&assets, &assets, &mut entries)?;
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let output_parent = output.parent().unwrap_or_else(|| Path::new("."));
    if !output_parent.is_dir() {
        return Err(CliError(format!(
            "output directory does not exist: {}",
            display_path(output_parent)
        )));
    }
    let staging = new_staging_path(output_parent, &output)?;
    write_deterministic_zip(&staging, &entries)?;
    let inspected = match inspect_checked_package(&staging, &context) {
        Ok(inspected) => inspected,
        Err(error) => {
            let _ = fs::remove_file(&staging);
            return Err(error);
        }
    };
    if let Err(error) = fs::hard_link(&staging, &output) {
        let _ = fs::remove_file(&staging);
        return Err(CliError(format!(
            "could not create new package output {}: {error}",
            display_path(&output)
        )));
    }
    fs::remove_file(&staging).map_err(io_error)?;
    println!(
        "{}",
        json!({
            "package": display_path(&output),
            "pluginId": inspected.manifest.plugin_id.to_string(),
            "sha256": lower_hex(&inspected.package_sha256),
            "bytes": inspected.package_size,
            "deterministic": true
        })
    );
    Ok(())
}

fn run_wasm(arguments: &[OsString], watch: bool) -> Result<()> {
    let mut parser = Arguments::new(arguments);
    let wasm = parser.required_path("plugin.wasm")?;
    let debug = parser.flag("--debug")?;
    parser.finish()?;
    ensure_regular_file(&wasm, "Wasm input")?;
    if watch {
        watch_wasm(&wasm, debug)
    } else {
        let mut runtime = load_runtime(&wasm)?;
        for line in io::stdin().lock().lines() {
            let request = parse_request(&line.map_err(io_error)?)?;
            execute_request(&mut runtime, &request, debug)?;
        }
        Ok(())
    }
}

fn watch_wasm(wasm: &Path, debug: bool) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if sender
                .send(line.map_err(|error| error.to_string()))
                .is_err()
            {
                return;
            }
        }
    });
    let mut runtime = load_runtime(wasm)?;
    let mut fingerprint = wasm_fingerprint(wasm)?;
    let mut initialize = None;
    loop {
        match receiver.recv_timeout(Duration::from_millis(120)) {
            Ok(Ok(line)) => {
                let request = parse_request(&line)?;
                if request.kind == PluginHostMessageKind::Initialize {
                    initialize = Some(request.clone());
                }
                execute_request(&mut runtime, &request, debug)?;
            }
            Ok(Err(error)) => return Err(CliError(format!("stdin read failed: {error}"))),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let next = wasm_fingerprint(wasm)?;
        if next == fingerprint {
            continue;
        }
        // Record this observed candidate even if it is invalid. A developer
        // saving a bad Wasm should get one clear rejection, not a log flood;
        // the next actual file change gets another isolated reload attempt.
        fingerprint = next;
        match load_runtime(wasm) {
            Ok(mut replacement) => {
                if let Some(request) = initialize.as_ref() {
                    execute_request(&mut replacement, request, debug)?;
                }
                runtime = replacement;
                debug_log(debug, "reload", "reloaded", None, 0, 0, 0);
            }
            Err(error) => {
                // Do not resume a failed candidate or pretend that reload succeeded.
                eprintln!("{TOOL_NAME}: reload rejected: {error}");
            }
        }
    }
}

fn execute_request(
    runtime: &mut WasmRuntime,
    request: &PluginHostRequest,
    debug: bool,
) -> Result<()> {
    let input_size = serde_json::to_vec(request)
        .map_err(|_| CliError("could not encode PluginHostRequest".into()))?
        .len();
    let mut outputs = Vec::new();
    let report = runtime
        .execute(request, |output| {
            outputs.push(output);
            Ok(())
        })
        .map_err(|error| CliError(format!("Wasm request rejected: {error}")))?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    for output in outputs {
        let output_size = serde_json::to_vec(&output)
            .map_err(|_| CliError("could not encode PluginRuntimeOutput".into()))?
            .len();
        serde_json::to_writer(&mut stdout, &output)
            .map_err(|_| CliError("could not write PluginRuntimeOutput".into()))?;
        writeln!(stdout).map_err(io_error)?;
        debug_log(
            debug,
            output.kind.as_str(),
            "emitted",
            Some(request.request_id.as_str()),
            input_size,
            output_size,
            report.elapsed_milliseconds,
        );
    }
    stdout.flush().map_err(io_error)?;
    debug_log(
        debug,
        host_kind_name(request.kind),
        "ok",
        Some(request.request_id.as_str()),
        input_size,
        0,
        report.elapsed_milliseconds,
    );
    Ok(())
}

fn debug_log(
    enabled: bool,
    method: &str,
    status: &str,
    call_id: Option<&str>,
    input_bytes: usize,
    output_bytes: usize,
    elapsed_ms: u128,
) {
    if enabled {
        eprintln!(
            "{}",
            json!({
                "method": method,
                "callId": call_id,
                "status": status,
                "inputBytes": input_bytes,
                "outputBytes": output_bytes,
                "elapsedMs": elapsed_ms
            })
        );
    }
}

fn host_kind_name(kind: PluginHostMessageKind) -> &'static str {
    match kind {
        PluginHostMessageKind::Initialize => "initialize",
        PluginHostMessageKind::Invoke => "invoke",
        PluginHostMessageKind::UiAction => "uiAction",
        PluginHostMessageKind::SshSyncResult => "sshSyncResult",
        PluginHostMessageKind::BrokerResult => "brokerResult",
        PluginHostMessageKind::TerminalObservation => "terminalObservation",
        PluginHostMessageKind::ProtocolEvent => "protocolEvent",
        PluginHostMessageKind::WorkflowEvent => "workflowEvent",
    }
}

fn inspect_checked_package(
    package: &Path,
    context: &PackageContext,
) -> Result<norishell_plugin_platform::InspectedPackage> {
    let inspected = inspect_local_package(
        package,
        &context.app_version,
        &context.architecture,
        PackageLimits::default(),
    )
    .map_err(|error| CliError(format!("package validation failed: {error}")))?;
    let wasm = read_wasm_from_package(package, &inspected.package_sha256)?;
    load_runtime_bytes(&wasm)
        .map_err(|error| CliError(format!("Wasm ABI validation failed: {error}")))?;
    Ok(inspected)
}

fn read_wasm_from_package(package: &Path, expected_sha256: &[u8; 32]) -> Result<Vec<u8>> {
    // The package validator already inspected a no-follow regular-file snapshot.
    // Recheck the bytes used for ABI validation so a changed path cannot combine a
    // valid manifest with another module in the developer report.
    let package_bytes = fs::read(package).map_err(io_error)?;
    if <[u8; 32]>::from(Sha256::digest(&package_bytes)) != *expected_sha256 {
        return Err(CliError(
            "package changed while it was being checked; retry with a stable ZIP".into(),
        ));
    }
    let mut archive = ZipArchive::new(Cursor::new(package_bytes))
        .map_err(|_| CliError("package validation failed: invalid ZIP archive".into()))?;
    let mut wasm = archive
        .by_name("plugin.wasm")
        .map_err(|_| CliError("package validation failed: plugin.wasm is missing".into()))?;
    let mut bytes = Vec::new();
    wasm.read_to_end(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

fn load_runtime(path: &Path) -> Result<WasmRuntime> {
    let bytes = fs::read(path).map_err(io_error)?;
    load_runtime_bytes(&bytes)
}

fn load_runtime_bytes(bytes: &[u8]) -> Result<WasmRuntime> {
    WasmRuntime::new(bytes, RuntimeLimits::default()).map_err(|error| CliError(error.to_string()))
}

fn parse_request(line: &str) -> Result<PluginHostRequest> {
    if line.trim().is_empty() {
        return Err(CliError("JSONL input cannot contain blank lines".into()));
    }
    serde_json::from_str(line).map_err(|_| CliError("invalid JSONL PluginHostRequest".into()))
}

struct PackageContext {
    app_version: Version,
    architecture: String,
}

fn package_context(parser: &mut Arguments<'_>) -> Result<PackageContext> {
    let version = parser
        .optional_string("--app-version")?
        .unwrap_or_else(|| DEFAULT_APP_VERSION.into());
    let app_version = Version::parse(&version)
        .map_err(|_| CliError("--app-version must be a semantic version".into()))?;
    let architecture = parser
        .optional_string("--architecture")?
        .unwrap_or_else(|| DEFAULT_ARCHITECTURE.into());
    if architecture.is_empty()
        || architecture.len() > 32
        || architecture.chars().any(char::is_control)
    {
        return Err(CliError("--architecture must be bounded plain text".into()));
    }
    Ok(PackageContext {
        app_version,
        architecture,
    })
}

struct Arguments<'a> {
    values: &'a [OsString],
    index: usize,
}

impl<'a> Arguments<'a> {
    fn new(values: &'a [OsString]) -> Self {
        Self { values, index: 0 }
    }

    fn required_path(&mut self, label: &str) -> Result<PathBuf> {
        let value = self
            .values
            .get(self.index)
            .cloned()
            .ok_or_else(|| CliError(format!("missing required {label}")))?;
        self.index += 1;
        if value.to_string_lossy().starts_with('-') {
            return Err(CliError(format!("missing required {label}")));
        }
        Ok(PathBuf::from(value))
    }

    fn required_option_path(&mut self, option: &str) -> Result<PathBuf> {
        self.expect_option(option).map(PathBuf::from)
    }

    fn optional_string(&mut self, option: &str) -> Result<Option<String>> {
        if self.peek_option(option) {
            let value = self.expect_option(option)?;
            return value
                .into_string()
                .map(Some)
                .map_err(|_| CliError(format!("{option} must be valid UTF-8")));
        }
        Ok(None)
    }

    fn flag(&mut self, option: &str) -> Result<bool> {
        if self.peek_option(option) {
            self.index += 1;
            return Ok(true);
        }
        Ok(false)
    }

    fn expect_option(&mut self, option: &str) -> Result<OsString> {
        if !self.peek_option(option) {
            return Err(CliError(format!("missing required {option}")));
        }
        self.index += 1;
        let value = self
            .values
            .get(self.index)
            .cloned()
            .ok_or_else(|| CliError(format!("missing value for {option}")))?;
        self.index += 1;
        if value.to_string_lossy().starts_with('-') {
            return Err(CliError(format!("missing value for {option}")));
        }
        Ok(value)
    }

    fn peek_option(&self, option: &str) -> bool {
        self.values
            .get(self.index)
            .is_some_and(|value| value == option)
    }

    fn finish(&self) -> Result<()> {
        if let Some(value) = self.values.get(self.index) {
            Err(CliError(format!(
                "unexpected argument {}",
                value.to_string_lossy()
            )))
        } else {
            Ok(())
        }
    }
}

fn ensure_regular_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CliError(format!(
            "{label} is unavailable at {}: {error}",
            display_path(path)
        ))
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CliError(format!(
            "{label} must be a regular non-symlink file: {}",
            display_path(path)
        )));
    }
    Ok(())
}

fn collect_assets(root: &Path, current: &Path, entries: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let metadata = fs::symlink_metadata(current).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(CliError(format!(
            "asset symlinks are not allowed: {}",
            display_path(current)
        )));
    }
    if current != root && metadata.file_type().is_file() {
        let relative = current
            .strip_prefix(root)
            .map_err(|_| CliError("could not derive asset path".into()))?;
        let relative = relative
            .to_str()
            .filter(|value| value.is_ascii())
            .ok_or_else(|| CliError("asset paths must be ASCII".into()))?;
        if relative.is_empty()
            || relative
                .split(std::path::MAIN_SEPARATOR)
                .any(|part| part.is_empty())
        {
            return Err(CliError("asset path is invalid".into()));
        }
        entries.push((
            format!(
                "assets/{}",
                relative.replace(std::path::MAIN_SEPARATOR, "/")
            ),
            current.to_owned(),
        ));
        return Ok(());
    }
    if !metadata.file_type().is_dir() {
        return Err(CliError(format!(
            "asset is neither a file nor directory: {}",
            display_path(current)
        )));
    }
    let mut children = fs::read_dir(current)
        .map_err(io_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(io_error)?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        collect_assets(root, &child.path(), entries)?;
    }
    Ok(())
}

fn write_deterministic_zip(staging: &Path, entries: &[(String, PathBuf)]) -> Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging)
        .map_err(io_error)?;
    let mut archive = ZipWriter::new(file);
    let timestamp = DateTime::from_date_and_time(
        ZIP_TIME.0, ZIP_TIME.1, ZIP_TIME.2, ZIP_TIME.3, ZIP_TIME.4, ZIP_TIME.5,
    )
    .expect("fixed ZIP timestamp is valid");
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9))
        .last_modified_time(timestamp)
        .unix_permissions(0o644);
    for (name, path) in entries {
        ensure_regular_file(path, "package input")?;
        archive
            .start_file(name, options)
            .map_err(|error| CliError(format!("could not add {name} to package: {error}")))?;
        let bytes = fs::read(path).map_err(io_error)?;
        archive.write_all(&bytes).map_err(io_error)?;
    }
    archive
        .finish()
        .map_err(|error| CliError(format!("could not finish package: {error}")))?;
    Ok(())
}

fn new_staging_path(parent: &Path, output: &Path) -> Result<PathBuf> {
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("plugin.zip");
    for attempt in 0..100_u32 {
        let candidate = parent.join(format!(".{name}.norishell-plugin-dev-{attempt}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(CliError("could not reserve package staging path".into()))
}

fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(contents).map_err(io_error)
}

fn wasm_fingerprint(path: &Path) -> Result<(u64, Option<SystemTime>, [u8; 32])> {
    ensure_regular_file(path, "Wasm input")?;
    let metadata = fs::metadata(path).map_err(io_error)?;
    let bytes = fs::read(path).map_err(io_error)?;
    Ok((
        metadata.len(),
        metadata.modified().ok(),
        Sha256::digest(&bytes).into(),
    ))
}

fn sanitize_plugin_id_segment(input: &str) -> String {
    input
        .chars()
        .map(|character| {
            if character.is_ascii_lowercase() || character.is_ascii_digit() {
                character
            } else if character.is_ascii_uppercase() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

fn toml_string(value: &str) -> String {
    // TOML basic strings accept the JSON escape repertoire used for a path.
    serde_json::to_string(value).expect("path string JSON")
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn io_error(error: io::Error) -> CliError {
    CliError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::sanitize_plugin_id_segment;

    #[test]
    fn plugin_id_segment_is_safe_for_a_scaffold_default() {
        assert_eq!(sanitize_plugin_id_segment("My Plugin!"), "my-plugin");
    }
}
