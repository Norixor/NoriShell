# permissionRevoke

Revoke one remembered operation owned by the current plugin.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::PermissionRevoke { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionRevoke" | Yes | Discriminator; use the exact listed value |
| `permissionId` | String | Yes | Remembered-operation ID returned by permissions |
| `expectedPolicyRevision` | [WireSequence](./types.en.md#wiresequence) | Yes | Echo permissions.policyRevision |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionRevoked" | Yes | Discriminator; use the exact listed value |
| `policyRevision` | [WireSequence](./types.en.md#wiresequence) | Yes | Nullable policy revision as a decimal string |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Obtain permissionId and expectedPolicyRevision from permissions. On conflict, refresh before another user decision. This does not revoke capability grants.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.permissionRevoke",
  "operation": {
    "kind": "permissionRevoke",
    "permissionId": "019d0000-0000-7000-8000-000000000001",
    "expectedPolicyRevision": "1"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="permissionRevoked"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[permissions](./permissions.en.md) · [permissionRequest](./permissionRequest.en.md) · [permissionsForget](./permissionsForget.en.md)
