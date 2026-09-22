# subscriptionStart

订阅允许范围内的元数据变更。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::SubscriptionStart { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "subscriptionStart" | 是 | 判别标签；必须使用表中固定值 |
| `topics` | Vec&lt;[PluginSubscriptionTopic](./types.zh-CN.md#pluginsubscriptiontopic)&gt; | 是 | 类型化元数据主题列表 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "subscriptionStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

topics 可为 pluginSettings、hostScope，或带 Core contextHandle 的 terminalContext。只返回元数据，不包含终端内容、凭据或任意主机数据。 topics 必须有 1–16 个不重复主题。terminalContext 需要 terminalMetadata，hostScope 需要 hostMetadataRead 的声明和有效授权；pluginSettings 不额外要求这两项能力。

## 调用示例

```json
{
  "callId": "example.subscriptionStart",
  "operation": {
    "kind": "subscriptionStart",
    "topics": [
      {
        "kind": "pluginSettings"
      }
    ]
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="subscriptionStarted"`，读取上表字段。保存 handle，在需要时调用 resourceEvents 并最终 resourceClose；接收成功不等于远端操作完成。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourcesList](./resourcesList.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
