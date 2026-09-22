# API Demo: one visible request and result

API Demo is the smallest complete visible plugin in the source examples. It requests only `uiPanel` and `clipboardWrite`. It does not read a host or terminal, make a network request, open a file, or access the Vault.

## User flow

After local import, approval, and enablement, open **API demo**. The `Initialize` message contributes a dialog at `app.header.actions`; no API call happens yet. Clicking **Query API** sends the typed `describe` operation. When Core returns `BrokerResult`, the guest renders Core's reported methods and limits in the same dialog. **Copy API description** performs a host-managed clipboard write from the current displayed text.

```text
Initialize -> ui.document("API demo")
Query API click -> UiAction -> api.request(describe)
Core reply -> BrokerResult -> ui.document(methods and limits)
Copy click -> clipboard.write + ui.document
```

## The guest code

The following action branch is an explanatory excerpt from the real source. It is **not** a complete `handle` implementation: keep the existing imports, payload parsing, state, and document helper when adapting it. The call id is correlation, not a permission token.

```rust
PluginHostMessageKind::UiAction => match body["actionId"].as_str() {
    Some("api-demo:describe") => Ok(vec![api_request(
        &request_id,
        "api-demo.describe",
        PluginApiOperation::Describe {},
    )?]),
    _ => Err(PluginError::InvalidRequest),
}
```

The corresponding runtime output carries a JSON payload like this:

```json
{
  "callId": "api-demo.describe",
  "operation": { "kind": "describe" }
}
```

Core sends the matching reply in a `BrokerResult` payload. The following is also an explanatory excerpt, not a paste-ready handler. The demo parses `PluginApiReply`, accepts a completed `description` value or a stable failure code, saves only the rendered non-secret text in its instance state, then emits the replacement document:

```rust
PluginHostMessageKind::BrokerResult => {
    let reply: PluginApiReply = serde_json::from_value(body["result"]["reply"].clone())
        .map_err(|_| PluginError::InvalidRequest)?;
    let text = render_description(&reply)?;
    self.latest_description = Some(text.clone());
    Ok(vec![document_output(&request_id, &text)?])
}
```

A successful reply has the same correlation id and a typed outcome. The JSON below is a **complete legal illustrative value** for the current DTO, but its method list, platform, and limits are examples only; use the running host's `describe` result as the source of truth:

```json
{
  "callId": "api-demo.describe",
  "outcome": {
    "kind": "completed",
    "value": {
      "kind": "description",
      "api": {
        "protocolMajor": 1,
        "protocolMinor": 13,
        "platform": "fixture",
        "methods": [{ "name": "describe", "capability": null, "availability": "available" }],
        "limits": {
          "maxCallBytes": 65536,
          "maxChunkBytes": 16384,
          "maxResources": 32,
          "maxPendingEvents": 32
        }
      }
    }
  }
}
```

Expected desktop result: after the click, the dialog changes from its “Select Query API” hint to Core API version, platform, methods, and limits. If Core reports a failure, the dialog renders that failure rather than pretending the request succeeded.

| If you see… | Check this first |
| --- | --- |
| No **API demo** entry after enablement | Confirm the package was enabled and that `Initialize` emitted one valid `ui.document` for `app.header.actions`. |
| The dialog opens but **Query API** has no result | Confirm the action id is `api-demo:describe`, the call id is valid, and the next message is the matching `BrokerResult`. |
| Copy does not update the clipboard | Use the rendered Copy control after the explicit click and retain the `clipboardWrite` capability approval. |

## Build and validate the local candidate

From the NoriShell checkout root:

```sh
python3 examples/plugins/api-demo/build.py --output /tmp/NoriShell-API-Demo-1.0.1.zip
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/NoriShell-API-Demo-1.0.1.zip
```

The script builds the Wasm, runs the persistent runtime ABI verifier, creates the ZIP, and writes `/tmp/NoriShell-API-Demo-1.0.1.zip.sha256`. The separate `check` command validates the package shape and Wasm ABI. These are local validations; complete the real import and click path described in [Quick start](../start/quickstart.en.md). For the contracts behind the snippets, continue with [calling Core](../development/calling-api.en.md) and [UI documents](../development/ui.en.md).
