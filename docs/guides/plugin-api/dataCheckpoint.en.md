# dataCheckpoint

Advance the local sync baseline only after Core verifies authoritative remote and any local-apply receipts.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataCheckpoint { … })`; match `PluginApiReply.outcome` and `value.kind="dataCheckpoint"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataCheckpoint"`; request: {profileId, categories, sourceHandle, authoritativeReceiptHandle, baseReceiptHandle?, exportHandle?, applyReceiptHandle?, expectedLocalSnapshotHandle?, remoteInspectionHandle?}.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataCheckpoint"` | Yes | Exact operation discriminator |
| `request` | `PluginDataCheckpointRequest` | Yes | See field contract above and [DTO types](./types.en.md#category-data-dtos-core-api-188) |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataCheckpoint"` | Yes | Match before reading fields |
| `syncedAtUnixMs` | typed value | Yes | Core-issued fact; see DTO types |

For an upload, provide the base GET receipt and `exportHandle` as well as the authoritative PUT receipt. For a download, omit those two fields and provide the apply receipt. If a fresh local snapshot and authenticated GET200 inspection already have identical selected content, set `sourceHandle` and `expectedLocalSnapshotHandle` to that snapshot, provide `remoteInspectionHandle` and the GET receipt, and omit the export, base and apply handles. Core rechecks local state, exact GET resource, ciphertext digest and content equality before establishing the baseline. An unknown HTTP outcome cannot advance it.

## Authority and status

`sshSync`; successful verified remote operation, not a request-start result. Core service wiring passes a Cargo check; native acceptance remains open. Handles are bound to the current plugin, profile and generation and cannot be copied to another instance. Check Core runtime availability and authorization before invoking an unfinished operation.

```json
{
  "callId": "example.dataCheckpoint",
  "operation": {
    "kind": "dataCheckpoint",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "sourceHandle": "00000000-0000-4000-8000-000000000001",
      "authoritativeReceiptHandle": "00000000-0000-4000-8000-000000000002",
      "baseReceiptHandle": "00000000-0000-4000-8000-000000000003",
      "exportHandle": "00000000-0000-4000-8000-000000000004",
      "applyReceiptHandle": null
    }
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
