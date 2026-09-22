# resourceClose

Close an owned resource and trigger its cleanup.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::ResourceClose { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "resourceClose" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "closed" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Obtain handle from resource creation or resourcesList. Do not persist it across instances. On cleanupIncomplete retain the incomplete-cleanup state rather than showing the resource as released.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.resourceClose",
  "operation": {
    "kind": "resourceClose",
    "handle": "019d0000-0000-7000-8000-000000000001"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="closed"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md)
