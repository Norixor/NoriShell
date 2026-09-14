# NoriShell UUID Generator

这是 NoriShell 本地插件格式的 UUID 生成示例。插件申请 `ui.panel` 与 `clipboard.write`，在 Global Header 的“+”右侧注入一个由宿主渲染的 UUID 按钮：

- 点击 UUID 按钮打开宿主管理的功能 Modal。
- “生成 UUID”刷新 Modal 中的 UUIDv4。
- “生成并复制”创建新的 UUIDv4，并在同一次明确点击后写入剪贴板。

插件不使用 WASI、网络、文件、Vault 或终端权限，也不向主 WebView 注入 HTML、脚本和样式。Header 扩展位、按钮、Modal、焦点管理与剪贴板写入均由宿主控制；插件只返回经过 Core 校验的声明式 UI 文档和有界 UUID 文本。

从仓库根目录生成 ZIP：

```sh
cargo run -p norishell-plugin-platform --example build_utility_demo -- /path/to/NoriShell-UUID-Generator-1.3.0.zip
```
