# protocolOpen

请求宿主为包内协议 Provider 创建终端启动记录。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::ProtocolOpen { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "protocolOpen" | 是 | 判别标签；必须使用表中固定值 |
| `request` | [PluginProtocolOpen](./types.zh-CN.md#pluginprotocolopen) | 是 | 本方法的类型化请求对象 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "protocolLaunched" | 是 | 判别标签；必须使用表中固定值 |
| `launchId` | String | 是 | 宿主终端启动记录标识，不表示已连接 |

## 权限、时机与资源范围

能力提示：`terminalProvider`。声明能力后仍须有当前有效授权及适用的精确操作批准。

providerId 必须是包中声明的 Provider；configuration 符合其设置 schema，只传允许的 boolean/number/string。示例采用 Protocol Demo 的 framedTcp 与 endpoint 配置，仅在声明该 Provider 的包中有意义。launchId 仅表示启动记录已创建，不表示终端已连接。

## 调用示例

```json
{
  "callId": "example.protocolOpen",
  "operation": {
    "kind": "protocolOpen",
    "request": {
      "providerId": "framedTcp",
      "configuration": {
        "endpoint": "tcp://127.0.0.1:19071"
      },
      "label": "Framed TCP demo"
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="protocolLaunched"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[describe](./describe.zh-CN.md) · [resourcesList](./resourcesList.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
