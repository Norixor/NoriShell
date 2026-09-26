# dataCatalog

Read the category catalog and effective Core availability; it grants no data access.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataCatalog { … })`; match `PluginApiReply.outcome` and `value.kind="dataCatalog"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataCatalog"`; No fields beyond kind.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataCatalog"` | Yes | Exact operation discriminator |


## Completed value

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `kind` | `"dataCatalog"` | Yes | Match before reading fields |
| `catalog` | typed value | Yes | Core-issued fact; see DTO types |

## Authority and status

No capability for discovery. Handles are bound to the current plugin, profile and generation and cannot be copied to another instance. Check Core runtime availability and authorization before invoking an unfinished operation.

```json
{
  "callId": "example.dataCatalog",
  "operation": {
    "kind": "dataCatalog"
  }
}
```

[DTO types](./types.en.md) · [Broker index](./README.en.md)
