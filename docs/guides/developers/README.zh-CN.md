# NoriShell 插件开发指南

从第一个可见插件开始，学习如何调用 Core 能力、构建界面、管理权限与资源，再打包为本地 ZIP。

## 入门

- [插件模型与阅读路径](start/overview.zh-CN.md)
- [快速开始：第一个可见插件](start/quickstart.zh-CN.md)
- [语言、Wasm 与隔离 UI](start/languages.zh-CN.md)

## 开发插件

- [调用 API 与处理结果](development/calling-api.zh-CN.md)
- [声明式 UI 与动作回调](development/ui.zh-CN.md)
- [权限与资源生命周期](development/resources.zh-CN.md)
- [打包、安装与升级](development/packaging.zh-CN.md)
- [错误处理与用户继续操作](development/errors.zh-CN.md)

## API 参考

- [35 个方法，按能力分类](../plugin-api/README.zh-CN.md)
- [请求、结果与嵌套类型](../plugin-api/types.zh-CN.md)

## 示例

- [示例选择与源码目录](examples/README.zh-CN.md)
- [API Demo：查询、展示与复制](examples/api-demo.zh-CN.md)
- [Service Demo：网络服务集成](examples/service-demo.zh-CN.md)
- [Workflow Demo：多步骤任务](examples/workflow.zh-CN.md)
- [应用集成：命令、通知、导航与文件](examples/app-integration.zh-CN.md)
- [隔离 UI：HTML、CSS 与 JavaScript](examples/isolated-ui.zh-CN.md)
- [Protocol Demo：终端协议 Provider](examples/protocol.zh-CN.md)

## 专题参考

- [Protocol 1.13 Wasm ABI](wasm-abi.zh-CN.md)
- [声明式主题插件](themes.zh-CN.md)
- [安全与发布前检查](security.zh-CN.md)
- [SDK 工具与开发循环](../../../examples/plugins/sdk-tooling/README.md)

插件只能使用公开的 Plugin API。应用维护者使用的 [Core 内部 API 目录](../core-api/README.zh-CN.md)不是插件可调用接口。
