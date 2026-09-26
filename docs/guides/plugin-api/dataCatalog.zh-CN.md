# dataCatalog

读取类别目录与 Core 当前可用性；目录本身不授予数据访问权限。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataCatalog { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataCatalog"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataCatalog"`；除 kind 外无字段。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataCatalog"` | 是 | 精确 operation 判别标签 |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataCatalog"` | 是 | 读取字段前先核对 |
| `catalog` | typed value | 是 | Core 签发的事实；见 DTO 类型 |

## 授权与状态

发现操作不要求数据权限。句柄绑定当前插件、profile 和 generation，不能传给另一实例。尚未完成的操作须先确认 Core 运行接线与授权状态。

```json
{
  "callId": "example.dataCatalog",
  "operation": {
    "kind": "dataCatalog"
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
