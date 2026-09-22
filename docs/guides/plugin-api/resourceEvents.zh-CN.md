# resourceEvents

分批取出资源产生的有界事件。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::ResourceEvents { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "resourceEvents" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `limit` | u16 | 是 | 本次最多返回的项目或事件数量 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "resourceEvents" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |
| `events` | Vec&lt;[PluginApiResourceEvent](./types.zh-CN.md#pluginapiresourceevent)&gt; | 是 | 按 sequence 处理的事件列表 |
| `backpressured` | bool | 是 | 资源事件队列是否发生背压 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

handle 来自资源创建结果。按 sequence 顺序处理事件并保持轮询有界；backpressured=true 表示队列存在背压。processOutput 的数据字段是 data_base64，processExited 的可选退出码是 exit_code；其他嵌套事件遵循各自 DTO。 limit 必须为 1–describe.limits.maxPendingEvents。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.resourceEvents",
  "operation": {
    "kind": "resourceEvents",
    "handle": "019d0000-0000-7000-8000-000000000001",
    "limit": 32
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="resourceEvents"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourcesList](./resourcesList.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
