# dataSnapshot

Freeze authorized local objects; receive an opaque snapshot handle and non-secret object descriptors.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataSnapshot { … })`; match `PluginApiReply.outcome` and `value.kind="dataSnapshot"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataSnapshot"`; request: {profileId, categories}. Categories must be sorted, unique and chosen from hosts, credentials, desktopProfiles.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataSnapshot"` | Yes | Exact operation discriminator |
| `request` | `PluginDataSnapshotRequest` | Yes | See field contract above and [DTO types](./types.en.md#category-data-dtos-core-api-188) |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataSnapshot"` | Yes | Match before reading fields |
| `snapshotHandle` | typed value | Yes | Core-issued fact; see DTO types |
| `keyPending` | typed value | Yes | Core-issued fact; see DTO types |
| `localCounts` | `PluginDataLocalCounts` | Yes | Local hosts, credentials, desktop profiles and tombstones |
| `objects` | typed value | Yes | Core-issued fact; see DTO types |

## Authority and status

`sshSync`. Core service wiring passes a Cargo check; native acceptance remains open. Handles are bound to the current plugin, profile and generation and cannot be copied to another instance. Check Core runtime availability and authorization before invoking an unfinished operation.

```json
{
  "callId": "example.dataSnapshot",
  "operation": {
    "kind": "dataSnapshot",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ]
    }
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
