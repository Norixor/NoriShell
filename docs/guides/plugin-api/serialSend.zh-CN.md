# serialSend

向自身已打开的串口资源发送二进制数据。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::SerialSend { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serialSend" | 是 | 判别标签；必须使用表中固定值 |
| `request` | [PluginSerialSendRequest](./types.zh-CN.md#pluginserialsendrequest) | 是 | 本方法的类型化请求对象 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serialSent" | 是 | 判别标签；必须使用表中固定值 |
| `handle` | String | 是 | Core 返回的当前所有者资源或凭据引用 |

## 权限、时机与资源范围

能力提示：`deviceSerial`。声明能力后仍须有当前有效授权及适用的受保护操作批准。

handle 来自 serialOpen，解码后一次最多 8 KiB；必须继续读取 serial 事件确认设备状态。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.serialSend",
  "operation": {
    "kind": "serialSend",
    "request": {
      "handle": "019d0000-0000-7000-8000-000000000001",
      "dataBase64": "aGVsbG8="
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="serialSent"`，读取上表字段。保存 handle，在需要时调用 resourceEvents 并最终 resourceClose；接收成功不等于远端操作完成。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[serialDevices](./serialDevices.zh-CN.md) · [serialOpen](./serialOpen.zh-CN.md)
