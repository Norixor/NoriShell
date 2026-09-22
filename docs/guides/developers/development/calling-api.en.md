# Calling APIs and handling replies

NoriShell plugins send typed requests to the host through the protocol-1.13 Wasm ABI. The current Rust SDK provides typed wrappers for that ABI; another ABI-compliant guest may serialize the same envelopes itself. You do not need an HTTP endpoint or an API key, and plugins must not call Tauri IPC. Query `describe` first, then use the methods available in the running host.

## The request and reply cycle

| Step | Incoming message | Your responsibility |
| --- | --- | --- |
| 1. Create the interface | `initialize` | Return an initial `ui.document` with an explicit action |
| 2. Handle a click | `uiAction` | Check `actionId` and send the requested operation with `api_request` |
| 3. Wait for the host | No successful result yet | Wait for permission or execution; do not report success early |
| 4. Receive the reply | `brokerResult` | Read `reply` inside `result.kind = "api"`, then check `callId` and `outcome` |
| 5. Update the interface | The current callback's `requestId` | Return a complete replacement `ui.document` with the result or next action |

Long-lived network, process and serial methods also return resource handles. **A handle is the beginning of an operation**, not evidence of remote success. Read resource events and close resources when finished. See [Resources and permissions](./resources.en.md).

## SDK request helper

```rust
use norishell_plugin_sdk::{
    api_request, PluginApiOperation, PluginError,
    PluginHostRequest, PluginRuntimeOutput,
};

fn request_methods(request: &PluginHostRequest)
    -> Result<PluginRuntimeOutput, PluginError>
{
    api_request(
        &request.request_id,
        "describe-1",
        PluginApiOperation::Describe {},
    )
}
```

This helper can be added to a plugin project. Return `Ok(vec![request_methods(&request)?])` from the matching `UiAction` branch. [API Demo](../examples/api-demo.en.md) covers the surrounding interface and event handling.

| Parameter | Type | How to set it |
| --- | --- | --- |
| `request_id` | `&str` | Use the current `PluginHostRequest.request_id`; do not invent one or reuse a previous callback's ID |
| `call_id` | String | Assign 1–80 bytes using only ASCII letters, digits, `.`, `_` and `-`; do not use colons |
| `operation` | `PluginApiOperation` | Choose a typed enum variant; this example uses `Describe {}` |
| Return value | `Result<PluginRuntimeOutput, PluginError>` | Add it to the current `handle` output array; the SDK serializes and wraps the request |

Give concurrent outstanding calls different `callId` values and track their actions in instance state. A `callId` correlates a reply; it is neither a resource handle nor a permission.

## Reading the JSON examples

The [method reference](../../plugin-api/README.en.md) shows `PluginApiCall` objects:

```json
{
  "callId": "describe-1",
  "operation": { "kind": "describe" }
}
```

This is not an HTTP body. The SDK serializes it into `PluginRuntimeOutput.payloadJson`, sets `kind: "api.request"`, and binds `requestId` to the current host request. With `api_request`, you do not need to manually escape nested JSON strings.

## Parsing success and failure

The helper below only parses the reply for `describe-1` and returns display text. After calling it, build the complete replacement document as demonstrated in API Demo.

```rust
use norishell_plugin_sdk::{
    payload, PluginApiOutcome, PluginApiReply, PluginApiValue,
    PluginError, PluginHostMessageKind, PluginHostRequest,
};
use serde_json::Value;

fn describe_text(request: &PluginHostRequest) -> Result<String, PluginError> {
    if request.kind != PluginHostMessageKind::BrokerResult {
        return Err(PluginError::InvalidRequest);
    }
    let body: Value = payload(request)?;
    let result = &body["result"];
    if result["kind"].as_str() != Some("api") {
        return Err(PluginError::InvalidRequest);
    }
    let reply: PluginApiReply = serde_json::from_value(result["reply"].clone())
        .map_err(|_| PluginError::InvalidRequest)?;
    if reply.call_id != "describe-1" {
        return Err(PluginError::InvalidRequest);
    }
    match reply.outcome {
        PluginApiOutcome::Completed {
            value: PluginApiValue::Description { api },
        } => serde_json::to_string_pretty(&api)
            .map_err(|_| PluginError::HandlerFailed),
        PluginApiOutcome::Failed { code } => Ok(format!("Request failed: {code:?}")),
        _ => Err(PluginError::InvalidRequest),
    }
}
```

| Reply layer | Content | Handling rule |
| --- | --- | --- |
| `PluginHostRequest.kind` | `brokerResult` | Parse in the correct message branch |
| `result.kind` inside `payloadJson` | `api` | Do not interpret other broker replies as API replies |
| `result.reply.callId` | The matching call's ID | Find the corresponding pending action and interface |
| `outcome.kind = completed` | A `value` union tagged by `kind` | Match the result variant documented for the method |
| `outcome.kind = failed` | A stable `code` | Show an actionable next step rather than swallowing the failure |

## Discover availability first

`describe` returns the protocol version, platform, methods and limits. Read each method's `name`, `capability` and `availability`. Only call methods marked `available`, and still satisfy their permission requirements. Represent `notImplemented` and `unsupportedPlatform` as unavailable features in your plugin.

`limits` reports the current bounds on call bytes, data chunks, resources and pending events. Use these runtime values instead of treating example numbers as a promise across every platform.

## Host message reference

| `kind` | Purpose | Handling |
| --- | --- | --- |
| `initialize` | Wasm instance initialization | Set up bounded in-memory state and initial UI; initialization does not authorize app actions |
| `uiAction` | A declarative UI action | Handle the action and fields; return a document or an explicit asynchronous broker request |
| `brokerResult` | A Core broker reply | Match the call and update state or UI |
| `workflowEvent` | Workflow steps and resource events | Use `workflow_event` / `workflow_response` for the matching task |
| `protocolEvent` | A protocol provider lifecycle event | Use `protocol_event` / `protocol_response` for bytes and state |
| `terminalObservation` | Approved terminal observation | Process only the observation data within the granted scope |
| `invoke` | A host-dispatched plugin invocation | Follow that feature's payload contract; it does not imply unrestricted user permission |
| `sshSyncResult` | An SSH synchronization result | Needed only by plugins implementing that integration |

The host serializes `handle` calls for a Wasm instance. Plugins do not need their own network thread or event loop. Store state needed across messages in the plugin struct, and account for disable, restart and resource closure.

Continue with [Declarative UI](./ui.en.md), [Resources and permissions](./resources.en.md), or the [API reference](../../plugin-api/README.en.md).

Availability does not grant access to every invocation path. In the current implementation, `permissionRequest` requires a Core declarative action; a Wasm API request cannot open it directly. `terminalRequestInput` also returns `interactionRequired` from an isolated Wasm invocation, including a click callback. Use the documented Core action flow for these interactions.
