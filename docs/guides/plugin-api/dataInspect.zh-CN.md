# dataInspect

认证并检查远端密文，不向 Wasm 暴露密文；返回修订、ETag、迁移状态与对象描述。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataInspect { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataInspect"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataInspect"`；request: {profileId, categories, receiptHandle, bodyBlobHandle}。两个句柄来自 Core 完成的网络交换。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataInspect"` | 是 | 精确 operation 判别标签 |
| `request` | `PluginDataInspectRequest` | 是 | 见上文字段约束与[DTO 类型](./types.zh-CN.md#分类数据-dtocore-api-189) |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataInspect"` | 是 | 读取字段前先核对 |
| `inspectionHandle` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `objects` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `remoteRevision` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `etag` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `migrationRequired` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `excludedCategories` | typed value | 是 | Core 签发的事实；见 DTO 类型 |

## 授权与状态

要求 `sshSync`、已认证交换与当前 owner/profile 栅栏；Core service 接线通过 Cargo 检查；原生验收尚未完成。句柄绑定当前插件、profile 和 generation，不能传给另一实例。尚未完成的操作须先确认 Core 运行接线与授权状态。

```json
{
  "callId": "example.dataInspect",
  "operation": {
    "kind": "dataInspect",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "receiptHandle": "00000000-0000-4000-8000-000000000001",
      "bodyBlobHandle": "00000000-0000-4000-8000-000000000002"
    }
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
