# networkStart

Create an HTTP, WebSocket, TCP, UDP, or TLS resource for one endpoint.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::NetworkStart { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "networkStart" | Yes | Discriminator; use the exact listed value |
| `endpoint` | [PluginNetworkEndpointRequest](./types.en.md#pluginnetworkendpointrequest) | Yes | Full endpoint URL for Core resolution and approval |
| `request` | [PluginNetworkStartRequest](./types.en.md#pluginnetworkstartrequest) | Yes | Typed request object for this method |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "networkStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

Capability hint: `networkDomain`. A declaration still requires an effective grant and any applicable exact-operation approval.

Core freezes the resolved endpoint and performs protected approval. Do not provide resolved IPs, proxies, or authorization flags. credential optionally references a credential handle/revision. After receiving the handle, consume network opened, httpResponse, data, closed, or error events. endpoint is at most 2048 bytes; timeoutMs is 100–120000.

## Example call

```json
{
  "callId": "example.networkStart",
  "operation": {
    "kind": "networkStart",
    "endpoint": {
      "endpoint": "https://example.test/status"
    },
    "request": {
      "timeoutMs": 10000,
      "operation": {
        "kind": "http",
        "method": "get",
        "headers": [],
        "bodyBase64": ""
      }
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="networkStarted"` before reading the fields above. Retain the handle, consume resourceEvents as needed, and eventually resourceClose. Acceptance is not remote-operation completion.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
