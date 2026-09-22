# Quick start and example selection

This guide is for plugin authors working from the NoriShell repository. Protocol **1.13** is the stable resident ABI baseline. Hosts accept plugin minors from 13 through their current minor within major 1; new features require their declared minimum minor/app version. The SDK is a repository source dependency; do not assume a crates.io release. Start with the visible API Demo, then select the example closest to your goal.

## Build your first visible plugin

Run from the repository root. You need Python 3, rustup toolchain `1.97.1`, and its `wasm32-unknown-unknown` target. The script uses `--locked --offline`, so dependencies must already be cached. Resolve missing dependencies or targets before diagnosing runtime permissions.

```sh
python3 examples/plugins/api-demo/build.py --output /tmp/NoriShell-API-Demo-1.0.1.zip
```

The script compiles real Wasm, runs ABI verification, and writes the ZIP and adjacent `.zip.sha256` file. It automatically removes temporary build directories. It does not install, grant permissions, or publish.

1. In a matching NoriShell host, open **Plugins → Import ZIP**, select the output, and inspect its identity and capabilities.
2. Complete installation, explicitly approve the required capabilities, and enable it. API Demo requests `uiPanel` and `clipboardWrite`.
3. Open **API demo** and select **Query API**. The view should show Core's current methods and limits; opening the view alone performs no query.
4. Use the copy button and compare the copied text with the displayed result. Disabling the plugin should remove its contribution.

If installation works but no UI appears, inspect the contribution, `ui.document`, and action callback. Enabled status does not prove functionality. See [Declarative UI](ui.en.md) for complete output requirements.

## Choose an example

| Goal | Source and instructions | Focus |
| --- | --- | --- |
| API discovery, display, copy | [API Demo](../../../examples/plugins/api-demo/README.md) | Explicit action → `api.request` → `BrokerResult` → replacement document |
| External service | [Service Demo](../../../examples/plugins/service-demo/README.md) | Core HTTPS, explicit permission continuation, HTTP result and cancellation |
| Multi-step tasks | [Workflow Demo](../../../examples/plugins/workflow-demo/README.md) | Catalog, timer, resource events, revisions and restart state |
| New terminal protocol | [Protocol Demo](../../../examples/plugins/protocol-demo/README.md) | Provider events, bytes, resize, reconnect and close |
| Commands, notifications, navigation, files | [App Demo](../../../examples/plugins/app-demo/README.md) | Explicit registration, shortcut enablement, native picker and file handles |
| Separate HTML surface | [Isolated Demo source](../../../examples/plugins/isolated-demo) | `assets/isolated/`, MessagePort, theme/locale and restricted bridge |
| Empty scaffold and ABI debugging | [SDK tooling](../../../examples/plugins/sdk-tooling/README.md) | `scaffold`, `pack`, `check`, `run`, `watch` |

When adapting an example, change its `pluginId`, name, version and required capabilities, and update the catalog, actions and assets together. Avoid replacing an existing installation with an unrelated experiment. `scaffold` writes an absolute dependency path to the local SDK; adjust it when moving or sharing the project. It creates an ABI skeleton: the default `example.ready` output is not a visible application contribution. Follow API Demo to add a valid document.

## Calls and failure handling

Construct requests and parse results with SDK types, without using Core internal IPC. After a UI action sends a call, update the view from the matching `BrokerResult`. A `callId` is 1–80 ASCII bytes containing only letters, digits, `.`, `_`, or `-`; do not use colons. See [Broker API](../plugin-api/broker.en.md) for fields.

| Result or symptom | Required handling |
| --- | --- |
| `interactionRequired` or task `needsUserAction` | Offer an explicit continuation; background callbacks must not loop on prompts |
| `vaultMissing` / `vaultLocked` / `vaultRequiresReload` | Preserve the distinct state and let an explicit action reach Core; never collect the Vault password |
| Permission denied, expired or revoked | Show failure and finish the affected operation; an old handle cannot grant access |
| `conflict` | Read the current task/storage/permission revision before a valid action submits again |
| `outcomeUnknown` | Check the actual result before retrying an operation with possible side effects |
| ABI succeeds but clicking fails | Check request/call ids, complete document, target and declared capabilities |

`run` / `watch` accept one `PluginHostRequest` JSON object per line. They have no Core broker: the test caller must supply typed replies to `api.request`. `watch` monitors the Wasm file without compiling source. Reload creates a new instance and replays the last explicit Initialize; in-memory state does not survive.

Finally, exercise allow, deny, revoke, cancel and close in the real application. Existing repository-example evidence and pending Windows/hardware acceptance remain in [implementation status](../../../README.en.md#installation-and-quick-start); they do not establish acceptance of your modified package.
