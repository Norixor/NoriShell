# Isolated UI: HTML/CSS/JS behind a Core bridge

Use an isolated UI when the normal declarative document is not enough for a richer package-owned surface. It is not a way to inject code into the NoriShell WebView. The Wasm guest still contributes a small host-rendered entry, then asks Core to open a verified HTML asset from the ZIP.

## Surface flow

The source demo's `Initialize` creates an `app.header.actions` dialog. Clicking **Open isolated demo** returns both a refreshed `ui.document` and `ui.webview.open` for its `isolated-demo` surface. The ZIP maps that surface to `assets/isolated/isolated-demo.html`. The Rust output below is an explanatory excerpt: keep the real function's imports, validated request, and document helper; it is not a complete plugin handler.

```rust
Ok(vec![
    document(&request.request_id, &body)?,
    output(&request.request_id, "ui.webview.open", &json!({
        "surfaceId": "isolated-demo",
        "title": "NoriShell Isolated API Demo",
        "width": 760,
        "height": 620
    }))?,
])
```

Core owns the isolated WebView. The page starts with its controls disabled, then waits for the parent window to send one `norishell.bridge.ready` event carrying protocol 13 and a MessagePort. Only that port carries its typed requests and replies. The JavaScript below is a message-shape excerpt, not a complete standalone page:

```js
port.postMessage({
  type: 'norishell.api.request',
  call: { callId: 'isolated-demo.describe.1', operation: { kind: 'describe' } }
});
```

The demo uses this bridge for API description, non-secret private storage, a protected HTTP GET, and a deliberately denied terminal-input test. The page has no direct network, Tauri, filesystem, Vault, terminal, or host-DOM access. A protected request can produce a pending state; it must wait for Core's typed reply instead of declaring success.

| If you see… | Input and expected result |
| --- | --- |
| The entry exists but no isolated window opens | The click must return both the refreshed document and `ui.webview.open`; confirm the requested `surfaceId` matches the packaged asset. |
| All controls remain disabled | Wait for the single Core `norishell.bridge.ready` event with protocol 13 and one MessagePort. Do not invent a port in page code. |
| A network control is pending | The needed input is Core's user review of the exact HTTPS request; the page must show pending until a typed reply arrives. |
| The terminal test is denied | That is the expected output: this package does not have terminal permission. |

## Package contents and validation

The isolated package contains `manifest.json`, `plugin.wasm`, and `assets/isolated/isolated-demo.html`. Its manifest requests `uiPanel`, `uiWebviewIsolated`, `storagePlugin`, and `networkDomain`.

From the NoriShell checkout root:

```sh
python3 examples/plugins/isolated-demo/build.py --output /tmp/NoriShell-Isolated-API-Demo-1.0.2.zip
```

The script builds Wasm and verifies the isolated-surface ABI before creating its ZIP. It does not install the package or prove the desktop WebView, MessagePort, protected prompt, or resource cleanup. Use [UI documents](../development/ui.en.md), [calling Core](../development/calling-api.en.md), and [packaging](../development/packaging.en.md) for the contracts that the asset must respect.
