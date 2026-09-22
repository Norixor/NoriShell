# 快速入门与示例选择

本指南面向在 NoriShell 仓库中开发插件的作者。**1.13** 是稳定常驻 ABI 基线；同 major 宿主接受从 13 到自身当前 minor 的插件，新增能力要求声明对应最低 minor/app version；SDK 使用仓库源码依赖，不假定已经发布到 crates.io。先运行可见的 API Demo，再根据目标选择示例。

## 构建第一个可见插件

在仓库根目录执行。需要 Python 3、rustup 的 `1.97.1` 工具链及其 `wasm32-unknown-unknown` target；脚本使用 `--locked --offline`，依赖必须已缓存。缺失依赖或 target 时先完成本机工具链准备，不能把构建失败当作权限问题。

```sh
python3 examples/plugins/api-demo/build.py --output /tmp/NoriShell-API-Demo-1.0.1.zip
```

脚本编译真实 Wasm、运行 ABI 验证，并生成 ZIP 与相邻 `.zip.sha256` 文件；临时编译目录自动清理。此步骤不安装插件、不授权，也不发布。

1. 在当前协议的 NoriShell 中打开“插件 → 导入 ZIP”，选择上述文件，核对包身份和权限。
2. 完成安装并显式批准所需能力、启用插件。API Demo 申请 `uiPanel` 和 `clipboardWrite`。
3. 打开 **API demo**，点击 **Query API**。应显示当前 Core 返回的方法与限制；打开页面本身不会查询。
4. 点击复制按钮，核对复制内容与展示结果一致。禁用插件后，其贡献入口应消失。

如果包能安装但没有界面，先检查贡献与 `ui.document`，再检查动作回调，不要用“已启用”代替功能验证。完整 UI 输出规则见[声明式 UI](ui.zh-CN.md)。

## 按目标选择源码示例

| 开发目标 | 示例与入口 | 重点 |
| --- | --- | --- |
| 查询 API、展示与复制 | [API Demo](../../../examples/plugins/api-demo/README.md) | 显式动作 → `api.request` → `BrokerResult` → 新 document |
| 调用外部服务 | [Service Demo](../../../examples/plugins/service-demo/README.md) | Core HTTPS、需要用户操作时继续、HTTP 结果与取消 |
| 多步骤任务 | [Workflow Demo](../../../examples/plugins/workflow-demo/README.md) | catalog、timer、资源事件、revision 与重启后的状态 |
| 新终端协议 | [Protocol Demo](../../../examples/plugins/protocol-demo/README.md) | provider 事件、字节流、resize、重连和资源关闭 |
| 命令、通知、导航、文件 | [App Demo](../../../examples/plugins/app-demo/README.md) | 显式注册、快捷键启用、原生选择与文件句柄 |
| 独立 HTML 页面 | [Isolated Demo 源码](../../../examples/plugins/isolated-demo) | `assets/isolated/`、MessagePort、主题/语言和受限桥 |
| 创建空骨架与调试 ABI | [SDK 工具](../../../examples/plugins/sdk-tooling/README.md) | `scaffold`、`pack`、`check`、`run`、`watch` |

复制示例时修改 `pluginId`、名称、版本及实际所需 capability，并同步 catalog、动作和静态资源。不要直接修改示例身份后覆盖已有安装。`scaffold` 会写入当前本机 SDK 的绝对依赖路径，迁移目录或分享项目时需要调整；它只提供 ABI 骨架，默认 `example.ready` 输出不是可见应用贡献，需按 API Demo 接入有效 document。

## 调用和失败处理

使用 SDK 导出的类型构造请求和解析结果，不直接依赖 Core 内部 IPC。一次 UI 动作发出调用后，按对应 `BrokerResult` 更新状态；`callId` 使用 1–80 字节的 ASCII 字母、数字、`.`、`_`、`-`，不要使用冒号。具体字段见[Broker API](../plugin-api/broker.zh-CN.md)。

| 结果或现象 | 作者应如何处理 |
| --- | --- |
| `interactionRequired`、任务 `needsUserAction` | 显示明确的继续操作；后台回调不得反复请求弹窗 |
| `vaultMissing` / `vaultLocked` / `vaultRequiresReload` | 保留不同状态，通过明确点击交给 Core 处理；插件不收集 Vault 密码 |
| 权限拒绝、过期或撤销 | 显示失败并结束受影响操作；旧 handle 不能作为再次授权 |
| `conflict` | 重新读取 task/storage/permission 的 revision，再由有效动作提交 |
| `outcomeUnknown` | 先核实实际结果，不能盲目重试可能产生副作用的请求 |
| ABI 通过但点击失败 | 核对 request/call id、完整 document、target 与声明的 capability |

`run` / `watch` 读取每行一个 `PluginHostRequest` JSON；它们没有 Core broker，`api.request` 的类型化回复须由测试调用方提供。`watch` 只监视 Wasm 文件，不编译源码；重载产生新实例并重放最后一个显式 Initialize，有界内存状态不会保留。

最后在真实应用检查允许、拒绝、撤销、取消与资源关闭。仓库示例的已验收证据和 Windows/硬件待验收边界统一见[实施状态](../../../README.md#安装与快速开始)，不要把它们等同于自己改造后的包已经通过验收。
