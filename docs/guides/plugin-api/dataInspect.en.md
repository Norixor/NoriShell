# dataInspect

Authenticate and inspect remote ciphertext without exposing it to Wasm; return revision, ETag, migration status and object descriptors.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataInspect { … })`; match `PluginApiReply.outcome` and `value.kind="dataInspect"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataInspect"`; request: {profileId, categories, receiptHandle, bodyBlobHandle}. Both handles come from a completed Core network exchange.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataInspect"` | Yes | Exact operation discriminator |
| `request` | `PluginDataInspectRequest` | Yes | See field contract above and [DTO types](./types.en.md#category-data-dtos-core-api-188) |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataInspect"` | Yes | Match before reading fields |
| `inspectionHandle` | typed value | Yes | Core-issued fact; see DTO types |
| `objects` | typed value | Yes | Core-issued fact; see DTO types |
| `remoteRevision` | typed value | Yes | Core-issued fact; see DTO types |
| `etag` | typed value | Yes | Core-issued fact; see DTO types |
| `migrationRequired` | typed value | Yes | Core-issued fact; see DTO types |
| `excludedCategories` | typed value | Yes | Core-issued fact; see DTO types |

## Authority and status

`sshSync`; authenticated exchange and current owner/profile fence. Core service wiring passes a Cargo check; native acceptance remains open. Handles are bound to the current plugin, profile and generation and cannot be copied to another instance. Check Core runtime availability and authorization before invoking an unfinished operation.

```json
{
  "callId": "example.dataInspect",
  "operation": {
    "kind": "dataInspect",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "receiptHandle": "00000000-0000-4000-8000-000000000001",
      "bodyBlobHandle": "00000000-0000-4000-8000-000000000002"
    }
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
