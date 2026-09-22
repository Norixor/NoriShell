# Core 内部 API 目录

Core API 是 NoriShell 自身 WebView 的应用 renderer IPC，**不是**插件 API 或 capability grant。插件只能使用[插件 API](../plugin-api/README.zh-CN.md)，不得调用 Tauri command、经 IPC 获取 Core DTO，或依赖内部 handle。

当前源码版本是 Core API **1.85**。生成的 TypeScript 契约位于 [`src/core-api/generated/core-api.ts`](../../../src/core-api/generated/core-api.ts)，Rust 公开类型与稳定 command 声明从 [`crates/core-api/src/lib.rs`](../../../crates/core-api/src/lib.rs) 开始，production command 注册在 [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs)。本检查点有 48 个公开 Core 模块、857 个导出的 TypeScript 声明、210 个稳定 `COMMAND_*` 名称和 264 个已注册 Tauri handler。后者可包含只用于 Wry 或未导出的应用内部接线，因此两个目录必须分开阅读。

- [模块与类型目录](modules.zh-CN.md)
- [应用 IPC command 目录](commands.zh-CN.md)
- [Core event 与 resource event 目录](events.zh-CN.md)
- [Plugin Host bridge 边界](plugin-host.zh-CN.md)
- [完整生成类型目录](types.generated.md)
- [稳定生成 command 目录](commands.generated.md)
- [已注册 handler 与主窗口 ACL 目录](handlers.generated.md)
- [生成 event payload 目录](events.generated.md)

实现变动时以生成 request/result type 为准。本目录记录源码接口，不能据此声称平台、原生窗口、硬件或 Windows 已验收；这些事实只在[实施状态](../../../README.md#安装与快速开始)记录。
