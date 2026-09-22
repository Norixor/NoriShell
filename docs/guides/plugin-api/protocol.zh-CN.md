# 插件协议与 ABI

当前唯一插件协议是 **1.13**。guest 与 Core 必须使用精确相同的 major/minor；不存在双协议 fallback。协议传递类型化 envelope，输出 request id 必须匹配输入 request id。嵌套 JSON string 应由 serializer 编码，不能手拼转义 JSON。

常驻 Wasm ABI 为 `memory`、`nvx_alloc`、`nvx_handle`、`nvx_dealloc`，唯一 import 是 `norishell.host.emit`。Core 负责 allocation/free 顺序。优先使用 `norishell_plugin_sdk` 获得精确 ABI export 及类型化 `PluginHostRequest`、`PluginRuntimeOutput`、`PluginApiCall` 和 reply。`Initialize` 只启动或刷新 host instance，不是 app command 注册 hook，也不会取得收到 request 之外的 authority；见[开发 ABI 指南](../developers/wasm-abi.zh-CN.md)。
