# permissions

读取当前能力授权和可管理的记住操作授权摘要。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::Permissions { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissions" | 是 | 判别标签；必须使用表中固定值 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissions" | 是 | 判别标签；必须使用表中固定值 |
| `grants` | Vec&lt;[PluginCapabilityGrant](./types.zh-CN.md#plugincapabilitygrant)&gt; | 是 | 当前能力授权记录 |
| `policyRevision` | Option&lt;[WireSequence](./types.zh-CN.md#wiresequence)&gt; | 否 | 可空的策略版本十进制字符串 |
| `operationPermissions` | Vec&lt;[PluginOperationPermission](./types.zh-CN.md#pluginoperationpermission)&gt; | 是 | 自身记住操作授权的非秘密摘要 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

policyRevision 是可空的十进制字符串。撤销时使用本次返回值；operationPermissions 只有当前包、签名与有效授权绑定拥有的非秘密摘要。

## 调用示例

```json
{
  "callId": "example.permissions",
  "operation": {
    "kind": "permissions"
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="permissions"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[permissionRequest](./permissionRequest.zh-CN.md) · [permissionRevoke](./permissionRevoke.zh-CN.md) · [permissionsForget](./permissionsForget.zh-CN.md)
