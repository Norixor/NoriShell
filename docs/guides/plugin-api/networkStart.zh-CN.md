# networkStart

为确定的端点创建 HTTP、WebSocket、TCP、UDP 或 TLS 资源。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::NetworkStart { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "networkStart" | 是 | 判别标签；必须使用表中固定值 |
| `endpoint` | [PluginNetworkEndpointRequest](./types.zh-CN.md#pluginnetworkendpointrequest) | 是 | 待 Core 解析与批准的完整端点 URL |
| `request` | [PluginNetworkStartRequest](./types.zh-CN.md#pluginnetworkstartrequest) | 是 | 本方法的类型化请求对象 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "networkStarted" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

## 权限、时机与资源范围

能力提示：`networkDomain`。声明能力后仍须有当前有效授权及适用的精确操作批准。

Core 固定解析后的目标并执行受保护授权；请求不能带解析 IP、代理或授权标记。credential 是可选的凭据 handle/revision 引用。返回 handle 后读取 network 事件中的 opened、httpResponse、data、closed 或 error。 endpoint 最多 2048 字节，timeoutMs 为 100–120000。

## 调用示例

```json
{
  "callId": "example.networkStart",
  "operation": {
    "kind": "networkStart",
    "endpoint": {
      "endpoint": "https://example.test/status"
    },
    "request": {
      "timeoutMs": 10000,
      "operation": {
        "kind": "http",
        "method": "get",
        "headers": [],
        "bodyBase64": ""
      }
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="networkStarted"`，读取上表字段。保存 handle，在需要时调用 resourceEvents 并最终 resourceClose；接收成功不等于远端操作完成。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourcesList](./resourcesList.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
