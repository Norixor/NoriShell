# serialDevices

列出可供用户选择的串口候选。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::SerialDevices { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serialDevices" | 是 | 判别标签；必须使用表中固定值 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "serialDevices" | 是 | 判别标签；必须使用表中固定值 |
| `devices` | Vec&lt;[PluginSerialDeviceCandidate](./types.zh-CN.md#pluginserialdevicecandidate)&gt; | 是 | 不含原生设备路径的候选元数据 |

## 权限、时机与资源范围

能力提示：`deviceSerial`。声明能力后仍须有当前有效授权及适用的精确操作批准。

candidateId 是短期查找标识，不是设备路径或打开许可。设备可能拔出或失效；再次选择时刷新列表。原生与硬件边界见索引。 候选有效期为两分钟，重复列举会替换当前消费者的旧候选。

## 调用示例

```json
{
  "callId": "example.serialDevices",
  "operation": {
    "kind": "serialDevices"
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="serialDevices"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[serialOpen](./serialOpen.zh-CN.md) · [serialSend](./serialSend.zh-CN.md)
