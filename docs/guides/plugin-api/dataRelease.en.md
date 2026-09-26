# dataRelease

Release temporary Core data-exchange states, ciphertext blobs and network receipts after a refresh or sync attempt ends.

Use SDK `api_request(request_id, call_id, PluginApiOperation::DataRelease { … })`; match `PluginApiReply.outcome` and `value.kind="dataRelease"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataRelease"`; request: `{profileId, stateHandles, blobHandles, receiptHandles}`. Each list contains only handles returned during this operation. Lists may be empty; limits are 24 states, 12 blobs and 24 receipts per call. Handles must be distinct UUIDs within each list.

```json
{
  "callId": "example.dataRelease",
  "operation": {
    "kind": "dataRelease",
    "request": {
      "profileId": "primary",
      "stateHandles": ["00000000-0000-4000-8000-000000000001"],
      "blobHandles": ["00000000-0000-4000-8000-000000000002"],
      "receiptHandles": ["00000000-0000-4000-8000-000000000003"]
    }
  }
}
```

## Completed value

`{"kind":"dataRelease"}` means the listed temporary resources are no longer held. Releasing a handle already expired or released succeeds. A handle owned by another plugin instance or profile is rejected. Core checks the current plugin owner, generation and permission fence; it never releases all resources for a profile implicitly. Release after success, review pause or failure. An abandoned attempt is also bounded by a 10-minute expiry.

[DTO types](./types.en.md) · [Broker index](./README.en.md)
