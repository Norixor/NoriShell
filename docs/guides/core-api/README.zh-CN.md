# Core 内部 API 目录

Core API 是 NoriShell 自身 WebView 的应用 renderer IPC，**不是**插件 API 或 capability grant。插件只能使用[插件 API](../plugin-api/README.zh-CN.md)，不得调用 Tauri command、经 IPC 获取 Core DTO，或依赖内部 handle。

当前源码版本是 Core API **1.88**。生成的 TypeScript 契约位于 [`src/core-api/generated/core-api.ts`](../../../src/core-api/generated/core-api.ts)，Rust 公开类型与稳定 command 声明从 [`crates/core-api/src/lib.rs`](../../../crates/core-api/src/lib.rs) 开始，production command 注册在 [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs)。分类数据接口仍在集成，模块、生成声明与 handler 数量可能变化；精确名称以所链接生成目录为准。已注册 handler 可包含 Wry 专用或未导出的应用接线，须与稳定 command 分开阅读。

- [模块与类型目录](modules.zh-CN.md)
- [应用 IPC command 目录](commands.zh-CN.md)
- [Core event 与 resource event 目录](events.zh-CN.md)
- [Plugin Host bridge 边界](plugin-host.zh-CN.md)
- [完整生成类型目录](types.generated.md)
- [稳定生成 command 目录](commands.generated.md)
- [已注册 handler 与主窗口 ACL 目录](handlers.generated.md)
- [生成 event payload 目录](events.generated.md)

实现变动时以生成 request/result type 为准。本目录记录源码接口，不能据此声称平台、原生窗口、硬件或 Windows 已验收；这些事实只在[实施状态](../../../README.zh-CN.md#安装与快速开始)记录。
