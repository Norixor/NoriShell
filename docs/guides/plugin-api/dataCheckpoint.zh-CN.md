# dataCheckpoint

仅当 Core 验证权威远端收据及必要的本机应用收据后，推进本机同步基线。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataCheckpoint { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataCheckpoint"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataCheckpoint"`；request: {profileId, categories, sourceHandle, authoritativeReceiptHandle, baseReceiptHandle?, exportHandle?, applyReceiptHandle?, expectedLocalSnapshotHandle?, remoteInspectionHandle?}。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataCheckpoint"` | 是 | 精确 operation 判别标签 |
| `request` | `PluginDataCheckpointRequest` | 是 | 见上文字段约束与[DTO 类型](./types.zh-CN.md#分类数据-dtocore-api-188) |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataCheckpoint"` | 是 | 读取字段前先核对 |
| `syncedAtUnixMs` | typed value | 是 | Core 签发的事实；见 DTO 类型 |

上传路径还应提交基线 GET 收据与 `exportHandle`，并提供权威 PUT 收据。下载路径省略这两项，提供本机应用收据。若新读取的本机快照与已认证的 GET200 检查结果在选定范围内一致，`sourceHandle` 与 `expectedLocalSnapshotHandle` 均指向该快照，同时提交 `remoteInspectionHandle` 和 GET 收据，省略导出、基线和应用句柄。Core 再次检查本机状态、精确 GET 资源、密文摘要和内容一致性后建立基线。HTTP 结果不确定时不得推进基线。

## 授权与状态

要求 `sshSync` 和已验证的远端完成事实，而非仅发起请求；Core service 接线通过 Cargo 检查；原生验收尚未完成。句柄绑定当前插件、profile 和 generation，不能传给另一实例。尚未完成的操作须先确认 Core 运行接线与授权状态。

```json
{
  "callId": "example.dataCheckpoint",
  "operation": {
    "kind": "dataCheckpoint",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "sourceHandle": "00000000-0000-4000-8000-000000000001",
      "authoritativeReceiptHandle": "00000000-0000-4000-8000-000000000002",
      "baseReceiptHandle": "00000000-0000-4000-8000-000000000003",
      "exportHandle": "00000000-0000-4000-8000-000000000004",
      "applyReceiptHandle": null
    }
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
