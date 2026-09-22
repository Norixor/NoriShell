# 插件权限与恢复

插件默认没有任何能力授权。它只能提出类型化操作；Core 会在准入前绑定包身份、授权 epoch、资源 owner 和运行 generation。关闭受保护提示即为拒绝，插件不能用普通可见操作代替受保护决定。

任何权限都不会暴露 Vault 密码、KEK/VMK、Vault payload、SecretRef/CredentialRef 原文、SSH 凭据、私钥、认证回答、host-key 决定、SSH transport/channel、SQLite、任意 Tauri IPC 或主 Vue realm。插件自有账号凭据走受保护输入流程，只返回不透明引用；插件永远不能读取秘密。对 HTTPS/WSS，Core 也只会在已批准的精确 origin 注入该引用。

安全模式会在恢复 Plugin Host、贡献或 owner CSS 之前启用，仍保留管理入口以禁用或移除包。能力或 API 名称不代表原生验收已完成，请查阅[实施状态](../../../README.md#安装与快速开始)。

批准与上下文绑定。受保护窗口可为当前精确 action 提供 `once`，只有 Core 能保存 exact-operation rule 时才可提供 `always`。Always 可选 15 分钟、1 小时、24 小时或无限期；有限期由 Core 记录为绝对 deadline，重新打开应用不会续期。`always` 不会暴露宽泛的 account、host、transport 或 Vault 权限；它仍可撤销，且每次执行都会再检查。后台刷新和 `onOpen` 永远不会提示创建或解锁 Vault，只会显示需要用户明确继续的 state。

remembered-operation 管理只显示非秘密描述和过期状态。已选择的本地文件/目录或 serial device 在保存历史中只显示带不透明后缀的通用标签，绝不显示原始路径或 device identity。已安装插件只能清除其当前已签名 package 所拥有的决定；应用可以保留旧 package 历史供查看。
