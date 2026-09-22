# serialSend

Send binary data to an owned serial resource.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::SerialSend { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialSend" | Yes | Discriminator; use the exact listed value |
| `request` | [PluginSerialSendRequest](./types.en.md#pluginserialsendrequest) | Yes | Typed request object for this method |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialSent" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

Capability hint: `deviceSerial`. A declaration still requires an effective grant and any applicable exact-operation approval.

Use the handle from serialOpen; one decoded send is at most 8 KiB. Continue reading serial events to observe device state.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.serialSend",
  "operation": {
    "kind": "serialSend",
    "request": {
      "handle": "019d0000-0000-7000-8000-000000000001",
      "dataBase64": "aGVsbG8="
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="serialSent"` before reading the fields above. Retain the handle, consume resourceEvents as needed, and eventually resourceClose. Acceptance is not remote-operation completion.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[serialDevices](./serialDevices.en.md) · [serialOpen](./serialOpen.en.md)
