# Broker 数据类型

本页只列出方法请求、返回和资源事件可达的 guest DTO，不包含 Core 的授权计划、秘密或内部请求。`String` 是 JSON 字符串，`bool` 是布尔值，`Vec<T>` 是数组，整数类型使用 JSON 整数；`Option<T>` 可省略或传 null。**WireSequence 使用十进制字符串**（如 `"1"`），不能传数字。`u64` 的 JSON 数字在 JavaScript 中应避免超过安全整数范围。枚举对象的 `kind` 和字段区分大小写。

## PluginApiCall

`PluginApiCall { callId, operation: PluginApiOperation }` 放在 SDK 的 `api.request` output 内发出。`callId` 只用于关联 reply，不是 authority。 `callId` 长度为 1–80 字节，只允许 ASCII 字母、数字、点、下划线和连字符，例如 `protocol-demo.open`；冒号不合法。它与 UI action ID 是不同字段，不应直接复用。

## PluginApiOperation

完整 method 清单见[Broker 方法](broker.zh-CN.md)。tagged operation enum 的嵌套 payload 包含 `PluginAppRegistration`、`PluginAppNotification`、`PluginAppNavigation`、`PluginWorkflowTaskId`、`PluginSerialSettings`、`PluginProtocolOpen`、`PluginNetworkEndpointRequest`、`PluginNetworkStartRequest`、`PluginRemoteExecStartRequest`、`PluginProcessSendRequest`、`PluginSftpOperation`、`PluginFileAccessRequest`、`PluginFileOperation`、`PluginCredentialOperation` 和 `PluginStorageOperation`。

## PluginApiAvailability

允许的字符串值：`available`, `notImplemented`, `unsupportedPlatform`.

