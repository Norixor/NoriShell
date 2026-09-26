# sftpOpen

Request an independent SFTP root resource for an authorized host.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::SftpOpen { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "sftpOpen" | Yes | Discriminator; use the exact listed value |
| `hostHandle` | String | Yes | Opaque reference from a Core host context |
| `rootPath` | String | Yes | Remote root path requested for approval |
| `write` | bool | Yes | Request writable scope; capability and approval still required |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "sftp" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginSftpResult](./types.en.md#pluginsftpresult) | Yes | Nested result union; inspect result.kind first |

## Permissions, timing, and resource scope

Capability hint: `sftpRead`. A declaration still requires an effective grant and any applicable protected-operation approval.

hostHandle comes from a Core host context, not a HostId or SSH session ID. Core approves rootPath before issuing a root handle; write=true also requires sftpWrite. The connection is independent of existing terminals.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.sftpOpen",
  "operation": {
    "kind": "sftpOpen",
    "hostHandle": "019d0000-0000-7000-8000-000000000001",
    "rootPath": ".",
    "write": false
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="sftp"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[sftp](./sftp.en.md) · [resourceClose](./resourceClose.en.md)
