# file

Read, write, list, and watch within a selected local-file scope.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::File { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "file" | Yes | Discriminator; use the exact listed value |
| `operation` | [PluginFileOperation](./types.en.md#pluginfileoperation) | Yes | Nested discriminated union; select fields by kind |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "file" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginFileResult](./types.en.md#pluginfileresult) | Yes | Nested result union; inspect result.kind first |

## Permissions, timing, and resource scope

Capability hint: `localFiles`. A declaration still requires an effective grant and any applicable exact-operation approval.

rootHandle comes from filePick. Read/write chunks are at most 16 KiB, materialized files 64 MiB, and listing pages 100 entries. Existing writes require the current fingerprint; omitting expectedFingerprint is create-only. Rename never overwrites. watchStart returns a separate event handle.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.file",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "read",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "",
      "offset": 0
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="file"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).

## Sub-operations

The inner `operation.kind` selects a branch below. Full fields and result objects are in the [type reference](./types.en.md#pluginfileoperation).

| kind | Parameters (? means optional) | result.kind |
| --- | --- | --- |
| `read` | `rootHandle`, `relativePath`, `offset` | `read` |
| `write` | `rootHandle`, `relativePath`, `expectedFingerprint?`, `offset`, `dataBase64`, `finalSize?` | `written` |
| `list` | `rootHandle`, `relativePath`, `cursor?`, `limit` | `listed` |
| `rename` | `rootHandle`, `fromPath`, `toPath`, `expectedFingerprint` | `renamed` |
| `remove` | `rootHandle`, `relativePath`, `expectedFingerprint`, `recursive` | `removed` |
| `watchStart` | `rootHandle`, `relativePath`, `intervalMs` | `watchStarted` |

Each example below is a standalone PluginApiCall. Replace every handle, fingerprint, precondition, and revision with preceding query results; these examples are not directly executable.

### read

```json
{
  "callId": "example.file.read",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "read",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "offset": 0
    }
  }
}
```

### write

```json
{
  "callId": "example.file.write",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "write",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "offset": 0,
      "dataBase64": "YQ=="
    }
  }
}
```

### list

```json
{
  "callId": "example.file.list",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "list",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "limit": 16
    }
  }
}
```

### rename

```json
{
  "callId": "example.file.rename",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "rename",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "fromPath": "notes.txt",
      "toPath": "renamed.txt",
      "expectedFingerprint": "fingerprint-from-read"
    }
  }
}
```

### remove

```json
{
  "callId": "example.file.remove",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "remove",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "expectedFingerprint": "fingerprint-from-read",
      "recursive": false
    }
  }
}
```

### watchStart

```json
{
  "callId": "example.file.watchStart",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "watchStart",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "intervalMs": 1000
    }
  }
}
```


## Related methods

[filePick](./filePick.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
