# remoteExecStart

Create an independent SSH command resource within an authorized host scope.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::RemoteExecStart { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remoteExecStart" | Yes | Discriminator; use the exact listed value |
| `request` | [PluginRemoteExecStartRequest](./types.en.md#pluginremoteexecstartrequest) | Yes | Typed request object for this method |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remoteExecStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

Capability hint: `remoteExecRequest`. A declaration still requires an effective grant and any applicable exact-operation approval.

hostHandle comes from the current Core-issued host context. Core resolves the saved host, credentials, route, and Known Hosts; no guest-controlled SSH session is accepted. Execution results arrive through remoteExec events. command is nonempty, at most 16 KiB, and contains no NUL; timeoutMs is 100–120000.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.remoteExecStart",
  "operation": {
    "kind": "remoteExecStart",
    "request": {
      "hostHandle": "019d0000-0000-7000-8000-000000000001",
      "command": "uname -s",
      "timeoutMs": 10000
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="remoteExecStarted"` before reading the fields above. Retain the handle, consume resourceEvents as needed, and eventually resourceClose. Acceptance is not remote-operation completion.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
