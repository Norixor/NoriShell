# timerStart

Create a one-shot or repeating timer.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::TimerStart { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "timerStart" | Yes | Discriminator; use the exact listed value |
| `delayMs` | u32 | Yes | Milliseconds until the first timer firing |
| `intervalMs` | Option&lt;u32&gt; | No | Repeat interval in milliseconds; omit for one-shot |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "timerStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Omit intervalMs for a one-shot timer. Consume timerFired through resourceEvents and close when done. Timer callbacks are background contexts and cannot create approvals or terminal input. delayMs and intervalMs, when present, must each be 100–86400000 milliseconds.

## Example call

```json
{
  "callId": "example.timerStart",
  "operation": {
    "kind": "timerStart",
    "delayMs": 1000
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="timerStarted"` before reading the fields above. Retain the handle, consume resourceEvents as needed, and eventually resourceClose. Acceptance is not remote-operation completion.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
