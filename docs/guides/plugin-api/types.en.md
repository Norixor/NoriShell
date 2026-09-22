# Plugin API request and result types

These are guest-safe protocol 1.13 DTOs re-exported by `norishell_plugin_sdk`. Core-only plans, scopes, leases, approval decisions, protected inputs, Vault material, and resource fences are deliberately absent.

## PluginApiCall

`PluginApiCall { callId, operation: PluginApiOperation }` is emitted inside the SDK `api.request` output. `callId` correlates a reply only; it is not authority. `callId` must contain 1–80 bytes using only ASCII letters, digits, dots, underscores, or hyphens, such as `protocol-demo.open`; colons are invalid. It is a separate field from the UI action ID and must satisfy its own validation.

## PluginApiOperation

The tagged operation enum has the complete method list in [broker methods](broker.en.md). Its nested payloads include `PluginAppRegistration`, `PluginAppNotification`, `PluginAppNavigation`, `PluginWorkflowTaskId`, `PluginSerialSettings`, `PluginProtocolOpen`, `PluginNetworkEndpointRequest`, `PluginNetworkStartRequest`, `PluginRemoteExecStartRequest`, `PluginProcessSendRequest`, `PluginSftpOperation`, `PluginFileAccessRequest`, `PluginFileOperation`, `PluginCredentialOperation`, and `PluginStorageOperation`.

## PluginApiReply, PluginApiOutcome, and PluginApiValue

Core returns `PluginApiReply { callId, outcome }`. `PluginApiOutcome` is `completed { value: PluginApiValue }` or `failed { code: PluginApiErrorCode }`. A completed reply records Core acceptance or a typed result; it does not prove remote peer receipt unless its relevant resource event reports it.

## Remembered-operation summaries

`permissions` returns `operationPermissions: PluginOperationPermission[]` only for the current enabled plugin owner. A summary has `permissionId`, operation, non-secret action and target labels, `createdAtUnixMs`, and nullable `expiresAtUnixMs`; `null` means unlimited. For a finite decision, Core calculates the absolute deadline and stores it before returning, so a plugin or renderer cannot choose a timestamp or renew it by restarting. `permissionRevoke { permissionId, expectedPolicyRevision }` returns the next revision after deleting one current summary. `permissionsForget` clears all current summaries. These calls do not expose exact scopes or change capability grants.

## PluginApiResourceSummary and PluginApiResourceEvent

Resources use opaque handles. `PluginApiResourceSummary` exposes only state and metadata; `PluginApiResourceEvent` carries ordered, bounded facts such as serial data, timer fired, file changed, network, process output/exit, remote-exec, SFTP, and subscriptions. The handle expires on close, revocation, disable, crash, package replacement, or owner/generation change.

## Workflow, app, and provider types

`PluginWorkflowTaskSnapshot`, `PluginWorkflowTaskResult`, `WorkflowEvent`, `PluginWorkflowEvent`, and `WorkflowResponse` describe package-declared workflows. `PluginWorkflowEvent { taskId, workflowId, event }` is Core-correlated and delivered only to the same in-process instance. `WorkflowResponse { stepId?, call?, complete }` may set `complete: false` with no call and no step only to wait for an already-owned task resource event; Core rejects that inert wait when the task owns no resource. Workflow summary and step timestamps are JavaScript `number` values. `PluginAppRegistration`, `PluginAppNotification`, and `PluginAppNavigation` are explicit broker actions; they are not an `Initialize` side effect. `PluginProtocolCatalog`, `PluginProtocolProvider`, `PluginProtocolEvent`, and `PluginProtocolResponse` describe a protocol provider boundary; they do not expose a raw socket or a Core session identity.

## Error and availability types

`PluginApiErrorCode` contains stable non-secret errors including `interactionRequired`, `vaultMissing`, `vaultLocked`, `vaultRequiresReload`, `revoked`, `outcomeUnknown`, and `cleanupIncomplete`. `PluginApiDescription`, `PluginApiMethod`, `PluginApiAvailability`, and `PluginApiLimits` are returned by `describe`; query them at runtime rather than treating a source type as an availability claim.

For the exact serialized fields, use the generated Core declaration [`core-api.ts`](../../../src/core-api/generated/core-api.ts) and the SDK re-export list in [`crates/plugin-sdk/src/lib.rs`](../../../crates/plugin-sdk/src/lib.rs).
