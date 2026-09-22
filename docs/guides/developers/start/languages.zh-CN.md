# 语言与 UI 边界

NoriShell 加载实现 protocol 1.13 Wasm ABI 的 guest。任何能产出该精确 ABI 的语言都可用于编写 guest；当前维护的作者 SDK 是 Rust。HTML、CSS 和 JavaScript 仅用于包自有的**隔离 UI**；它们不是第二套主插件 SDK。

| 技术 | 当前角色 | 可以做什么 | 必须保持的边界 |
| --- | --- | --- | --- |
| Rust + `norishell-plugin-sdk` | 官方 guest 作者路径 | 为 `wasm32-unknown-unknown` 构建 `cdylib`，处理类型化 host message，输出 `ui.document` 与类型化 `api.request` | 使用 SDK 的 `export_plugin!`；guest 没有桌面、网络、文件、Vault 或 Tauri 的环境权限。 |
| Wasm target | guest 的运行时格式 | 暴露由 Core 加载包时验证的 protocol 1.13 guest ABI | 当前只有一套 ABI；没有 WASI、原生库、动态下载或旧协议 fallback。 |
| HTML/CSS/JavaScript | 隔离 WebView 的包内资源 | Core 打开已校验资源后渲染更丰富的独立页面；使用 Core 提供的 MessagePort | 页面不直接获得网络、Tauri、文件、Vault、终端或宿主 DOM 访问。参见 [隔离 UI](../examples/isolated-ui.zh-CN.md)。 |
| 其他可编译到 Wasm 的语言 | 自行实现当前精确 Wasm ABI 与包规则 | 产出规定的 export、类型化协议 envelope 与包内容 | NoriShell 当前只提供 Rust SDK；其他语言需自行实现并验证 ABI。宿主按 ABI 合规性加载 guest，不按实现语言限制。 |

## Rust guest 模型

SDK 提供 ABI export、协议 envelope 和类型化 request/result DTO。普通插件在 Wasm 实例生命周期中有一个有界状态对象：

```rust
use norishell_plugin_sdk::{Plugin, PluginError, PluginHostRequest, PluginRuntimeOutput, export_plugin};

#[derive(Default)]
struct MyPlugin;

impl Plugin for MyPlugin {
    fn handle(&mut self, request: PluginHostRequest)
        -> Result<Vec<PluginRuntimeOutput>, PluginError> {
        // 匹配 Initialize、UiAction、BrokerResult，以及插件拥有的事件。
        Ok(Vec::new())
    }
}

export_plugin!(MyPlugin);
```

实例状态不是持久化存储，也不代表权限。Core 可以停止或替换实例；资源句柄是不透明且归属受限的，撤销、禁用、替换、取消或 generation 过期都可能关闭它。

## 隔离 HTML 页面模型

隔离 UI 使用普通 guest contribution 请求 Core 打开包内 HTML 资源。Core 创建隔离 WebView，并在 `norishell.bridge.ready` handshake 后交付一个受限 MessagePort。页面通过该 port 发送类型化 `norishell.api.request` 并接收类型化回复；在收到 port 前必须禁用控件。

这让 HTML/CSS/JS 负责展示，同时保持相同的 Core 审批与资源边界；浏览器 API 不是绕过边界的入口。选择这种 surface 前请阅读 [隔离 UI](../examples/isolated-ui.zh-CN.md)，操作与错误规则请看 [调用 Core](../development/calling-api.zh-CN.md)。
