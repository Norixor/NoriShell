# Plugin protocol and ABI

The sole current plugin protocol is **1.13**. The guest and Core must use that exact major/minor pair; there is no dual-protocol fallback. The protocol carries typed envelopes whose output request id matches the input request id. Serialize nested JSON strings with a serializer instead of building escaped JSON by hand.

The persistent Wasm ABI is `memory`, `nvx_alloc`, `nvx_handle`, and `nvx_dealloc`, with only `norishell.host.emit` imported. Core owns allocation/free ordering. Prefer `norishell_plugin_sdk` to obtain exact ABI exports and typed `PluginHostRequest`, `PluginRuntimeOutput`, `PluginApiCall`, and replies. `Initialize` starts or refreshes a host instance; it is not an app-command registration hook and has no authority beyond the received request. See the [developer ABI guide](../developers/wasm-abi.en.md).
