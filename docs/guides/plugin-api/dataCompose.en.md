# dataCompose

Compose a candidate from Core-verified objects. The plugin builds a complete candidate for the same scope; Core checks dependencies and rejects invalid combinations. For conflicts requiring a human choice, build both local and remote candidates, then pass them to the single protected [dataReview](./dataReview.en.md) window.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataCompose { … })`; match `PluginApiReply.outcome` and `value.kind="dataCompose"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataCompose"`; request: {profileId, categories, localSnapshotHandle, remoteInspectionHandle, decisions[]}. Each decision selects local or remote for a Core-issued objectHandle.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataCompose"` | Yes | Exact operation discriminator |
| `request` | `PluginDataComposeRequest` | Yes | See field contract above and [DTO types](./types.en.md#category-data-dtos-core-api-189) |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataCompose"` | Yes | Match before reading fields |
| `composedHandle` | typed value | Yes | Core-issued fact; see DTO types |
| `objects` | typed value | Yes | Core-issued fact; see DTO types |

## Authority and status

`sshSync`; matching live handles. Core service wiring passes a Cargo check; native acceptance remains open. Handles are bound to the current plugin, profile and generation and cannot be copied to another instance. Check Core runtime availability and authorization before invoking an unfinished operation.

```json
{
  "callId": "example.dataCompose",
  "operation": {
    "kind": "dataCompose",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "localSnapshotHandle": "00000000-0000-4000-8000-000000000001",
      "remoteInspectionHandle": "00000000-0000-4000-8000-000000000002",
      "decisions": []
    }
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
