# Broker methods, resources, requests, and results

Every row is a `PluginApiOperation` variant inside [`PluginApiCall`](types.en.md#pluginapicall). A successful result is a `PluginApiValue`; failures are [`PluginApiErrorCode`](types.en.md#pluginapierrorcode). The operation list is complete for protocol 1.13; [`describe`](types.en.md#pluginapidescription) determines which entries are available in the current runtime.

| Method | Request payload | Completed result |
| --- | --- | --- |
| `appRegister` | [`PluginAppRegistration`](types.en.md#pluginappregistration) | `appAccepted` |
| `appNotify` | [`PluginAppNotification`](types.en.md#pluginappnotification) | `appAccepted` |
| `appNavigate` | [`PluginAppNavigation`](types.en.md#pluginappnavigation) | `appAccepted` |
| `taskStart` | workflow id, in-memory input JSON, opaque preselected file scopes | `task { PluginWorkflowTaskSnapshot }` |
| `taskGet` | `PluginWorkflowTaskId` | `task { PluginWorkflowTaskSnapshot }` |
| `taskList` | none | `tasks { PluginWorkflowTaskSnapshot[] }` |
| `taskCancel` | task id and revision | `task { PluginWorkflowTaskSnapshot }` |
| `taskResume` | task id and revision | `task { PluginWorkflowTaskSnapshot }` |
| `serialDevices` | none | `serialDevices { PluginSerialDeviceCandidate[] }` |
| `serialOpen` | `PluginSerialSettings`, selected candidate id | `serialStarted { handle }` |
| `serialSend` | `PluginSerialSendRequest` | `serialSent { handle }` |
| `protocolOpen` | `PluginProtocolOpen` | `protocolLaunched { launchId }` |
| `describe` | none | `description { PluginApiDescription }` |
| `permissions` | none | `permissions { PluginCapabilityGrant[], policyRevision, operationPermissions }` |
| `permissionRequest` | `PluginCapability` | `permissionRequested { approvalId }` |
| `permissionRevoke` | permission id and expected policy revision | `permissionRevoked { policyRevision }` |
| `permissionsForget` | none | `permissionsForgotten` |
| `resourcesList` | none | `resources { PluginApiResourceSummary[] }` |
| `resourceClose` | opaque resource handle | `closed { handle }` |
| `subscriptionStart` | `PluginSubscriptionTopic[]` | `subscriptionStarted { handle }` |
| `timerStart` | delay and optional interval | `timerStarted { handle }` |
| `resourceEvents` | handle and bounded limit | `resourceEvents { handle, events, backpressured }` |
| `networkStart` | `PluginNetworkEndpointRequest`, `PluginNetworkStartRequest` | `networkStarted { handle }` |
| `networkSend` | `PluginNetworkSendRequest` | `networkSent { handle }` |
| `remoteExecStart` | `PluginRemoteExecStartRequest` | `remoteExecStarted { handle }` |
| `remoteExecSend` | `PluginRemoteExecSendRequest` | `remoteExecSent { handle }` |
| `processStart` | executable, arguments, timeout | `processStarted { handle }` |
| `processSend` | `PluginProcessSendRequest` | `processSent { handle }` |
| `sftpOpen` | approved host handle, root path, write flag | `sftp { PluginSftpResult }` |
| `sftp` | `PluginSftpOperation` | `sftp { PluginSftpResult }` |
| `filePick` | `PluginFilePickerKind`, `PluginFileAccessRequest` | `filePicked { rootHandle, label }` |
| `file` | `PluginFileOperation` | `file { PluginFileResult }` |
| `credential` | `PluginCredentialOperation` | `credential { PluginCredentialResult }` |
| `storage` | `PluginStorageOperation` | `storage { PluginStorageResult }` |
| `terminalRequestInput` | terminal handle, payload, append-enter flag | `inputApprovalRequested` or `inputSent` |

`appRegister`, `appNotify`, and `appNavigate` are explicitly documented broker operations. `Initialize` only establishes the host request lifecycle; it does not register commands, navigate, or grant an app action. A Core `PluginWorkflowEvent { taskId, workflowId, event }` reaches only the resident matching instance. `WorkflowResponse { stepId?, call?, complete }` may wait with `complete: false` and no call only for an already-owned task resource event; a task without one is rejected. Task timestamps are JavaScript `number` values. `taskStart` input, file-handle mapping, and runtime replies stay in process. At restart, Core marks unfinished tasks `interrupted`; it does not persist payloads, replay a dispatch, or resume a workflow automatically. `taskResume` is a new explicit action for the stored task state, never a replay of the earlier payload.

`permissions` exposes only non-secret remembered-operation summaries owned by the exact currently enabled package, signer, and still-effective capability/security binding. Each summary contains its opaque `permissionId`, operation, action and management labels, creation time, and optional absolute `expiresAtUnixMs`; it never contains a raw selected path, serial identity, exact scope, command, or payload. `permissionRevoke` removes one of those current summaries with the returned `policyRevision` as its optimistic-concurrency fence. `permissionsForget` removes all remembered operations that the current guest still owns. Neither method changes capability grants or a historical row from a prior signer/package; host-side management retains that history.

Resource handles are opaque and scoped to owner, package hash, grant state, consumer, and runtime generation. Close is explicit; cancellation, revocation, disable, replacement, crash, or a stale fence can close them too. A returned handle or `*Sent` result is local Core acceptance, not proof of remote I/O. Resource events carry bounded actual open/data/exit/error/close facts.

Serial and protocol-provider APIs are included because they are in the current ABI and broker implementation. They are not claims of native, hardware, Windows, or end-to-end acceptance. See [implementation status](../../../README.md#installation-and-quick-start) and [developer broker guidance](../developers/broker.en.md).

The main application owns opted-in plugin shortcut bindings across page changes. Conflicting bindings do not execute. Terminal input, editable controls, IME composition, and blocking dialogs remain protected; application teardown removes the listener. A file command invokes its registered document action, which must still obtain a fresh handle through native FilePick selection and scope approval. Declaring extensions does not grant file access.
