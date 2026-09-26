# sftp

Operate on remote files using root and entry handles.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::Sftp { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "sftp" | Yes | Discriminator; use the exact listed value |
| `operation` | [PluginSftpOperation](./types.en.md#pluginsftpoperation) | Yes | Nested discriminated union; select fields by kind |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "sftp" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginSftpResult](./types.en.md#pluginsftpresult) | Yes | Nested result union; inspect result.kind first |

## Permissions, timing, and resource scope

Capability hint: `sftpRead`. A declaration still requires an effective grant and any applicable protected-operation approval.

rootHandle comes from sftpOpen; directoryHandle, entryHandle, and precondition come from list/read. Mutations also require sftpWrite. Pages are at most 100 entries, chunks 16 KiB, and staged uploads 64 MiB. Only regular files and real directories are exposed. Do not invent remote paths or session IDs. Upload ordered chunks, then commit; abort on failure. Closing the root also cleans staging.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.sftp",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "list",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "limit": 20
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="sftp"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).

## Sub-operations

The inner `operation.kind` selects a branch below. Full fields and result objects are in the [type reference](./types.en.md#pluginsftpoperation).

| kind | Parameters (? means optional) | result.kind |
| --- | --- | --- |
| `list` | `rootHandle`, `directoryHandle?`, `childEntryHandle?`, `cursor?`, `limit` | `page` |
| `read` | `rootHandle`, `directoryHandle`, `entryHandle`, `offset`, `length` | `read` |
| `writeBinary` | `rootHandle`, `directoryHandle`, `entryHandle`, `precondition`, `dataBase64` | `written` |
| `uploadStart` | `rootHandle`, `directoryHandle`, `name`, `expectedBytes` | `uploadStarted` |
| `uploadReplaceStart` | `rootHandle`, `directoryHandle`, `entryHandle`, `precondition`, `expectedBytes` | `uploadStarted` |
| `uploadChunk` | `rootHandle`, `uploadHandle`, `offset`, `dataBase64` | `uploadProgress` |
| `uploadCommit` | `rootHandle`, `uploadHandle` | `uploaded` |
| `uploadAbort` | `rootHandle`, `uploadHandle` | `uploadAborted` |
| `createDirectory` | `rootHandle`, `directoryHandle`, `name` | `created` |
| `createEmptyFile` | `rootHandle`, `directoryHandle`, `name` | `created` |
| `renameNoReplace` | `rootHandle`, `sourceDirectoryHandle`, `sourceEntryHandle`, `sourcePrecondition`, `targetDirectoryHandle`, `targetName` | `renamed` |
| `remove` | `rootHandle`, `directoryHandle`, `entryHandle`, `precondition` | `removed` |

Each example below is a standalone PluginApiCall. Replace every handle, fingerprint, precondition, and revision with preceding query results; these examples are not directly executable.

### list

```json
{
  "callId": "example.sftp.list",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "list",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "limit": 16
    }
  }
}
```

### read

```json
{
  "callId": "example.sftp.read",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "read",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "offset": 0,
      "length": 16
    }
  }
}
```

### writeBinary

```json
{
  "callId": "example.sftp.writeBinary",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "writeBinary",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "precondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      },
      "dataBase64": "YQ=="
    }
  }
}
```

### uploadStart

```json
{
  "callId": "example.sftp.uploadStart",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadStart",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "name": "renamed.txt",
      "expectedBytes": 1
    }
  }
}
```

### uploadReplaceStart

```json
{
  "callId": "example.sftp.uploadReplaceStart",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadReplaceStart",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "precondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      },
      "expectedBytes": 1
    }
  }
}
```

### uploadChunk

```json
{
  "callId": "example.sftp.uploadChunk",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadChunk",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "uploadHandle": "019d0000-0000-7000-8000-000000000001",
      "offset": 0,
      "dataBase64": "YQ=="
    }
  }
}
```

### uploadCommit

```json
{
  "callId": "example.sftp.uploadCommit",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadCommit",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "uploadHandle": "019d0000-0000-7000-8000-000000000001"
    }
  }
}
```

### uploadAbort

```json
{
  "callId": "example.sftp.uploadAbort",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadAbort",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "uploadHandle": "019d0000-0000-7000-8000-000000000001"
    }
  }
}
```

### createDirectory

```json
{
  "callId": "example.sftp.createDirectory",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "createDirectory",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "name": "renamed.txt"
    }
  }
}
```

### createEmptyFile

```json
{
  "callId": "example.sftp.createEmptyFile",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "createEmptyFile",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "name": "renamed.txt"
    }
  }
}
```

### renameNoReplace

```json
{
  "callId": "example.sftp.renameNoReplace",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "renameNoReplace",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "sourceDirectoryHandle": "019d0000-0000-7000-8000-000000000001",
      "sourceEntryHandle": "019d0000-0000-7000-8000-000000000001",
      "sourcePrecondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      },
      "targetDirectoryHandle": "019d0000-0000-7000-8000-000000000001",
      "targetName": "renamed.txt"
    }
  }
}
```

### remove

```json
{
  "callId": "example.sftp.remove",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "remove",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "precondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      }
    }
  }
}
```


## Related methods

[sftpOpen](./sftpOpen.en.md) · [resourceEvents](./resourceEvents.en.md) · [resourceClose](./resourceClose.en.md)
