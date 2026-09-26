# dataApply

经 Core 本机状态核验和受保护提交应用组合候选，返回应用收据。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataApply { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataApply"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataApply"`；request: {profileId, categories, expectedLocalSnapshotHandle, composedHandle, exportHandle?, authoritativeReceiptHandle}。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataApply"` | 是 | 精确 operation 判别标签 |
| `request` | `PluginDataApplyRequest` | 是 | 见上文字段约束与[DTO 类型](./types.zh-CN.md#分类数据-dtocore-api-189) |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataApply"` | 是 | 读取字段前先核对 |
| `applyReceiptHandle` | typed value | 是 | Core 签发的事实；见 DTO 类型 |

上传路径应传入 `dataExport` 的 `exportHandle` 与已完成 PUT 的 `authoritativeReceiptHandle`；Core 在应用本机改动前核对精确密文、修订、ETag/CAS 和幂等键。下载路径可省略 `exportHandle`，但仍需经过验证的权威远端收据。

## 授权与状态

要求 `sshSync`、显式动作和当前本机快照；Core service 接线通过 Cargo 检查；原生验收尚未完成。句柄绑定当前插件、profile 和 generation，不能传给另一实例。尚未完成的操作须先确认 Core 运行接线与授权状态。

```json
{
  "callId": "example.dataApply",
  "operation": {
    "kind": "dataApply",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "expectedLocalSnapshotHandle": "00000000-0000-4000-8000-000000000001",
      "composedHandle": "00000000-0000-4000-8000-000000000002",
      "exportHandle": "00000000-0000-4000-8000-000000000003",
      "authoritativeReceiptHandle": "00000000-0000-4000-8000-000000000004"
    }
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
