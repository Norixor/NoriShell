# timerStart

创建一次性或周期定时器。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::TimerStart { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "timerStart" | 是 | 判别标签；必须使用表中固定值 |
| `delayMs` | u32 | 是 | 首次触发前延迟毫秒数 |
| `intervalMs` | Option&lt;u32&gt; | 否 | 周期毫秒数；省略时为一次性 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "timerStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

省略 intervalMs 为一次性定时器。timerFired 经 resourceEvents 读取；结束时 resourceClose。定时器回调属于后台上下文，不得借此弹出授权或输入终端。 delayMs 和提供时的 intervalMs 均为 100–86400000 毫秒。

## 调用示例

```json
{
  "callId": "example.timerStart",
  "operation": {
    "kind": "timerStart",
    "delayMs": 1000
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="timerStarted"`，读取上表字段。保存 handle，在需要时调用 resourceEvents 并最终 resourceClose；接收成功不等于远端操作完成。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourcesList](./resourcesList.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
