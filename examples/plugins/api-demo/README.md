# NoriShell protocol-13 API SDK demo

This is a real Rust/Wasm plugin built with `norishell-plugin-sdk`. It keeps one
small state object for the lifetime of its Wasm instance, exports the complete
protocol-13 ABI through `export_plugin!`, and imports only
`norishell.host.emit`.

At initialization it contributes an `app.header.actions` dialog with a **Query
API** button. That initialization call does not make an API request. The direct
button click emits one typed `api.request` containing `PluginApiCall::Describe`.
The broker callback reads only its typed `PluginApiReply`, displays Core's real
method list and limits, and lets the user copy that displayed result. It does
not request Host, terminal, network, file, Vault, or system data.

The manifest is a local example package: its `publisher` is deliberately marked
as self-reported and it has no marketplace signature or publication step.

From the repository root, after `crates/plugin-sdk` has been added to the root
workspace by the integration change, build, run the production persistent
runtime ABI check, and make a local ZIP candidate with an adjacent SHA-256 file:

```sh
python3 examples/plugins/api-demo/build.py --output /tmp/NoriShell-API-Demo-1.0.1.zip
```

The script builds only this guest and the SDK verification example. The verifier
instantiates the generated Wasm with `WasmRuntime`, checks the sole import and
all required ABI exports, verifies the initial document, the typed `describe`
request, and a `brokerResult` callback using a fixture `PluginApiReply`.
It also verifies that the same persistent guest instance copies the callback's
description in response to the explicit copy action.
It does not install, publish, or test a live desktop broker.
