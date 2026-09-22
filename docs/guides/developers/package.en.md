# Package and manifest

An executable Wasm plugin package is a constrained ZIP with exactly `manifest.json`, `plugin.wasm`, and declared static assets below `assets/`. Native libraries, WASI, post-install steps, runtime downloads, arbitrary build scripts, symlinks, unsafe paths, duplicate normalized paths, encrypted entries, and unknown root files are rejected. Core makes a private copy and rechecks it as part of the installation transaction. Pure-data [theme packages](themes.en.md) instead contain only `manifest.json` and `assets/theme.json`; the two package branches cannot be mixed.

Current packages use `protocolMajor: 1` and `protocolMinor: 13`, the stable resident ABI baseline. Future major-1 hosts must retain minors from 13 through their current minor. A package requiring a future minor or unknown capability remains visible but cannot install on an older host. Breaking changes require a new major; pre-13 exports and fallback parsers are unsupported. The following is a complete local-package manifest baseline. `publisher` is self-reported, not a signing identity. `platform`, `architectures`, and `minimumAppVersion` are required too. Declaring a platform does not establish acceptance on that platform.

```json
{
  "pluginId": "com.example.service-inspector",
  "name": "Service Inspector",
  "publisher": "Example publisher (self-reported)",
  "version": "1.0.0",
  "protocolMajor": 1,
  "protocolMinor": 13,
  "platform": "desktop",
  "architectures": [
    "universal"
  ],
  "capabilities": [
    "uiPanel",
    "storagePlugin"
  ],
  "minimumAppVersion": "0.1.0"
}
```

Capability declarations request review; they never grant access. Refer to [permissions and resources](broker.en.md) and the typed [Plugin API reference](../plugin-api/README.en.md).

Use `norishell-plugin-dev scaffold`, `pack`, and `check` to create and inspect a local package. `run` and `watch` are Wasm ABI harnesses only: they feed request JSON to the guest and require a test caller to supply a typed broker reply. They do not create a Core broker, approve a capability, create/unlock Vault, or exercise the protected native install flow. `watch` observes only the supplied Wasm file and replays the last explicit `Initialize` request after a successful reload.

The source examples `verify_api_demo`, `verify_guest_api_types`, and `verify_isolated_demo` live under [`crates/plugin-sdk/examples`](../../../crates/plugin-sdk/examples). They can be run with `cargo run -p norishell-plugin-sdk --example <name>` when their local fixture prerequisites are connected. They are SDK/ABI demonstrations, not a substitute for the protected application integration or P19 native acceptance.

[`examples/plugins/workflow-demo`](../../../examples/plugins/workflow-demo) is a package-shaped task example. Its README gives the current Wasm build, ZIP pack, and `check` commands; the resulting local ZIP has source-level package validation, each modified plugin still needs its own installation, task execution, and restart checks. Existing repository-example acceptance is recorded in [implementation status](../../../README.en.md#installation-and-quick-start).

[`examples/plugins/protocol-demo`](../../../examples/plugins/protocol-demo) builds a real protocol-13 Wasm package with `build.py`. Its fixture harness runs the generated guest in the production persistent `WasmRuntime` through a test broker and verifies loopback TCP input/resize, split ANSI/UTF-8 bytes, two handles, and old-handle close isolation; the ZIP also passes SDK package checking. The harness does not install the ZIP, start `ProtocolSessionActor`, enter the Tauri application, or exercise a protected approval surface. It is therefore evidence for the guest/fixture boundary only, not native protocol or Windows acceptance.

[`examples/plugins/app-demo`](../../../examples/plugins/app-demo) demonstrates explicit command/shortcut registration, status, notification, controlled navigation, and a file command. The file action obtains a read-only handle through native `FilePick` and scope approval; separate clicks read the preview and close the handle. Extension labels categorize the command and do not register OS file associations. Its README distinguishes package checks from native acceptance.

[`examples/plugins/service-demo`](../../../examples/plugins/service-demo) queries public GitHub repository metadata through Core workflows and NetworkStart. Query, permission continuation, refresh, and cancellation are explicit actions. The guest has no direct network access and validates HTTP/JSON and response limits. Live desktop network acceptance is tracked in implementation status separately from guest parsing tests.

For the complete source-to-desktop flow, see [Quick start and example selection](quickstart.en.md).

- [Declarative theme plugins](themes.en.md)
