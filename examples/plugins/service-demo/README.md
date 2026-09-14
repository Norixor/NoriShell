# Core-brokered public service demo

This protocol 1.13 package fetches the public, non-secret metadata for `rust-lang/rust` from `https://api.github.com/repos/rust-lang/rust`. The guest has no HTTP client, socket, proxy, credential, filesystem, or Tauri access. Its only network call is `PluginApiOperation::NetworkStart`; Core resolves the exact HTTPS endpoint, asks for the `networkDomain` approval when the user explicitly continues the task, performs the request without redirects or ambient proxying, and returns real HTTP resource events.

The dialog deliberately has no background `onOpen` call. Select **Query GitHub** to create a Core workflow, then use **Refresh** until the task asks for a user action. Select **Continue query** to open the Core permission prompt and perform the request. Refresh again after completion to render the actual HTTP status and the validated repository name, description, and star count. **Cancel query** revision-fences cancellation while the task is active. Resource events are consumed only by the Core-owned workflow; finishing, failing, or cancelling the task closes its owner-scoped resource.

The guest accepts a single `200` JSON response with a JSON content type, caps accumulated body bytes at 64 KiB, validates UTF-8 and the expected repository fields, and rejects incomplete, backpressured, non-JSON, non-200, or malformed responses. Returned response data stays in the live Wasm instance only; it is not written to plugin storage, task persistence, or logs.

Build and package from the repository root:

```sh
python3 examples/plugins/service-demo/build.py --output /tmp/NoriShell-Service-Demo-1.0.1.zip
```

Version 1.0.1 fixes the header dialog startup by using a host-supported icon; its initialization test validates the complete document through the production dialog validator.

The script runs guest parsing and initial dialog contract tests, builds a `wasm32-unknown-unknown` release module in a temporary target, uses the SDK's deterministic `pack`, validates the package with SDK `check`, and writes a SHA-256 sidecar. These checks prove the guest ABI and package structure only. Native desktop installation, the `networkDomain` approval UI, live GitHub HTTPS request, response rendering, cancellation, and Core resource cleanup remain required acceptance checks.
