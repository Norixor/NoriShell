# dataRead

按独立授权读取一个本机类别。应用偏好返回 Core 持久化的分组快照及各组修订号；尚未完成首次迁移的组列在 `migrationRequired`，不能把它们当作空设置。终端历史受 Vault、采集设置和显式用户动作约束。自建同步插件不申请这两项读取权限。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataRead { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataRead"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataRead"`；request: {category, offset, limit}。appPreferences 要求 offset=0、limit=1；terminalHistory 每页 1–100 条。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataRead"` | 是 | 精确 operation 判别标签 |
| `request` | `PluginDataReadRequest` | 是 | 见上文字段约束与[DTO 类型](./types.zh-CN.md#分类数据-dtocore-api-188) |


## 成功返回

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | `"dataRead"` | 是 | 读取字段前先核对 |
| `result` | typed value | 是 | Core 签发的事实；见 DTO 类型 |

## 授权与状态

按所选类别要求 `appPreferencesRead` 或 `terminalHistoryRead`。偏好组包括 `application`、`appearance`、`interaction`、`highlights`、`shortcuts`、`files`、`desktop` 和 `commandNotifications`；未迁移组不出现在 `groups` 中。终端历史仅允许明确的用户动作读取，每页最多 100 条。

```json
{
  "callId": "example.dataRead",
  "operation": {
    "kind": "dataRead",
    "request": {
      "category": "appPreferences",
      "offset": 0,
      "limit": 1
    }
  }
}
```

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
