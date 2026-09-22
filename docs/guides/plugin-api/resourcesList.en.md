# resourcesList

Enumerate resources in the current caller ownership scope.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::ResourcesList { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "resourcesList" | Yes | Discriminator; use the exact listed value |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "resources" | Yes | Discriminator; use the exact listed value |
| `resources` | Vec&lt;[PluginApiResourceSummary](./types.en.md#pluginapiresourcesummary)&gt; | Yes | Resource summaries for the current owner |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Returns only owner-visible non-secret summaries. state distinguishes opening/open/closing/cleanupIncomplete.

## Example call

```json
{
  "callId": "example.resourcesList",
  "operation": {
    "kind": "resourcesList"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="resources"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
