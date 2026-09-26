# 能力与受保护操作

`PluginCapability` 是可审阅的意图，绝不是环境权限。当前取值为 `uiPanel`、`uiNavigation`、`uiPage`、`uiWebviewIsolated`、`uiHostDomObserve`、`uiHostDomMutate`、`uiHostCss`、`clipboardWrite`、`terminalProvider`、`deviceSerial`、`terminalMetadata`、`terminalObserve`、`terminalAnnotation`、`terminalProposeInput`、`terminalRequestInput`、`hostMetadataRead`、`hostMutationPropose`、`hostSessionRequest`、`remoteInspect`、`remoteExecRequest`、`networkDomain`、`localFiles`、`localProcess`、`storagePlugin`、`credentialsPlugin`、`sftpRead`、`sftpWrite`、`metricsRead`、`sshSync`、`appPreferencesRead` 和 `terminalHistoryRead`。

Core 会在准入和真正受保护写入前，核验 operation、精确 target、package identity/hash、当前 grant epoch、owner、scope 与 generation。因此 capability grant 不能序列化到 handle、复制给其他 resource，也不能跨 replacement/revoke 存活。网络支持类型化 HTTP、WSS、TCP、UDP 和 TLS resource。SFTP 与 remote exec 是独立 Core resource，均不暴露原始 SSH channel。本地 process 不是系统 sandbox。credential 输入与精确 origin 注入由 Core 控制。terminal input 是逐次受保护审批，实际写入时会重核对 target focus、input ownership 和 generation。

受保护 prompt 可显示 `once` 或 `always`。`once` 只授权当前 pending 的精确 action。`always` 也不是宽泛的插件授权：只有 Core 返回 `exactOperation` 时才可记住，仍绑定 package identity、action、frozen target、当前 policy revision 和执行时 fence。选择 `always` 时可选 15 分钟、1 小时、24 小时或无限期（默认）。Core 在批准时计算并持久化绝对过期时间，重新打开应用不会延长有限期决定。`unavailable`、`unstableTarget` 或 `storageUnavailable` 代表不可记住。

guest 只能通过 `permissions`、`permissionRevoke` 和 `permissionsForget` 查看或撤销自己当前拥有的 remembered-operation 摘要，不能借此扩大 capability grant。基于选择器的记录只保留带 Core 生成不透明后缀的通用标签，不保留本地路径或 serial identity。网络 `host:port` 与本地 process basename 可以作为可读管理标签，但精确冻结 scope 仍只在 Core 内。宿主管理可以保留历史和已过期记录供查看。

`onOpen`、timer、provider callback 与 workflow 自动 step 都是后台上下文，不能打开 protected prompt、创建/解锁 Vault、请求 terminal input 或把可见 surface 当作批准。应返回 `interactionRequired`、`vaultMissing`、`vaultLocked` 或 `vaultRequiresReload`，等待明确用户 action。源码出现或 `describe` 列出均不代表 native、hardware、macOS 或 Windows 已验收；以[实施状态](../../../README.zh-CN.md#安装与快速开始)为准。


## 分类权限

`sshSync` 保护主机、凭据和远程桌面的 exchange 原语，不授予应用偏好或终端历史读取权。`appPreferencesRead` 与 `terminalHistoryRead` 是独立权限；后者还要求显式用户动作、历史采集已开启且 Vault 可用。`dataCatalog` 仅供发现，不构成授权。

## SSH 与远程桌面同步

`sshSync` 提供按类别受保护的 exchange 原语。插件选择类别、HTTP 操作和冲突方向；Core 守住加密、秘密、本机应用与基线核验。Core service 接线通过 Cargo 检查；原生和端到端验收尚未完成。
