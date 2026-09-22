# Broker wire types

Only guest DTOs reachable from method requests, results, and resource events are listed; Core authorization plans, secrets, and internal requests are excluded. `String` is a JSON string, `bool` a boolean, `Vec<T>` an array, and integer types JSON integers. `Option<T>` may be omitted or null. **WireSequence uses a decimal string** such as `"1"`, not a number. Avoid unsafe JavaScript integer precision for numeric `u64` values. Enum discriminators and field names are case-sensitive.

## PluginApiCall

`PluginApiCall { callId, operation: PluginApiOperation }` is emitted inside the SDK `api.request` output. `callId` correlates a reply only; it is not authority. `callId` must contain 1–80 bytes using only ASCII letters, digits, dots, underscores, or hyphens, such as `protocol-demo.open`; colons are invalid. It is a separate field from the UI action ID and must satisfy its own validation.

## PluginApiOperation

The tagged operation enum has the complete method list in [broker methods](broker.en.md). Its nested payloads include `PluginAppRegistration`, `PluginAppNotification`, `PluginAppNavigation`, `PluginWorkflowTaskId`, `PluginSerialSettings`, `PluginProtocolOpen`, `PluginNetworkEndpointRequest`, `PluginNetworkStartRequest`, `PluginRemoteExecStartRequest`, `PluginProcessSendRequest`, `PluginSftpOperation`, `PluginFileAccessRequest`, `PluginFileOperation`, `PluginCredentialOperation`, and `PluginStorageOperation`.

## PluginApiAvailability

Allowed string values: `available`, `notImplemented`, `unsupportedPlatform`.

