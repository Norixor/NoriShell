# permissionRequest

由用户明确操作发起一次能力授权请求。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::PermissionRequest { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

**入口限制：** 本方法的成功路径使用宿主 declarative action。下面 JSON 展示 wire 结构；Wasm SDK 可以构造它，但当前 isolated 入口不会执行该受保护交互。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissionRequest" | 是 | 判别标签；必须使用表中固定值 |
| `capability` | [PluginCapability](./types.zh-CN.md#plugincapability) | 是 | PluginCapability 枚举值；不是已授予权限 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "permissionRequested" | 是 | 判别标签；必须使用表中固定值 |
| `approvalId` | String | 是 | 待处理授权请求标识，不表示已经批准 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

当前只支持宿主 declarative action 入口。Wasm isolated 的 api_request 返回 unsupported；即使源自 uiAction 也不例外。能力必须已在包中声明且属于 Core 的特殊权限集合；未声明返回 permissionDenied，不受支持的能力返回 unsupported。approvalId 表示请求已建立，不表示权限已经授予；后续重新读取 permissions。

## 调用示例

```json
{
  "callId": "example.permissionRequest",
  "operation": {
    "kind": "permissionRequest",
    "capability": "networkDomain"
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="permissionRequested"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[permissions](./permissions.zh-CN.md) · [permissionRevoke](./permissionRevoke.zh-CN.md) · [permissionsForget](./permissionsForget.zh-CN.md)
