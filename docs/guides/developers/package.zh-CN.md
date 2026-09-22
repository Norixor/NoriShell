# 包与 manifest

可执行 Wasm 插件包是受限 ZIP，只能包含 `manifest.json`、`plugin.wasm` 以及 `assets/` 下已声明的静态资源。原生库、WASI、postinstall、运行时下载、任意构建脚本、符号链接、不安全路径、归一化后冲突路径、加密条目和未知根文件都会被拒绝。Core 会复制私有副本，并在安装事务中再次核验。纯数据[主题包](themes.zh-CN.md)只包含 `manifest.json` 与 `assets/theme.json`，两种包分支不能混合。

当前包使用 `protocolMajor: 1`、`protocolMinor: 13`，这是稳定常驻 ABI 基线。后续 major 1 宿主保留从 13 到自身 minor 的兼容范围；要求未来 minor 或未知 capability 的条目可展示，但不能在旧宿主安装。破坏性变更必须升级 major；不支持 13 以前的 export 或 fallback parser。下面是完整的本地包 manifest 基础示例；`publisher` 是自述发布者，不能当作签名身份。`platform`、`architectures` 与 `minimumAppVersion` 也必须提供。平台声明不代表已完成对应平台验收。

```json
{
  "pluginId": "com.example.service-inspector",
  "name": "Service Inspector",
  "publisher": "Example publisher (self-reported)",
  "version": "1.0.0",
  "protocolMajor": 1,
  "protocolMinor": 13,
  "platform": "desktop",
  "architectures": [
    "universal"
  ],
  "capabilities": [
    "uiPanel",
    "storagePlugin"
  ],
  "minimumAppVersion": "0.1.0"
}
```

能力声明只是申请，并不授权。请继续阅读[权限与资源](broker.zh-CN.md)和类型化的[插件 API 参考](../plugin-api/README.zh-CN.md)。

可用 `norishell-plugin-dev scaffold`、`pack`、`check` 创建和检查本地包。`run` 与 `watch` 只是 Wasm ABI harness：它们把 request JSON 交给 guest，测试调用方还必须自己提供类型化 broker reply。它们不会创建 Core broker、批准 capability、创建/解锁 Vault，也不会覆盖受保护的 native 安装流程。`watch` 只观察指定 Wasm 文件，成功重载后只重放最后一个显式 `Initialize` request。

源码示例 `verify_api_demo`、`verify_guest_api_types` 与 `verify_isolated_demo` 位于 [`crates/plugin-sdk/examples`](../../../crates/plugin-sdk/examples)。本地 fixture 前提接通后，可用 `cargo run -p norishell-plugin-sdk --example <name>` 运行。它们只是 SDK/ABI 演示，不能替代受保护应用接入或 P19 native 验收。

[`examples/plugins/workflow-demo`](../../../examples/plugins/workflow-demo) 是 package 形式的 task 示例。其 README 给出当前 Wasm build、ZIP pack 与 `check` 命令；产出的本地 ZIP 已有源码级 package validation，每个改造后的插件仍须独立完成导入、任务执行与重启检查。仓库示例的已有验收记录见[实施状态](../../../README.md#安装与快速开始)。

[`examples/plugins/protocol-demo`](../../../examples/plugins/protocol-demo) 通过 `build.py` 构建真实 protocol-13 Wasm package。其 fixture harness 在测试 broker 下用 production persistent `WasmRuntime` 执行生成的 guest，验证 loopback TCP input/resize、分段 ANSI/UTF-8 byte、双 handle 与旧 handle close 隔离；ZIP 也通过 SDK package check。harness 不会安装 ZIP、启动 `ProtocolSessionActor`、进入 Tauri 应用或经过 protected approval surface。因此它只证明 guest/fixture 边界，不能作为 native protocol 或 Windows 验收。

[`examples/plugins/app-demo`](../../../examples/plugins/app-demo) 演示显式注册命令/快捷键、状态、通知、受控导航及文件命令。文件动作通过原生 `FilePick` 和范围批准取得只读句柄，再由明确点击读取预览和关闭；扩展名仅用于命令分类，不注册系统文件关联。构建与原生验收的区别见示例 README。

[`examples/plugins/service-demo`](../../../examples/plugins/service-demo) 使用实际 Core workflow 与 NetworkStart 查询 GitHub 公开仓库信息。初始查询、继续网络审批、刷新及取消均为显式动作；guest 不直接联网，严格处理 HTTP/JSON 和响应上限。真实桌面联网验收见实施状态，不由 guest 解析测试替代。

从源码构建到桌面导入的完整步骤见[快速入门与示例选择](quickstart.zh-CN.md)。

- [声明式主题插件](themes.zh-CN.md)
