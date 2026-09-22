# appRegister

Register commands, optional shortcuts, and status text.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::AppRegister { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "appRegister" | Yes | Discriminator; use the exact listed value |
| `registration` | [PluginAppRegistration](./types.en.md#pluginappregistration) | Yes | Command and status declarations; see the object table |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "appAccepted" | Yes | Discriminator; use the exact listed value |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Call from an explicit user action. At most 24 commands and 8 statuses; actionId must reference an enabled button in a verified document. Shortcuts support Alt+Shift+uppercase letter and require host opt-in. File extensions do not grant file access.

## Example call

```json
{
  "callId": "example.appRegister",
  "operation": {
    "kind": "appRegister",
    "registration": {
      "commands": [
        {
          "id": "open-text",
          "label": "Open text file",
          "targetId": "app.header.actions",
          "actionId": "app-demo.pick",
          "pageId": null,
          "shortcut": "Alt+Shift+O",
          "fileExtensions": [
            "txt"
          ]
        }
      ],
      "statuses": [
        {
          "id": "ready",
          "text": "Ready"
        }
      ]
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="appAccepted"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[appNotify](./appNotify.en.md) · [appNavigate](./appNavigate.en.md) · [filePick](./filePick.en.md)
