# Broker 方法、资源、请求与结果

每行都是 [`PluginApiCall`](types.zh-CN.md#pluginapicall) 内的一个 `PluginApiOperation` variant。成功结果为 `PluginApiValue`，失败为 [`PluginApiErrorCode`](types.zh-CN.md#pluginapierrorcode)。下表是 protocol 1.13 的完整 operation 清单；当前 runtime 实际开放的子集必须由 [`describe`](types.zh-CN.md#pluginapidescription) 查询。

| 方法 | 请求 payload | 成功结果 |
| --- | --- | --- |
| [`dataCatalog`](./dataCatalog.zh-CN.md) | 按类别划分的请求 | `dataCatalog` |
| [`dataRead`](./dataRead.zh-CN.md) | 按类别划分的请求 | `dataRead` |
| [`dataSnapshot`](./dataSnapshot.zh-CN.md) | 按类别划分的请求 | `dataSnapshot` |
| [`dataInspect`](./dataInspect.zh-CN.md) | 按类别划分的请求 | `dataInspect` |
| [`dataCompose`](./dataCompose.zh-CN.md) | 按类别划分的请求 | `dataCompose` |
| [`dataApply`](./dataApply.zh-CN.md) | 按类别划分的请求 | `dataApply` |
| [`dataExport`](./dataExport.zh-CN.md) | 按类别划分的请求 | `dataExport` |
| [`dataCheckpoint`](./dataCheckpoint.zh-CN.md) | 按类别划分的请求 | `dataCheckpoint` |
| [`dataRelease`](./dataRelease.zh-CN.md) | 精确临时句柄 | `dataRelease` |
| `appRegister` | [`PluginAppRegistration`](types.zh-CN.md#pluginappregistration) | `appAccepted` |
| `appNotify` | [`PluginAppNotification`](types.zh-CN.md#pluginappnotification) | `appAccepted` |
| `appNavigate` | [`PluginAppNavigation`](types.zh-CN.md#pluginappnavigation) | `appAccepted` |
| `taskStart` | workflow id、进程内 input JSON、不透明的预选 file scope | `task { PluginWorkflowTaskSnapshot }` |
| `taskGet` | `PluginWorkflowTaskId` | `task { PluginWorkflowTaskSnapshot }` |
| `taskList` | 无 | `tasks { PluginWorkflowTaskSnapshot[] }` |
| `taskCancel` | task id 和 revision | `task { PluginWorkflowTaskSnapshot }` |
| `taskResume` | task id 和 revision | `task { PluginWorkflowTaskSnapshot }` |
| `serialDevices` | 无 | `serialDevices { PluginSerialDeviceCandidate[] }` |
| `serialOpen` | `PluginSerialSettings`、选定 candidate id | `serialStarted { handle }` |
| `serialSend` | `PluginSerialSendRequest` | `serialSent { handle }` |
| `protocolOpen` | `PluginProtocolOpen` | `protocolLaunched { launchId }` |
| `describe` | 无 | `description { PluginApiDescription }` |
| `permissions` | 无 | `permissions { PluginCapabilityGrant[], policyRevision, operationPermissions }` |
| `permissionRequest` | `PluginCapability` | `permissionRequested { approvalId }` |
| `permissionRevoke` | permission id 与 expected policy revision | `permissionRevoked { policyRevision }` |
| `permissionsForget` | 无 | `permissionsForgotten` |
| `resourcesList` | 无 | `resources { PluginApiResourceSummary[] }` |
| `resourceClose` | 不透明 resource handle | `closed { handle }` |
| `subscriptionStart` | `PluginSubscriptionTopic[]` | `subscriptionStarted { handle }` |
| `timerStart` | delay 与可选 interval | `timerStarted { handle }` |
| `resourceEvents` | handle 与有界 limit | `resourceEvents { handle, events, backpressured }` |
| `networkStart` | `PluginNetworkEndpointRequest`、`PluginNetworkStartRequest` | `networkStarted { handle }` |
| `networkSend` | `PluginNetworkSendRequest` | `networkSent { handle }` |
| `remoteExecStart` | `PluginRemoteExecStartRequest` | `remoteExecStarted { handle }` |
| `remoteExecSend` | `PluginRemoteExecSendRequest` | `remoteExecSent { handle }` |
| `processStart` | executable、arguments、timeout | `processStarted { handle }` |
| `processSend` | `PluginProcessSendRequest` | `processSent { handle }` |
| `sftpOpen` | 已批准 host handle、root path、write flag | `sftp { PluginSftpResult }` |
| `sftp` | `PluginSftpOperation` | `sftp { PluginSftpResult }` |
| `filePick` | `PluginFilePickerKind`、`PluginFileAccessRequest` | `filePicked { rootHandle, label }` |
| `file` | `PluginFileOperation` | `file { PluginFileResult }` |
| `credential` | `PluginCredentialOperation` | `credential { PluginCredentialResult }` |
| `storage` | `PluginStorageOperation` | `storage { PluginStorageResult }` |
| `terminalRequestInput` | terminal handle、payload、append-enter flag | `inputApprovalRequested` 或 `inputSent` |

`appRegister`、`appNotify` 与 `appNavigate` 是显式记录的 broker operation。`Initialize` 只建立 host request 生命周期，不能注册 command、导航或取得 app action。Core `PluginWorkflowEvent { taskId, workflowId, event }` 只到达匹配的常驻 instance。`WorkflowResponse { stepId?, call?, complete }` 只有在 task 已拥有 resource event 时可用 `complete: false` 且无 call 等待；task 没有该 resource 时会被拒绝。task timestamp 是 JavaScript `number`。`taskStart` input、file-handle mapping 和 runtime reply 只在进程内保存；重启时 Core 将未完成 task 标为 `interrupted`，不会持久化 payload、重放 dispatch 或自动恢复 workflow。`taskResume` 是对已存 task state 的新显式动作，绝不会重放旧 payload。

`permissions` 只返回当前已启用 package、signer 和仍有效 capability/security binding 共同拥有的、非秘密 remembered-operation 摘要。每项仅含不透明 `permissionId`、operation、action 与管理标签、创建时间及可选的绝对 `expiresAtUnixMs`；绝不包含原始选择路径、serial identity、精确 scope、command 或 payload。`permissionRevoke` 用当前 `policyRevision` 作为乐观并发栅栏，撤销其中一项当前摘要。`permissionsForget` 清除当前 guest 仍拥有的全部 remembered operation。两者都不改变 capability grant，也不能删除旧 signer/package 的历史记录；这些历史仍由宿主管理界面保留。

resource handle 是绑定 owner、package hash、grant state、consumer 和 runtime generation 的不透明引用。必须显式 close；取消、revoke、disable、replacement、crash 或 stale fence 也可将其关闭。拿到 handle 或 `*Sent` 只表示本地 Core 已接受，不证明远端 I/O；resource event 才传递有界的实际 open/data/exit/error/close 事实。

serial 和 protocol-provider API 已纳入当前 ABI 与 broker 实现，但不能据此声称已完成 native、hardware、Windows 或端到端验收。请查看[实施状态](../../../README.zh-CN.md#安装与快速开始)和[开发 Broker 指南](../developers/broker.zh-CN.md)。

插件快捷键由主应用生命周期管理，用户显式启用后可跨页面使用；同一组合键存在多个匹配时不执行。终端、可编辑控件、输入法组合输入和阻断式对话框中的按键仍被保护，应用销毁时移除监听。文件命令调用注册的文档动作；该动作仍须通过 FilePick 原生选择及范围审批取得新的文件句柄，扩展名声明本身不授予文件访问权。
