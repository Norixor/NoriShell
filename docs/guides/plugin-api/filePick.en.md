# filePick

Open a native picker and request an exact local-file access scope.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::FilePick { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "filePick" | Yes | Discriminator; use the exact listed value |
| `pickerKind` | [PluginFilePickerKind](./types.en.md#pluginfilepickerkind) | Yes | Native file or directory picker mode |
| `access` | [PluginFileAccessRequest](./types.en.md#pluginfileaccessrequest) | Yes | Independent access flags, each defaulting to false |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "filePicked" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `label` | String | Yes | User-facing display label |

## Permissions, timing, and resource scope

Capability hint: `localFiles`. A declaration still requires an effective grant and any applicable exact-operation approval.

Native selection requires an explicit user action. Each omitted access boolean defaults to false. Use an empty relativePath for a file handle and relative paths for a directory handle; arbitrary absolute paths cannot replace selection.

## Example call

```json
{
  "callId": "example.filePick",
  "operation": {
    "kind": "filePick",
    "pickerKind": "file",
    "access": {
      "read": true
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="filePicked"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[file](./file.en.md) · [taskStart](./taskStart.en.md)
