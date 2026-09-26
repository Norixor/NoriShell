# dataReview

Choose one complete local or remote candidate in a single Core-owned protected review. The plugin first calls `dataCompose` twice against the same `DataSnapshot`, `DataInspect`, and verified GET receipt. Core verifies both handles, contents, and provenance, then shows the before and after counts, differences, deletions, and upload requirement for each choice. Cancellation or failed validation grants no authority to write.

Call `PluginApiOperation::DataReview { … }` through SDK `api_request(request_id, call_id, ...)`. Match `PluginApiReply.outcome` and `value.kind="dataReview"`. [Broker calls](../developers/development/calling-api.en.md).

## Request

`operation.kind="dataReview"`; `request: {profileId, categories, localSnapshotHandle, remoteInspectionHandle, baseReceiptHandle, localComposedHandle, remoteComposedHandle}`. All five handles are opaque Core-issued values. Both candidates must cover the same categories, local snapshot, and remote inspection. The plugin supplies neither a decision nor an approval token.

## Completed value

`value.kind="dataReview"` returns a new `composedHandle` for the selected candidate, `source: local | remote`, and `objects[]`. The returned handle differs from both input candidate handles. Core records a one-use internal approval; later `dataApply` / `dataExport` still verify the active generation, scope, and receipt. The plugin applies directly against the authenticated GET receipt when the chosen candidate equals the remote data, or performs a conditional PUT first. This response alone does not mean the remote or local data has been committed.

Release the snapshot, inspection, both original composed candidates, the selected `composedHandle`, blobs, and receipts with [`dataRelease`](./dataRelease.en.md) after completion, cancellation, or failure. If PUT succeeded but local apply or checkpoint failed, report that the cloud changed together with the original failure reason.

[DTO types](./types.en.md) · [Broker index](./README.en.md)
