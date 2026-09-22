# resourcesList

枚举当前调用所有者范围内的资源。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::ResourcesList { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "resourcesList" | 是 | 判别标签；必须使用表中固定值 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "resources" | 是 | 判别标签；必须使用表中固定值 |
| `resources` | Vec&lt;[PluginApiResourceSummary](./types.zh-CN.md#pluginapiresourcesummary)&gt; | 是 | 当前所有者的资源摘要 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

仅返回当前所有者可见的摘要，不包含秘密。state 区分 opening/open/closing/cleanupIncomplete。

## 调用示例

```json
{
  "callId": "example.resourcesList",
  "operation": {
    "kind": "resourcesList"
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="resources"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
