# resourceEvents

Drain bounded resource events in batches.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::ResourceEvents { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "resourceEvents" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `limit` | u16 | Yes | Maximum items or events returned in this call |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "resourceEvents" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `events` | Vec&lt;[PluginApiResourceEvent](./types.en.md#pluginapiresourceevent)&gt; | Yes | Events to process in sequence order |
| `backpressured` | bool | Yes | Whether the resource-event queue is backpressured |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Use a resource-creation handle. Process events in sequence order and bound polling; backpressured=true reports queue backpressure. processOutput uses data_base64 and processExited uses optional exit_code; other nested events follow their own DTOs. limit must be 1–describe.limits.maxPendingEvents.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.resourceEvents",
  "operation": {
    "kind": "resourceEvents",
    "handle": "019d0000-0000-7000-8000-000000000001",
    "limit": 32
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="resourceEvents"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceClose](./resourceClose.en.md)

For `httpExchange`, `network.httpExchangeCompleted` carries an opaque receipt and optional response blob handle; pass them to dataInspect or dataCheckpoint after checking status. The event never contains exchange bytes.
