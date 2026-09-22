# permissionRevoke

撤销当前插件拥有的一项记住操作授权。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::PermissionRevoke { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissionRevoke" | 是 | 判别标签；必须使用表中固定值 |
| `permissionId` | String | 是 | permissions 返回的记住操作授权标识 |
| `expectedPolicyRevision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 回传 permissions.policyRevision |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissionRevoked" | 是 | 判别标签；必须使用表中固定值 |
| `policyRevision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 可空的策略版本十进制字符串 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

permissionId 与 expectedPolicyRevision 均取自 permissions。冲突时重新读取再让用户决定；本操作不撤销 capability grant。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.permissionRevoke",
  "operation": {
    "kind": "permissionRevoke",
    "permissionId": "019d0000-0000-7000-8000-000000000001",
    "expectedPolicyRevision": "1"
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="permissionRevoked"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[permissions](./permissions.zh-CN.md) · [permissionRequest](./permissionRequest.zh-CN.md) · [permissionsForget](./permissionsForget.zh-CN.md)
