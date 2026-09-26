# dataApply

Apply the composed candidate through Core local-state checks and a protected commit; return an apply receipt.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataApply { … })`; match `PluginApiReply.outcome` and `value.kind="dataApply"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataApply"`; request: {profileId, categories, expectedLocalSnapshotHandle, composedHandle, exportHandle?, authoritativeReceiptHandle}.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataApply"` | Yes | Exact operation discriminator |
| `request` | `PluginDataApplyRequest` | Yes | See field contract above and [DTO types](./types.en.md#category-data-dtos-core-api-189) |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataApply"` | Yes | Match before reading fields |
| `applyReceiptHandle` | typed value | Yes | Core-issued fact; see DTO types |

For an upload, pass the `exportHandle` from `dataExport` and the completed PUT receipt as `authoritativeReceiptHandle`; Core verifies the exact bytes, revision, ETag/CAS and idempotency key before applying local changes. A download path may omit `exportHandle` but still needs the verified authoritative receipt.

## Authority and status

`sshSync`; explicit action and current local snapshot. Core service wiring passes a Cargo check; native acceptance remains open. Handles are bound to the current plugin, profile and generation and cannot be copied to another instance. Check Core runtime availability and authorization before invoking an unfinished operation.

```json
{
  "callId": "example.dataApply",
  "operation": {
    "kind": "dataApply",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "expectedLocalSnapshotHandle": "00000000-0000-4000-8000-000000000001",
      "composedHandle": "00000000-0000-4000-8000-000000000002",
      "exportHandle": "00000000-0000-4000-8000-000000000003",
      "authoritativeReceiptHandle": "00000000-0000-4000-8000-000000000004"
    }
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
