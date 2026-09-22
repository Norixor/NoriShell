# Languages and UI boundaries

NoriShell loads a guest that implements the protocol-1.13 Wasm ABI. A guest may be authored in any language that can produce that exact ABI; the current maintained authoring SDK is Rust. HTML, CSS, and JavaScript have a separate role inside a package-owned **isolated UI**; they are not a second main-plugin SDK.

| Technology | Current role | What it may do | Boundary to keep |
| --- | --- | --- | --- |
| Rust + `norishell-plugin-sdk` | Official guest authoring path | Build a `cdylib` for `wasm32-unknown-unknown`, handle typed host messages, emit `ui.document` and typed `api.request` outputs | Use the SDK's `export_plugin!`; the guest has no ambient desktop, network, filesystem, Vault, or Tauri access. |
| Wasm target | Runtime format for the guest | Expose the protocol-1.13 guest ABI that Core validates when loading the package | One current ABI only; no WASI, native library, dynamic download, or fallback protocol. |
| HTML/CSS/JavaScript | Package asset for an isolated WebView surface | Render a richer self-contained surface after Core opens its verified asset; use the Core-provided MessagePort | The page does not gain direct network, Tauri, filesystem, Vault, terminal, or host-DOM access. See [Isolated UI](../examples/isolated-ui.en.md). |
| Other compiled-to-Wasm languages | Author a guest that implements the exact current Wasm ABI and package rules | Produce the required exports, typed protocol envelopes, and package contents | NoriShell currently ships a Rust SDK only. Other languages must implement and validate the ABI themselves; the host accepts an ABI-compliant guest regardless of its implementation language. |

## Rust guest model

The SDK provides the ABI exports, protocol envelopes, and typed request/result DTOs. A normal plugin has one bounded state object for the life of its Wasm instance:

```rust
use norishell_plugin_sdk::{Plugin, PluginError, PluginHostRequest, PluginRuntimeOutput, export_plugin};

#[derive(Default)]
struct MyPlugin;

impl Plugin for MyPlugin {
    fn handle(&mut self, request: PluginHostRequest)
        -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        // Match Initialize, UiAction, BrokerResult, and the events your plugin owns.
        Ok(Vec::new())
    }
}

export_plugin!(MyPlugin);
```

The instance state is not durable storage and never represents permission. Core can stop or replace an instance; resource handles are opaque, owner-scoped, and can be closed by revocation, disablement, replacement, cancellation, or a stale generation.

## Isolated HTML surface model

An isolated UI uses a normal guest contribution to ask Core to open a packaged HTML asset. Core creates the isolated WebView and passes a single constrained MessagePort after the `norishell.bridge.ready` handshake. The page sends typed `norishell.api.request` messages over that port and receives typed replies; it must wait for the port before enabling controls.

That design keeps HTML/CSS/JS useful for presentation while retaining the same Core approval and resource boundaries. It does not make browser APIs an escape hatch. Read [Isolated UI](../examples/isolated-ui.en.md) before choosing this surface, and use [calling Core](../development/calling-api.en.md) for the operation and error rules.
