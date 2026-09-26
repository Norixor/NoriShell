# dataRead

Read one separately authorized local category. Application preferences return Core-persisted group snapshots and their revisions. Groups awaiting first-run migration appear in `migrationRequired`; they must not be treated as empty settings. Terminal history obeys Vault, collection, and explicit user-action gates. The self-hosted sync plugin requests neither read capability.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataRead { … })`; match `PluginApiReply.outcome` and `value.kind="dataRead"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataRead"`; request: {category, offset, limit}. appPreferences requires offset=0, limit=1; terminalHistory accepts 1–100 entries per page.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataRead"` | Yes | Exact operation discriminator |
| `request` | `PluginDataReadRequest` | Yes | See field contract above and [DTO types](./types.en.md#category-data-dtos-core-api-189) |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataRead"` | Yes | Match before reading fields |
| `result` | typed value | Yes | Core-issued fact; see DTO types |

## Authority and status

The selected category requires `appPreferencesRead` or `terminalHistoryRead`. Preference groups are `application`, `appearance`, `interaction`, `highlights`, `shortcuts`, `files`, `desktop`, and `commandNotifications`; groups awaiting migration are absent from `groups`. Terminal history requires an explicit user action and returns at most 100 entries per page.

```json
{
  "callId": "example.dataRead",
  "operation": {
    "kind": "dataRead",
    "request": {
      "category": "appPreferences",
      "offset": 0,
      "limit": 1
    }
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
