# 插件示例

本节每个示例都对应 NoriShell 维护的源码。请把它作为面向一个目标的起点，而非适用于所有产品的可安装模板：包身份、声明 capabilities、动作、状态和资源要一起修改。

| 示例 | 适用目标 | 本地包结果 | 仍需桌面验收的内容 |
| --- | --- | --- | --- |
| [API Demo](./api-demo.zh-CN.md) | 第一个可见面板、类型化 API 查询和结果复制 | Wasm ABI verifier、ZIP 与校验和 | 导入、审批、启用、面板贡献、点击结果和禁用行为 |
| [Service Demo](./service-demo.zh-CN.md) | 经显式审批的 HTTPS workflow | 测试、包检查、ZIP 与校验和 | 权限继续操作、真实网络响应、取消与资源清理 |
| [Workflow Demo](./workflow.zh-CN.md) | 带 revision 和重启状态的定时任务 | Wasm 构建与 SDK 包检查 | 应用中的 task 生命周期、取消和重启行为 |
| [应用集成](./app-integration.zh-CN.md) | 命令、可选快捷键、通知、导航或原生文件句柄 | Wasm 构建与 SDK 包检查 | 命令/快捷键、原生选择器、范围审阅、通知和句柄清理 |
| [隔离 UI](./isolated-ui.zh-CN.md) | 通过受限桥接的包内 HTML/CSS/JS 页面 | Wasm 构建和隔离页面 ABI 验证 | 隔离 WebView、MessagePort、审批提示与资源清理 |
| [Protocol Demo](./protocol.zh-CN.md) | framed TCP 终端 provider | loopback fixture harness、ZIP 检查与校验和 | 桌面终端生命周期与受保护审批流程 |

前置条件和精确的 API Demo 构建命令请先看 [快速入门](../start/quickstart.zh-CN.md)。每个示例在通过 NoriShell 导入流程选择前都只是本地包；构建命令不会发布或安装它。

专题页中的构建命令可在所述 NoriShell 源码检出根目录运行。Rust、JSON、JavaScript 和事件流程代码块除非页面另有明确说明，都是解释性节选；不要把孤立代码块当作完整插件直接粘贴，应保留对应源码示例的 import、状态、manifest、资源和验证。

## 共通消息循环

多数面向用户的插件遵循下面顺序：

```text
Initialize
  -> ui.document（Core 在允许 target 渲染）
显式点击产生 UiAction
  -> api.request（类型化操作；没有环境授权）
Core 发送 BrokerResult
  -> ui.document（替换或更新可见状态）
```

`BrokerResult` 是 guest 将类型化成功或失败转换为下一份 document 的时机。收到它之前不要虚构成功；也不要从初始化、timer、provider 回调或其他后台路径请求受保护审批。请结合 [调用 Core](../development/calling-api.zh-CN.md)、[UI document](../development/ui.zh-CN.md)、[错误](../development/errors.zh-CN.md) 与 [打包](../development/packaging.zh-CN.md) 阅读专题示例。
