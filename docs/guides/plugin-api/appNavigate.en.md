# appNavigate

Navigate to an allowed app route or your plugin page in response to a user action.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::AppNavigate { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "appNavigate" | Yes | Discriminator; use the exact listed value |
| `destination` | [PluginAppNavigation](./types.en.md#pluginappnavigation) | Yes | Allowed app route or own-page destination |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "appAccepted" | Yes | Discriminator; use the exact listed value |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Requires an explicit user action. For destination.kind=pluginPage the field is page_id, not pageId; app uses path. Host route restrictions still apply.

## Example call

```json
{
  "callId": "example.appNavigate",
  "operation": {
    "kind": "appNavigate",
    "destination": {
      "kind": "app",
      "path": "/plugins"
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="appAccepted"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[appRegister](./appRegister.en.md) · [protocolOpen](./protocolOpen.en.md)
