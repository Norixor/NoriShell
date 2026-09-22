# 插件 API 参考

- [协议边界与 ABI](protocol.zh-CN.md)
- [类型化 Broker 方法、资源、事件、请求与结果](broker.zh-CN.md)
- [guest-safe 请求与结果类型](types.zh-CN.md)
- [声明式 UI、字段与目标](ui.zh-CN.md)
- [能力与受保护操作](capabilities.zh-CN.md)
- [Protocol provider、catalog 与集成中的 serial/session](providers.zh-CN.md)

这是 protocol **1.13** 的公开 guest 契约，包含 35 个 `PluginApiOperation` variant；运行时可用子集由 `describe` 返回。插件不得调用[Core 内部 API 目录](../core-api/README.zh-CN.md)的任何项。
