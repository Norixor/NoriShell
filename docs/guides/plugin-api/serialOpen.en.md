# serialOpen

Request protected approval for a selected serial candidate and settings, then create its resource.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::SerialOpen { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialOpen" | Yes | Discriminator; use the exact listed value |
| `candidateId` | String | Yes | Short-lived candidate returned by serialDevices |
| `settings` | [PluginSerialSettings](./types.en.md#pluginserialsettings) | Yes | Baud rate, data bits, parity, stop bits, and flow control |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

Capability hint: `deviceSerial`. A declaration still requires an effective grant and any applicable protected-operation approval.

candidateId must come from serialDevices; baudRate is 300–4000000. Other settings use the enum strings in the type reference. Core rechecks device identity after protected confirmation. Observe serial opened/data/error/closed events for actual state.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.serialOpen",
  "operation": {
    "kind": "serialOpen",
    "candidateId": "019d0000-0000-7000-8000-000000000001",
    "settings": {
      "baudRate": 115200,
      "dataBits": "eight",
      "parity": "none",
      "stopBits": "one",
      "flowControl": "none"
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="serialStarted"` before reading the fields above. Retain the handle, consume resourceEvents as needed, and eventually resourceClose. Acceptance is not remote-operation completion.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[serialDevices](./serialDevices.en.md) · [serialSend](./serialSend.en.md)
