# dataRelease

一次刷新或同步尝试结束后，释放 Core 暂存的数据交换状态、密文 Blob 和网络收据。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataRelease { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataRelease"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataRelease"`；request: `{profileId, stateHandles, blobHandles, receiptHandles}`。各列表只传本次操作收到的句柄；可为空。单次最多 24 个状态、12 个 Blob、24 个收据，同一列表内 UUID 不得重复。

```json
{
  "callId": "example.dataRelease",
  "operation": {
    "kind": "dataRelease",
    "request": {
      "profileId": "primary",
      "stateHandles": ["00000000-0000-4000-8000-000000000001"],
      "blobHandles": ["00000000-0000-4000-8000-000000000002"],
      "receiptHandles": ["00000000-0000-4000-8000-000000000003"]
    }
  }
}
```

## 成功返回

`{"kind":"dataRelease"}` 表示列出的临时资源已释放。已过期或已释放的句柄可重复释放；其他插件实例或 profile 的句柄会被拒绝。Core 核对当前插件所有者、generation 和权限栅栏，不会按 profile 隐式批量释放。成功、待审阅或失败后都应调用；异常中断仍由 10 分钟过期时间兜底。

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
