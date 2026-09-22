# permissionsForget

Forget all remembered operations still owned by the current plugin.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::PermissionsForget { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionsForget" | Yes | Discriminator; use the exact listed value |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionsForgotten" | Yes | Discriminator; use the exact listed value |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Only removes remembered operations still owned by the current package; it does not change capability grants or delete history owned by old packages or other signers.

## Example call

```json
{
  "callId": "example.permissionsForget",
  "operation": {
    "kind": "permissionsForget"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="permissionsForgotten"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[permissions](./permissions.en.md) · [permissionRequest](./permissionRequest.en.md) · [permissionRevoke](./permissionRevoke.en.md)
