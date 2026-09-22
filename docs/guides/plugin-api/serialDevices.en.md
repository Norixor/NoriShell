# serialDevices

List serial-device candidates for user selection.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::SerialDevices { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialDevices" | Yes | Discriminator; use the exact listed value |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialDevices" | Yes | Discriminator; use the exact listed value |
| `devices` | Vec&lt;[PluginSerialDeviceCandidate](./types.en.md#pluginserialdevicecandidate)&gt; | Yes | Candidate metadata without native device paths |

## Permissions, timing, and resource scope

Capability hint: `deviceSerial`. A declaration still requires an effective grant and any applicable exact-operation approval.

candidateId is a short-lived lookup reference, not a device path or opening grant. Devices can disappear; refresh when selecting again. See the index for native and hardware availability. Candidates expire after two minutes; rediscovery replaces old candidates for the current consumer.

## Example call

```json
{
  "callId": "example.serialDevices",
  "operation": {
    "kind": "serialDevices"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="serialDevices"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[serialOpen](./serialOpen.en.md) · [serialSend](./serialSend.en.md)
