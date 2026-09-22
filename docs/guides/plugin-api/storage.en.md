# storage

Access plugin-private non-secret KV, blobs, cache, and schema state.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::Storage { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "storage" | Yes | Discriminator; use the exact listed value |
| `operation` | [PluginStorageOperation](./types.en.md#pluginstorageoperation) | Yes | Nested discriminated union; select fields by kind |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "storage" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginStorageResult](./types.en.md#pluginstorageresult) | Yes | Nested result union; inspect result.kind first |

## Permissions, timing, and resource scope

Capability hint: `storagePlugin`. A declaration still requires an effective grant and any applicable exact-operation approval.

Do not store secrets. Use standard Base64 without a data URL prefix. KV/blob mutations echo entry revision; use 0 for creation. schemaCommit applies declarative mutations in one transaction, not executable migration code. Cache entries may expire; an absent entry is not a decode error. Storage u64 revisions are JSON integers, unlike WireSequence.

## Example call

```json
{
  "callId": "example.storage",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvGet",
      "key": "theme"
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="storage"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).

## Sub-operations

The inner `operation.kind` selects a branch below. Full fields and result objects are in the [type reference](./types.en.md#pluginstorageoperation).

| kind | Parameters (? means optional) | result.kind |
| --- | --- | --- |
| `kvGet` | `key` | `kvValue` |
| `kvSet` | `key`, `valueBase64`, `expectedRevision` | `kvValue` |
| `kvDelete` | `key`, `expectedRevision` | `state` |
| `kvList` | `prefix`, `cursor?`, `limit` | `kvPage` |
| `blobRead` | `key`, `offset`, `length` | `blobRead` |
| `blobWrite` | `key`, `offset`, `dataBase64`, `expectedRevision` | `blobWritten` |
| `blobDelete` | `key`, `expectedRevision` | `state` |
| `cacheGet` | `key` | `cacheValue` |
| `cacheSet` | `key`, `valueBase64`, `ttlMs` | `cacheValue` |
| `cacheDelete` | `key` | `state` |
| `cacheClear` | — | `state` |
| `schemaGet` | — | `state` |
| `schemaCommit` | `expectedVersion`, `newVersion`, `mutations` | `state` |

Each example below is a standalone PluginApiCall. Replace every handle, fingerprint, precondition, and revision with preceding query results; these examples are not directly executable.

### kvGet

```json
{
  "callId": "example.storage.kvGet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvGet",
      "key": "theme"
    }
  }
}
```

### kvSet

```json
{
  "callId": "example.storage.kvSet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvSet",
      "key": "theme",
      "valueBase64": "YQ==",
      "expectedRevision": 0
    }
  }
}
```

### kvDelete

```json
{
  "callId": "example.storage.kvDelete",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvDelete",
      "key": "theme",
      "expectedRevision": 0
    }
  }
}
```

### kvList

```json
{
  "callId": "example.storage.kvList",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvList",
      "prefix": "",
      "limit": 16
    }
  }
}
```

### blobRead

```json
{
  "callId": "example.storage.blobRead",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "blobRead",
      "key": "theme",
      "offset": 0,
      "length": 16
    }
  }
}
```

### blobWrite

```json
{
  "callId": "example.storage.blobWrite",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "blobWrite",
      "key": "theme",
      "offset": 0,
      "dataBase64": "YQ==",
      "expectedRevision": 0
    }
  }
}
```

### blobDelete

```json
{
  "callId": "example.storage.blobDelete",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "blobDelete",
      "key": "theme",
      "expectedRevision": 0
    }
  }
}
```

### cacheGet

```json
{
  "callId": "example.storage.cacheGet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheGet",
      "key": "theme"
    }
  }
}
```

### cacheSet

```json
{
  "callId": "example.storage.cacheSet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheSet",
      "key": "theme",
      "valueBase64": "YQ==",
      "ttlMs": 60000
    }
  }
}
```

### cacheDelete

```json
{
  "callId": "example.storage.cacheDelete",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheDelete",
      "key": "theme"
    }
  }
}
```

### cacheClear

```json
{
  "callId": "example.storage.cacheClear",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheClear"
    }
  }
}
```

### schemaGet

```json
{
  "callId": "example.storage.schemaGet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "schemaGet"
    }
  }
}
```

### schemaCommit

```json
{
  "callId": "example.storage.schemaCommit",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "schemaCommit",
      "expectedVersion": 0,
      "newVersion": 1,
      "mutations": []
    }
  }
}
```


## Related methods

[credential](./credential.en.md) · [permissions](./permissions.en.md)
