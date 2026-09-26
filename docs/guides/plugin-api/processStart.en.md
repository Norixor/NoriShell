# processStart

Request execution of an absolute executable path with a fixed argument list.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::ProcessStart { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "processStart" | Yes | Discriminator; use the exact listed value |
| `program` | String | Yes | Absolute executable path on this platform |
| `arguments` | Vec&lt;String&gt; | Yes | Separate arguments, not a concatenated shell command |
| `timeoutMs` | u32 | Yes | Timeout in milliseconds, subject to Core bounds |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "processStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## Permissions, timing, and resource scope

Capability hint: `localProcess`. A declaration still requires an effective grant and any applicable protected-operation approval.
Remembered approval is bound to the executable path, Core-verified file identity, and full argument list; changing the program or arguments requires a new approval. Changing `timeoutMs` does not change the approved command, but Core still validates every request and revocation.

program must be an absolute executable available on this platform; /bin/cat applies only where that executable exists. timeoutMs is 1–120000; at most 128 arguments, each at most 4096 bytes without NUL. Core owns the working directory and environment.

## Example call

```json
{
  "callId": "example.processStart",
  "operation": {
    "kind": "processStart",
    "program": "/bin/cat",
    "arguments": [],
    "timeoutMs": 10000
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="processStarted"` before reading the fields above. Retain the handle, consume resourceEvents as needed, and eventually resourceClose. Acceptance is not remote-operation completion.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[describe](./describe.en.md) · [resourcesList](./resourcesList.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
