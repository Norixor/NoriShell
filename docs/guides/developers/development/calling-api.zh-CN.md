# 调用 API 与处理回调

NoriShell 插件通过 protocol 1.13 Wasm ABI 向宿主发送类型化请求。当前 Rust SDK 为该 ABI 提供类型化封装；其他 ABI 合规的 guest 也可自行序列化同一 envelope。你不需要 HTTP 地址或 API Key，也不应从插件调用 Tauri IPC。先用 `describe` 查询当前运行环境，再选择对应的方法。

## 一次调用怎样完成

| 步骤 | 插件接收到什么 | 你需要做什么 |
| --- | --- | --- |
| 1. 建立界面 | `initialize` | 返回初始 `ui.document`，展示可点击的操作 |
| 2. 用户点击 | `uiAction` | 检查 `actionId`，用 `api_request` 发出一次明确调用 |
| 3. 宿主处理 | 插件暂时没有成功结果 | 等待权限决定或实际操作结果，不提前显示成功 |
| 4. 接收结果 | `brokerResult` | 读取 `result.kind = "api"` 下的 `reply`，检查 `callId` 与 `outcome` |
| 5. 更新界面 | 当前回调的 `requestId` | 返回新的完整 `ui.document`，显示结果或下一步操作 |

网络、进程和串口等长连接方法还会返回资源句柄。**取得句柄只是开始**，之后需要读取资源事件，并在结束时关闭资源。详见[资源与权限](./resources.zh-CN.md)。

## 请求参数与 SDK 辅助函数

```rust
use norishell_plugin_sdk::{
    api_request, PluginApiOperation, PluginError,
    PluginHostRequest, PluginRuntimeOutput,
};

fn request_methods(request: &PluginHostRequest)
    -> Result<PluginRuntimeOutput, PluginError>
{
    api_request(
        &request.request_id,
        "describe-1",
        PluginApiOperation::Describe {},
    )
}
```

这是可放入插件项目的辅助函数。在对应的 `UiAction` 分支中返回 `Ok(vec![request_methods(&request)?])`；完整的界面与事件循环见 [API Demo](../examples/api-demo.zh-CN.md)。

| 参数 | 类型 | 取值方式 |
| --- | --- | --- |
| `request_id` | `&str` | 当前 `PluginHostRequest.request_id`，不要自行生成或沿用旧回调的值 |
| `call_id` | 字符串 | 由插件分配，1–80 字节；只使用 ASCII 字母、数字、`.`、`_`、`-`，不用冒号 |
| `operation` | `PluginApiOperation` | 选择类型化 enum variant；在示例中是 `Describe {}` |
| 返回值 | `Result<PluginRuntimeOutput, PluginError>` | 放进当前 `handle` 的输出数组，SDK 负责序列化与协议封装 |

多个未结束调用应有不同的 `callId`，在实例内记录它们对应的动作。`callId` 用来匹配结果，不是资源句柄，也不授予权限。

## API 参考中的 JSON 如何使用

[方法参考](../../plugin-api/README.zh-CN.md)的请求示例展示的是 `PluginApiCall` 内容：

```json
{
  "callId": "describe-1",
  "operation": { "kind": "describe" }
}
```

这不是 HTTP 请求体。SDK 会将它序列化为 `PluginRuntimeOutput.payloadJson`，并设置 `kind: "api.request"` 和匹配当前宿主请求的 `requestId`。使用 `api_request` 时，不必手动拼接带转义的嵌套 JSON。

## 解析成功与失败

下面的辅助函数只解析这个 `describe-1` 调用的回复，返回可显示的文本。调用它以后，仍需像 API Demo 一样生成完整的替换 document。

