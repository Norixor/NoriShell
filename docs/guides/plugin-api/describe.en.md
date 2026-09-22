# describe

Inspect broker methods, capability hints, and runtime limits.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::Describe { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "describe" | Yes | Discriminator; use the exact listed value |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "description" | Yes | Discriminator; use the exact listed value |
| `api` | [PluginApiDescription](./types.en.md#pluginapidescription) | Yes | Protocol version, platform, methods, and limits |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

availability=available means the interface is implemented, not that permissions, hardware, or native interaction are ready; handle actual call failures.

## Example call

```json
{
  "callId": "example.describe",
  "operation": {
    "kind": "describe"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="description"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
