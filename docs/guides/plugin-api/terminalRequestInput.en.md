# terminalRequestInput

Request text input into an existing terminal, with Core rechecking focus and authority before writing.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::TerminalRequestInput { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

**Entry restriction:** The success path uses a host declarative action. The JSON below documents the wire shape; Wasm SDK can construct it, but the current isolated entry will not execute this protected interaction.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "terminalRequestInput" | Yes | Discriminator; use the exact listed value |
| `terminalHandle` | String | Yes | Reference from the current Core terminal context |
| `payload` | String | Yes | Text proposed for terminal input |
| `appendEnter` | bool | Yes | Whether to append Enter after the text |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "inputApprovalRequested" | Yes | Discriminator; use the exact listed value |
| `approvalId` | String | Yes | Pending approval request ID, not an approval grant |

May also return `{"kind":"inputSent"}` with no other fields.

## Permissions, timing, and resource scope

Capability hint: `terminalRequestInput`. A declaration still requires an effective grant and any applicable exact-operation approval.

terminalHandle comes from the current Core terminal context. Requires an explicit user action. Core rechecks generation, focus, and input ownership immediately before writing. inputApprovalRequested awaits approval; inputSent means input was written, not shell command completion. The current Wasm isolated entry always returns interactionRequired; actual input requires the host declarative-action path. api_request cannot manufacture focus or change entry identity.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.terminalRequestInput",
  "operation": {
    "kind": "terminalRequestInput",
    "terminalHandle": "019d0000-0000-7000-8000-000000000001",
    "payload": "pwd",
    "appendEnter": false
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="inputApprovalRequested"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
