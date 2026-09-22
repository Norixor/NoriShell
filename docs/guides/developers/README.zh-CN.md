# 插件开发指南

- [快速入门与示例选择](quickstart.zh-CN.md)
- [包与 manifest](package.zh-CN.md)
- [Protocol 13 Wasm ABI](wasm-abi.zh-CN.md)
- [Broker 调用、资源、存储、凭据与订阅](broker.zh-CN.md)
- [声明式 UI 与扩展目标](ui.zh-CN.md)
- [安全与发布前检查](security.zh-CN.md)

guest 代码只能使用[插件 API 参考](../plugin-api/README.zh-CN.md)。[Core API 目录](../core-api/README.zh-CN.md)记录应用 renderer IPC，绝不是插件 API。SDK 与本地 CLI 面向稳定常驻 ABI 基线 **1.13**，同 major 宿主后续升级保留基线插件兼容；源码接线或 ABI harness 成功都不代表已完成 native 验收。

- [声明式主题插件](themes.zh-CN.md)
