# Protocol-13 Wasm ABI

One Wasm instance remains resident for the life of its Plugin Host instance, so bounded guest state may live in the plugin object. It is still invalid to treat guest memory as permission or durable storage. Core serializes calls to one instance and may stop it on lifecycle, quota, or failure boundaries.

The module exports linear `memory`, `nvx_alloc(i32) -> i32`, `nvx_handle(i32, i32) -> i32`, and `nvx_dealloc(i32, i32)`. Its **only** import is `norishell.host.emit(i32, i32) -> i32`. There is no WASI surface. Core allocates request bytes with `nvx_alloc`, invokes `nvx_handle`, then releases the same allocation through `nvx_dealloc` exactly once. A non-zero `nvx_handle` status reports a guest ABI failure without serializing private details.

Use `norishell_plugin_sdk::export_plugin!` rather than copying an ABI shim. The SDK generates the complete exports, the sole host import, envelopes bound to the manifest-declared supported minor, and serializes outputs tied to the received request id.

```rust
use norishell_plugin_sdk::{Plugin, PluginError, PluginHostRequest, PluginRuntimeOutput};

#[derive(Default)]
struct Inspector;
impl Plugin for Inspector {
    fn handle(&mut self, request: PluginHostRequest)
      -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        Ok(Vec::new())
    }
}
norishell_plugin_sdk::export_plugin!(Inspector);
```

See [broker calls](broker.en.md) for useful outputs. Do not invent a second ABI for a previous protocol minor.