```rust
use norishell_plugin_sdk::{
    payload, PluginApiOutcome, PluginApiReply, PluginApiValue,
    PluginError, PluginHostMessageKind, PluginHostRequest,
};
use serde_json::Value;

fn describe_text(request: &PluginHostRequest) -> Result<String, PluginError> {
    if request.kind != PluginHostMessageKind::BrokerResult {
        return Err(PluginError::InvalidRequest);
    }
    let body: Value = payload(request)?;
    let result = &body["result"];
    if result["kind"].as_str() != Some("api") {
        return Err(PluginError::InvalidRequest);
    }
    let reply: PluginApiReply = serde_json::from_value(result["reply"].clone())
        .map_err(|_| PluginError::InvalidRequest)?;
    if reply.call_id != "describe-1" {
        return Err(PluginError::InvalidRequest);
    }
    match reply.outcome {
        PluginApiOutcome::Completed {
            value: PluginApiValue::Description { api },
        } => serde_json::to_string_pretty(&api)
            .map_err(|_| PluginError::HandlerFailed),
        PluginApiOutcome::Failed { code } => Ok(format!("Request failed: {code:?}")),
        _ => Err(PluginError::InvalidRequest),
    }
}
```

| 回复层级 | 内容 | 处理要求 |
| --- | --- | --- |
| `PluginHostRequest.kind` | `brokerResult` | 在正确的消息分支中解析 |
| `payloadJson` 中的 `result.kind` | `api` | 不把其他 broker 结果当成 API 回复 |
| `result.reply.callId` | 对应请求的 ID | 找到相应的等待状态和目标界面 |
| `outcome.kind = completed` | `value` 是带 `kind` 的结果 union | 匹配该方法声明的返回种类 |
| `outcome.kind = failed` | `code` 是稳定错误码 | 给出用户能执行的下一步，不吞掉错误 |

## 先查询可用性

`describe` 返回协议版本、平台、方法和限制。遍历 `methods` 时读取 `name`、`capability` 和 `availability`；只有 `available` 的方法才进入调用流程，仍要通过对应权限检查。`notImplemented` 与 `unsupportedPlatform` 应在插件中显示为不可用。

`limits` 提供当前的请求字节、数据块、资源和待处理事件上限。按运行时返回值控制请求规模，不把文档中的示例数字写死为跨平台承诺。

## 宿主事件速查

| `kind` | 适用场景 | 下一步 |
| --- | --- | --- |
| `initialize` | Wasm 实例初始化 | 初始化有界内存状态，发布初始界面；不要借初始化注册受保护的 app action |
| `uiAction` | 用户操作声明式界面 | 处理动作与字段，返回 document 或明确的异步 broker 请求 |
| `brokerResult` | Core broker 回调 | 匹配调用并更新状态/界面 |
| `workflowEvent` | 任务步骤与资源事件 | 用 SDK `workflow_event` / `workflow_response` 处理匹配任务 |
| `protocolEvent` | protocol provider 生命周期 | 用 SDK `protocol_event` / `protocol_response` 处理字节与状态 |
| `terminalObservation` | 已获准的终端观察 | 只处理授权范围内的观察结果 |
| `invoke` | 宿主分派的插件调用 | 按该功能的 payload 契约处理，不等同于任意用户授权 |
| `sshSyncResult` | SSH 同步专用结果 | 仅实现相应集成的插件需要处理 |

同一 Wasm 实例的 `handle` 调用由宿主串行执行。插件内不需要自行创建事件循环或网络线程；把需要跨消息的状态保存在插件结构体中，并处理禁用、重启、资源关闭等生命周期变化。

接下来阅读[声明式界面](./ui.zh-CN.md)、[资源与权限](./resources.zh-CN.md)或按方法检索 [API 参考](../../plugin-api/README.zh-CN.md)。

方法可用不表示每一种调用入口都能使用。当前 `permissionRequest` 要求由 Core 声明式动作进入，Wasm API 请求不能直接打开授权流程；`terminalRequestInput` 从隔离 Wasm 入口调用时（包括点击回调）仍返回 `interactionRequired`。这两类交互需要使用对应的 Core 动作流程。
