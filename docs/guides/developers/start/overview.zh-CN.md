# NoriShell 插件作者指南

NoriShell 插件是运行在桌面应用 Vue WebView 与原生进程之外的 Rust/Wasm guest。插件可以贡献由宿主渲染的面板、请求 Core 执行类型化且经过审阅的操作、处理类型化结果，并更新自己的 document。它不能直接调用 Tauri、打开 socket、读取 Vault、访问 SQLite、注入宿主 DOM，也不能把 capability 声明当作自动授权。

当前公开的作者契约是协议 **1.13**。请使用本地 NoriShell 源码检出中同版本的 SDK 构建，并打出一个受限 ZIP：`manifest.json`、`plugin.wasm` 与已声明静态资源。

## 可以构建什么

| 目标 | 从这里开始 | Core 仍然拥有的职责 |
| --- | --- | --- |
| 展示类型化 API 信息的小工具 | [第一个可见插件](./quickstart.zh-CN.md) 与 [API Demo](../examples/api-demo.zh-CN.md) | 面板目标、渲染、剪贴板审批与 API 可用性 |
| 服务集成 | [Service Demo](../examples/service-demo.zh-CN.md) | HTTPS 传输、精确 endpoint 审阅、资源生命周期与用户审批 |
| 带定时器和重启状态的任务 | [Workflow Demo](../examples/workflow.zh-CN.md) | Task 记录、定时器、资源清理、revision 与重启协调 |
| 受控命令、通知、导航或文件选择 | [应用集成](../examples/app-integration.zh-CN.md) | 命令注册、快捷键启用、原生选择器、文件句柄与范围审批 |
| 独立 HTML/CSS/JS 页面 | [隔离 UI](../examples/isolated-ui.zh-CN.md) | 隔离 WebView、包内资源查找与受限 MessagePort |
| 终端协议 Provider | [Protocol Demo](../examples/protocol.zh-CN.md) | 网络资源、终端生命周期、权限检查与字节传递 |

## 阅读路径

| 你要完成的事 | 建议顺序 |
| --- | --- |
| 让第一个按钮显示并响应 | [快速入门](./quickstart.zh-CN.md) → [API Demo](../examples/api-demo.zh-CN.md) → [UI document](../development/ui.zh-CN.md) → [打包](../development/packaging.zh-CN.md) |
| 点击后调用 Core 能力 | [调用 Core](../development/calling-api.zh-CN.md) → [API Demo](../examples/api-demo.zh-CN.md) → [错误与用户继续操作](../development/errors.zh-CN.md) |
| 选择语言或 HTML 页面方案 | [语言与边界](./languages.zh-CN.md) → [隔离 UI](../examples/isolated-ui.zh-CN.md) |
| 把样例改造成自己的产品 | [示例索引](../examples/README.zh-CN.md) → 对应专题页 → [打包](../development/packaging.zh-CN.md) |

通常 `Initialize` 贡献 document；用户点击产生 `UiAction`；guest 可以输出类型化 `api.request`；Core 随后发送 `BrokerResult`；guest 再返回 `ui.document`。完整往返请看 [API Demo](../examples/api-demo.zh-CN.md)。

guest 代码只使用 SDK 及其类型化 DTO。Core 内部 API 目录面向应用维护者，不是插件 API。包检查或 ABI harness 只证明本地产物边界；导入、capability 审批、启用/禁用行为和实际用户流程仍须在匹配的 NoriShell 桌面应用中验证。
