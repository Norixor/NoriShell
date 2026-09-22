# terminalRequestInput

请求向已有终端输入文字，写入前由 Core 复核焦点和授权。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::TerminalRequestInput { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

**入口限制：** 本方法的成功路径使用宿主 declarative action。下面 JSON 展示 wire 结构；Wasm SDK 可以构造它，但当前 isolated 入口不会执行该受保护交互。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "terminalRequestInput" | 是 | 判别标签；必须使用表中固定值 |
| `terminalHandle` | String | 是 | Core 当前终端上下文中的引用 |
| `payload` | String | 是 | 希望提交到终端的文本 |
| `appendEnter` | bool | 是 | 是否在文本之后追加回车 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "inputApprovalRequested" | 是 | 判别标签；必须使用表中固定值 |
| `approvalId` | String | 是 | 待处理授权请求标识，不表示已经批准 |

也可能返回 `{"kind":"inputSent"}`，无其他字段。

## 权限、时机与资源范围

能力提示：`terminalRequestInput`。声明能力后仍须有当前有效授权及适用的精确操作批准。

terminalHandle 来自 Core 当前终端上下文。只能响应显式用户动作；Core 在实际写入前复核终端 generation、焦点和输入所有权。inputApprovalRequested 需要等待授权结果；inputSent 表示输入写入，不表示 shell 命令完成。 当前 Wasm isolated 入口总是返回 interactionRequired；真正写入只能经宿主的 declarative action 路径。不能通过 api_request 伪造焦点或改变入口身份。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.terminalRequestInput",
  "operation": {
    "kind": "terminalRequestInput",
    "terminalHandle": "019d0000-0000-7000-8000-000000000001",
    "payload": "pwd",
    "appendEnter": false
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="inputApprovalRequested"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourcesList](./resourcesList.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
