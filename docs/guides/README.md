# NoriShell 文档 / Documentation

按使用场景选择指南。插件作者可以从快速开始构建第一个可见插件，再按开发流程查阅 API 和示例。

Choose a guide for your task. Plugin authors can build a first visible plugin with the quick start, then explore the API and examples as needed.

| 文档 / Guide | 中文 | English |
| --- | --- | --- |
| 用户指南 / User guides | [安装、权限与外观](users/README.zh-CN.md) | [Installation, permissions, and appearance](users/README.en.md) |
| 插件开发 / Plugin development | [完整开发目录](developers/README.zh-CN.md) | [Complete development guide](developers/README.en.md) |
| 快速开始 / Quick start | [第一个可见插件](developers/start/quickstart.zh-CN.md) | [Your first visible plugin](developers/start/quickstart.en.md) |
| API 参考 / API reference | [35 个方法与类型](plugin-api/README.zh-CN.md) | [35 methods and their types](plugin-api/README.en.md) |
| 示例 / Examples | [按开发目标选择](developers/examples/README.zh-CN.md) | [Choose by development goal](developers/examples/README.en.md) |
| Core 内部参考 / Core internals | [应用维护者目录](core-api/README.zh-CN.md) | [Application maintainer catalog](core-api/README.en.md) |

## 插件开发阅读顺序 / Plugin development reading order

1. [认识插件](developers/start/overview.zh-CN.md) / [Plugin overview](developers/start/overview.en.md)
2. [构建第一个插件](developers/start/quickstart.zh-CN.md) / [Build your first plugin](developers/start/quickstart.en.md)
3. [调用 API](developers/development/calling-api.zh-CN.md) / [Call APIs](developers/development/calling-api.en.md)
4. [构建界面](developers/development/ui.zh-CN.md) / [Build UI](developers/development/ui.en.md)
5. [管理权限与资源](developers/development/resources.zh-CN.md) / [Manage permissions and resources](developers/development/resources.en.md)
6. [打包、安装与升级](developers/development/packaging.zh-CN.md) / [Package, install, and upgrade](developers/development/packaging.en.md)

Plugin API 是插件的公开调用边界；Core 内部目录记录应用 IPC，不能从插件直接调用。

The Plugin API is the public plugin boundary. Core internals document application IPC and cannot be called directly from plugins.
