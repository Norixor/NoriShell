# appNotify

Show a plugin notification in the host.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::AppNotify { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "appNotify" | Yes | Discriminator; use the exact listed value |
| `notification` | [PluginAppNotification](./types.en.md#pluginappnotification) | Yes | Notification ID and user-facing text |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "appAccepted" | Yes | Discriminator; use the exact listed value |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Requires an explicit user action; notifications belong to the current plugin instance.

## Example call

```json
{
  "callId": "example.appNotify",
  "operation": {
    "kind": "appNotify",
    "notification": {
      "id": "saved",
      "text": "Settings saved"
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="appAccepted"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[appRegister](./appRegister.en.md) · [appNavigate](./appNavigate.en.md)
