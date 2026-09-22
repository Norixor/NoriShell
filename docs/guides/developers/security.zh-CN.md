# 安全与发布前检查

把插件视为不可信的 proposer。每个结果都必须绑定 Core 提供的 request 和 resource handle；不要把 handle 持久化为 authority，不要自动重试 `outcomeUnknown`，也不要在取消后重新创建资源。发送 document 前先验证输入并限制渲染。

发布前核对受支持的 manifest minor（major 1 内从基线 13 到宿主 minor）与对应常驻 SDK ABI、拒绝和撤销 grant、陈旧 target/generation 拒绝、resource close 与 event backpressure、storage 冲突、精确 origin 凭据行为，以及输出、state、storage、audit 文本和错误中没有秘密。相关 capability 必须分别在真实 macOS 和 Windows Tauri 生命周期中验证。本检查点的 native desktop、Windows 与 hardware 验收仍属 P19；源码分支、package check、ABI harness 或 unit test 都不是 native 验收。[实施状态](../../../README.md#安装与快速开始)是实际已验收能力的唯一事实来源。
