# protocolOpen

Ask the host to create a terminal launch record for a packaged protocol provider.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::ProtocolOpen { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "protocolOpen" | Yes | Discriminator; use the exact listed value |
| `request` | [PluginProtocolOpen](./types.en.md#pluginprotocolopen) | Yes | Typed request object for this method |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "protocolLaunched" | Yes | Discriminator; use the exact listed value |
| `launchId` | String | Yes | Host terminal launch record ID, not a connected state |

## Permissions, timing, and resource scope

Capability hint: `terminalProvider`. A declaration still requires an effective grant and any applicable exact-operation approval.

providerId must name a provider declared by the package; configuration follows its settings schema with allowed boolean/number/string values. The example uses Protocol Demo framedTcp and its endpoint setting; it applies only to a package declaring that provider. launchId identifies a launch record, not a connected terminal.

## Example call

```json
{
  "callId": "example.protocolOpen",
  "operation": {
    "kind": "protocolOpen",
    "request": {
      "providerId": "framedTcp",
      "configuration": {
        "endpoint": "tcp://127.0.0.1:19071"
      },
      "label": "Framed TCP demo"
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="protocolLaunched"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