## PluginApiDescription

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `protocolMajor` | u16 | 是 | 插件协议主版本，当前为 1 |
| `protocolMinor` | u16 | 是 | 插件协议次版本，当前为 13 |
| `platform` | String | 是 | Core 运行平台标识 |
| `methods` | Vec&lt;[PluginApiMethod](./types.zh-CN.md#pluginapimethod)&gt; | 是 | 方法名、能力提示和实现状态列表 |
| `limits` | [PluginApiLimits](./types.zh-CN.md#pluginapilimits) | 是 | 本运行时的调用、分块、资源和事件上限 |

## PluginApiErrorCode

允许的字符串值：`invalidRequest`, `permissionDenied`, `interactionRequired`, `vaultMissing`, `vaultLocked`, `vaultRequiresReload`, `unsupported`, `revoked`, `notFound`, `conflict`, `busy`, `quotaExceeded`, `timedOut`, `cancelled`, `outcomeUnknown`, `cleanupIncomplete`, `unavailable`.

## PluginApiLimits

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `maxCallBytes` | u32 | 是 | 单次 API 调用最大序列化字节数 |
| `maxChunkBytes` | u32 | 是 | 通用数据块最大字节数；专用操作可能更低 |
| `maxResources` | u16 | 是 | 插件资源数量上限 |
| `maxPendingEvents` | u16 | 是 | 资源事件队列上限 |

## PluginApiMethod

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | String | 是 | 当前对象的名称或标识 |
| `capability` | Option&lt;[PluginCapability](./types.zh-CN.md#plugincapability)&gt; | 否 | PluginCapability 枚举值；不是已授予权限 |
| `availability` | [PluginApiAvailability](./types.zh-CN.md#pluginapiavailability) | 是 | 方法实现状态；不代表权限或设备已就绪 |

## PluginApiOutcome

### completed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "completed" | 是 | 判别标签；必须使用表中固定值 |
| `value` | [PluginApiValue](./types.zh-CN.md#pluginapivalue) | 是 | 成功返回的类型化值 |

### failed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "failed" | 是 | 判别标签；必须使用表中固定值 |
| `code` | [PluginApiErrorCode](./types.zh-CN.md#pluginapierrorcode) | 是 | 稳定错误代码，不包含上游秘密信息 |

## PluginApiReply

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `callId` | String | 是 | 插件分配的关联标识；不是授权凭据 |
| `outcome` | [PluginApiOutcome](./types.zh-CN.md#pluginapioutcome) | 是 | completed 或 failed 结果联合 |

## PluginApiResourceEvent

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `sequence` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 事件十进制序号字符串 |
| `kind` | [PluginApiResourceEventKind](./types.zh-CN.md#pluginapiresourceeventkind) | 是 | 嵌套事件对象，其内 kind 才是字符串判别标签 |

## PluginApiResourceEventKind

### serial

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serial" | 是 | 判别标签；必须使用表中固定值 |
| `event` | [PluginSerialEvent](./types.zh-CN.md#pluginserialevent) | 是 | 按内层 kind 解析的资源事件 |

### timerFired

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "timerFired" | 是 | 判别标签；必须使用表中固定值 |

### cancelled

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "cancelled" | 是 | 判别标签；必须使用表中固定值 |

### fileChanged

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "fileChanged" | 是 | 判别标签；必须使用表中固定值 |
| `change` | [PluginFileWatchChange](./types.zh-CN.md#pluginfilewatchchange) | 是 | 文件监听产生的变更元数据 |

### sftp

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "sftp" | 是 | 判别标签；必须使用表中固定值 |
| `event` | [PluginSftpEvent](./types.zh-CN.md#pluginsftpevent) | 是 | 按内层 kind 解析的资源事件 |

### subscription

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "subscription" | 是 | 判别标签；必须使用表中固定值 |
| `event` | [PluginSubscriptionEvent](./types.zh-CN.md#pluginsubscriptionevent) | 是 | 按内层 kind 解析的资源事件 |

### network

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "network" | 是 | 判别标签；必须使用表中固定值 |
| `event` | [PluginNetworkEvent](./types.zh-CN.md#pluginnetworkevent) | 是 | 按内层 kind 解析的资源事件 |

### processOutput

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "processOutput" | 是 | 判别标签；必须使用表中固定值 |
| `stream` | [PluginProcessOutputStream](./types.zh-CN.md#pluginprocessoutputstream) | 是 | stdout 或 stderr |
| `data_base64` | String | 是 | 标准 Base64 字节；此字段保留 snake_case |

### processExited

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "processExited" | 是 | 判别标签；必须使用表中固定值 |
| `exit_code` | Option&lt;i64&gt; | 否 | 进程退出码，wire 保留下划线 |

### remoteExec

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "remoteExec" | 是 | 判别标签；必须使用表中固定值 |
| `event` | [PluginRemoteExecEvent](./types.zh-CN.md#pluginremoteexecevent) | 是 | 按内层 kind 解析的资源事件 |

## PluginApiResourceState

允许的字符串值：`opening`, `open`, `closing`, `cleanupIncomplete`.

## PluginApiResourceSummary

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `generation` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 当前实例或上下文代次的十进制字符串 |
| `resourceKind` | String | 是 | 资源类别摘要 |
| `state` | [PluginApiResourceState](./types.zh-CN.md#pluginapiresourcestate) | 是 | 当前状态，按对应枚举处理 |

## PluginApiValue

### appAccepted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "appAccepted" | 是 | 判别标签；必须使用表中固定值 |

### task

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "task" | 是 | 判别标签；必须使用表中固定值 |
| `snapshot` | [PluginWorkflowTaskSnapshot](./types.zh-CN.md#pluginworkflowtasksnapshot) | 是 | 包含 task、steps、可选 result 的任务快照 |

### tasks

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "tasks" | 是 | 判别标签；必须使用表中固定值 |
| `snapshots` | Vec&lt;[PluginWorkflowTaskSnapshot](./types.zh-CN.md#pluginworkflowtasksnapshot)&gt; | 是 | 当前插件可见的任务快照数组 |

### serialDevices

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serialDevices" | 是 | 判别标签；必须使用表中固定值 |
| `devices` | Vec&lt;[PluginSerialDeviceCandidate](./types.zh-CN.md#pluginserialdevicecandidate)&gt; | 是 | 不含原生设备路径的候选元数据 |

### serialStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serialStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### serialSent

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serialSent" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### protocolLaunched

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "protocolLaunched" | 是 | 判别标签；必须使用表中固定值 |
| `launchId` | String | 是 | 宿主终端启动记录标识，不表示已连接 |

### description

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "description" | 是 | 判别标签；必须使用表中固定值 |
| `api` | [PluginApiDescription](./types.zh-CN.md#pluginapidescription) | 是 | 协议版本、平台、方法与限额 |

### permissions

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissions" | 是 | 判别标签；必须使用表中固定值 |
| `grants` | Vec&lt;[PluginCapabilityGrant](./types.zh-CN.md#plugincapabilitygrant)&gt; | 是 | 当前能力授权记录 |
| `policyRevision` | Option&lt;[WireSequence](./types.zh-CN.md#wiresequence)&gt; | 否 | 可空的策略版本十进制字符串 |
| `operationPermissions` | Vec&lt;[PluginOperationPermission](./types.zh-CN.md#pluginoperationpermission)&gt; | 是 | 自身记住操作授权的非秘密摘要 |

### permissionRequested

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissionRequested" | 是 | 判别标签；必须使用表中固定值 |
| `approvalId` | String | 是 | 待处理授权请求标识，不表示已经批准 |

### permissionRevoked

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissionRevoked" | 是 | 判别标签；必须使用表中固定值 |
| `policyRevision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 可空的策略版本十进制字符串 |

### permissionsForgotten

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissionsForgotten" | 是 | 判别标签；必须使用表中固定值 |

### resources

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "resources" | 是 | 判别标签；必须使用表中固定值 |
| `resources` | Vec&lt;[PluginApiResourceSummary](./types.zh-CN.md#pluginapiresourcesummary)&gt; | 是 | 当前所有者的资源摘要 |

### closed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "closed" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### timerStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "timerStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### resourceEvents

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "resourceEvents" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `events` | Vec&lt;[PluginApiResourceEvent](./types.zh-CN.md#pluginapiresourceevent)&gt; | 是 | 按 sequence 处理的事件列表 |
| `backpressured` | bool | 是 | 资源事件队列是否发生背压 |

### sftp

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "sftp" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginSftpResult](./types.zh-CN.md#pluginsftpresult) | 是 | 嵌套结果联合；先检查 result.kind |

### subscriptionStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "subscriptionStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### credential

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "credential" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginCredentialResult](./types.zh-CN.md#plugincredentialresult) | 是 | 嵌套结果联合；先检查 result.kind |

### networkStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "networkStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### networkSent

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "networkSent" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### remoteExecStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "remoteExecStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### remoteExecSent

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "remoteExecSent" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### processStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "processStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### processSent

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "processSent" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

### filePicked

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "filePicked" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `label` | String | 是 | 供用户识别的显示文字 |

### file

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "file" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginFileResult](./types.zh-CN.md#pluginfileresult) | 是 | 嵌套结果联合；先检查 result.kind |

### storage

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "storage" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginStorageResult](./types.zh-CN.md#pluginstorageresult) | 是 | 嵌套结果联合；先检查 result.kind |

### inputApprovalRequested

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "inputApprovalRequested" | 是 | 判别标签；必须使用表中固定值 |
| `approvalId` | String | 是 | 待处理授权请求标识，不表示已经批准 |

### inputSent

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "inputSent" | 是 | 判别标签；必须使用表中固定值 |

## PluginAppCommand

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `id` | String | 是 | 当前声明范围内的唯一标识 |
| `label` | String | 是 | 供用户识别的显示文字 |
| `targetId` | [PluginExtensionTargetId](./types.zh-CN.md#pluginextensiontargetid) | 是 | 已声明的插件扩展目标标识 |
| `actionId` | [PluginUiActionId](./types.zh-CN.md#pluginuiactionid) | 是 | 已验证文档中的动作标识 |
| `pageId` | Option&lt;String&gt; | 否 | app.page 命令对应的页面 id |
| `shortcut` | Option&lt;String&gt; | 否 | 可选 Alt+Shift+大写字母快捷键 |
| `fileExtensions` | Vec&lt;String&gt; | 是 | 不带点的文件扩展名，最多 12 项 |

## PluginAppNavigation

### app

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "app" | 是 | 判别标签；必须使用表中固定值 |
| `path` | String | 是 | 宿主允许的应用路由 |

### pluginPage

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "pluginPage" | 是 | 判别标签；必须使用表中固定值 |
| `page_id` | String | 是 | 自身插件页面 id；wire 使用下划线 |

## PluginAppNotification

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `id` | String | 是 | 当前声明范围内的唯一标识 |
| `text` | String | 是 | 用户可见文本 |

## PluginAppRegistration

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `commands` | Vec&lt;[PluginAppCommand](./types.zh-CN.md#pluginappcommand)&gt; | 是 | 注册命令列表，最多 24 项 |
| `statuses` | Vec&lt;[PluginAppStatus](./types.zh-CN.md#pluginappstatus)&gt; | 是 | 插件状态文字列表，最多 8 项 |

## PluginAppStatus

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `id` | String | 是 | 当前声明范围内的唯一标识 |
| `text` | String | 是 | 用户可见文本 |

## PluginApprovalOperation

允许的字符串值：`remoteExecute`, `forwardStart`, `forwardStop`, `networkRequest`, `fileAccess`, `sftpRead`, `sftpWrite`, `serialAccess`, `localExecute`, `terminalInput`, `hostMutation`, `hostSession`.

## PluginCapability

允许的字符串值：`uiPanel`, `uiNavigation`, `uiPage`, `uiWebviewIsolated`, `uiHostDomObserve`, `uiHostDomMutate`, `uiHostCss`, `clipboardWrite`, `terminalProvider`, `deviceSerial`, `terminalMetadata`, `terminalObserve`, `terminalAnnotation`, `terminalProposeInput`, `terminalRequestInput`, `hostMetadataRead`, `hostMutationPropose`, `hostSessionRequest`, `remoteInspect`, `remoteExecRequest`, `networkDomain`, `localFiles`, `localProcess`, `storagePlugin`, `credentialsPlugin`, `sftpRead`, `sftpWrite`, `metricsRead`, `sshSync`.

## PluginCapabilityGrant

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `capability` | [PluginCapability](./types.zh-CN.md#plugincapability) | 是 | PluginCapability 枚举值；不是已授予权限 |
| `granted` | bool | 是 | 此能力是否已授予 |

## PluginCredentialInjection

### bearer

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "bearer" | 是 | 判别标签；必须使用表中固定值 |

### header

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "header" | 是 | 判别标签；必须使用表中固定值 |
| `name` | String | 是 | 当前对象的名称或标识 |

## PluginCredentialOperation

### create

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "create" | 是 | 判别标签；必须使用表中固定值 |
| `operationId` | String | 是 | 当前创建意图的操作关联标识 |
| `idempotencyKey` | String | 是 | 同一凭据创建意图使用的稳定幂等键 |
| `label` | String | 是 | 供用户识别的显示文字 |
| `target` | [PluginCredentialTarget](./types.zh-CN.md#plugincredentialtarget) | 是 | 凭据的 origin 与注入规则 |

### list

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "list" | 是 | 判别标签；必须使用表中固定值 |

### revoke

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "revoke" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `expectedRevision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 回传最近读取的 revision；不要自行递增 |

## PluginCredentialResult

### created

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "created" | 是 | 判别标签；必须使用表中固定值 |
| `credential` | [PluginCredentialSummary](./types.zh-CN.md#plugincredentialsummary) | 是 | 非秘密凭据摘要或引用，按对应类型填写 |

### list

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "list" | 是 | 判别标签；必须使用表中固定值 |
| `credentials` | Vec&lt;[PluginCredentialSummary](./types.zh-CN.md#plugincredentialsummary)&gt; | 是 | 当前插件可见的非秘密凭据摘要 |

### revoked

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "revoked" | 是 | 判别标签；必须使用表中固定值 |
| `credential` | [PluginCredentialSummary](./types.zh-CN.md#plugincredentialsummary) | 是 | 非秘密凭据摘要或引用，按对应类型填写 |

## PluginCredentialState

允许的字符串值：`pendingVault`, `ready`, `cleanupPending`, `revoked`.

## PluginCredentialSummary

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `label` | String | 是 | 供用户识别的显示文字 |
| `target` | [PluginCredentialTarget](./types.zh-CN.md#plugincredentialtarget) | 是 | 凭据的 origin 与注入规则 |
| `revision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 当前版本；后续条件写入回传此值 |
| `state` | [PluginCredentialState](./types.zh-CN.md#plugincredentialstate) | 是 | 当前状态，按对应枚举处理 |

## PluginCredentialTarget

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `origin` | String | 是 | 凭据绑定的精确 origin |
| `injection` | [PluginCredentialInjection](./types.zh-CN.md#plugincredentialinjection) | 是 | bearer 或受允许的 header 注入方式 |

## PluginExtensionTargetId

不透明字符串标识；使用 Core 提供的原值。

## PluginFileAccessRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `read` | bool | 否 | 申请读取权限，默认 false |
| `write` | bool | 否 | 申请写入范围；仍需对应能力和批准 |
| `list` | bool | 否 | 申请目录列举权限，默认 false |
| `rename` | bool | 否 | 申请重命名权限，默认 false |
| `remove` | bool | 否 | 申请删除权限，默认 false |
| `recursiveRemove` | bool | 否 | 申请递归删除权限，默认 false |
| `watch` | bool | 否 | 申请变更监听权限，默认 false |

## PluginFileEntry

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | String | 是 | 当前对象的名称或标识 |
| `kind` | [PluginFileEntryKind](./types.zh-CN.md#pluginfileentrykind) | 是 | 判别标签；必须使用表中固定值 |
| `size` | Option&lt;u64&gt; | 否 | Core 观测的对象字节数，可缺失 |
| `fingerprint` | Option&lt;String&gt; | 否 | Core 观测的文件版本指纹 |

## PluginFileEntryKind

允许的字符串值：`file`, `directory`.

## PluginFileOperation

### read

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "read" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `relativePath` | String | 是 | 根范围内的相对路径；文件根使用空字符串 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |

### write

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "write" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `relativePath` | String | 是 | 根范围内的相对路径；文件根使用空字符串 |
| `expectedFingerprint` | Option&lt;String&gt; | 否 | 最近读/列举的指纹；write 省略时仅新建 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |
| `finalSize` | Option&lt;u64&gt; | 否 | 写入后最终文件字节数，存在时应用 |

### list

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "list" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `relativePath` | String | 是 | 根范围内的相对路径；文件根使用空字符串 |
| `cursor` | Option&lt;String&gt; | 否 | 原样回传上页 nextCursor；首请求省略 |
| `limit` | u16 | 是 | 本次最多返回的项目或事件数量 |

### rename

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "rename" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `fromPath` | String | 是 | 根范围内源相对路径 |
| `toPath` | String | 是 | 根范围内的新相对路径，目标必须不存在 |
| `expectedFingerprint` | String | 是 | 最近读/列举的指纹；write 省略时仅新建 |

### remove

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "remove" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `relativePath` | String | 是 | 根范围内的相对路径；文件根使用空字符串 |
| `expectedFingerprint` | String | 是 | 最近读/列举的指纹；write 省略时仅新建 |
| `recursive` | bool | 是 | 是否递归删除，默认 false，仍需对应授权 |

### watchStart

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "watchStart" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `relativePath` | String | 是 | 根范围内的相对路径；文件根使用空字符串 |
| `intervalMs` | u32 | 是 | 周期毫秒数；省略时为一次性 |

## PluginFilePickerKind

允许的字符串值：`file`, `directory`.

## PluginFileResult

### read

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "read" | 是 | 判别标签；必须使用表中固定值 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |
| `fingerprint` | String | 是 | Core 观测的文件版本指纹 |
| `eof` | bool | 是 | 是否已到数据末尾 |

### written

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "written" | 是 | 判别标签；必须使用表中固定值 |
| `fingerprint` | String | 是 | Core 观测的文件版本指纹 |

### listed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "listed" | 是 | 判别标签；必须使用表中固定值 |
| `entries` | Vec&lt;[PluginFileEntry](./types.zh-CN.md#pluginfileentry)&gt; | 是 | 本页存储条目或目录条目 |
| `nextCursor` | Option&lt;String&gt; | 否 | 存在时用于下一页；省略表示已结束 |

### renamed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "renamed" | 是 | 判别标签；必须使用表中固定值 |
| `fingerprint` | String | 是 | Core 观测的文件版本指纹 |

### removed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "removed" | 是 | 判别标签；必须使用表中固定值 |

### watchStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "watchStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

## PluginFileWatchChange

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `relativePath` | String | 是 | 根范围内的相对路径；文件根使用空字符串 |
| `fingerprint` | Option&lt;String&gt; | 否 | Core 观测的文件版本指纹 |

## PluginHttpMethod

允许的字符串值：`get`, `head`, `post`, `put`, `patch`, `delete`.

## PluginId

不透明字符串标识；使用 Core 提供的原值。

## PluginNetworkCredentialRef

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `expectedRevision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 回传最近读取的 revision；不要自行递增 |

## PluginNetworkEndpointRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `endpoint` | String | 是 | 待 Core 解析与批准的完整端点 URL |

## PluginNetworkErrorCode

允许的字符串值：`invalidEndpoint`, `resolveFailed`, `connectFailed`, `tlsFailed`, `httpFailed`, `protocolFailed`, `timedOut`, `revoked`, `cancelled`, `unavailable`.

## PluginNetworkEvent

### opened

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "opened" | 是 | 判别标签；必须使用表中固定值 |
| `protocol` | [PluginNetworkProtocol](./types.zh-CN.md#pluginnetworkprotocol) | 是 | 实际资源协议枚举 |
| `peerAddress` | Option&lt;String&gt; | 否 | 实际连接的对端地址，可能为 null |

### httpResponse

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "httpResponse" | 是 | 判别标签；必须使用表中固定值 |
| `status` | u16 | 是 | HTTP 响应状态码 |
| `headers` | Vec&lt;[PluginNetworkHeader](./types.zh-CN.md#pluginnetworkheader)&gt; | 是 | HTTP/WebSocket 头列表；敏感注入由 Core 处理 |

### data

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "data" | 是 | 判别标签；必须使用表中固定值 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

### datagram

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "datagram" | 是 | 判别标签；必须使用表中固定值 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

### closed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "closed" | 是 | 判别标签；必须使用表中固定值 |

### error

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "error" | 是 | 判别标签；必须使用表中固定值 |
| `code` | [PluginNetworkErrorCode](./types.zh-CN.md#pluginnetworkerrorcode) | 是 | 稳定错误代码，不包含上游秘密信息 |

## PluginNetworkHeader

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | String | 是 | 当前对象的名称或标识 |
| `value` | String | 是 | 成功返回的类型化值 |

## PluginNetworkOperation

### http

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "http" | 是 | 判别标签；必须使用表中固定值 |
| `method` | [PluginHttpMethod](./types.zh-CN.md#pluginhttpmethod) | 是 | 对应方法或 HTTP 动词的枚举值 |
| `headers` | Vec&lt;[PluginNetworkHeader](./types.zh-CN.md#pluginnetworkheader)&gt; | 是 | HTTP/WebSocket 头列表；敏感注入由 Core 处理 |
| `bodyBase64` | String | 是 | HTTP 请求体的标准 Base64；无正文用空字符串 |

### webSocket

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "webSocket" | 是 | 判别标签；必须使用表中固定值 |
| `headers` | Vec&lt;[PluginNetworkHeader](./types.zh-CN.md#pluginnetworkheader)&gt; | 是 | HTTP/WebSocket 头列表；敏感注入由 Core 处理 |

### tcp

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "tcp" | 是 | 判别标签；必须使用表中固定值 |

### udp

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "udp" | 是 | 判别标签；必须使用表中固定值 |

### tls

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "tls" | 是 | 判别标签；必须使用表中固定值 |

## PluginNetworkProtocol

允许的字符串值：`http`, `webSocket`, `tcp`, `udp`, `tls`.

## PluginNetworkSendRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

## PluginNetworkStartRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `timeoutMs` | u32 | 是 | 毫秒超时，仍受 Core 上限约束 |
| `credential` | Option&lt;[PluginNetworkCredentialRef](./types.zh-CN.md#pluginnetworkcredentialref)&gt; | 否 | 非秘密凭据摘要或引用，按对应类型填写 |
| `operation` | [PluginNetworkOperation](./types.zh-CN.md#pluginnetworkoperation) | 是 | 嵌套判别联合；按 kind 选择字段 |

## PluginOperationPermission

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `permissionId` | String | 是 | permissions 返回的记住操作授权标识 |
| `pluginId` | [PluginId](./types.zh-CN.md#pluginid) | 是 | Core 归属的插件标识，不可当作输入授权 |
| `operation` | [PluginApprovalOperation](./types.zh-CN.md#pluginapprovaloperation) | 是 | 嵌套判别联合；按 kind 选择字段 |
| `actionLabel` | String | 是 | 操作的非秘密显示摘要 |
| `targetLabel` | String | 是 | 目标的非秘密管理显示摘要 |
| `createdAtUnixMs` | i64 | 是 | 创建时间，Unix 毫秒数 |
| `expiresAtUnixMs` | Option&lt;i64&gt; | 否 | 绝对过期时间（Unix 毫秒）；null 表示无限期 |

## PluginProcessOutputStream

允许的字符串值：`stdout`, `stderr`.

## PluginProcessSendRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |
| `closeStdin` | bool | 否 | 是否发送 EOF，默认 false；EOF 搭配空数据 |

## PluginProtocolOpen

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `providerId` | String | 是 | 包内声明的协议 Provider id |
| `configuration` | BTreeMap&lt;String, serde_json::Value&gt; | 是 | Provider schema 接受的设置值 |
| `label` | Option&lt;String&gt; | 否 | 供用户识别的显示文字 |

## PluginRemoteExecEvent

### remoteExecOutput

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "remoteExecOutput" | 是 | 判别标签；必须使用表中固定值 |
| `stream` | [PluginProcessOutputStream](./types.zh-CN.md#pluginprocessoutputstream) | 是 | stdout 或 stderr |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

### remoteExecExited

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "remoteExecExited" | 是 | 判别标签；必须使用表中固定值 |
| `exitCode` | Option&lt;i64&gt; | 否 | 进程退出码；无确定退出码时省略 |

## PluginRemoteExecSendRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |
| `closeStdin` | bool | 否 | 是否发送 EOF，默认 false；EOF 搭配空数据 |

## PluginRemoteExecStartRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `hostHandle` | String | 是 | Core 主机上下文提供的不透明引用 |
| `command` | String | 是 | 请求在批准的远程主机执行的命令 |
| `timeoutMs` | u32 | 是 | 毫秒超时，仍受 Core 上限约束 |

## PluginSerialDataBits

允许的字符串值：`five`, `six`, `seven`, `eight`.

## PluginSerialDeviceCandidate

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `candidateId` | String | 是 | serialDevices 返回的短期候选标识 |
| `metadata` | [PluginSerialPortMetadata](./types.zh-CN.md#pluginserialportmetadata) | 是 | 可显示设备信息，不含原生路径 |

## PluginSerialEvent

### opened

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "opened" | 是 | 判别标签；必须使用表中固定值 |

### data

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "data" | 是 | 判别标签；必须使用表中固定值 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

### closed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "closed" | 是 | 判别标签；必须使用表中固定值 |

### error

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "error" | 是 | 判别标签；必须使用表中固定值 |
| `stableCode` | [PluginApiErrorCode](./types.zh-CN.md#pluginapierrorcode) | 是 | 串口事件的稳定错误代码 |

## PluginSerialFlowControl

允许的字符串值：`none`, `software`, `hardware`.

## PluginSerialParity

允许的字符串值：`none`, `odd`, `even`.

## PluginSerialPortKind

允许的字符串值：`usb`, `pci`, `bluetooth`, `unknown`.

## PluginSerialPortMetadata

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `label` | String | 是 | 供用户识别的显示文字 |
| `kind` | [PluginSerialPortKind](./types.zh-CN.md#pluginserialportkind) | 是 | 判别标签；必须使用表中固定值 |
| `manufacturer` | Option&lt;String&gt; | 否 | 设备制造商显示信息，可缺失 |
| `product` | Option&lt;String&gt; | 否 | 设备产品显示信息，可缺失 |
| `usbVendorId` | Option&lt;u16&gt; | 否 | USB 厂商数值标识，可缺失 |
| `usbProductId` | Option&lt;u16&gt; | 否 | USB 产品数值标识，可缺失 |

## PluginSerialSendRequest

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

## PluginSerialSettings

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `baudRate` | u32 | 是 | 波特率，300–4000000 |
| `dataBits` | [PluginSerialDataBits](./types.zh-CN.md#pluginserialdatabits) | 是 | 数据位：five/six/seven/eight |
| `parity` | [PluginSerialParity](./types.zh-CN.md#pluginserialparity) | 是 | 校验：none/odd/even |
| `stopBits` | [PluginSerialStopBits](./types.zh-CN.md#pluginserialstopbits) | 是 | 停止位：one/two |
| `flowControl` | [PluginSerialFlowControl](./types.zh-CN.md#pluginserialflowcontrol) | 是 | 流控：none/software/hardware |

## PluginSerialStopBits

允许的字符串值：`one`, `two`.

## PluginSftpDirectoryEntry

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `entryHandle` | String | 是 | 目录列表返回的对象引用 |
| `name` | String | 是 | 当前对象的名称或标识 |
| `kind` | [PluginSftpEntryKind](./types.zh-CN.md#pluginsftpentrykind) | 是 | 判别标签；必须使用表中固定值 |
| `size` | Option&lt;u64&gt; | 否 | Core 观测的对象字节数，可缺失 |
| `modifiedAtUnixMs` | Option&lt;i64&gt; | 否 | Core 观测的修改时间，Unix 毫秒数 |
| `precondition` | [PluginSftpObjectPrecondition](./types.zh-CN.md#pluginsftpobjectprecondition) | 是 | 原样回传最近列表或读取提供的对象事实 |

## PluginSftpEntryKind

允许的字符串值：`file`, `directory`.

## PluginSftpEvent

### uploadStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadStarted" | 是 | 判别标签；必须使用表中固定值 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |
| `expectedBytes` | u64 | 是 | 上传开始时声明的总字节数 |

### uploadProgress

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadProgress" | 是 | 判别标签；必须使用表中固定值 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |
| `receivedBytes` | u64 | 是 | 当前已接受的上传字节数 |
| `expectedBytes` | u64 | 是 | 上传开始时声明的总字节数 |

### uploadCompleted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadCompleted" | 是 | 判别标签；必须使用表中固定值 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |
| `totalBytes` | u64 | 是 | 完整对象的字节数 |

### downloadProgress

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "downloadProgress" | 是 | 判别标签；必须使用表中固定值 |
| `entryHandle` | String | 是 | 目录列表返回的对象引用 |
| `nextOffset` | u64 | 是 | 下次读取应使用的字节偏移 |
| `totalBytes` | u64 | 是 | 完整对象的字节数 |
| `eof` | bool | 是 | 是否已到数据末尾 |

## PluginSftpObjectPrecondition

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | [PluginSftpEntryKind](./types.zh-CN.md#pluginsftpentrykind) | 是 | 判别标签；必须使用表中固定值 |
| `size` | Option&lt;u64&gt; | 否 | Core 观测的对象字节数，可缺失 |
| `modifiedAtUnixMs` | Option&lt;i64&gt; | 否 | Core 观测的修改时间，Unix 毫秒数 |

## PluginSftpOperation

### list

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "list" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | Option&lt;String&gt; | 否 | SFTP list 返回的目录引用 |
| `childEntryHandle` | Option&lt;String&gt; | 否 | 上一页列出的子目录条目句柄 |
| `cursor` | Option&lt;String&gt; | 否 | 原样回传上页 nextCursor；首请求省略 |
| `limit` | u16 | 是 | 本次最多返回的项目或事件数量 |

### read

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "read" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `entryHandle` | String | 是 | 目录列表返回的对象引用 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |
| `length` | u16 | 是 | 请求读取的字节数 |

### writeBinary

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "writeBinary" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `entryHandle` | String | 是 | 目录列表返回的对象引用 |
| `precondition` | [PluginSftpObjectPrecondition](./types.zh-CN.md#pluginsftpobjectprecondition) | 是 | 原样回传最近列表或读取提供的对象事实 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

### uploadStart

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadStart" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `name` | String | 是 | 当前对象的名称或标识 |
| `expectedBytes` | u64 | 是 | 上传开始时声明的总字节数 |

### uploadReplaceStart

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadReplaceStart" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `entryHandle` | String | 是 | 目录列表返回的对象引用 |
| `precondition` | [PluginSftpObjectPrecondition](./types.zh-CN.md#pluginsftpobjectprecondition) | 是 | 原样回传最近列表或读取提供的对象事实 |
| `expectedBytes` | u64 | 是 | 上传开始时声明的总字节数 |

### uploadChunk

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadChunk" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |

### uploadCommit

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadCommit" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |

### uploadAbort

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadAbort" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |

### createDirectory

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "createDirectory" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `name` | String | 是 | 当前对象的名称或标识 |

### createEmptyFile

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "createEmptyFile" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `name` | String | 是 | 当前对象的名称或标识 |

### renameNoReplace

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "renameNoReplace" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `sourceDirectoryHandle` | String | 是 | 源目录 list 结果中的目录句柄 |
| `sourceEntryHandle` | String | 是 | 源目录 list 结果中的条目句柄 |
| `sourcePrecondition` | [PluginSftpObjectPrecondition](./types.zh-CN.md#pluginsftpobjectprecondition) | 是 | 原样回传源条目的对象事实 |
| `targetDirectoryHandle` | String | 是 | 目标目录 list 返回的目录句柄 |
| `targetName` | String | 是 | 目标目录内的新条目名，不是路径 |

### remove

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "remove" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `entryHandle` | String | 是 | 目录列表返回的对象引用 |
| `precondition` | [PluginSftpObjectPrecondition](./types.zh-CN.md#pluginsftpobjectprecondition) | 是 | 原样回传最近列表或读取提供的对象事实 |

## PluginSftpResult

### opened

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "opened" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |

### page

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "page" | 是 | 判别标签；必须使用表中固定值 |
| `directoryHandle` | String | 是 | SFTP list 返回的目录引用 |
| `entries` | Vec&lt;[PluginSftpDirectoryEntry](./types.zh-CN.md#pluginsftpdirectoryentry)&gt; | 是 | 本页存储条目或目录条目 |
| `nextCursor` | Option&lt;String&gt; | 否 | 存在时用于下一页；省略表示已结束 |

### read

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "read" | 是 | 判别标签；必须使用表中固定值 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |
| `nextOffset` | u64 | 是 | 下次读取应使用的字节偏移 |
| `totalBytes` | u64 | 是 | 完整对象的字节数 |
| `eof` | bool | 是 | 是否已到数据末尾 |
| `precondition` | [PluginSftpObjectPrecondition](./types.zh-CN.md#pluginsftpobjectprecondition) | 是 | 原样回传最近列表或读取提供的对象事实 |

### written

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "written" | 是 | 判别标签；必须使用表中固定值 |
| `totalBytes` | u64 | 是 | 完整对象的字节数 |

### uploadStarted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadStarted" | 是 | 判别标签；必须使用表中固定值 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |
| `expectedBytes` | u64 | 是 | 上传开始时声明的总字节数 |

### uploadProgress

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadProgress" | 是 | 判别标签；必须使用表中固定值 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |
| `receivedBytes` | u64 | 是 | 当前已接受的上传字节数 |
| `expectedBytes` | u64 | 是 | 上传开始时声明的总字节数 |

### uploaded

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploaded" | 是 | 判别标签；必须使用表中固定值 |
| `uploadHandle` | String | 是 | uploadStart/uploadReplaceStart 返回的暂存上传引用 |
| `totalBytes` | u64 | 是 | 完整对象的字节数 |

### uploadAborted

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "uploadAborted" | 是 | 判别标签；必须使用表中固定值 |

### created

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "created" | 是 | 判别标签；必须使用表中固定值 |

### renamed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "renamed" | 是 | 判别标签；必须使用表中固定值 |

### removed

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "removed" | 是 | 判别标签；必须使用表中固定值 |

## PluginStorageBlobRead

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `revision` | u64 | 是 | 当前版本；后续条件写入回传此值 |
| `totalBytes` | u64 | 是 | 完整对象的字节数 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |
| `eof` | bool | 是 | 是否已到数据末尾 |

## PluginStorageEntry

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `valueBase64` | String | 是 | 非秘密存储值的标准 Base64 |
| `revision` | u64 | 是 | 当前版本；后续条件写入回传此值 |

## PluginStorageMutation

### kvSet

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvSet" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `valueBase64` | String | 是 | 非秘密存储值的标准 Base64 |
| `expectedRevision` | u64 | 是 | 回传最近读取的 revision；不要自行递增 |

### kvDelete

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvDelete" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `expectedRevision` | u64 | 是 | 回传最近读取的 revision；不要自行递增 |

## PluginStorageOperation

### kvGet

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvGet" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |

### kvSet

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvSet" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `valueBase64` | String | 是 | 非秘密存储值的标准 Base64 |
| `expectedRevision` | u64 | 是 | 回传最近读取的 revision；不要自行递增 |

### kvDelete

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvDelete" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `expectedRevision` | u64 | 是 | 回传最近读取的 revision；不要自行递增 |

### kvList

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvList" | 是 | 判别标签；必须使用表中固定值 |
| `prefix` | String | 是 | 当前命名空间内的键前缀过滤条件 |
| `cursor` | Option&lt;String&gt; | 否 | 原样回传上页 nextCursor；首请求省略 |
| `limit` | u16 | 是 | 本次最多返回的项目或事件数量 |

### blobRead

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "blobRead" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |
| `length` | u16 | 是 | 请求读取的字节数 |

### blobWrite

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "blobWrite" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `offset` | u64 | 是 | 从零开始的字节偏移 |
| `dataBase64` | String | 是 | 标准 Base64 编码的二进制字节 |
| `expectedRevision` | u64 | 是 | 回传最近读取的 revision；不要自行递增 |

### blobDelete

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "blobDelete" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `expectedRevision` | u64 | 是 | 回传最近读取的 revision；不要自行递增 |

### cacheGet

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "cacheGet" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |

### cacheSet

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "cacheSet" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `valueBase64` | String | 是 | 非秘密存储值的标准 Base64 |
| `ttlMs` | u64 | 是 | 缓存存活时间，毫秒 |

### cacheDelete

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "cacheDelete" | 是 | 判别标签；必须使用表中固定值 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |

### cacheClear

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "cacheClear" | 是 | 判别标签；必须使用表中固定值 |

### schemaGet

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "schemaGet" | 是 | 判别标签；必须使用表中固定值 |

### schemaCommit

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "schemaCommit" | 是 | 判别标签；必须使用表中固定值 |
| `expectedVersion` | u64 | 是 | schemaGet 返回的当前 schema 版本 |
| `newVersion` | u64 | 是 | 成功提交后采用的 schema 版本 |
| `mutations` | Vec&lt;[PluginStorageMutation](./types.zh-CN.md#pluginstoragemutation)&gt; | 是 | 同一 schema 事务中的 KV 修改声明 |

## PluginStorageResult

### kvValue

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvValue" | 是 | 判别标签；必须使用表中固定值 |
| `state` | [PluginStorageState](./types.zh-CN.md#pluginstoragestate) | 是 | 当前状态，按对应枚举处理 |
| `entry` | Option&lt;[PluginStorageEntry](./types.zh-CN.md#pluginstorageentry)&gt; | 否 | 存在时返回条目；缺失或过期时可省略 |

### kvPage

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "kvPage" | 是 | 判别标签；必须使用表中固定值 |
| `state` | [PluginStorageState](./types.zh-CN.md#pluginstoragestate) | 是 | 当前状态，按对应枚举处理 |
| `entries` | Vec&lt;[PluginStorageEntry](./types.zh-CN.md#pluginstorageentry)&gt; | 是 | 本页存储条目或目录条目 |
| `nextCursor` | Option&lt;String&gt; | 否 | 存在时用于下一页；省略表示已结束 |

### blobRead

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "blobRead" | 是 | 判别标签；必须使用表中固定值 |
| `state` | [PluginStorageState](./types.zh-CN.md#pluginstoragestate) | 是 | 当前状态，按对应枚举处理 |
| `blob` | [PluginStorageBlobRead](./types.zh-CN.md#pluginstorageblobread) | 是 | 当前 Blob 的分块读取结果 |

### blobWritten

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "blobWritten" | 是 | 判别标签；必须使用表中固定值 |
| `state` | [PluginStorageState](./types.zh-CN.md#pluginstoragestate) | 是 | 当前状态，按对应枚举处理 |
| `key` | String | 是 | 当前插件私有命名空间中的键 |
| `revision` | u64 | 是 | 当前版本；后续条件写入回传此值 |
| `totalBytes` | u64 | 是 | 完整对象的字节数 |

### cacheValue

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "cacheValue" | 是 | 判别标签；必须使用表中固定值 |
| `state` | [PluginStorageState](./types.zh-CN.md#pluginstoragestate) | 是 | 当前状态，按对应枚举处理 |
| `entry` | Option&lt;[PluginStorageEntry](./types.zh-CN.md#pluginstorageentry)&gt; | 否 | 存在时返回条目；缺失或过期时可省略 |

### state

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "state" | 是 | 判别标签；必须使用表中固定值 |
| `state` | [PluginStorageState](./types.zh-CN.md#pluginstoragestate) | 是 | 当前状态，按对应枚举处理 |

## PluginStorageState

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `storeRevision` | u64 | 是 | 整个私有存储的当前整数版本 |
| `schemaVersion` | u64 | 是 | 当前存储 schema 版本 |

## PluginSubscriptionEvent

### terminalContextChanged

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "terminalContextChanged" | 是 | 判别标签；必须使用表中固定值 |
| `contextHandle` | String | 是 | Core 下发的终端上下文引用 |
| `targetId` | [PluginExtensionTargetId](./types.zh-CN.md#pluginextensiontargetid) | 是 | 已声明的插件扩展目标标识 |
| `generation` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 当前实例或上下文代次的十进制字符串 |
| `state` | [PluginTerminalState](./types.zh-CN.md#pluginterminalstate) | 是 | 当前状态，按对应枚举处理 |

### terminalContextEnded

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "terminalContextEnded" | 是 | 判别标签；必须使用表中固定值 |
| `contextHandle` | String | 是 | Core 下发的终端上下文引用 |
| `generation` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 当前实例或上下文代次的十进制字符串 |

### hostScopeChanged

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "hostScopeChanged" | 是 | 判别标签；必须使用表中固定值 |
| `scopeStateVersion` | Option&lt;[WireSequence](./types.zh-CN.md#wiresequence)&gt; | 否 | 当前主机范围版本，可能为 null |

### pluginSettingsChanged

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "pluginSettingsChanged" | 是 | 判别标签；必须使用表中固定值 |
| `revision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 当前版本；后续条件写入回传此值 |

## PluginSubscriptionTopic

### terminalContext

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "terminalContext" | 是 | 判别标签；必须使用表中固定值 |
| `contextHandle` | String | 是 | Core 下发的终端上下文引用 |

### hostScope

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "hostScope" | 是 | 判别标签；必须使用表中固定值 |

### pluginSettings

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "pluginSettings" | 是 | 判别标签；必须使用表中固定值 |

## PluginTerminalState

允许的字符串值：`starting`, `running`, `awaitingUser`, `closing`, `closed`, `failed`.

## PluginUiActionId

不透明字符串标识；使用 Core 提供的原值。

## PluginWorkflowApiMethod

允许的字符串值：`serialDevices`, `serialOpen`, `serialSend`, `protocolOpen`, `describe`, `permissions`, `permissionRequest`, `permissionRevoke`, `permissionsForget`, `resourcesList`, `resourceClose`, `subscriptionStart`, `timerStart`, `resourceEvents`, `networkStart`, `networkSend`, `remoteExecStart`, `remoteExecSend`, `processStart`, `processSend`, `sftpOpen`, `sftp`, `filePick`, `file`, `credential`, `storage`, `terminalRequestInput`.

## PluginWorkflowStepState

允许的字符串值：`dispatching`, `succeeded`, `failed`, `needsUserAction`, `outcomeUnknown`, `interrupted`.

## PluginWorkflowTaskId

不透明字符串标识；使用 Core 提供的原值。

## PluginWorkflowTaskResult

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `taskId` | [PluginWorkflowTaskId](./types.zh-CN.md#pluginworkflowtaskid) | 是 | 任务快照提供的不透明任务标识 |
| `replies` | Vec&lt;[PluginApiReply](./types.zh-CN.md#pluginapireply)&gt; | 是 | 进程内保留的步骤 Broker 回复 |

## PluginWorkflowTaskSnapshot

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `task` | [PluginWorkflowTaskSummary](./types.zh-CN.md#pluginworkflowtasksummary) | 是 | 任务身份、状态、revision 和时间摘要 |
| `steps` | Vec&lt;[PluginWorkflowTaskStepSummary](./types.zh-CN.md#pluginworkflowtaskstepsummary)&gt; | 是 | 任务步骤状态摘要列表 |
| `result` | Option&lt;[PluginWorkflowTaskResult](./types.zh-CN.md#pluginworkflowtaskresult)&gt; | 否 | 嵌套结果联合；先检查 result.kind |

## PluginWorkflowTaskState

允许的字符串值：`running`, `dispatching`, `needsUserAction`, `cancelling`, `cancelled`, `completed`, `failed`, `interrupted`, `cleanupIncomplete`.

## PluginWorkflowTaskStepSummary

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `stepId` | String | 是 | 工作流 catalog 中的步骤 id |
| `method` | [PluginWorkflowApiMethod](./types.zh-CN.md#pluginworkflowapimethod) | 是 | 对应方法或 HTTP 动词的枚举值 |
| `state` | [PluginWorkflowStepState](./types.zh-CN.md#pluginworkflowstepstate) | 是 | 当前状态，按对应枚举处理 |
| `revision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 当前版本；后续条件写入回传此值 |
| `outcomeUnknown` | bool | 是 | 先前副作用结果未知，不能盲目重放 |
| `createdAtUnixMs` | i64 | 是 | 创建时间，Unix 毫秒数 |
| `updatedAtUnixMs` | i64 | 是 | 最近更新时间，Unix 毫秒数 |

## PluginWorkflowTaskSummary

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `taskId` | [PluginWorkflowTaskId](./types.zh-CN.md#pluginworkflowtaskid) | 是 | 任务快照提供的不透明任务标识 |
| `pluginId` | [PluginId](./types.zh-CN.md#pluginid) | 是 | Core 归属的插件标识，不可当作输入授权 |
| `workflowId` | String | 是 | 包内 workflow catalog 中的 id |
| `state` | [PluginWorkflowTaskState](./types.zh-CN.md#pluginworkflowtaskstate) | 是 | 当前状态，按对应枚举处理 |
| `revision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 当前版本；后续条件写入回传此值 |
| `currentStepId` | Option&lt;String&gt; | 否 | 当前执行步骤，存在时返回 |
| `outcomeUnknown` | bool | 是 | 先前副作用结果未知，不能盲目重放 |
| `cleanupIncomplete` | bool | 是 | 是否仍存在未完成的资源清理 |
| `createdAtUnixMs` | i64 | 是 | 创建时间，Unix 毫秒数 |
| `updatedAtUnixMs` | i64 | 是 | 最近更新时间，Unix 毫秒数 |

## WireSequence

十进制非负整数的 JSON 字符串，例如 `"1"`。
