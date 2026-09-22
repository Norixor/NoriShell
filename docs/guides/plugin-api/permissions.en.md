# permissions

Read current capability grants and manageable remembered-operation summaries.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::Permissions { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissions" | Yes | Discriminator; use the exact listed value |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissions" | Yes | Discriminator; use the exact listed value |
| `grants` | Vec&lt;[PluginCapabilityGrant](./types.en.md#plugincapabilitygrant)&gt; | Yes | Current capability grant records |
| `policyRevision` | Option&lt;[WireSequence](./types.en.md#wiresequence)&gt; | No | Nullable policy revision as a decimal string |
| `operationPermissions` | Vec&lt;[PluginOperationPermission](./types.en.md#pluginoperationpermission)&gt; | Yes | Non-secret summaries of owned remembered operations |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

policyRevision is a nullable decimal string. Use the returned value for revocation. operationPermissions contains only non-secret summaries owned by the current package, signer, and effective grant binding.

## Example call

```json
{
  "callId": "example.permissions",
  "operation": {
    "kind": "permissions"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="permissions"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[permissionRequest](./permissionRequest.en.md) · [permissionRevoke](./permissionRevoke.en.md) · [permissionsForget](./permissionsForget.en.md)
