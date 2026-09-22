# processStart

请求执行一个绝对路径程序和确定的参数列表。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::ProcessStart { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "processStart" | 是 | 判别标签；必须使用表中固定值 |
| `program` | String | 是 | 当前平台上的绝对可执行路径 |
| `arguments` | Vec&lt;String&gt; | 是 | 独立参数数组，不是 shell 拼接文本 |
| `timeoutMs` | u32 | 是 | 毫秒超时，仍受 Core 上限约束 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "processStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

## 权限、时机与资源范围

能力提示：`localProcess`。声明能力后仍须有当前有效授权及适用的精确操作批准。

program 必须是当前平台可用的绝对可执行路径；示例 /bin/cat 适用于具备该程序的平台。timeoutMs 为 1–120000，参数最多 128 项，每项最多 4096 字节且不能含 NUL。这是受批准的本地进程能力，工作目录和环境由 Core 拥有。

## 调用示例

```json
{
  "callId": "example.processStart",
  "operation": {
    "kind": "processStart",
    "program": "/bin/cat",
    "arguments": [],
    "timeoutMs": 10000
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="processStarted"`，读取上表字段。保存 handle，在需要时调用 resourceEvents 并最终 resourceClose；接收成功不等于远端操作完成。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourcesList](./resourcesList.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
