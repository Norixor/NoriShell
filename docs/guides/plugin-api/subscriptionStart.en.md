# subscriptionStart

Subscribe to metadata changes within the allowed scope.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::SubscriptionStart { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "subscriptionStart" | Yes | Discriminator; use the exact listed value |
| `topics` | Vec&lt;[PluginSubscriptionTopic](./types.en.md#pluginsubscriptiontopic)&gt; | Yes | Typed metadata subscription topics |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "subscriptionStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

topics may be pluginSettings, hostScope, or terminalContext with a Core contextHandle. Events expose metadata only, not terminal contents, credentials, or arbitrary host data. Supply 1–16 unique topics. terminalContext requires declared and granted terminalMetadata; hostScope requires declared and granted hostMetadataRead. pluginSettings does not require those additional capabilities.

## Example call

```json
{
  "callId": "example.subscriptionStart",
  "operation": {
    "kind": "subscriptionStart",
    "topics": [
      {
        "kind": "pluginSettings"
      }
    ]
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="subscriptionStarted"` before reading the fields above. Retain the handle, consume resourceEvents as needed, and eventually resourceClose. Acceptance is not remote-operation completion.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
