# permissionRequest

Request a capability grant from an explicit user action.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::PermissionRequest { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

**Entry restriction:** The success path uses a host declarative action. The JSON below documents the wire shape; Wasm SDK can construct it, but the current isolated entry will not execute this protected interaction.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionRequest" | Yes | Discriminator; use the exact listed value |
| `capability` | [PluginCapability](./types.en.md#plugincapability) | Yes | PluginCapability enum value, not an existing grant |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionRequested" | Yes | Discriminator; use the exact listed value |
| `approvalId` | String | Yes | Pending approval request ID, not an approval grant |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Currently supported only through the host declarative-action entry. api_request from Wasm isolated returns unsupported, including calls originating from uiAction. The capability must be declared by the package and belong to the Core special-permission set. Undeclared capabilities return permissionDenied; unsupported capability kinds return unsupported. approvalId identifies a request, not a grant; refresh permissions afterward.

## Example call

```json
{
  "callId": "example.permissionRequest",
  "operation": {
    "kind": "permissionRequest",
    "capability": "networkDomain"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="permissionRequested"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[permissions](./permissions.en.md) · [permissionRevoke](./permissionRevoke.en.md) · [permissionsForget](./permissionsForget.en.md)
