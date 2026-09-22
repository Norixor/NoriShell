# 插件 API 请求与结果类型

这些是 `norishell_plugin_sdk` 重新导出的 protocol 1.13 guest-safe DTO。Core 专有的 plan、scope、lease、approval decision、受保护输入、Vault 材料和 resource fence 被刻意排除。

## PluginApiCall

`PluginApiCall { callId, operation: PluginApiOperation }` 放在 SDK 的 `api.request` output 内发出。`callId` 只用于关联 reply，不是 authority。 `callId` 长度为 1–80 字节，只允许 ASCII 字母、数字、点、下划线和连字符，例如 `protocol-demo.open`；冒号不合法。它与 UI action ID 是不同字段，不应直接复用。

## PluginApiOperation

完整 method 清单见[Broker 方法](broker.zh-CN.md)。tagged operation enum 的嵌套 payload 包含 `PluginAppRegistration`、`PluginAppNotification`、`PluginAppNavigation`、`PluginWorkflowTaskId`、`PluginSerialSettings`、`PluginProtocolOpen`、`PluginNetworkEndpointRequest`、`PluginNetworkStartRequest`、`PluginRemoteExecStartRequest`、`PluginProcessSendRequest`、`PluginSftpOperation`、`PluginFileAccessRequest`、`PluginFileOperation`、`PluginCredentialOperation` 和 `PluginStorageOperation`。

## PluginApiReply、PluginApiOutcome 与 PluginApiValue

Core 返回 `PluginApiReply { callId, outcome }`。`PluginApiOutcome` 为 `completed { value: PluginApiValue }` 或 `failed { code: PluginApiErrorCode }`。completed reply 只表示 Core 已接受或返回类型化结果；除非相应 resource event 说明，不能证明远端 peer 已收到。

## Remembered-operation 摘要

`permissions` 只为当前已启用的插件 owner 返回 `operationPermissions: PluginOperationPermission[]`。摘要包含 `permissionId`、operation、非秘密 action/target 标签、`createdAtUnixMs` 和可空的 `expiresAtUnixMs`；`null` 表示无限期。有限期决定由 Core 在批准前计算并写入绝对 deadline，插件或 renderer 不能自行提交 timestamp，也不能靠重启续期。`permissionRevoke { permissionId, expectedPolicyRevision }` 删除一个当前摘要并返回下一 revision；`permissionsForget` 清空全部当前摘要。它们不会暴露精确 scope，也不会更改 capability grant。

## PluginApiResourceSummary 与 PluginApiResourceEvent

resource 使用不透明 handle。`PluginApiResourceSummary` 只暴露 state 和非秘密 metadata；`PluginApiResourceEvent` 传递有序、有界的 serial data、timer fired、file changed、network、process output/exit、remote-exec、SFTP 与 subscription 等事实。handle 在 close、revoke、disable、crash、package replacement 或 owner/generation 变化后失效。

## Workflow、app 与 provider 类型

`PluginWorkflowTaskSnapshot`、`PluginWorkflowTaskResult`、`WorkflowEvent`、`PluginWorkflowEvent` 与 `WorkflowResponse` 描述包声明的 workflow。`PluginWorkflowEvent { taskId, workflowId, event }` 由 Core 关联，只交给同一个进程内 instance。`WorkflowResponse { stepId?, call?, complete }` 可在 `complete: false` 且无 call、无 step 时等待 task 已拥有的 resource event；task 没有 resource 时 Core 会拒绝这种无效等待。workflow summary 与 step timestamp 是 JavaScript `number`。`PluginAppRegistration`、`PluginAppNotification` 和 `PluginAppNavigation` 是显式 broker action，不能作为 `Initialize` 副作用。`PluginProtocolCatalog`、`PluginProtocolProvider`、`PluginProtocolEvent` 与 `PluginProtocolResponse` 描述 protocol provider 边界，既不提供原始 socket，也不提供 Core session identity。

## Error 与 availability 类型

`PluginApiErrorCode` 提供无秘密的稳定错误，包括 `interactionRequired`、`vaultMissing`、`vaultLocked`、`vaultRequiresReload`、`revoked`、`outcomeUnknown` 和 `cleanupIncomplete`。`PluginApiDescription`、`PluginApiMethod`、`PluginApiAvailability` 与 `PluginApiLimits` 由 `describe` 返回；必须在运行时查询，不能把源码类型当作可用性声明。

精确序列化字段见生成的 Core 声明 [`core-api.ts`](../../../src/core-api/generated/core-api.ts) 和 SDK re-export 清单 [`crates/plugin-sdk/src/lib.rs`](../../../crates/plugin-sdk/src/lib.rs)。
