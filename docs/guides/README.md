# NoriShell 文档目录 / Documentation

这里是 NoriShell 可公开发布并可嵌入 Web 的文档入口。全部页面使用相对链接，并按用户使用、插件开发、Plugin API 与 Core 内部 API 分开维护。

This directory is the public, Web-embeddable entry point for NoriShell documentation. Every page uses relative links and belongs to one of four clearly separated collections.

## 文档分类 / Documentation map

| 范围 / Collection | 中文 | English | 面向对象与边界 / Audience and boundary |
| --- | --- | --- | --- |
| 用户指南 / User guides | [打开](users/README.zh-CN.md) | [Open](users/README.en.md) | 本地 ZIP 导入、权限、恢复、安全提示与外观 / Local ZIP import, permissions, recovery, safety, and appearance |
| 插件开发指南 / Developer guides | [打开](developers/README.zh-CN.md) | [Open](developers/README.en.md) | 包、manifest、Wasm ABI、Broker、声明式 UI 与发布检查 / Packages, manifest, Wasm ABI, brokers, declarative UI, and release checks |
| Plugin API 参考 / Plugin API reference | [打开](plugin-api/README.zh-CN.md) | [Open](plugin-api/README.en.md) | 插件 guest 唯一可调用的协议、能力、类型与资源 / The only guest-facing protocols, capabilities, types, and resources |
| Core API 目录 / Core API catalog | [打开](core-api/README.zh-CN.md) | [Open](core-api/README.en.md) | 应用 renderer IPC 与 Plugin Host 内部边界，不是插件权限面 / Application renderer IPC and Plugin Host internals, never a plugin permission surface |

## 插件开发快速入口 / Plugin development shortcuts

- [快速入门](developers/quickstart.zh-CN.md) / [Quick start](developers/quickstart.en.md)
- [包与 manifest](developers/package.zh-CN.md) / [Package and manifest](developers/package.en.md)
- [Protocol 13 Wasm ABI](developers/wasm-abi.zh-CN.md) / [Protocol-13 Wasm ABI](developers/wasm-abi.en.md)
- [Broker、资源与存储](developers/broker.zh-CN.md) / [Brokers, resources, and storage](developers/broker.en.md)
- [声明式 UI](developers/ui.zh-CN.md) / [Declarative UI](developers/ui.en.md)
- [安全与发布检查](developers/security.zh-CN.md) / [Security and release checklist](developers/security.en.md)

文档描述源码契约，不把待验收能力写成已完成。当前进度和平台边界见[中文项目状态](../../README.md#安装与快速开始)或[English project status](../../README.en.md#installation-and-quick-start)。

The documentation describes source contracts and does not present pending acceptance as complete. See the [Chinese project status](../../README.md#安装与快速开始) or [English project status](../../README.en.md#installation-and-quick-start) for current progress and platform boundaries.

## 兼容入口 / Retained compatibility URLs

- [插件指南 / Plugin guide](plugins.zh-CN.md) · [English](plugins.en.md)
- [Plugin API 参考](plugin-api-reference.zh-CN.md) · [English](plugin-api-reference.en.md)
- [Core API 目录](core-api-reference.zh-CN.md) · [English](core-api-reference.en.md)
