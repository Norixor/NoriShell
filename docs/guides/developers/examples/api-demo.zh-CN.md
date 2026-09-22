# API Demo：一次可见的请求与结果

API Demo 是源码示例中最小的完整可见插件。它只申请 `uiPanel` 与 `clipboardWrite`，不会读取 Host 或终端、发起网络请求、打开文件或访问 Vault。

## 用户流程

本地导入、审批并启用后，打开 **API demo**。`Initialize` 会在 `app.header.actions` 贡献一个 dialog；此时不会调用 API。点击 **Query API** 会发送类型化 `describe` 操作。Core 返回 `BrokerResult` 后，guest 会在同一 dialog 中渲染 Core 报告的方法与限制。**Copy API description** 使用当前展示文本发起宿主托管的剪贴板写入。

```text
Initialize -> ui.document("API demo")
点击 Query API -> UiAction -> api.request(describe)
Core 回复 -> BrokerResult -> ui.document（方法与限制）
点击 Copy -> clipboard.write + ui.document
```

## guest 代码

下面动作分支是对真实源码的解释性节选，**不是**完整的 `handle` 实现；改造时必须保留已有 import、payload 解析、状态和 document helper。call id 只用于关联，不是权限 token。

```rust
PluginHostMessageKind::UiAction => match body["actionId"].as_str() {
    Some("api-demo:describe") => Ok(vec![api_request(
        &request_id,
        "api-demo.describe",
        PluginApiOperation::Describe {},
    )?]),
    _ => Err(PluginError::InvalidRequest),
}
```

对应 runtime output 的 JSON payload 形状如下：

```json
{
  "callId": "api-demo.describe",
  "operation": { "kind": "describe" }
}
```

Core 把匹配回复放入 `BrokerResult` payload。下面同样是解释性节选，不能直接粘成完整 handler。Demo 解析 `PluginApiReply`，接收完成的 `description` 值或稳定失败码，只把渲染后的非秘密文本留在实例状态中，然后输出替换 document：

```rust
PluginHostMessageKind::BrokerResult => {
    let reply: PluginApiReply = serde_json::from_value(body["result"]["reply"].clone())
        .map_err(|_| PluginError::InvalidRequest)?;
    let text = render_description(&reply)?;
    self.latest_description = Some(text.clone());
    Ok(vec![document_output(&request_id, &text)?])
}
```

成功回复带有相同关联 id 和类型化 outcome。下面 JSON 是当前 DTO 的**完整合法示意值**，但方法列表、平台和限制都只是示例；请以运行中宿主的 `describe` 结果为准：

```json
{
  "callId": "api-demo.describe",
  "outcome": {
    "kind": "completed",
    "value": {
      "kind": "description",
      "api": {
        "protocolMajor": 1,
        "protocolMinor": 13,
        "platform": "fixture",
        "methods": [{ "name": "describe", "capability": null, "availability": "available" }],
        "limits": {
          "maxCallBytes": 65536,
          "maxChunkBytes": 16384,
          "maxResources": 32,
          "maxPendingEvents": 32
        }
      }
    }
  }
}
```

预期桌面结果：点击后，对话框会从“Select Query API”提示变为 Core API 版本、平台、方法和限制。若 Core 返回失败，对话框会展示失败，而不是假装请求成功。

| 看到的现象 | 先检查什么 |
| --- | --- |
| 启用后没有 **API demo** 入口 | 确认包已启用，并确认 `Initialize` 为 `app.header.actions` 输出了一份有效 `ui.document`。 |
| 对话框打开但 **Query API** 没有结果 | 确认 action id 是 `api-demo:describe`、call id 合法，下一条消息是匹配的 `BrokerResult`。 |
| Copy 没有更新剪贴板 | 从渲染出的 Copy 控件显式点击，并保留 `clipboardWrite` capability 审批。 |

## 构建并检查本地候选包

在 NoriShell 源码检出根目录运行：

```sh
python3 examples/plugins/api-demo/build.py --output /tmp/NoriShell-API-Demo-1.0.1.zip
CARGO_INCREMENTAL=0 cargo run --locked -p norishell-plugin-sdk --bin norishell-plugin-dev -- \
  check /tmp/NoriShell-API-Demo-1.0.1.zip
```

脚本会构建 Wasm、运行持久 runtime ABI verifier、创建 ZIP，并写入 `/tmp/NoriShell-API-Demo-1.0.1.zip.sha256`。独立的 `check` 命令校验包结构和 Wasm ABI。这些都是本地验证；真实导入和点击路径请按 [快速入门](../start/quickstart.zh-CN.md) 完成。代码片段背后的契约请继续看 [调用 Core](../development/calling-api.zh-CN.md) 与 [UI document](../development/ui.zh-CN.md)。
