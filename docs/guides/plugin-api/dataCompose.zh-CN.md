# dataCompose

以 Core 已验证对象组合候选。插件为同一范围生成完整候选；Core 检查依赖关系并拒绝无效组合。发生需要人工选择的冲突时，分别生成本机与云端候选，再交给 [dataReview](./dataReview.zh-CN.md) 的受保护窗口一次选择。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataCompose { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataCompose"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataCompose"`；request: {profileId, categories, localSnapshotHandle, remoteInspectionHandle, decisions[]}。每项决定按 Core 签发的 objectHandle 选择 local 或 remote。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataCompose"` | 是 | 精确 operation 判别标签 |
| `request` | `PluginDataComposeRequest` | 是 | 见上文字段约束与[DTO 类型](./types.zh-CN.md#分类数据-dtocore-api-189) |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataCompose"` | 是 | 读取字段前先核对 |
| `composedHandle` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `objects` | typed value | 是 | Core 签发的事实；见 DTO 类型 |

## 授权与状态

要求 `sshSync` 和匹配的有效句柄；Core service 接线通过 Cargo 检查；原生验收尚未完成。句柄绑定当前插件、profile 和 generation，不能传给另一实例。尚未完成的操作须先确认 Core 运行接线与授权状态。

```json
{
  "callId": "example.dataCompose",
  "operation": {
    "kind": "dataCompose",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ],
      "localSnapshotHandle": "00000000-0000-4000-8000-000000000001",
      "remoteInspectionHandle": "00000000-0000-4000-8000-000000000002",
      "decisions": []
    }
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
