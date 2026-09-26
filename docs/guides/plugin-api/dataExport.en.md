# dataExport

Encrypt the selected local/composed data into an opaque Core blob; return revision and idempotency key for the plugin-selected HTTP write. Export alone is not a sync success.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataExport { … })`; match `PluginApiReply.outcome` and `value.kind="dataExport"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataExport"`; request: {profileId, categories, sourceHandle, baseReceiptHandle}.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataExport"` | Yes | Exact operation discriminator |
| `request` | `PluginDataExportRequest` | Yes | See field contract above and [DTO types](./types.en.md#category-data-dtos-core-api-189) |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataExport"` | Yes | Match before reading fields |
| `exportHandle` | typed value | Yes | Core export state for verified checkpoint |
| `blobHandle` | typed value | Yes | Core-issued fact; see DTO types |
| `revision` | typed value | Yes | Core-issued fact; see DTO types |
| `idempotencyKey` | typed value | Yes | Core-issued fact; see DTO types |
| `contentType` | typed value | Yes | Core-issued fact; see DTO types |
| `objects` | typed value | Yes | Non-secret object descriptors |

## Authority and status

`sshSync`; matching verified base receipt. Core service wiring passes a Cargo check; native acceptance remains open. Handles are bound to the current plugin, profile and generation and cannot be copied to another instance. Check Core runtime availability and authorization before invoking an unfinished operation.

```json
{
  "callId": "example.dataExport",
  "operation": {
    "kind": "dataExport",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "sourceHandle": "00000000-0000-4000-8000-000000000001",
      "baseReceiptHandle": "00000000-0000-4000-8000-000000000002"
    }
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