## PluginApiDescription

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `protocolMajor` | u16 | Yes | Plugin protocol major version, currently 1 |
| `protocolMinor` | u16 | Yes | Plugin protocol minor version, currently 13 |
| `platform` | String | Yes | Platform identifier of the running Core |
| `methods` | Vec&lt;[PluginApiMethod](./types.en.md#pluginapimethod)&gt; | Yes | Method names, capability hints, and implementation status |
| `limits` | [PluginApiLimits](./types.en.md#pluginapilimits) | Yes | Runtime call, chunk, resource, and event limits |

## PluginApiErrorCode

Allowed string values: `invalidRequest`, `permissionDenied`, `interactionRequired`, `vaultMissing`, `vaultLocked`, `vaultRequiresReload`, `unsupported`, `revoked`, `notFound`, `conflict`, `busy`, `quotaExceeded`, `timedOut`, `cancelled`, `outcomeUnknown`, `cleanupIncomplete`, `unavailable`.

## PluginApiLimits

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `maxCallBytes` | u32 | Yes | Maximum serialized bytes per API call |
| `maxChunkBytes` | u32 | Yes | General maximum chunk bytes; specialized methods may be lower |
| `maxResources` | u16 | Yes | Plugin resource-count limit |
| `maxPendingEvents` | u16 | Yes | Resource-event queue bound |

## PluginApiMethod

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | String | Yes | Name or identifier for this object |
| `capability` | Option&lt;[PluginCapability](./types.en.md#plugincapability)&gt; | No | PluginCapability enum value, not an existing grant |
| `availability` | [PluginApiAvailability](./types.en.md#pluginapiavailability) | Yes | Implementation status, not permission or device readiness |

## PluginApiOutcome

### completed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "completed" | Yes | Discriminator; use the exact listed value |
| `value` | [PluginApiValue](./types.en.md#pluginapivalue) | Yes | Typed success value |

### failed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "failed" | Yes | Discriminator; use the exact listed value |
| `code` | [PluginApiErrorCode](./types.en.md#pluginapierrorcode) | Yes | Stable error code without upstream secret information |

## PluginApiReply

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `callId` | String | Yes | Plugin-assigned correlation ID, not authority |
| `outcome` | [PluginApiOutcome](./types.en.md#pluginapioutcome) | Yes | Completed or failed outcome union |

## PluginApiResourceEvent

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `sequence` | [WireSequence](./types.en.md#wiresequence) | Yes | Event sequence as a decimal string |
| `kind` | [PluginApiResourceEventKind](./types.en.md#pluginapiresourceeventkind) | Yes | Nested event object; its inner kind is the string discriminator |

## PluginApiResourceEventKind

### serial

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serial" | Yes | Discriminator; use the exact listed value |
| `event` | [PluginSerialEvent](./types.en.md#pluginserialevent) | Yes | Resource event selected by its inner kind |

### timerFired

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "timerFired" | Yes | Discriminator; use the exact listed value |

### cancelled

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "cancelled" | Yes | Discriminator; use the exact listed value |

### fileChanged

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "fileChanged" | Yes | Discriminator; use the exact listed value |
| `change` | [PluginFileWatchChange](./types.en.md#pluginfilewatchchange) | Yes | Change metadata produced by a file watch |

### sftp

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "sftp" | Yes | Discriminator; use the exact listed value |
| `event` | [PluginSftpEvent](./types.en.md#pluginsftpevent) | Yes | Resource event selected by its inner kind |

### subscription

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "subscription" | Yes | Discriminator; use the exact listed value |
| `event` | [PluginSubscriptionEvent](./types.en.md#pluginsubscriptionevent) | Yes | Resource event selected by its inner kind |

### network

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "network" | Yes | Discriminator; use the exact listed value |
| `event` | [PluginNetworkEvent](./types.en.md#pluginnetworkevent) | Yes | Resource event selected by its inner kind |

### processOutput

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "processOutput" | Yes | Discriminator; use the exact listed value |
| `stream` | [PluginProcessOutputStream](./types.en.md#pluginprocessoutputstream) | Yes | stdout or stderr |
| `data_base64` | String | Yes | Base64 bytes; this field retains snake_case |

### processExited

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "processExited" | Yes | Discriminator; use the exact listed value |
| `exit_code` | Option&lt;i64&gt; | No | Process exit code; wire retains the underscore |

### remoteExec

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remoteExec" | Yes | Discriminator; use the exact listed value |
| `event` | [PluginRemoteExecEvent](./types.en.md#pluginremoteexecevent) | Yes | Resource event selected by its inner kind |

## PluginApiResourceState

Allowed string values: `opening`, `open`, `closing`, `cleanupIncomplete`.

## PluginApiResourceSummary

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `generation` | [WireSequence](./types.en.md#wiresequence) | Yes | Current instance or context generation as a decimal string |
| `resourceKind` | String | Yes | Resource category summary |
| `state` | [PluginApiResourceState](./types.en.md#pluginapiresourcestate) | Yes | Current state; handle the corresponding enum |

## PluginApiValue

### appAccepted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "appAccepted" | Yes | Discriminator; use the exact listed value |

### task

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "task" | Yes | Discriminator; use the exact listed value |
| `snapshot` | [PluginWorkflowTaskSnapshot](./types.en.md#pluginworkflowtasksnapshot) | Yes | Task snapshot containing task, steps, and optional result |

### tasks

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "tasks" | Yes | Discriminator; use the exact listed value |
| `snapshots` | Vec&lt;[PluginWorkflowTaskSnapshot](./types.en.md#pluginworkflowtasksnapshot)&gt; | Yes | Task snapshots visible to the current plugin |

### serialDevices

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialDevices" | Yes | Discriminator; use the exact listed value |
| `devices` | Vec&lt;[PluginSerialDeviceCandidate](./types.en.md#pluginserialdevicecandidate)&gt; | Yes | Candidate metadata without native device paths |

### serialStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### serialSent

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "serialSent" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### protocolLaunched

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "protocolLaunched" | Yes | Discriminator; use the exact listed value |
| `launchId` | String | Yes | Host terminal launch record ID, not a connected state |

### description

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "description" | Yes | Discriminator; use the exact listed value |
| `api` | [PluginApiDescription](./types.en.md#pluginapidescription) | Yes | Protocol version, platform, methods, and limits |

### permissions

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissions" | Yes | Discriminator; use the exact listed value |
| `grants` | Vec&lt;[PluginCapabilityGrant](./types.en.md#plugincapabilitygrant)&gt; | Yes | Current capability grant records |
| `policyRevision` | Option&lt;[WireSequence](./types.en.md#wiresequence)&gt; | No | Nullable policy revision as a decimal string |
| `operationPermissions` | Vec&lt;[PluginOperationPermission](./types.en.md#pluginoperationpermission)&gt; | Yes | Non-secret summaries of owned remembered operations |

### permissionRequested

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionRequested" | Yes | Discriminator; use the exact listed value |
| `approvalId` | String | Yes | Pending approval request ID, not an approval grant |

### permissionRevoked

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionRevoked" | Yes | Discriminator; use the exact listed value |
| `policyRevision` | [WireSequence](./types.en.md#wiresequence) | Yes | Nullable policy revision as a decimal string |

### permissionsForgotten

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "permissionsForgotten" | Yes | Discriminator; use the exact listed value |

### resources

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "resources" | Yes | Discriminator; use the exact listed value |
| `resources` | Vec&lt;[PluginApiResourceSummary](./types.en.md#pluginapiresourcesummary)&gt; | Yes | Resource summaries for the current owner |

### closed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "closed" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### timerStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "timerStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### resourceEvents

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "resourceEvents" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `events` | Vec&lt;[PluginApiResourceEvent](./types.en.md#pluginapiresourceevent)&gt; | Yes | Events to process in sequence order |
| `backpressured` | bool | Yes | Whether the resource-event queue is backpressured |

### sftp

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "sftp" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginSftpResult](./types.en.md#pluginsftpresult) | Yes | Nested result union; inspect result.kind first |

### subscriptionStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "subscriptionStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### credential

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "credential" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginCredentialResult](./types.en.md#plugincredentialresult) | Yes | Nested result union; inspect result.kind first |

### networkStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "networkStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### networkSent

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "networkSent" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### remoteExecStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remoteExecStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### remoteExecSent

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remoteExecSent" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### processStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "processStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### processSent

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "processSent" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

### filePicked

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "filePicked" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `label` | String | Yes | User-facing display label |

### file

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "file" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginFileResult](./types.en.md#pluginfileresult) | Yes | Nested result union; inspect result.kind first |

### storage

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "storage" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginStorageResult](./types.en.md#pluginstorageresult) | Yes | Nested result union; inspect result.kind first |

### inputApprovalRequested

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "inputApprovalRequested" | Yes | Discriminator; use the exact listed value |
| `approvalId` | String | Yes | Pending approval request ID, not an approval grant |

### inputSent

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "inputSent" | Yes | Discriminator; use the exact listed value |

## PluginAppCommand

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | String | Yes | Unique identifier within this declaration scope |
| `label` | String | Yes | User-facing display label |
| `targetId` | [PluginExtensionTargetId](./types.en.md#pluginextensiontargetid) | Yes | Declared plugin extension target ID |
| `actionId` | [PluginUiActionId](./types.en.md#pluginuiactionid) | Yes | Action ID in a verified document |
| `pageId` | Option&lt;String&gt; | No | Page ID for an app.page command |
| `shortcut` | Option&lt;String&gt; | No | Optional Alt+Shift+uppercase-letter shortcut |
| `fileExtensions` | Vec&lt;String&gt; | Yes | File extensions without dots, at most 12 |

## PluginAppNavigation

### app

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "app" | Yes | Discriminator; use the exact listed value |
| `path` | String | Yes | Host-allowed application route |

### pluginPage

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "pluginPage" | Yes | Discriminator; use the exact listed value |
| `page_id` | String | Yes | Own plugin page ID; wire field uses an underscore |

## PluginAppNotification

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | String | Yes | Unique identifier within this declaration scope |
| `text` | String | Yes | User-visible text |

## PluginAppRegistration

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `commands` | Vec&lt;[PluginAppCommand](./types.en.md#pluginappcommand)&gt; | Yes | Registered commands, at most 24 |
| `statuses` | Vec&lt;[PluginAppStatus](./types.en.md#pluginappstatus)&gt; | Yes | Plugin status-text entries, at most 8 |

## PluginAppStatus

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | String | Yes | Unique identifier within this declaration scope |
| `text` | String | Yes | User-visible text |

## PluginApprovalOperation

Allowed string values: `remoteExecute`, `forwardStart`, `forwardStop`, `networkRequest`, `fileAccess`, `sftpRead`, `sftpWrite`, `serialAccess`, `localExecute`, `terminalInput`, `hostMutation`, `hostSession`.

## PluginCapability

Allowed string values: `uiPanel`, `uiNavigation`, `uiPage`, `uiWebviewIsolated`, `uiHostDomObserve`, `uiHostDomMutate`, `uiHostCss`, `clipboardWrite`, `terminalProvider`, `deviceSerial`, `terminalMetadata`, `terminalObserve`, `terminalAnnotation`, `terminalProposeInput`, `terminalRequestInput`, `hostMetadataRead`, `hostMutationPropose`, `hostSessionRequest`, `remoteInspect`, `remoteExecRequest`, `networkDomain`, `localFiles`, `localProcess`, `storagePlugin`, `credentialsPlugin`, `sftpRead`, `sftpWrite`, `metricsRead`, `sshSync`.

## PluginCapabilityGrant

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `capability` | [PluginCapability](./types.en.md#plugincapability) | Yes | PluginCapability enum value, not an existing grant |
| `granted` | bool | Yes | Whether this capability has been granted |

## PluginCredentialInjection

### bearer

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "bearer" | Yes | Discriminator; use the exact listed value |

### header

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "header" | Yes | Discriminator; use the exact listed value |
| `name` | String | Yes | Name or identifier for this object |

## PluginCredentialOperation

### create

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "create" | Yes | Discriminator; use the exact listed value |
| `operationId` | String | Yes | Operation correlation ID for the current creation intent |
| `idempotencyKey` | String | Yes | Stable idempotency key for the same credential-creation intent |
| `label` | String | Yes | User-facing display label |
| `target` | [PluginCredentialTarget](./types.en.md#plugincredentialtarget) | Yes | Credential origin and injection rule |

### list

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "list" | Yes | Discriminator; use the exact listed value |

### revoke

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "revoke" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `expectedRevision` | [WireSequence](./types.en.md#wiresequence) | Yes | Echo the latest read revision; do not increment it yourself |

## PluginCredentialResult

### created

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "created" | Yes | Discriminator; use the exact listed value |
| `credential` | [PluginCredentialSummary](./types.en.md#plugincredentialsummary) | Yes | Non-secret credential summary or reference, as typed |

### list

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "list" | Yes | Discriminator; use the exact listed value |
| `credentials` | Vec&lt;[PluginCredentialSummary](./types.en.md#plugincredentialsummary)&gt; | Yes | Non-secret credential summaries visible to this plugin |

### revoked

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "revoked" | Yes | Discriminator; use the exact listed value |
| `credential` | [PluginCredentialSummary](./types.en.md#plugincredentialsummary) | Yes | Non-secret credential summary or reference, as typed |

## PluginCredentialState

Allowed string values: `pendingVault`, `ready`, `cleanupPending`, `revoked`.

## PluginCredentialSummary

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `label` | String | Yes | User-facing display label |
| `target` | [PluginCredentialTarget](./types.en.md#plugincredentialtarget) | Yes | Credential origin and injection rule |
| `revision` | [WireSequence](./types.en.md#wiresequence) | Yes | Current revision; echo for conditional mutation |
| `state` | [PluginCredentialState](./types.en.md#plugincredentialstate) | Yes | Current state; handle the corresponding enum |

## PluginCredentialTarget

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `origin` | String | Yes | Exact origin bound to the credential |
| `injection` | [PluginCredentialInjection](./types.en.md#plugincredentialinjection) | Yes | Bearer or permitted header injection mode |

## PluginExtensionTargetId

Opaque string identifier; preserve the value provided by Core.

## PluginFileAccessRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `read` | bool | No | Request read access; defaults to false |
| `write` | bool | No | Request writable scope; capability and approval still required |
| `list` | bool | No | Request directory listing; defaults to false |
| `rename` | bool | No | Request rename access; defaults to false |
| `remove` | bool | No | Request remove access; defaults to false |
| `recursiveRemove` | bool | No | Request recursive-remove access; defaults to false |
| `watch` | bool | No | Request change-watch access; defaults to false |

## PluginFileEntry

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | String | Yes | Name or identifier for this object |
| `kind` | [PluginFileEntryKind](./types.en.md#pluginfileentrykind) | Yes | Discriminator; use the exact listed value |
| `size` | Option&lt;u64&gt; | No | Object byte size observed by Core, if known |
| `fingerprint` | Option&lt;String&gt; | No | File-version fingerprint observed by Core |

## PluginFileEntryKind

Allowed string values: `file`, `directory`.

## PluginFileOperation

### read

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "read" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `relativePath` | String | Yes | Relative path within the root; empty string for a file root |
| `offset` | u64 | Yes | Zero-based byte offset |

### write

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "write" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `relativePath` | String | Yes | Relative path within the root; empty string for a file root |
| `expectedFingerprint` | Option&lt;String&gt; | No | Latest read/list fingerprint; omission on write is create-only |
| `offset` | u64 | Yes | Zero-based byte offset |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |
| `finalSize` | Option&lt;u64&gt; | No | Final file size in bytes when supplied |

### list

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "list" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `relativePath` | String | Yes | Relative path within the root; empty string for a file root |
| `cursor` | Option&lt;String&gt; | No | Echo the prior nextCursor unchanged; omit on first page |
| `limit` | u16 | Yes | Maximum items or events returned in this call |

### rename

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "rename" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `fromPath` | String | Yes | Source relative path inside the root |
| `toPath` | String | Yes | New relative path inside the root; destination must not exist |
| `expectedFingerprint` | String | Yes | Latest read/list fingerprint; omission on write is create-only |

### remove

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remove" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `relativePath` | String | Yes | Relative path within the root; empty string for a file root |
| `expectedFingerprint` | String | Yes | Latest read/list fingerprint; omission on write is create-only |
| `recursive` | bool | Yes | Recursive removal, defaults to false, still requires permission |

### watchStart

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "watchStart" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `relativePath` | String | Yes | Relative path within the root; empty string for a file root |
| `intervalMs` | u32 | Yes | Repeat interval in milliseconds; omit for one-shot |

## PluginFilePickerKind

Allowed string values: `file`, `directory`.

## PluginFileResult

### read

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "read" | Yes | Discriminator; use the exact listed value |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |
| `fingerprint` | String | Yes | File-version fingerprint observed by Core |
| `eof` | bool | Yes | Whether the end of data was reached |

### written

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "written" | Yes | Discriminator; use the exact listed value |
| `fingerprint` | String | Yes | File-version fingerprint observed by Core |

### listed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "listed" | Yes | Discriminator; use the exact listed value |
| `entries` | Vec&lt;[PluginFileEntry](./types.en.md#pluginfileentry)&gt; | Yes | Storage or directory entries in this page |
| `nextCursor` | Option&lt;String&gt; | No | Pass to the next page if present; absent means the end |

### renamed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "renamed" | Yes | Discriminator; use the exact listed value |
| `fingerprint` | String | Yes | File-version fingerprint observed by Core |

### removed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "removed" | Yes | Discriminator; use the exact listed value |

### watchStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "watchStarted" | Yes | Discriminator; use the exact listed value |
| `handle` | String | Yes | Core-issued reference owned by the current caller |

## PluginFileWatchChange

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `relativePath` | String | Yes | Relative path within the root; empty string for a file root |
| `fingerprint` | Option&lt;String&gt; | No | File-version fingerprint observed by Core |

## PluginHttpMethod

Allowed string values: `get`, `head`, `post`, `put`, `patch`, `delete`.

## PluginId

Opaque string identifier; preserve the value provided by Core.

## PluginNetworkCredentialRef

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `expectedRevision` | [WireSequence](./types.en.md#wiresequence) | Yes | Echo the latest read revision; do not increment it yourself |

## PluginNetworkEndpointRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `endpoint` | String | Yes | Full endpoint URL for Core resolution and approval |

## PluginNetworkErrorCode

Allowed string values: `invalidEndpoint`, `resolveFailed`, `connectFailed`, `tlsFailed`, `httpFailed`, `protocolFailed`, `timedOut`, `revoked`, `cancelled`, `unavailable`.

## PluginNetworkEvent

### opened

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "opened" | Yes | Discriminator; use the exact listed value |
| `protocol` | [PluginNetworkProtocol](./types.en.md#pluginnetworkprotocol) | Yes | Actual resource protocol enum |
| `peerAddress` | Option&lt;String&gt; | No | Actual peer address, possibly null |

### httpResponse

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "httpResponse" | Yes | Discriminator; use the exact listed value |
| `status` | u16 | Yes | HTTP response status code |
| `headers` | Vec&lt;[PluginNetworkHeader](./types.en.md#pluginnetworkheader)&gt; | Yes | HTTP/WebSocket headers; Core handles credential injection |

### data

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "data" | Yes | Discriminator; use the exact listed value |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

### datagram

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "datagram" | Yes | Discriminator; use the exact listed value |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

### closed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "closed" | Yes | Discriminator; use the exact listed value |

### error

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "error" | Yes | Discriminator; use the exact listed value |
| `code` | [PluginNetworkErrorCode](./types.en.md#pluginnetworkerrorcode) | Yes | Stable error code without upstream secret information |

## PluginNetworkHeader

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | String | Yes | Name or identifier for this object |
| `value` | String | Yes | Typed success value |

## PluginNetworkOperation

### http

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "http" | Yes | Discriminator; use the exact listed value |
| `method` | [PluginHttpMethod](./types.en.md#pluginhttpmethod) | Yes | Typed broker method or HTTP verb |
| `headers` | Vec&lt;[PluginNetworkHeader](./types.en.md#pluginnetworkheader)&gt; | Yes | HTTP/WebSocket headers; Core handles credential injection |
| `bodyBase64` | String | Yes | Standard Base64 HTTP body; empty string for no body |

### webSocket

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "webSocket" | Yes | Discriminator; use the exact listed value |
| `headers` | Vec&lt;[PluginNetworkHeader](./types.en.md#pluginnetworkheader)&gt; | Yes | HTTP/WebSocket headers; Core handles credential injection |

### tcp

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "tcp" | Yes | Discriminator; use the exact listed value |

### udp

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "udp" | Yes | Discriminator; use the exact listed value |

### tls

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "tls" | Yes | Discriminator; use the exact listed value |

## PluginNetworkProtocol

Allowed string values: `http`, `webSocket`, `tcp`, `udp`, `tls`.

## PluginNetworkSendRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

## PluginNetworkStartRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `timeoutMs` | u32 | Yes | Timeout in milliseconds, subject to Core bounds |
| `credential` | Option&lt;[PluginNetworkCredentialRef](./types.en.md#pluginnetworkcredentialref)&gt; | No | Non-secret credential summary or reference, as typed |
| `operation` | [PluginNetworkOperation](./types.en.md#pluginnetworkoperation) | Yes | Nested discriminated union; select fields by kind |

## PluginOperationPermission

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `permissionId` | String | Yes | Remembered-operation ID returned by permissions |
| `pluginId` | [PluginId](./types.en.md#pluginid) | Yes | Core-owned plugin ID, not guest-supplied authority |
| `operation` | [PluginApprovalOperation](./types.en.md#pluginapprovaloperation) | Yes | Nested discriminated union; select fields by kind |
| `actionLabel` | String | Yes | Non-secret display summary of the action |
| `targetLabel` | String | Yes | Non-secret management summary of the target |
| `createdAtUnixMs` | i64 | Yes | Creation timestamp in Unix milliseconds |
| `expiresAtUnixMs` | Option&lt;i64&gt; | No | Absolute Unix-millisecond expiry; null means unlimited |

## PluginProcessOutputStream

Allowed string values: `stdout`, `stderr`.

## PluginProcessSendRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |
| `closeStdin` | bool | No | Send EOF; defaults to false; use empty data with EOF |

## PluginProtocolOpen

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `providerId` | String | Yes | Protocol provider ID declared by the package |
| `configuration` | BTreeMap&lt;String, serde_json::Value&gt; | Yes | Settings accepted by the provider schema |
| `label` | Option&lt;String&gt; | No | User-facing display label |

## PluginRemoteExecEvent

### remoteExecOutput

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remoteExecOutput" | Yes | Discriminator; use the exact listed value |
| `stream` | [PluginProcessOutputStream](./types.en.md#pluginprocessoutputstream) | Yes | stdout or stderr |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

### remoteExecExited

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remoteExecExited" | Yes | Discriminator; use the exact listed value |
| `exitCode` | Option&lt;i64&gt; | No | Process exit code; absent if unavailable |

## PluginRemoteExecSendRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |
| `closeStdin` | bool | No | Send EOF; defaults to false; use empty data with EOF |

## PluginRemoteExecStartRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `hostHandle` | String | Yes | Opaque reference from a Core host context |
| `command` | String | Yes | Command requested on the approved remote host |
| `timeoutMs` | u32 | Yes | Timeout in milliseconds, subject to Core bounds |

## PluginSerialDataBits

Allowed string values: `five`, `six`, `seven`, `eight`.

## PluginSerialDeviceCandidate

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `candidateId` | String | Yes | Short-lived candidate returned by serialDevices |
| `metadata` | [PluginSerialPortMetadata](./types.en.md#pluginserialportmetadata) | Yes | Display-safe device information without native paths |

## PluginSerialEvent

### opened

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "opened" | Yes | Discriminator; use the exact listed value |

### data

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "data" | Yes | Discriminator; use the exact listed value |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

### closed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "closed" | Yes | Discriminator; use the exact listed value |

### error

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "error" | Yes | Discriminator; use the exact listed value |
| `stableCode` | [PluginApiErrorCode](./types.en.md#pluginapierrorcode) | Yes | Stable serial-event error code |

## PluginSerialFlowControl

Allowed string values: `none`, `software`, `hardware`.

## PluginSerialParity

Allowed string values: `none`, `odd`, `even`.

## PluginSerialPortKind

Allowed string values: `usb`, `pci`, `bluetooth`, `unknown`.

## PluginSerialPortMetadata

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `label` | String | Yes | User-facing display label |
| `kind` | [PluginSerialPortKind](./types.en.md#pluginserialportkind) | Yes | Discriminator; use the exact listed value |
| `manufacturer` | Option&lt;String&gt; | No | Optional device manufacturer display information |
| `product` | Option&lt;String&gt; | No | Optional device product display information |
| `usbVendorId` | Option&lt;u16&gt; | No | Optional numeric USB vendor ID |
| `usbProductId` | Option&lt;u16&gt; | No | Optional numeric USB product ID |

## PluginSerialSendRequest

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `handle` | String | Yes | Core-issued reference owned by the current caller |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

## PluginSerialSettings

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `baudRate` | u32 | Yes | Baud rate, 300–4000000 |
| `dataBits` | [PluginSerialDataBits](./types.en.md#pluginserialdatabits) | Yes | Data bits: five/six/seven/eight |
| `parity` | [PluginSerialParity](./types.en.md#pluginserialparity) | Yes | Parity: none/odd/even |
| `stopBits` | [PluginSerialStopBits](./types.en.md#pluginserialstopbits) | Yes | Stop bits: one/two |
| `flowControl` | [PluginSerialFlowControl](./types.en.md#pluginserialflowcontrol) | Yes | Flow control: none/software/hardware |

## PluginSerialStopBits

Allowed string values: `one`, `two`.

## PluginSftpDirectoryEntry

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `entryHandle` | String | Yes | Object reference returned by a directory listing |
| `name` | String | Yes | Name or identifier for this object |
| `kind` | [PluginSftpEntryKind](./types.en.md#pluginsftpentrykind) | Yes | Discriminator; use the exact listed value |
| `size` | Option&lt;u64&gt; | No | Object byte size observed by Core, if known |
| `modifiedAtUnixMs` | Option&lt;i64&gt; | No | Modification time observed by Core in Unix milliseconds |
| `precondition` | [PluginSftpObjectPrecondition](./types.en.md#pluginsftpobjectprecondition) | Yes | Echo object facts from the latest listing or read |

## PluginSftpEntryKind

Allowed string values: `file`, `directory`.

## PluginSftpEvent

### uploadStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadStarted" | Yes | Discriminator; use the exact listed value |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |
| `expectedBytes` | u64 | Yes | Total byte count declared when starting an upload |

### uploadProgress

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadProgress" | Yes | Discriminator; use the exact listed value |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |
| `receivedBytes` | u64 | Yes | Upload bytes accepted so far |
| `expectedBytes` | u64 | Yes | Total byte count declared when starting an upload |

### uploadCompleted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadCompleted" | Yes | Discriminator; use the exact listed value |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |
| `totalBytes` | u64 | Yes | Total byte count of the object |

### downloadProgress

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "downloadProgress" | Yes | Discriminator; use the exact listed value |
| `entryHandle` | String | Yes | Object reference returned by a directory listing |
| `nextOffset` | u64 | Yes | Byte offset for the next read |
| `totalBytes` | u64 | Yes | Total byte count of the object |
| `eof` | bool | Yes | Whether the end of data was reached |

## PluginSftpObjectPrecondition

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | [PluginSftpEntryKind](./types.en.md#pluginsftpentrykind) | Yes | Discriminator; use the exact listed value |
| `size` | Option&lt;u64&gt; | No | Object byte size observed by Core, if known |
| `modifiedAtUnixMs` | Option&lt;i64&gt; | No | Modification time observed by Core in Unix milliseconds |

## PluginSftpOperation

### list

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "list" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | Option&lt;String&gt; | No | Directory reference returned by SFTP list |
| `childEntryHandle` | Option&lt;String&gt; | No | Child-directory entry handle from a previous listing |
| `cursor` | Option&lt;String&gt; | No | Echo the prior nextCursor unchanged; omit on first page |
| `limit` | u16 | Yes | Maximum items or events returned in this call |

### read

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "read" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `entryHandle` | String | Yes | Object reference returned by a directory listing |
| `offset` | u64 | Yes | Zero-based byte offset |
| `length` | u16 | Yes | Requested byte count |

### writeBinary

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "writeBinary" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `entryHandle` | String | Yes | Object reference returned by a directory listing |
| `precondition` | [PluginSftpObjectPrecondition](./types.en.md#pluginsftpobjectprecondition) | Yes | Echo object facts from the latest listing or read |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

### uploadStart

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadStart" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `name` | String | Yes | Name or identifier for this object |
| `expectedBytes` | u64 | Yes | Total byte count declared when starting an upload |

### uploadReplaceStart

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadReplaceStart" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `entryHandle` | String | Yes | Object reference returned by a directory listing |
| `precondition` | [PluginSftpObjectPrecondition](./types.en.md#pluginsftpobjectprecondition) | Yes | Echo object facts from the latest listing or read |
| `expectedBytes` | u64 | Yes | Total byte count declared when starting an upload |

### uploadChunk

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadChunk" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |
| `offset` | u64 | Yes | Zero-based byte offset |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |

### uploadCommit

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadCommit" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |

### uploadAbort

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadAbort" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |

### createDirectory

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "createDirectory" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `name` | String | Yes | Name or identifier for this object |

### createEmptyFile

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "createEmptyFile" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `name` | String | Yes | Name or identifier for this object |

### renameNoReplace

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "renameNoReplace" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `sourceDirectoryHandle` | String | Yes | Source directory handle from list |
| `sourceEntryHandle` | String | Yes | Source entry handle from list |
| `sourcePrecondition` | [PluginSftpObjectPrecondition](./types.en.md#pluginsftpobjectprecondition) | Yes | Echo the source entry object facts unchanged |
| `targetDirectoryHandle` | String | Yes | Target directory handle returned by list |
| `targetName` | String | Yes | New entry name within the target directory, not a path |

### remove

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "remove" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `entryHandle` | String | Yes | Object reference returned by a directory listing |
| `precondition` | [PluginSftpObjectPrecondition](./types.en.md#pluginsftpobjectprecondition) | Yes | Echo object facts from the latest listing or read |

## PluginSftpResult

### opened

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "opened" | Yes | Discriminator; use the exact listed value |
| `rootHandle` | String | Yes | Root scope returned by filePick or sftpOpen |

### page

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "page" | Yes | Discriminator; use the exact listed value |
| `directoryHandle` | String | Yes | Directory reference returned by SFTP list |
| `entries` | Vec&lt;[PluginSftpDirectoryEntry](./types.en.md#pluginsftpdirectoryentry)&gt; | Yes | Storage or directory entries in this page |
| `nextCursor` | Option&lt;String&gt; | No | Pass to the next page if present; absent means the end |

### read

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "read" | Yes | Discriminator; use the exact listed value |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |
| `offset` | u64 | Yes | Zero-based byte offset |
| `nextOffset` | u64 | Yes | Byte offset for the next read |
| `totalBytes` | u64 | Yes | Total byte count of the object |
| `eof` | bool | Yes | Whether the end of data was reached |
| `precondition` | [PluginSftpObjectPrecondition](./types.en.md#pluginsftpobjectprecondition) | Yes | Echo object facts from the latest listing or read |

### written

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "written" | Yes | Discriminator; use the exact listed value |
| `totalBytes` | u64 | Yes | Total byte count of the object |

### uploadStarted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadStarted" | Yes | Discriminator; use the exact listed value |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |
| `expectedBytes` | u64 | Yes | Total byte count declared when starting an upload |

### uploadProgress

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadProgress" | Yes | Discriminator; use the exact listed value |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |
| `receivedBytes` | u64 | Yes | Upload bytes accepted so far |
| `expectedBytes` | u64 | Yes | Total byte count declared when starting an upload |

### uploaded

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploaded" | Yes | Discriminator; use the exact listed value |
| `uploadHandle` | String | Yes | Staged upload reference returned by uploadStart/uploadReplaceStart |
| `totalBytes` | u64 | Yes | Total byte count of the object |

### uploadAborted

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "uploadAborted" | Yes | Discriminator; use the exact listed value |

### created

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "created" | Yes | Discriminator; use the exact listed value |

### renamed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "renamed" | Yes | Discriminator; use the exact listed value |

### removed

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "removed" | Yes | Discriminator; use the exact listed value |

## PluginStorageBlobRead

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | String | Yes | Key within this plugin private namespace |
| `revision` | u64 | Yes | Current revision; echo for conditional mutation |
| `totalBytes` | u64 | Yes | Total byte count of the object |
| `offset` | u64 | Yes | Zero-based byte offset |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |
| `eof` | bool | Yes | Whether the end of data was reached |

## PluginStorageEntry

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | String | Yes | Key within this plugin private namespace |
| `valueBase64` | String | Yes | Standard Base64 non-secret storage value |
| `revision` | u64 | Yes | Current revision; echo for conditional mutation |

## PluginStorageMutation

### kvSet

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvSet" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `valueBase64` | String | Yes | Standard Base64 non-secret storage value |
| `expectedRevision` | u64 | Yes | Echo the latest read revision; do not increment it yourself |

### kvDelete

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvDelete" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `expectedRevision` | u64 | Yes | Echo the latest read revision; do not increment it yourself |

## PluginStorageOperation

### kvGet

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvGet" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |

### kvSet

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvSet" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `valueBase64` | String | Yes | Standard Base64 non-secret storage value |
| `expectedRevision` | u64 | Yes | Echo the latest read revision; do not increment it yourself |

### kvDelete

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvDelete" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `expectedRevision` | u64 | Yes | Echo the latest read revision; do not increment it yourself |

### kvList

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvList" | Yes | Discriminator; use the exact listed value |
| `prefix` | String | Yes | Key-prefix filter inside the current namespace |
| `cursor` | Option&lt;String&gt; | No | Echo the prior nextCursor unchanged; omit on first page |
| `limit` | u16 | Yes | Maximum items or events returned in this call |

### blobRead

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "blobRead" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `offset` | u64 | Yes | Zero-based byte offset |
| `length` | u16 | Yes | Requested byte count |

### blobWrite

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "blobWrite" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `offset` | u64 | Yes | Zero-based byte offset |
| `dataBase64` | String | Yes | Binary bytes encoded as standard Base64 |
| `expectedRevision` | u64 | Yes | Echo the latest read revision; do not increment it yourself |

### blobDelete

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "blobDelete" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `expectedRevision` | u64 | Yes | Echo the latest read revision; do not increment it yourself |

### cacheGet

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "cacheGet" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |

### cacheSet

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "cacheSet" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |
| `valueBase64` | String | Yes | Standard Base64 non-secret storage value |
| `ttlMs` | u64 | Yes | Cache lifetime in milliseconds |

### cacheDelete

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "cacheDelete" | Yes | Discriminator; use the exact listed value |
| `key` | String | Yes | Key within this plugin private namespace |

### cacheClear

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "cacheClear" | Yes | Discriminator; use the exact listed value |

### schemaGet

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "schemaGet" | Yes | Discriminator; use the exact listed value |

### schemaCommit

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "schemaCommit" | Yes | Discriminator; use the exact listed value |
| `expectedVersion` | u64 | Yes | Current schema version returned by schemaGet |
| `newVersion` | u64 | Yes | Schema version after successful commit |
| `mutations` | Vec&lt;[PluginStorageMutation](./types.en.md#pluginstoragemutation)&gt; | Yes | KV mutation declarations in one schema transaction |

## PluginStorageResult

### kvValue

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvValue" | Yes | Discriminator; use the exact listed value |
| `state` | [PluginStorageState](./types.en.md#pluginstoragestate) | Yes | Current state; handle the corresponding enum |
| `entry` | Option&lt;[PluginStorageEntry](./types.en.md#pluginstorageentry)&gt; | No | Entry when present; may be absent for missing/expired data |

### kvPage

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "kvPage" | Yes | Discriminator; use the exact listed value |
| `state` | [PluginStorageState](./types.en.md#pluginstoragestate) | Yes | Current state; handle the corresponding enum |
| `entries` | Vec&lt;[PluginStorageEntry](./types.en.md#pluginstorageentry)&gt; | Yes | Storage or directory entries in this page |
| `nextCursor` | Option&lt;String&gt; | No | Pass to the next page if present; absent means the end |

### blobRead

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "blobRead" | Yes | Discriminator; use the exact listed value |
| `state` | [PluginStorageState](./types.en.md#pluginstoragestate) | Yes | Current state; handle the corresponding enum |
| `blob` | [PluginStorageBlobRead](./types.en.md#pluginstorageblobread) | Yes | Chunked read result for the current blob |

### blobWritten

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "blobWritten" | Yes | Discriminator; use the exact listed value |
| `state` | [PluginStorageState](./types.en.md#pluginstoragestate) | Yes | Current state; handle the corresponding enum |
| `key` | String | Yes | Key within this plugin private namespace |
| `revision` | u64 | Yes | Current revision; echo for conditional mutation |
| `totalBytes` | u64 | Yes | Total byte count of the object |

### cacheValue

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "cacheValue" | Yes | Discriminator; use the exact listed value |
| `state` | [PluginStorageState](./types.en.md#pluginstoragestate) | Yes | Current state; handle the corresponding enum |
| `entry` | Option&lt;[PluginStorageEntry](./types.en.md#pluginstorageentry)&gt; | No | Entry when present; may be absent for missing/expired data |

### state

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "state" | Yes | Discriminator; use the exact listed value |
| `state` | [PluginStorageState](./types.en.md#pluginstoragestate) | Yes | Current state; handle the corresponding enum |

## PluginStorageState

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `storeRevision` | u64 | Yes | Current integer revision of the private store |
| `schemaVersion` | u64 | Yes | Current storage schema version |

## PluginSubscriptionEvent

### terminalContextChanged

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "terminalContextChanged" | Yes | Discriminator; use the exact listed value |
| `contextHandle` | String | Yes | Core-issued terminal-context reference |
| `targetId` | [PluginExtensionTargetId](./types.en.md#pluginextensiontargetid) | Yes | Declared plugin extension target ID |
| `generation` | [WireSequence](./types.en.md#wiresequence) | Yes | Current instance or context generation as a decimal string |
| `state` | [PluginTerminalState](./types.en.md#pluginterminalstate) | Yes | Current state; handle the corresponding enum |

### terminalContextEnded

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "terminalContextEnded" | Yes | Discriminator; use the exact listed value |
| `contextHandle` | String | Yes | Core-issued terminal-context reference |
| `generation` | [WireSequence](./types.en.md#wiresequence) | Yes | Current instance or context generation as a decimal string |

### hostScopeChanged

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "hostScopeChanged" | Yes | Discriminator; use the exact listed value |
| `scopeStateVersion` | Option&lt;[WireSequence](./types.en.md#wiresequence)&gt; | No | Current host-scope revision, possibly null |

### pluginSettingsChanged

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "pluginSettingsChanged" | Yes | Discriminator; use the exact listed value |
| `revision` | [WireSequence](./types.en.md#wiresequence) | Yes | Current revision; echo for conditional mutation |

## PluginSubscriptionTopic

### terminalContext

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "terminalContext" | Yes | Discriminator; use the exact listed value |
| `contextHandle` | String | Yes | Core-issued terminal-context reference |

### hostScope

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "hostScope" | Yes | Discriminator; use the exact listed value |

### pluginSettings

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "pluginSettings" | Yes | Discriminator; use the exact listed value |

## PluginTerminalState

Allowed string values: `starting`, `running`, `awaitingUser`, `closing`, `closed`, `failed`.

## PluginUiActionId

Opaque string identifier; preserve the value provided by Core.

## PluginWorkflowApiMethod

Allowed string values: `serialDevices`, `serialOpen`, `serialSend`, `protocolOpen`, `describe`, `permissions`, `permissionRequest`, `permissionRevoke`, `permissionsForget`, `resourcesList`, `resourceClose`, `subscriptionStart`, `timerStart`, `resourceEvents`, `networkStart`, `networkSend`, `remoteExecStart`, `remoteExecSend`, `processStart`, `processSend`, `sftpOpen`, `sftp`, `filePick`, `file`, `credential`, `storage`, `terminalRequestInput`.

## PluginWorkflowStepState

Allowed string values: `dispatching`, `succeeded`, `failed`, `needsUserAction`, `outcomeUnknown`, `interrupted`.

## PluginWorkflowTaskId

Opaque string identifier; preserve the value provided by Core.

## PluginWorkflowTaskResult

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `taskId` | [PluginWorkflowTaskId](./types.en.md#pluginworkflowtaskid) | Yes | Opaque task ID from a snapshot |
| `replies` | Vec&lt;[PluginApiReply](./types.en.md#pluginapireply)&gt; | Yes | Step broker replies retained in-process |

## PluginWorkflowTaskSnapshot

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `task` | [PluginWorkflowTaskSummary](./types.en.md#pluginworkflowtasksummary) | Yes | Task identity, state, revision, and timestamps |
| `steps` | Vec&lt;[PluginWorkflowTaskStepSummary](./types.en.md#pluginworkflowtaskstepsummary)&gt; | Yes | Task step-state summaries |
| `result` | Option&lt;[PluginWorkflowTaskResult](./types.en.md#pluginworkflowtaskresult)&gt; | No | Nested result union; inspect result.kind first |

## PluginWorkflowTaskState

Allowed string values: `running`, `dispatching`, `needsUserAction`, `cancelling`, `cancelled`, `completed`, `failed`, `interrupted`, `cleanupIncomplete`.

## PluginWorkflowTaskStepSummary

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `stepId` | String | Yes | Step ID in the workflow catalog |
| `method` | [PluginWorkflowApiMethod](./types.en.md#pluginworkflowapimethod) | Yes | Typed broker method or HTTP verb |
| `state` | [PluginWorkflowStepState](./types.en.md#pluginworkflowstepstate) | Yes | Current state; handle the corresponding enum |
| `revision` | [WireSequence](./types.en.md#wiresequence) | Yes | Current revision; echo for conditional mutation |
| `outcomeUnknown` | bool | Yes | Prior side-effect outcome is unknown; do not blindly replay |
| `createdAtUnixMs` | i64 | Yes | Creation timestamp in Unix milliseconds |
| `updatedAtUnixMs` | i64 | Yes | Last update timestamp in Unix milliseconds |

## PluginWorkflowTaskSummary

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `taskId` | [PluginWorkflowTaskId](./types.en.md#pluginworkflowtaskid) | Yes | Opaque task ID from a snapshot |
| `pluginId` | [PluginId](./types.en.md#pluginid) | Yes | Core-owned plugin ID, not guest-supplied authority |
| `workflowId` | String | Yes | ID declared in the package workflow catalog |
| `state` | [PluginWorkflowTaskState](./types.en.md#pluginworkflowtaskstate) | Yes | Current state; handle the corresponding enum |
| `revision` | [WireSequence](./types.en.md#wiresequence) | Yes | Current revision; echo for conditional mutation |
| `currentStepId` | Option&lt;String&gt; | No | Current executing step, if present |
| `outcomeUnknown` | bool | Yes | Prior side-effect outcome is unknown; do not blindly replay |
| `cleanupIncomplete` | bool | Yes | Whether resource cleanup is still incomplete |
| `createdAtUnixMs` | i64 | Yes | Creation timestamp in Unix milliseconds |
| `updatedAtUnixMs` | i64 | Yes | Last update timestamp in Unix milliseconds |

## WireSequence

Non-negative decimal integer encoded as a JSON string, for example `"1"`.
