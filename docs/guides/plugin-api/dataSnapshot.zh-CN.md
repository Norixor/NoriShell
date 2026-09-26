# dataSnapshot

冻结授权的本机对象；返回不透明快照句柄与非秘密对象描述。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataSnapshot { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataSnapshot"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataSnapshot"`；request: {profileId, categories}。类别须排序、去重，并从 hosts、credentials、desktopProfiles 中选择。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataSnapshot"` | 是 | 精确 operation 判别标签 |
| `request` | `PluginDataSnapshotRequest` | 是 | 见上文字段约束与[DTO 类型](./types.zh-CN.md#分类数据-dtocore-api-188) |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataSnapshot"` | 是 | 读取字段前先核对 |
| `snapshotHandle` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `keyPending` | typed value | 是 | Core 签发的事实；见 DTO 类型 |
| `localCounts` | `PluginDataLocalCounts` | 是 | 本机主机、凭据、远程桌面与删除记录数量 |
| `objects` | typed value | 是 | Core 签发的事实；见 DTO 类型 |

## 授权与状态

要求 `sshSync`；Core service 接线通过 Cargo 检查；原生验收尚未完成。句柄绑定当前插件、profile 和 generation，不能传给另一实例。尚未完成的操作须先确认 Core 运行接线与授权状态。

```json
{
  "callId": "example.dataSnapshot",
  "operation": {
    "kind": "dataSnapshot",
    "request": {
      "profileId": "primary",
      "categories": [
        "hosts",
        "credentials",
        "desktopProfiles"
      ]
    }
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
