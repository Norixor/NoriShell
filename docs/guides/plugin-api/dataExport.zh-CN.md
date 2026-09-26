# dataExport

将所选本机或组合数据加密为 Core 内不透明 Blob；返回插件所选 HTTP 写入需要的修订与幂等键。仅导出不代表同步成功。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataExport { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataExport"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataExport"`；request: {profileId, categories, sourceHandle, baseReceiptHandle}。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataExport"` | 是 | 精确 operation 判别标签 |
| `request` | `PluginDataExportRequest` | 是 | 见上文字段约束与[DTO 类型](./types.zh-CN.md#分类数据-dtocore-api-188) |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataExport"` | 是 | 读取字段前先核对 |
| `exportHandle` | typed value | 是 | Core 导出状态，用于验证基线提交 |
| `blobHandle` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `revision` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `idempotencyKey` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `contentType` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `objects` | typed value | 是 | 非秘密对象描述 |

## 授权与状态

要求 `sshSync` 和匹配的已验证基线收据；Core service 接线通过 Cargo 检查；原生验收尚未完成。句柄绑定当前插件、profile 和 generation，不能传给另一实例。尚未完成的操作须先确认 Core 运行接线与授权状态。

```json
{
  "callId": "example.dataExport",
  "operation": {
    "kind": "dataExport",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "sourceHandle": "00000000-0000-4000-8000-000000000001",
      "baseReceiptHandle": "00000000-0000-4000-8000-000000000002"
    }
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
