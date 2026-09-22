# Protocol provider、serial resource、task 与 app integration

`protocolOpen` 是插件 protocol provider 的类型化入口。Core 拥有网络/设备 broker 与生命周期；常驻 Wasm instance 接收 `PluginProtocolEvent`，以类型化 `PluginProtocolOutput` 或一个 broker continuation 响应。它绝不能声明 Core session identity 或创建原始 socket。`serialDevices`、`serialOpen` 与 `serialSend` 使用 Core 持有的 serial resource，包含选定 candidate identity、已验证 settings、有界 send、close/revoke fence 和有序 resource event。

`taskStart`、`taskGet`、`taskList`、`taskCancel` 与 `taskResume` 只操作包的 workflow catalog。task 每次只收到一个 `PluginWorkflowEvent { taskId, workflowId, event }`，固定 step 最多产生一个类型化 broker call。只有 task 已拥有 resource event 时，`WorkflowResponse` 才可故意等待（`complete: false`、无 call、无 step）；没有已拥有 resource 的等待会被 Core 拒绝。持久 summary/step timestamp 是 JavaScript number。它没有已持久化 payload 或 continuation interpreter：启动时未完成 durable row 会变为 `interrupted`，不会重放 dispatch 或恢复旧 input。

`appRegister`、`appNotify` 与 `appNavigate` 是显式可见 integration action，只有 Core 核验 package owner 与当前 invocation 后才返回 `appAccepted`。它们不会由 `Initialize`、page load、`onOpen` 或任何隐式后台执行注册。

`appNavigate` 只接受受控应用路由或插件自身页面。主窗口可以关闭同一插件、包哈希与运行代次拥有的声明式弹窗后导航；未知、其他插件或应用安全弹窗会立即丢弃导航，不会在弹窗消失后重放。关闭弹窗后再次核验当前 Core owner 与阻断状态。

Core集成fixture已验证实际ProtocolSessionActor与任务执行/取消，macOS原生工作流已验证执行、刷新后取消、正常退出取消和异常退出中断恢复，服务示例已完成受保护批准后的真实HTTPS查询。其余逐项平台、协议与硬件验收以[实施状态](../../../README.md#安装与快速开始)为准。包作者仍须查询`describe`；源码、包校验或单测不能替代对应原生验收。

## 声明 provider 包目录

协议 provider 必须在 ZIP 的固定路径 `assets/protocols.json` 声明，安装时校验；它不是运行时注册接口。下面直接采用 Protocol Demo 的目录结构：

```json
{
  "schemaVersion": 1,
  "providers": [
    {
      "id": "framedTcp",
      "label": {
        "en": "Framed TCP demo",
        "zh-CN": "Framed TCP 示例"
      },
      "configuration": {
        "schemaVersion": 1,
        "fields": [
          {
            "type": "string",
            "key": "endpoint",
            "label": {
              "en": "TCP endpoint",
              "zh-CN": "TCP 端点"
            },
            "default": "tcp://127.0.0.1:19071",
            "maxLength": 255
          }
        ]
      },
      "features": {
        "terminal": true,
        "resize": "supported",
        "reconnect": true
      },
      "resources": ["tcp"]
    }
  ]
}
```

安装校验要求：catalog 的 `schemaVersion` 为 1，原始及规范化 JSON 均不超过 64 KiB，最多 8 个 provider。每个 `id` 唯一，1–64 字节 ASCII，以字母或数字开头，其余字符只允许字母、数字、`.`、`_`、`-`。`label.en` 和 `label.zh-CN` 均必填、非空且各不超过 512 字节，不允许控制字符或双向文本控制符。

`configuration` 复用 schema v1 的插件设置校验，必须有 1–32 个字段，键不能重复或表示秘密；默认值与字段限制也必须有效。`features.terminal` 必须为 `true`，`resize` 只能是 `supported` 或 `unsupported`，`reconnect` 是布尔值。`resources` 只接受 `tcp`、`tls`、`websocket`、`serial`，最多四个且不可重复；未知字段被拒绝。资源声明不授予能力，manifest 仍须声明实际需要的 capability，运行时仍经过 Core broker 的授权。

构建、输入/resize 与关闭示例见[Protocol Demo](../../../examples/plugins/protocol-demo/README.md)。修改 catalog 后重新打包，不能只替换 Wasm 并期待已安装 catalog 自动改变。
